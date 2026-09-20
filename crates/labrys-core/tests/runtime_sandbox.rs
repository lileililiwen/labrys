use labrys_core::inspector::ProjectSnapshot;
use labrys_core::{
    detect_runtime, enforce_run_usage, evaluate_health, prepare, simulate_build, BuildStatus,
    Endpoint, HealthStatus, NetworkMode, RuntimeKind, RuntimeProfile, SandboxLimits, SupportTier,
};

fn limits() -> SandboxLimits {
    SandboxLimits::docker_default()
}

fn haskell_snapshot() -> ProjectSnapshot {
    ProjectSnapshot::new([
        (
            "Dockerfile",
            "FROM haskell:9.4\nCOPY . /app\nRUN cabal build\nCMD [\"./app\"]",
        ),
        ("app/Main.hs", "main = putStrLn \"hi\""),
    ])
}

fn dotnet_snapshot() -> ProjectSnapshot {
    ProjectSnapshot::new([
        ("App.csproj", "<Project Sdk=\"Microsoft.NET.Sdk.Web\" />"),
        ("appsettings.json", "{}"),
        ("Program.cs", "using Microsoft.AspNetCore.Builder;"),
    ])
}

#[test]
fn haskell_container_falls_back_to_generic_tier0() {
    // WHEN a Haskell project supplies a valid Dockerfile ...
    let detected = detect_runtime(&haskell_snapshot()).unwrap();
    // THEN the generic runtime builds and starts it ...
    assert_eq!(detected.kind, RuntimeKind::Generic);
    // AND the project is classified Tier 0 rather than rejected by language.
    assert_eq!(detected.tier, SupportTier::Tier0);

    let config = prepare(
        &detected,
        RuntimeProfile::Production,
        3000,
        None,
        None,
        limits(),
    )
    .unwrap();
    assert_eq!(config.kind, RuntimeKind::Generic);
    assert_eq!(config.tier, SupportTier::Tier0);
    assert!(config.oci_image.is_some(), "prod uses OCI interchange");
    let built = simulate_build(&config, 10);
    assert!(built.is_success());
    assert_eq!(built.status, BuildStatus::Succeeded);
}

#[test]
fn dockerfile_without_from_is_unsupported() {
    let snapshot = ProjectSnapshot::new([("Dockerfile", "# empty")]);
    assert!(detect_runtime(&snapshot).is_err());
}

#[test]
fn aspnet_runs_dev_watch_and_prod_artifact() {
    let detected = detect_runtime(&dotnet_snapshot()).unwrap();
    assert_eq!(detected.kind, RuntimeKind::DotNet);
    assert_eq!(detected.tier, SupportTier::Tier2);

    // WHEN development is requested THEN the runtime may use `dotnet watch` ...
    let dev = prepare(
        &detected,
        RuntimeProfile::Development,
        5000,
        None,
        None,
        limits(),
    )
    .unwrap();
    assert!(dev.run_command.iter().any(|c| c == "watch"));
    assert!(dev.oci_image.is_none(), "dev never mints OCI images");

    // AND production uses a reproducible build artifact or OCI image.
    let prod = prepare(
        &detected,
        RuntimeProfile::Production,
        5000,
        None,
        None,
        limits(),
    )
    .unwrap();
    assert!(prod.run_command.iter().any(|c| c == "publish"));
    assert!(!prod.run_command.iter().any(|c| c == "watch"));
    assert!(prod.oci_image.is_some());
    assert_ne!(dev.run_command, prod.run_command);
}

#[test]
fn dev_server_is_never_a_production_command() {
    for snapshot in [
        ProjectSnapshot::new([("package.json", "{}")]),
        dotnet_snapshot(),
        ProjectSnapshot::new([
            ("requirements.txt", "fastapi\n"),
            ("main.py", "import fastapi"),
        ]),
        ProjectSnapshot::new([(
            "Cargo.toml",
            "[package]\nname=\"x\"\n[dependencies]\naxum=\"0.7\"",
        )]),
    ] {
        let detected = detect_runtime(&snapshot).unwrap();
        let dev = prepare(
            &detected,
            RuntimeProfile::Development,
            3000,
            None,
            None,
            limits(),
        )
        .unwrap();
        let prod = prepare(
            &detected,
            RuntimeProfile::Production,
            3000,
            None,
            None,
            limits(),
        )
        .unwrap();
        assert_ne!(dev.run_command, prod.run_command);
        assert!(prod.oci_image.is_some() || detected.kind == RuntimeKind::Expo);
        assert!(dev.oci_image.is_none());
    }
}

#[test]
fn node_dotnet_python_rust_expo_detection_by_tier() {
    // Node Tier1, Tier2 with Next.js layout.
    let node = detect_runtime(&ProjectSnapshot::new([("package.json", "{}")])).unwrap();
    assert_eq!(
        (node.kind, node.tier),
        (RuntimeKind::Node, SupportTier::Tier1)
    );
    let next = detect_runtime(&ProjectSnapshot::new([
        ("package.json", "{}"),
        ("next.config.mjs", "export default {}"),
    ]))
    .unwrap();
    assert_eq!(
        (next.kind, next.tier),
        (RuntimeKind::Node, SupportTier::Tier2)
    );

    // .NET Tier2 via web conventions tested above; plain library is Tier1.
    let lib = detect_runtime(&ProjectSnapshot::new([("Lib.csproj", "<Project />")])).unwrap();
    assert_eq!(
        (lib.kind, lib.tier),
        (RuntimeKind::DotNet, SupportTier::Tier1)
    );

    // Python Tier1, Tier2 via Django/FastAPI.
    let py = detect_runtime(&ProjectSnapshot::new([("requirements.txt", "requests\n")])).unwrap();
    assert_eq!(
        (py.kind, py.tier),
        (RuntimeKind::Python, SupportTier::Tier1)
    );
    let dj = detect_runtime(&ProjectSnapshot::new([
        ("requirements.txt", "django\n"),
        ("manage.py", "import django"),
    ]))
    .unwrap();
    assert_eq!(
        (dj.kind, dj.tier),
        (RuntimeKind::Python, SupportTier::Tier2)
    );

    // Rust Tier1, Tier2 via web framework deps.
    let rs = detect_runtime(&ProjectSnapshot::new([(
        "Cargo.toml",
        "[package]\nname=\"x\"",
    )]))
    .unwrap();
    assert_eq!((rs.kind, rs.tier), (RuntimeKind::Rust, SupportTier::Tier1));

    // Expo requires app.json + expo in package.json.
    let expo = detect_runtime(&ProjectSnapshot::new([
        ("package.json", "{\"dependencies\":{\"expo\":\"~50\"}}"),
        ("app.json", "{}"),
    ]))
    .unwrap();
    assert_eq!(expo.kind, RuntimeKind::Expo);
    // package.json alone without expo/app.json stays Node.
    assert_eq!(node.kind, RuntimeKind::Node);
}

#[test]
fn expo_without_app_json_is_not_expo() {
    let detected = detect_runtime(&ProjectSnapshot::new([(
        "package.json",
        "{\"dependencies\":{\"expo\":\"~50\"}}",
    )]))
    .unwrap();
    assert_eq!(detected.kind, RuntimeKind::Node);
}

#[test]
fn build_exceeding_timeout_is_cancelled_with_limit_and_recovery() {
    // WHEN a build exceeds its configured timeout ...
    let detected = detect_runtime(&haskell_snapshot()).unwrap();
    let mut capped = limits();
    capped.timeout_secs = 60;
    let config = prepare(
        &detected,
        RuntimeProfile::Production,
        3000,
        None,
        None,
        capped,
    )
    .unwrap();
    let result = simulate_build(&config, 61);
    // THEN it is cancelled and marked failed ...
    assert_eq!(result.status, BuildStatus::Cancelled);
    assert!(result.cancelled);
    assert!(!result.is_success());
    // AND the event includes the enforced limit and recovery detail.
    assert_eq!(result.limit_enforced, Some("timeout_secs=60".to_string()));
    assert!(result.recovery.is_some());
    assert!(result.detail.contains("60"));
}

#[test]
fn build_within_timeout_succeeds() {
    let detected = detect_runtime(&haskell_snapshot()).unwrap();
    let config = prepare(
        &detected,
        RuntimeProfile::Development,
        3000,
        None,
        None,
        limits(),
    )
    .unwrap();
    let ok = simulate_build(&config, 60);
    assert!(ok.is_success());
    assert!(ok.limit_enforced.is_none());
}

#[test]
fn sandbox_limits_validate_and_enforce() {
    // Zeroed limits are rejected before scheduling.
    let mut bad = limits();
    bad.cpu_millicpus = 0;
    assert!(bad.validate().is_err());
    let mut bad = limits();
    bad.memory_mb = 0;
    assert!(bad.validate().is_err());
    let mut bad = limits();
    bad.timeout_secs = 0;
    assert!(bad.validate().is_err());
    let mut bad = limits();
    bad.max_processes = 0;
    assert!(bad.validate().is_err());
    assert!(limits().validate().is_ok());

    // Docker default drops capabilities, pins read-only root, isolates network.
    let defaults = SandboxLimits::docker_default();
    assert!(defaults.readonly_root);
    assert!(defaults.drop_capabilities.contains(&"ALL".to_string()));
    assert_eq!(defaults.network, NetworkMode::Isolated);

    // Process and memory breaches name the enforced limit.
    let detected = detect_runtime(&haskell_snapshot()).unwrap();
    let config = prepare(
        &detected,
        RuntimeProfile::Production,
        3000,
        None,
        None,
        limits(),
    )
    .unwrap();
    let err = enforce_run_usage(&config, 10_000, 64).unwrap_err();
    assert!(matches!(err, labrys_core::CoreError::LimitExceeded { .. }));
    assert!(format!("{err}").contains("max_processes"));
    let err = enforce_run_usage(&config, 4, 1_000_000).unwrap_err();
    assert!(format!("{err}").contains("memory_mb"));
    assert!(enforce_run_usage(&config, 4, 64).is_ok());
}

#[test]
fn port_exposure_follows_network_mode_and_rejects_zero() {
    assert!(Endpoint::new("127.0.0.1", 0, true).is_err());

    let detected = detect_runtime(&haskell_snapshot()).unwrap();
    let open = prepare(
        &detected,
        RuntimeProfile::Development,
        8080,
        None,
        None,
        limits(),
    )
    .unwrap();
    assert!(open.endpoint().unwrap().exposed);

    let mut closed_limits = limits();
    closed_limits.network = NetworkMode::Disabled;
    let closed = prepare(
        &detected,
        RuntimeProfile::Development,
        8080,
        None,
        None,
        closed_limits,
    )
    .unwrap();
    assert!(!closed.endpoint().unwrap().exposed);

    // Zero ports are rejected at plan time.
    assert!(prepare(
        &detected,
        RuntimeProfile::Development,
        0,
        None,
        None,
        limits()
    )
    .is_err());
}

#[test]
fn health_failures_distinguish_dev_from_prod() {
    let detected = detect_runtime(&dotnet_snapshot()).unwrap();
    let dev = prepare(
        &detected,
        RuntimeProfile::Development,
        5000,
        None,
        None,
        limits(),
    )
    .unwrap();
    let prod = prepare(
        &detected,
        RuntimeProfile::Production,
        5000,
        None,
        None,
        limits(),
    )
    .unwrap();

    // Dev server 2xx/3xx counts as healthy; 5xx does not.
    assert!(evaluate_health(&dev, Some(200), Some("/")).is_healthy());
    assert!(evaluate_health(&dev, Some(302), Some("/")).is_healthy());
    assert!(!evaluate_health(&dev, Some(500), Some("/")).is_healthy());

    // Production requires the health path with 2xx.
    assert!(evaluate_health(&prod, Some(200), Some("/healthz")).is_healthy());
    let wrong_path = evaluate_health(&prod, Some(200), Some("/"));
    assert!(!wrong_path.is_healthy());
    assert!(matches!(wrong_path, HealthStatus::Unhealthy { .. }));
    let redirect = evaluate_health(&prod, Some(302), Some("/healthz"));
    assert!(
        !redirect.is_healthy(),
        "dev-shaped redirect is not prod healthy"
    );
    let missing = evaluate_health(&prod, None, None);
    assert!(!missing.is_healthy());
}

#[test]
fn unsupported_runtime_without_manifest_or_dockerfile() {
    let snapshot = ProjectSnapshot::new([("README.md", "just docs")]);
    let err = detect_runtime(&snapshot).unwrap_err();
    assert!(
        matches!(err, labrys_core::CoreError::UnsupportedRuntime(_)),
        "expected unsupported runtime, got {err:?}"
    );
}

#[test]
fn config_json_roundtrip_is_strict() {
    let detected = detect_runtime(&haskell_snapshot()).unwrap();
    let config = prepare(
        &detected,
        RuntimeProfile::Production,
        3000,
        Some("/healthz".to_string()),
        None,
        limits(),
    )
    .unwrap();
    let restored = labrys_core::RuntimeConfig::from_json(&config.to_json().unwrap()).unwrap();
    assert_eq!(restored, config);
    let mut value: serde_json::Value = serde_json::from_str(&config.to_json().unwrap()).unwrap();
    value["unknown_future_field"] = serde_json::json!("nope");
    assert!(
        labrys_core::RuntimeConfig::from_json(&serde_json::to_string(&value).unwrap()).is_err()
    );
}
