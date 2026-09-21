//! `runtime-tier-depth`: Tier 3 framework integrations (Django, FastAPI,
//! Axum, deeper Rust/Expo) with deep conventions, plus the generic Tier 0
//! fallback guarantee.

use labrys_core::inspector::ProjectSnapshot;
use labrys_core::{
    detect_runtime, evaluate_health, prepare, AdoptionItemKind, AdoptionPlan, Application,
    FrameworkConventions, Inspector, Origin, RuntimeKind, RuntimeNotes, RuntimeProfile,
    SandboxLimits, SupportTier,
};

fn limits() -> SandboxLimits {
    SandboxLimits::docker_default()
}

fn snapshot(files: &[(&str, &str)]) -> ProjectSnapshot {
    ProjectSnapshot::new(files.iter().map(|(p, c)| (p.to_string(), c.to_string())))
}

fn django_full() -> ProjectSnapshot {
    snapshot(&[
        ("requirements.txt", "django==5.0\ngunicorn==21.2\n"),
        (
            "manage.py",
            "#!/usr/bin/env python\nimport os\nos.environ.setdefault('DJANGO_SETTINGS_MODULE', 'mysite.settings')\n",
        ),
        ("mysite/__init__.py", ""),
        ("mysite/settings.py", "SECRET_KEY = 'x'\nROOT_URLCONF = 'mysite.urls'\n"),
        ("mysite/urls.py", "urlpatterns = []\n"),
        ("shop/__init__.py", ""),
        ("shop/migrations/__init__.py", ""),
        ("shop/migrations/0001_initial.py", "operations = []\n"),
    ])
}

fn fastapi_full() -> ProjectSnapshot {
    snapshot(&[
        ("requirements.txt", "fastapi==0.110\nuvicorn==0.29\nalembic==1.13\n"),
        (
            "main.py",
            "from fastapi import FastAPI\napp = FastAPI()\n@app.get('/health')\nasync def health(): return {}\n",
        ),
        ("alembic.ini", "[alembic]\nscript_location = alembic\n"),
        ("tests/test_health.py", "def test_health(): pass\n"),
    ])
}

fn axum_full() -> ProjectSnapshot {
    snapshot(&[
        (
            "Cargo.toml",
            "[package]\nname = \"api\"\n[dependencies]\naxum = \"0.7\"\nsqlx = \"0.8\"\n",
        ),
        (
            "src/main.rs",
            "async fn health() {}\n#[tokio::main]\nasync fn main() { let app: axum::Router = axum::Router::new().route(\"/health\", axum::routing::get(health)); }\n",
        ),
        ("migrations/0001_init.sql", "CREATE TABLE t (id INT);\n"),
    ])
}

// ---------------------------------------------------------------------------
// Requirement: frameworks detect with deep conventions — Django
// ---------------------------------------------------------------------------

#[test]
fn django_project_detects_with_deep_conventions() {
    // WHEN a Django project with settings, manage.py, and migrations is
    // detected ...
    let detected = detect_runtime(&django_full()).unwrap();
    assert_eq!(detected.kind, RuntimeKind::Python);
    assert_eq!(detected.tier, SupportTier::Tier3);
    assert_eq!(detected.framework.as_deref(), Some("django"));
    assert_eq!(detected.prod_entry.as_deref(), Some("mysite"));
    assert_eq!(
        detected.migrate_command,
        vec!["python", "manage.py", "migrate"]
    );
    assert_eq!(detected.test_command, vec!["python", "manage.py", "test"]);

    // THEN planning yields migration, test, dev, production, and health
    // conventions with file evidence and a non-generic adapter decision.
    let conventions = detected.conventions().expect("conventions");
    assert_eq!(conventions.framework, "django");
    assert_eq!(
        conventions.prod_command,
        vec!["gunicorn", "mysite.wsgi:application"]
    );
    assert!(conventions.dev_command.contains(&"runserver".to_string()));
    assert_eq!(conventions.health_path, None);

    let dev = prepare(
        &detected,
        RuntimeProfile::Development,
        8000,
        None,
        None,
        limits(),
    )
    .unwrap();
    assert!(dev.run_command.contains(&"runserver".to_string()));
    assert!(dev.oci_image.is_none(), "dev never mints OCI");
    assert_eq!(
        dev.healthcheck_path, None,
        "Django has no framework health default"
    );

    let prod = prepare(
        &detected,
        RuntimeProfile::Production,
        8000,
        None,
        None,
        limits(),
    )
    .unwrap();
    assert_eq!(
        prod.run_command[..2],
        ["gunicorn", "mysite.wsgi:application"]
    );
    assert!(prod.oci_image.is_some(), "prod always mints OCI");
    assert_ne!(dev.run_command, prod.run_command);

    // Inspector corroborates through the existing machinery.
    let profile = Inspector::new().inspect(&django_full());
    assert_eq!(profile.framework.as_deref(), Some("django"));
    assert_eq!(
        profile.fact("migration_runner").map(|f| f.value.as_str()),
        Some("manage.py migrate")
    );
    assert_eq!(
        profile.fact("test_runner").map(|f| f.value.as_str()),
        Some("manage.py test")
    );
}

#[test]
fn django_flat_layout_stays_tier2_without_invented_wsgi() {
    let flat = snapshot(&[
        ("requirements.txt", "django==5.0\n"),
        ("manage.py", "import django\n"),
        ("settings.py", "SECRET_KEY = 'x'\n"),
    ]);
    let detected = detect_runtime(&flat).unwrap();
    assert_eq!(detected.kind, RuntimeKind::Python);
    assert_eq!(detected.tier, SupportTier::Tier2);
    assert_eq!(detected.framework, None);
    assert!(detected.conventions().is_none());
}

// ---------------------------------------------------------------------------
// FastAPI matrix
// ---------------------------------------------------------------------------

#[test]
fn fastapi_project_detects_uvicorn_alembic_conventions() {
    let detected = detect_runtime(&fastapi_full()).unwrap();
    assert_eq!(detected.kind, RuntimeKind::Python);
    assert_eq!(detected.tier, SupportTier::Tier3);
    assert_eq!(detected.framework.as_deref(), Some("fastapi"));
    assert_eq!(detected.prod_entry.as_deref(), Some("main"));

    let dev = prepare(
        &detected,
        RuntimeProfile::Development,
        8000,
        None,
        None,
        limits(),
    )
    .unwrap();
    assert_eq!(
        dev.run_command,
        vec!["python", "-m", "uvicorn", "main:app", "--reload"]
    );
    assert_eq!(dev.healthcheck_path.as_deref(), Some("/health"));

    let prod = prepare(
        &detected,
        RuntimeProfile::Production,
        8000,
        None,
        None,
        limits(),
    )
    .unwrap();
    assert_eq!(
        prod.run_command,
        vec!["python", "-m", "uvicorn", "main:app"]
    );
    assert!(!prod.run_command.iter().any(|c| c == "--reload"));

    // Health semantics: dev accepts 3xx, production requires 2xx on /health.
    assert!(evaluate_health(&dev, Some(301), Some("/")).is_healthy());
    assert!(evaluate_health(&prod, Some(200), Some("/health")).is_healthy());
    assert!(!evaluate_health(&prod, Some(301), Some("/health")).is_healthy());

    let profile = Inspector::new().inspect(&fastapi_full());
    assert_eq!(profile.framework.as_deref(), Some("fastapi"));
    assert_eq!(
        profile.fact("migration_runner").map(|f| f.value.as_str()),
        Some("alembic upgrade head")
    );
}

#[test]
fn fastapi_without_tooling_reports_no_runners_instead_of_inventing() {
    let bare = snapshot(&[
        ("requirements.txt", "fastapi==0.110\n"),
        ("app.py", "from fastapi import FastAPI\napp = FastAPI()\n"),
    ]);
    let detected = detect_runtime(&bare).unwrap();
    // Entry alone is not deep: Tier 2, no conventions invented.
    assert_eq!(detected.tier, SupportTier::Tier2);
    assert!(detected.migrate_command.is_empty());
    assert!(detected.test_command.is_empty());
}

// ---------------------------------------------------------------------------
// Axum and deeper Rust matrix
// ---------------------------------------------------------------------------

#[test]
fn axum_project_detects_migration_health_conventions() {
    let detected = detect_runtime(&axum_full()).unwrap();
    assert_eq!(detected.kind, RuntimeKind::Rust);
    assert_eq!(detected.tier, SupportTier::Tier3);
    assert_eq!(detected.framework.as_deref(), Some("axum"));
    assert_eq!(detected.migrate_command, vec!["sqlx", "migrate", "run"]);
    assert_eq!(detected.test_command, vec!["cargo", "test"]);

    let dev = prepare(
        &detected,
        RuntimeProfile::Development,
        3000,
        None,
        None,
        limits(),
    )
    .unwrap();
    assert_eq!(dev.run_command, vec!["cargo", "run"]);
    let prod = prepare(
        &detected,
        RuntimeProfile::Production,
        3000,
        None,
        None,
        limits(),
    )
    .unwrap();
    assert_eq!(prod.run_command, vec!["cargo", "build", "--release"]);
    assert_eq!(prod.healthcheck_path.as_deref(), Some("/health"));

    let profile = Inspector::new().inspect(&axum_full());
    assert_eq!(profile.framework.as_deref(), Some("axum"));
    assert_eq!(
        profile.fact("migration_runner").map(|f| f.value.as_str()),
        Some("sqlx migrate run")
    );
}

#[test]
fn axum_without_tooling_stays_tier2() {
    let bare = snapshot(&[(
        "Cargo.toml",
        "[package]\nname=\"x\"\n[dependencies]\naxum=\"0.7\"",
    )]);
    let detected = detect_runtime(&bare).unwrap();
    assert_eq!(detected.tier, SupportTier::Tier2);
    assert_eq!(detected.framework, None);
}

#[test]
fn rust_workspace_with_tooling_detects_deep() {
    let workspace = snapshot(&[
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"api\"]\n[workspace.dependencies]\nsqlx = \"0.8\"\n",
        ),
        ("api/src/main.rs", "fn main() {}\n"),
        ("migrations/0001_init.sql", "CREATE TABLE t (id INT);\n"),
    ]);
    let detected = detect_runtime(&workspace).unwrap();
    assert_eq!(detected.kind, RuntimeKind::Rust);
    assert_eq!(detected.tier, SupportTier::Tier3);
    assert_eq!(detected.framework.as_deref(), Some("rust"));
    let conventions = detected.conventions().expect("conventions");
    assert_eq!(conventions.migrate_command, vec!["sqlx", "migrate", "run"]);

    let profile = Inspector::new().inspect(&workspace);
    assert_eq!(
        profile.fact("workspace").map(|f| f.value.as_str()),
        Some("cargo workspace")
    );
}

// ---------------------------------------------------------------------------
// Expo matrix
// ---------------------------------------------------------------------------

#[test]
fn expo_with_routes_and_build_profiles_detects_deep() {
    let expo = snapshot(&[
        (
            "package.json",
            r#"{"name":"app","dependencies":{"expo":"50.0.0"}}"#,
        ),
        ("app.json", r#"{"expo":{"name":"app"}}"#),
        ("app/index.tsx", "export default function Index() {}\n"),
        ("eas.json", r#"{"build":{"production":{}}}"#),
    ]);
    let detected = detect_runtime(&expo).unwrap();
    assert_eq!(detected.kind, RuntimeKind::Expo);
    assert_eq!(detected.tier, SupportTier::Tier3);
    assert_eq!(detected.framework.as_deref(), Some("expo"));

    let dev = prepare(
        &detected,
        RuntimeProfile::Development,
        19000,
        None,
        None,
        limits(),
    )
    .unwrap();
    assert_eq!(dev.run_command, vec!["expo", "start"]);
    let prod = prepare(
        &detected,
        RuntimeProfile::Production,
        19000,
        None,
        None,
        limits(),
    )
    .unwrap();
    assert_eq!(prod.run_command, vec!["expo", "export"]);

    let profile = Inspector::new().inspect(&expo);
    assert_eq!(profile.framework.as_deref(), Some("expo"));
    assert_eq!(
        profile.fact("build_profile").map(|f| f.value.as_str()),
        Some("eas")
    );
}

#[test]
fn bare_expo_stays_tier1() {
    let bare = snapshot(&[
        (
            "package.json",
            r#"{"name":"app","dependencies":{"expo":"50.0.0"}}"#,
        ),
        ("app.json", r#"{"expo":{"name":"app"}}"#),
    ]);
    let detected = detect_runtime(&bare).unwrap();
    assert_eq!(detected.tier, SupportTier::Tier1);
    assert_eq!(detected.framework, None);
}

// ---------------------------------------------------------------------------
// Conflicting signals, monorepos, missing manifests, generic fallback
// ---------------------------------------------------------------------------

#[test]
fn rival_high_confidence_framework_claims_become_explicit_unknown() {
    // WHEN rival high-confidence framework claims conflict ...
    let snap = snapshot(&[
        ("requirements.txt", "django==5.0\n"),
        ("manage.py", "import django\n"),
        ("mysite/settings.py", "SECRET_KEY = 'x'\n"),
        ("shop/migrations/__init__.py", ""),
        (
            "Cargo.toml",
            "[package]\nname = \"api\"\n[dependencies]\naxum = \"0.7\"\n",
        ),
        ("src/main.rs", "use axum::Router;\nfn main() {}\n"),
    ]);
    let profile = Inspector::new().inspect(&snap);

    // THEN the conflict becomes an explicit unknown with candidate facts
    // preserved instead of a false detection.
    assert_eq!(profile.framework, None);
    assert!(profile.is_unknown("framework"));
    let unknown = profile
        .unknowns
        .iter()
        .find(|u| u.field == "framework")
        .expect("framework unknown");
    assert!(unknown.reason.contains("django"), "{}", unknown.reason);
    assert!(unknown.reason.contains("axum"), "{}", unknown.reason);
    assert!(
        unknown
            .evidence_considered
            .iter()
            .any(|s| s.contains("manage.py")),
        "{:?}",
        unknown.evidence_considered
    );
    assert!(
        unknown
            .evidence_considered
            .iter()
            .any(|s| s.contains("Cargo.toml")),
        "{:?}",
        unknown.evidence_considered
    );
}

#[test]
fn monorepo_keeps_rival_languages_unknown_but_framework_evidence() {
    let mono = snapshot(&[
        ("backend/requirements.txt", "django==5.0\n"),
        ("backend/manage.py", "import django\n"),
        ("frontend/package.json", r#"{"name":"web"}"#),
    ]);
    let profile = Inspector::new().inspect(&mono);
    assert_eq!(profile.language, None);
    assert!(profile.is_unknown("language"));
    // Single framework signal still stands beside the language conflict.
    assert_eq!(profile.framework.as_deref(), Some("django"));
}

#[test]
fn framework_files_without_manifest_still_inspect_but_do_not_detect() {
    let bare = snapshot(&[
        ("manage.py", "import django\n"),
        ("mysite/settings.py", "SECRET_KEY = 'x'\n"),
        ("shop/migrations/__init__.py", ""),
    ]);
    // No Python manifest: the runtime detector declines ...
    assert!(detect_runtime(&bare).is_err());
    // ... while the inspector still reports framework evidence.
    let profile = Inspector::new().inspect(&bare);
    assert_eq!(profile.framework.as_deref(), Some("django"));
    assert!(profile.fact("migration_runner").is_some());
}

#[test]
fn unknown_framework_with_valid_dockerfile_keeps_tier0() {
    // WHEN no framework adapter claims the project but a valid Dockerfile
    // exists ...
    let snap = snapshot(&[
        (
            "Dockerfile",
            "FROM ruby:3.3\nCOPY . /app\nRUN bundle install\nCMD [\"rails\",\"server\"]",
        ),
        ("Gemfile", "source 'https://rubygems.org'\ngem 'rails'\n"),
    ]);
    // THEN the generic adapter handles build/run and Tier 0 guarantees hold.
    let detected = detect_runtime(&snap).unwrap();
    assert_eq!(detected.kind, RuntimeKind::Generic);
    assert_eq!(detected.tier, SupportTier::Tier0);
    let prod = prepare(
        &detected,
        RuntimeProfile::Production,
        3000,
        None,
        None,
        limits(),
    )
    .unwrap();
    assert!(prod.oci_image.is_some());
}

// ---------------------------------------------------------------------------
// Adoption proposals and manifest portability
// ---------------------------------------------------------------------------

#[test]
fn adoption_plan_proposes_runners_without_applying() {
    let profile = Inspector::new().inspect(&django_full());
    let plan = AdoptionPlan::generate(&profile);
    let adopts: Vec<String> = plan
        .items_of_kind(AdoptionItemKind::Adopt)
        .iter()
        .map(|i| i.subject.clone())
        .collect();
    assert!(
        adopts.iter().any(|s| s.contains("manage.py migrate")),
        "{adopts:?}"
    );
    assert!(!plan.applied);
}

#[test]
fn manifest_carries_portable_runtime_notes() {
    let detected = detect_runtime(&django_full()).unwrap();
    let notes = RuntimeNotes::from_detection(&detected);
    assert_eq!(notes.kind, RuntimeKind::Python);
    assert_eq!(notes.tier, SupportTier::Tier3);
    assert_eq!(notes.framework.as_deref(), Some("django"));
    assert_eq!(
        notes.prod_command,
        vec!["gunicorn", "mysite.wsgi:application"]
    );

    let app = Application::new(
        "shop",
        Origin::ImportedLocal {
            path: ".".to_string(),
        },
        "tester",
    );
    let mut manifest = labrys_core::Manifest::from_application(&app);
    manifest.runtime_notes = Some(notes);
    let yaml = manifest.to_yaml().expect("yaml");
    assert!(yaml.contains("django"));
    let restored = labrys_core::Manifest::from_yaml(&yaml).expect("parse");
    assert_eq!(restored, manifest);

    // Manifests written before runtime notes stay parseable.
    let legacy = "manifest_version: 1\nname: old\norigin: {type: imported_local, path: \".\"}\n";
    let parsed = labrys_core::Manifest::from_yaml(legacy).expect("legacy parses");
    assert_eq!(parsed.runtime_notes, None);
}

#[test]
fn framework_conventions_table_stays_honest() {
    // Unknown pairs resolve to no conventions rather than guesses.
    assert!(FrameworkConventions::lookup(RuntimeKind::Node, "next.js", "").is_none());
    // Django/FastAPI need a derived entry; empty entries never mint.
    assert!(FrameworkConventions::lookup(RuntimeKind::Python, "django", "").is_none());
    assert!(FrameworkConventions::lookup(RuntimeKind::Python, "fastapi", "").is_none());
    let django = FrameworkConventions::lookup(RuntimeKind::Python, "django", "mysite").unwrap();
    assert!(django.health_path.is_none(), "no invented health endpoint");
    let fastapi = FrameworkConventions::lookup(RuntimeKind::Python, "fastapi", "main").unwrap();
    assert_eq!(fastapi.migrate_command, Vec::<String>::new());
}
