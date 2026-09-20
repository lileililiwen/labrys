use labrys_core::{
    AdoptionItemKind, AdoptionPlan, BroadChangeGate, ChangeStep, Evidence, ExplicitApproval,
    Inspector, InspectorPlugin, PluginFinding, ProjectSnapshot,
};

fn snapshot(files: &[(&str, &str)]) -> ProjectSnapshot {
    ProjectSnapshot::new(
        files
            .iter()
            .map(|(p, c)| ((*p).to_string(), (*c).to_string())),
    )
}

fn stage_positions(profile: &labrys_core::ApplicationProfile) -> Vec<usize> {
    profile
        .evidence
        .iter()
        .map(|e| {
            labrys_core::INSPECTION_ORDER
                .iter()
                .position(|s| *s == e.stage)
                .unwrap_or(labrys_core::INSPECTION_ORDER.len())
        })
        .collect()
}

#[test]
fn dockerfile_without_manifest_yields_container_profile_with_unknowns() {
    // WHEN a project contains a valid Dockerfile but no recognized manifest ...
    let snap = snapshot(&[(
        "Dockerfile",
        "FROM some-internal-base:1.2\nRUN ./start.sh\n",
    )]);
    let profile = Inspector::new().inspect(&snap);

    // THEN the inspector emits a container-compatible profile ...
    assert!(profile.container_compatible);
    // AND records language and framework as unknown rather than inventing them.
    assert_eq!(profile.language, None);
    assert_eq!(profile.framework, None);
    assert!(profile.is_unknown("language"));
    assert!(profile.is_unknown("framework"));
    assert!(profile.evidence.iter().any(|e| e.stage == "dockerfile"));
}

#[test]
fn deterministic_detectors_cover_supported_stacks() {
    let node = snapshot(&[(
        "package.json",
        r#"{"name":"web","dependencies":{"next":"14.0.0","react":"18.0.0"}}"#,
    )]);
    let profile = Inspector::new().inspect(&node);
    assert_eq!(profile.language.as_deref(), Some("node"));
    assert_eq!(profile.framework.as_deref(), Some("next.js"));

    let dotnet = snapshot(&[(
        "Shop/Shop.csproj",
        r#"<Project Sdk="Microsoft.NET.Sdk.Web"><PropertyGroup><TargetFramework>net8.0</TargetFramework></PropertyGroup></Project>"#,
    )]);
    let profile = Inspector::new().inspect(&dotnet);
    assert_eq!(profile.language.as_deref(), Some("dotnet"));
    assert_eq!(profile.framework.as_deref(), Some("aspnetcore"));

    let python = snapshot(&[("requirements.txt", "fastapi==0.110\nuvicorn==0.29\n")]);
    let profile = Inspector::new().inspect(&python);
    assert_eq!(profile.language.as_deref(), Some("python"));
    assert_eq!(profile.framework.as_deref(), Some("fastapi"));

    let rust = snapshot(&[(
        "Cargo.toml",
        "[package]\nname = \"api\"\n[dependencies]\naxum = \"0.7\"\n",
    )]);
    let profile = Inspector::new().inspect(&rust);
    assert_eq!(profile.language.as_deref(), Some("rust"));
    assert_eq!(profile.framework.as_deref(), Some("axum"));

    let git = snapshot(&[
        (
            ".git/config",
            "[remote \"origin\"]\nurl = https://example.com/acme/shop.git\n",
        ),
        (
            "Dockerfile",
            "FROM node:20-alpine\nCMD [\"node\",\"server.js\"]\n",
        ),
    ]);
    let profile = Inspector::new().inspect(&git);
    assert_eq!(profile.language.as_deref(), Some("node"));
    assert!(profile.fact("git").is_some(), "git detector must fire");
    assert!(profile.container_compatible);
}

#[test]
fn inspection_evidence_follows_design_order() {
    let snap = snapshot(&[
        (
            ".github/workflows/ci.yml",
            "on: [push]\njobs:\n  build:\n    runs-on: ubuntu-latest\n",
        ),
        ("src/index.ts", "export const x = 1;\n"),
        ("requirements.txt", "django==5.0\n"),
        (
            "Dockerfile",
            "FROM python:3.12-slim\nCMD [\"python\",\"app.py\"]\n",
        ),
        ("docker-compose.yml", "services:\n  web:\n    build: .\n"),
        (
            "package.json",
            r#"{"name":"web","dependencies":{"express":"4.19.0"}}"#,
        ),
        (".env", "GOOGLE_CLIENT_ID=abc123\n"),
    ]);
    let profile = Inspector::new().inspect(&snap);
    let positions = stage_positions(&profile);
    let mut sorted = positions.clone();
    sorted.sort_unstable();
    assert_eq!(positions, sorted, "evidence must follow INSPECTION_ORDER");
    // Manifest-stage conflict (node vs python at equal confidence) stays unknown.
    assert_eq!(profile.language, None);
    assert!(profile.is_unknown("language"));
    // Configuration evidence still recorded alongside the conflict.
    assert_eq!(
        profile.fact("auth_provider").map(|f| f.value.as_str()),
        Some("google-oauth")
    );
}

struct FixedPlugin {
    findings: Vec<PluginFinding>,
}

impl InspectorPlugin for FixedPlugin {
    fn name(&self) -> &str {
        "test-plugin"
    }

    fn detect(&self, _snapshot: &ProjectSnapshot) -> Vec<PluginFinding> {
        self.findings.clone()
    }
}

fn language_hypothesis(value: &str, confidence: f32) -> PluginFinding {
    PluginFinding::new(
        "language",
        value,
        Evidence::new(
            "hypothesis",
            "plugin",
            format!("plugin guesses {value}"),
            confidence,
        ),
        confidence,
    )
}

#[test]
fn plugin_agreement_corroborates_and_conflict_stays_unknown() {
    let snap = snapshot(&[("package.json", r#"{"name":"web"}"#)]);

    // Agreement keeps the deterministic value and retains plugin evidence.
    let mut inspector = Inspector::new();
    inspector.register(FixedPlugin {
        findings: vec![language_hypothesis("node", 0.6)],
    });
    let profile = inspector.inspect(&snap);
    assert_eq!(profile.language.as_deref(), Some("node"));
    assert!(profile
        .evidence
        .iter()
        .any(|e| e.source.contains("test-plugin")));

    // High-confidence disagreement becomes an explicit unknown.
    let mut inspector = Inspector::new();
    inspector.register(FixedPlugin {
        findings: vec![language_hypothesis("python", 1.0)],
    });
    let profile = inspector.inspect(&snap);
    assert_eq!(profile.language, None);
    assert!(profile.is_unknown("language"));

    // Low-confidence disagreement is kept as a candidate, not a rewrite.
    let mut inspector = Inspector::new();
    inspector.register(FixedPlugin {
        findings: vec![language_hypothesis("rust", 0.2)],
    });
    let profile = inspector.inspect(&snap);
    assert_eq!(profile.language.as_deref(), Some("node"));
    assert_eq!(
        profile.fact("candidate_language").map(|f| f.value.as_str()),
        Some("rust")
    );
}

#[test]
fn adoption_plan_keeps_oauth_and_proposes_without_applying() {
    // WHEN an imported project contains configured Google OAuth ...
    let snap = snapshot(&[
        (
            ".env",
            "GOOGLE_CLIENT_ID=abc123.apps.googleusercontent.com\n",
        ),
        (
            "Dockerfile",
            "FROM node:20-alpine\nCMD [\"node\",\"server.js\"]\n",
        ),
        (".github/workflows/ci.yml", "on: [push]\n"),
    ]);
    let profile = Inspector::new().inspect(&snap);
    let plan = AdoptionPlan::generate(&profile);

    // THEN the plan recommends KEEP for that auth integration ...
    let keeps = plan.items_of_kind(AdoptionItemKind::Keep);
    assert!(
        keeps.iter().any(|i| i.subject.contains("google-oauth")),
        "OAuth integration must be KEEP, got: {:?}",
        keeps.iter().map(|i| &i.subject).collect::<Vec<_>>()
    );
    // AND may recommend ADOPT for secrets, preview, deployment, and logs.
    for subject in [
        "secrets management",
        "preview environments",
        "deployment pipeline",
        "log aggregation",
    ] {
        assert!(
            plan.items_of_kind(AdoptionItemKind::Adopt)
                .iter()
                .any(|i| i.subject == subject),
            "missing ADOPT {subject}"
        );
    }
    // Generation never applies.
    assert!(!plan.applied);
    assert!(!plan.profile_digest.is_empty());
}

#[test]
fn adoption_apply_requires_explicit_approval() {
    let snap = snapshot(&[("Dockerfile", "FROM node:20-alpine\n")]);
    let profile = Inspector::new().inspect(&snap);
    let mut plan = AdoptionPlan::generate(&profile);

    let denied = ExplicitApproval::denied("human:reviewer", "adopt preview + logs");
    assert!(plan.apply(&denied).is_err());
    assert!(!plan.applied);

    let unnamed = ExplicitApproval::granted("", "adopt preview + logs");
    assert!(plan.apply(&unnamed).is_err());
    assert!(!plan.applied);

    let granted = ExplicitApproval::granted("human:reviewer", "adopt preview + logs");
    assert!(plan.apply(&granted).is_ok());
    assert!(plan.applied);
}

#[test]
fn broad_changes_require_full_sequence_with_evidence() {
    // WHEN an agent wants to replace existing storage ...
    let snap = snapshot(&[("requirements.txt", "django==5.0\n")]);
    let profile = Inspector::new().inspect(&snap);
    let plan = AdoptionPlan::generate(&profile);
    let migration = plan
        .items_of_kind(AdoptionItemKind::OptionalMigration)
        .into_iter()
        .next()
        .expect("migration proposal");
    assert!(
        !migration.evidence.is_empty(),
        "proposal must present affected evidence"
    );

    // THEN the platform blocks apply until the full sequence completes ...
    let partial = vec![ChangeStep::Inspect, ChangeStep::Understand];
    assert!(BroadChangeGate::check(&partial, ChangeStep::Apply).is_err());
    assert!(BroadChangeGate::check(&[], ChangeStep::Propose).is_err());
    // AND does not apply it without explicit approval (covered) — full
    // sequence plus approval is the only path forward.
    let full = vec![
        ChangeStep::Inspect,
        ChangeStep::Understand,
        ChangeStep::Model,
        ChangeStep::Propose,
    ];
    assert!(BroadChangeGate::check(&full, ChangeStep::Apply).is_ok());
    assert!(BroadChangeGate::check(&full, ChangeStep::Propose).is_ok());
}

#[test]
fn imported_project_without_manifest_inspects_safely() {
    // Imported projects operate without labrys.yaml; inspection is pure.
    let before = snapshot(&[("src/main.rs", "fn main() {}\n")]);
    let profile = Inspector::new().inspect(&before);
    let plan = AdoptionPlan::generate(&profile);
    assert!(!plan.applied);
    // Weak source-only evidence never invents a language.
    assert_eq!(profile.language, None);
    assert!(profile.is_unknown("language"));
    // Snapshot untouched by inspection.
    assert!(before.get("src/main.rs").is_some());
    assert!(before.get("labrys.yaml").is_none());
}

#[test]
fn profile_and_plan_survive_strict_json_roundtrip() {
    let snap = snapshot(&[(
        "package.json",
        r#"{"name":"web","dependencies":{"next":"14.0.0"}}"#,
    )]);
    let profile = Inspector::new().inspect(&snap);
    let restored =
        labrys_core::ApplicationProfile::from_json(&profile.to_json().expect("serialize"))
            .expect("deserialize");
    assert_eq!(restored, profile);

    let plan = AdoptionPlan::generate(&profile);
    let restored = labrys_core::AdoptionPlan::from_json(&plan.to_json().expect("serialize"))
        .expect("deserialize");
    assert_eq!(restored, plan);

    let mut value: serde_json::Value =
        serde_json::from_str(&profile.to_json().expect("serialize")).expect("json value");
    value["unknown_future_field"] = serde_json::json!("nope");
    let json = serde_json::to_string(&value).expect("reserialize");
    assert!(labrys_core::ApplicationProfile::from_json(&json).is_err());
}
