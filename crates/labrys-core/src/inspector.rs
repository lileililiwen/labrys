//! Deterministic project inspection and incremental adoption planning.
//!
//! Covers requirement `project-inspector`: evidence precedes inference,
//! adoption is incremental and reviewable, and broad agent changes require an
//! inspect → understand → model → propose → apply sequence with explicit
//! approval. Inspection never mutates the inspected project; [`AdoptionPlan`]
//! generation never applies anything.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// Deterministic inspection order (design contract).
///
/// Dockerfiles first, then compose files, manifests, framework conventions,
/// configuration, CI, and source last. Agent inference (plugins) always runs
/// after every deterministic stage.
pub const INSPECTION_ORDER: [&str; 7] = [
    "dockerfile",
    "compose",
    "manifest",
    "framework_convention",
    "configuration",
    "ci",
    "source",
];

/// Stage index inside [`INSPECTION_ORDER`]; unknown stages sort last.
pub fn stage_order(stage: &str) -> usize {
    INSPECTION_ORDER
        .iter()
        .position(|s| *s == stage)
        .unwrap_or(INSPECTION_ORDER.len())
}

/// In-memory view of a project repository supplied to the inspector.
///
/// Paths use forward slashes (`docker/Dockerfile`, `.github/workflows/ci.yml`).
/// The map form keeps inspection pure and deterministic: no filesystem access,
/// insertion order irrelevant ([`BTreeMap`] iteration is sorted).
#[derive(Debug, Clone, Default)]
pub struct ProjectSnapshot {
    files: BTreeMap<String, String>,
}

impl ProjectSnapshot {
    /// Builds a snapshot from `(path, content)` pairs.
    pub fn new(files: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>) -> Self {
        Self {
            files: files
                .into_iter()
                .map(|(p, c)| (normalize_path(&p.into()), c.into()))
                .collect(),
        }
    }

    /// Raw content of one file, if present.
    pub fn get(&self, path: &str) -> Option<&str> {
        self.files.get(&normalize_path(path)).map(String::as_str)
    }

    /// True when the exact path exists.
    pub fn has_file(&self, path: &str) -> bool {
        self.files.contains_key(&normalize_path(path))
    }

    /// All files whose file name (final segment) equals `name`.
    pub fn files_with_name(&self, name: &str) -> Vec<(&str, &str)> {
        self.files
            .iter()
            .filter(|(p, _)| file_name(p) == name)
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect()
    }

    /// All files whose path ends with the given extension (e.g. `.csproj`).
    pub fn files_with_extension(&self, extension: &str) -> Vec<(&str, &str)> {
        self.files
            .iter()
            .filter(|(p, _)| p.ends_with(extension))
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect()
    }

    /// All files with the exact file name, regardless of directory.
    pub fn files_named_anywhere(&self, names: &[&str]) -> Vec<(&str, &str)> {
        self.files
            .iter()
            .filter(|(p, _)| names.contains(&file_name(p)))
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect()
    }

    /// True when any path starts with `prefix/` or equals `prefix`.
    pub fn has_dir(&self, prefix: &str) -> bool {
        let marker = format!("{prefix}/");
        self.files
            .keys()
            .any(|p| p == prefix || p.starts_with(&marker))
    }

    /// True when a directory named `name` exists at any depth
    /// (`migrations/`, `shop/polls/migrations/`, ...).
    pub fn has_dir_anywhere(&self, name: &str) -> bool {
        let root = format!("{name}/");
        let nested = format!("/{name}/");
        self.files
            .keys()
            .any(|p| *p == name || p.starts_with(&root) || p.contains(&nested))
    }

    /// True when any file content contains `needle` (case-sensitive).
    pub fn any_content_contains(&self, needle: &str) -> Option<(&str, &str)> {
        self.files
            .iter()
            .find(|(_, c)| c.contains(needle))
            .map(|(p, c)| (p.as_str(), c.as_str()))
    }

    /// True when any file content contains `needle` (ASCII case-insensitive).
    pub fn any_content_contains_insensitive(&self, needle: &str) -> Option<(&str, &str)> {
        let needle = needle.to_ascii_lowercase();
        self.files
            .iter()
            .find(|(_, c)| c.to_ascii_lowercase().contains(&needle))
            .map(|(p, c)| (p.as_str(), c.as_str()))
    }
}

fn normalize_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    p.strip_prefix("./").unwrap_or(&p).to_string()
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// One deterministic observation. Every finding retains its source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    /// File path (or `plugin:<name>`) the observation came from.
    pub source: String,
    /// One of [`INSPECTION_ORDER`], or `plugin` for agent inference.
    pub stage: String,
    /// Human-readable observation detail. Never a secret value.
    pub detail: String,
    /// Detector confidence in `[0.0, 1.0]`.
    #[serde(default)]
    pub confidence: f32,
}

impl Evidence {
    pub fn new(
        source: impl Into<String>,
        stage: impl Into<String>,
        detail: impl Into<String>,
        confidence: f32,
    ) -> Self {
        Self {
            source: source.into(),
            stage: stage.into(),
            detail: detail.into(),
            confidence: confidence.clamp(0.0, 1.0),
        }
    }
}

/// A field the inspector could not determine deterministically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unknown {
    /// Field name, e.g. `language` or `framework`.
    pub field: String,
    /// Why the field stayed unknown (e.g. conflicting evidence).
    pub reason: String,
    /// Sources consulted before giving up, newest last.
    #[serde(default)]
    pub evidence_considered: Vec<String>,
}

/// A named detected fact backed by evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fact {
    pub name: String,
    pub value: String,
    pub evidence: Evidence,
}

/// Result of inspecting one project: detected facts plus explicit unknowns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationProfile {
    /// True when a valid Dockerfile was found.
    pub container_compatible: bool,
    /// Detected language (`node`, `dotnet`, `python`, `rust`), if determined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Detected framework (`next.js`, `django`, ...), if determined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    /// Named facts (auth providers, CI, compose, ...).
    #[serde(default)]
    pub facts: Vec<Fact>,
    /// Fields that stayed unknown, with reasons.
    #[serde(default)]
    pub unknowns: Vec<Unknown>,
    /// Every observation in [`INSPECTION_ORDER`], plugins last.
    #[serde(default)]
    pub evidence: Vec<Evidence>,
}

impl ApplicationProfile {
    /// Canonical JSON serialization.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(CoreError::from)
    }

    /// Strict JSON deserialization.
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(CoreError::from)
    }

    /// Looks up a named fact.
    pub fn fact(&self, name: &str) -> Option<&Fact> {
        self.facts.iter().find(|f| f.name == name)
    }

    /// True when `field` has an unknown entry.
    pub fn is_unknown(&self, field: &str) -> bool {
        self.unknowns.iter().any(|u| u.field == field)
    }
}

/// One agent/plugin hypothesis. Handled as inference: it may corroborate or
/// compete with deterministic evidence but never silently overrides it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginFinding {
    /// `language`, `framework`, or `fact:<name>`.
    pub field: String,
    pub value: String,
    pub evidence: Evidence,
    pub confidence: f32,
}

impl PluginFinding {
    pub fn new(
        field: impl Into<String>,
        value: impl Into<String>,
        evidence: Evidence,
        confidence: f32,
    ) -> Self {
        Self {
            field: field.into(),
            value: value.into(),
            evidence,
            confidence: confidence.clamp(0.0, 1.0),
        }
    }
}

/// Language/framework detector supplied through the plugin boundary.
///
/// Built-in deterministic detectors always run first; plugins run after and
/// carry their own confidence and evidence.
pub trait InspectorPlugin {
    fn name(&self) -> &str;
    fn detect(&self, snapshot: &ProjectSnapshot) -> Vec<PluginFinding>;
}

/// Deterministic inspector with pluggable inference.
#[derive(Default)]
pub struct Inspector {
    plugins: Vec<Box<dyn InspectorPlugin>>,
}

impl std::fmt::Debug for Inspector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inspector")
            .field("plugin_count", &self.plugins.len())
            .finish()
    }
}

impl Inspector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a language/framework detector plugin.
    pub fn register<P: InspectorPlugin + 'static>(&mut self, plugin: P) {
        self.plugins.push(Box::new(plugin));
    }

    /// Plugin names in registration order.
    pub fn plugin_names(&self) -> Vec<&str> {
        self.plugins.iter().map(|p| p.name()).collect()
    }

    /// Inspects a snapshot in [`INSPECTION_ORDER`], then applies plugins.
    pub fn inspect(&self, snapshot: &ProjectSnapshot) -> ApplicationProfile {
        let mut builder = ProfileBuilder::default();
        detect_docker(snapshot, &mut builder);
        detect_compose(snapshot, &mut builder);
        detect_manifests(snapshot, &mut builder);
        detect_framework_conventions(snapshot, &mut builder);
        detect_deep_framework_conventions(snapshot, &mut builder);
        detect_configuration(snapshot, &mut builder);
        detect_ci(snapshot, &mut builder);
        detect_source_fallback(snapshot, &mut builder);
        builder.finish_deterministic();
        for plugin in &self.plugins {
            apply_plugin(plugin.as_ref(), snapshot, &mut builder);
        }
        builder.build()
    }
}

#[derive(Default)]
struct FieldState {
    value: Option<String>,
    confidence: f32,
    evidence: Option<Evidence>,
    candidates: Vec<(String, Evidence, f32)>,
}

#[derive(Default)]
struct ProfileBuilder {
    container_compatible: bool,
    language: FieldState,
    framework: FieldState,
    facts: Vec<Fact>,
    unknowns: Vec<Unknown>,
    evidence: Vec<Evidence>,
}

impl ProfileBuilder {
    fn push_evidence(&mut self, evidence: Evidence) {
        self.evidence.push(evidence);
    }

    fn push_fact(&mut self, name: &str, value: String, evidence: Evidence) {
        self.push_evidence(evidence.clone());
        self.facts.push(Fact {
            name: name.to_string(),
            value,
            evidence,
        });
    }

    fn set_field(&mut self, field: &str, value: &str, evidence: Evidence, confidence: f32) {
        let target = match field {
            "language" => &mut self.language,
            _ => &mut self.framework,
        };
        if target.value.is_none() {
            target.value = Some(value.to_string());
            target.confidence = confidence;
            target.evidence = Some(evidence.clone());
        } else {
            target
                .candidates
                .push((value.to_string(), evidence.clone(), confidence));
        }
        self.push_evidence(evidence);
    }

    /// Resolves conflicts after deterministic stages: a competing candidate
    /// at equal-or-higher confidence turns the field into an explicit
    /// unknown instead of guessing.
    fn finish_deterministic(&mut self) {
        for (name, state) in [
            ("language", &mut self.language),
            ("framework", &mut self.framework),
        ] {
            if let Some(current) = state.value.clone() {
                let conflict = state
                    .candidates
                    .iter()
                    .find(|(v, _, c)| !v.eq_ignore_ascii_case(&current) && *c >= state.confidence);
                if let Some((other, _, _)) = conflict {
                    let mut considered =
                        vec![state.evidence.clone().map(|e| e.source).unwrap_or_default()];
                    considered.extend(state.candidates.iter().map(|(_, e, _)| e.source.clone()));
                    self.unknowns.push(Unknown {
                        field: name.to_string(),
                        reason: format!(
                            "conflicting evidence: '{current}' vs '{other}'; recorded as unknown rather than inventing a value"
                        ),
                        evidence_considered: considered,
                    });
                    state.value = None;
                }
            }
        }
    }

    fn build(mut self) -> ApplicationProfile {
        // Fields never observed become explicit unknowns.
        for (name, state) in [("language", &self.language), ("framework", &self.framework)] {
            if state.value.is_none() && !self.unknowns.iter().any(|u| u.field == name) {
                let considered: Vec<String> = state
                    .candidates
                    .iter()
                    .map(|(_, e, _)| e.source.clone())
                    .collect();
                self.unknowns.push(Unknown {
                    field: name.to_string(),
                    reason:
                        "no recognized evidence; recorded as unknown rather than inventing a value"
                            .to_string(),
                    evidence_considered: considered,
                });
            }
        }
        ApplicationProfile {
            container_compatible: self.container_compatible,
            language: self.language.value.clone(),
            framework: self.framework.value.clone(),
            facts: self.facts,
            unknowns: self.unknowns,
            evidence: self.evidence,
        }
    }
}

fn detect_docker(snapshot: &ProjectSnapshot, builder: &mut ProfileBuilder) {
    let mut dockerfiles = snapshot.files_with_name("Dockerfile");
    dockerfiles.sort_by(|a, b| a.0.cmp(b.0));
    let Some((path, content)) = dockerfiles.first() else {
        return;
    };
    builder.container_compatible = true;
    let from_images: Vec<String> = content
        .lines()
        .filter_map(|line| {
            let t = line.trim();
            if t.to_ascii_uppercase().starts_with("FROM ") {
                t.split_whitespace().nth(1).map(str::to_string)
            } else {
                None
            }
        })
        .collect();
    let first_image = from_images.first().cloned().unwrap_or_default();
    let lower = first_image.to_ascii_lowercase();
    let evidence = |detail: String, confidence: f32| {
        Evidence::new(path.to_string(), "dockerfile", detail, confidence)
    };
    builder.push_evidence(evidence(
        format!("Dockerfile present (base: {first_image})"),
        1.0,
    ));
    if lower.contains("node:")
        || lower == "node"
        || lower.starts_with("node-")
        || lower.contains("/node")
    {
        builder.set_field(
            "language",
            "node",
            evidence(format!("base image suggests Node ({first_image})"), 0.9),
            0.9,
        );
    } else if lower.contains("dotnet") || lower.contains("mcr.microsoft.com") {
        builder.set_field(
            "language",
            "dotnet",
            evidence(format!("base image suggests .NET ({first_image})"), 0.9),
            0.9,
        );
    } else if lower.contains("python") {
        builder.set_field(
            "language",
            "python",
            evidence(format!("base image suggests Python ({first_image})"), 0.9),
            0.9,
        );
    } else if lower.contains("rust") {
        builder.set_field(
            "language",
            "rust",
            evidence(format!("base image suggests Rust ({first_image})"), 0.9),
            0.9,
        );
    } else {
        builder.unknowns.push(Unknown {
            field: "docker_base_language".to_string(),
            reason: format!(
                "unrecognized base image '{first_image}'; language stays to later stages"
            ),
            evidence_considered: vec![path.to_string()],
        });
    }
}

fn detect_compose(snapshot: &ProjectSnapshot, builder: &mut ProfileBuilder) {
    let mut found: Vec<(&str, &str)> = snapshot.files_named_anywhere(&[
        "docker-compose.yml",
        "docker-compose.yaml",
        "compose.yml",
        "compose.yaml",
    ]);
    found.sort_by(|a, b| a.0.cmp(b.0));
    if let Some((path, _)) = found.first() {
        builder.push_fact(
            "compose",
            "present".to_string(),
            Evidence::new(path.to_string(), "compose", "compose file present", 1.0),
        );
    }
}

struct ManifestHit {
    language: &'static str,
    confidence: f32,
    detail: String,
    source: String,
    framework: Option<(&'static str, f32, String)>,
}

fn detect_manifests(snapshot: &ProjectSnapshot, builder: &mut ProfileBuilder) {
    // Node: package.json.
    let mut manifests = snapshot.files_with_name("package.json");
    manifests.sort_by(|a, b| a.0.cmp(b.0));
    if let Some((path, content)) = manifests.first() {
        let hit = ManifestHit {
            language: "node",
            confidence: 1.0,
            detail: "package.json manifest present".to_string(),
            source: path.to_string(),
            framework: detect_node_framework(content),
        };
        apply_manifest_hit(hit, "manifest", builder);
    }
    // .NET: any .csproj / .sln / global.json.
    let mut csproj = snapshot.files_with_extension(".csproj");
    csproj.sort_by(|a, b| a.0.cmp(b.0));
    let sln = snapshot.files_with_extension(".sln");
    let global = snapshot.files_with_name("global.json");
    if let Some((path, content)) = csproj
        .first()
        .or_else(|| sln.first())
        .or_else(|| global.first())
    {
        let framework = if content.contains("Microsoft.NET.Sdk.Web") {
            Some(("aspnetcore", 0.9, "Web SDK in project file".to_string()))
        } else if content.contains("Microsoft.NET.Sdk") || !sln.is_empty() {
            Some(("dotnet-sdk", 0.7, ".NET SDK project layout".to_string()))
        } else {
            None
        };
        apply_manifest_hit(
            ManifestHit {
                language: "dotnet",
                confidence: 1.0,
                detail: format!("{} manifest present", file_name(path)),
                source: path.to_string(),
                framework,
            },
            "manifest",
            builder,
        );
    }
    // Python: requirements / pyproject / setup.
    let mut py: Vec<(&str, &str)> = Vec::new();
    for name in [
        "requirements.txt",
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "Pipfile",
        "poetry.lock",
    ] {
        py.extend(snapshot.files_with_name(name));
    }
    py.sort_by(|a, b| a.0.cmp(b.0));
    if let Some((path, content)) = py.first() {
        let lower = content.to_ascii_lowercase();
        let framework = if lower.contains("django") {
            Some(("django", 0.9, "django dependency declared".to_string()))
        } else if lower.contains("fastapi") {
            Some(("fastapi", 0.9, "fastapi dependency declared".to_string()))
        } else if lower.contains("flask") {
            Some(("flask", 0.9, "flask dependency declared".to_string()))
        } else {
            None
        };
        apply_manifest_hit(
            ManifestHit {
                language: "python",
                confidence: 1.0,
                detail: format!("{} manifest present", file_name(path)),
                source: path.to_string(),
                framework,
            },
            "manifest",
            builder,
        );
    }
    // Rust: Cargo.toml.
    let mut cargo = snapshot.files_with_name("Cargo.toml");
    cargo.sort_by(|a, b| a.0.cmp(b.0));
    if let Some((path, content)) = cargo.first() {
        let lower = content.to_ascii_lowercase();
        let framework = if lower.contains("axum") {
            Some(("axum", 0.9, "axum dependency declared".to_string()))
        } else if lower.contains("actix-web") {
            Some((
                "actix-web",
                0.9,
                "actix-web dependency declared".to_string(),
            ))
        } else if lower.contains("rocket") {
            Some(("rocket", 0.9, "rocket dependency declared".to_string()))
        } else {
            None
        };
        apply_manifest_hit(
            ManifestHit {
                language: "rust",
                confidence: 1.0,
                detail: "Cargo.toml manifest present".to_string(),
                source: path.to_string(),
                framework,
            },
            "manifest",
            builder,
        );
    }
    // Git presence is version-control evidence for imported projects.
    if snapshot.has_dir(".git")
        || snapshot.has_file(".git/config")
        || snapshot.has_file(".git/HEAD")
    {
        builder.push_fact(
            "git",
            "present".to_string(),
            Evidence::new(".git", "manifest", "git metadata present", 1.0),
        );
    }
}

fn apply_manifest_hit(hit: ManifestHit, stage: &str, builder: &mut ProfileBuilder) {
    builder.set_field(
        "language",
        hit.language,
        Evidence::new(hit.source.clone(), stage, hit.detail, hit.confidence),
        hit.confidence,
    );
    if let Some((framework, confidence, detail)) = hit.framework {
        builder.set_field(
            "framework",
            framework,
            Evidence::new(hit.source, stage, detail, confidence),
            confidence,
        );
    }
}

fn detect_node_framework(content: &str) -> Option<(&'static str, f32, String)> {
    if content.contains("\"next\"") || content.contains("'next'") {
        Some((
            "next.js",
            0.9,
            "next dependency in package.json".to_string(),
        ))
    } else if content.contains("@nestjs/core") {
        Some((
            "nestjs",
            0.9,
            "nestjs dependency in package.json".to_string(),
        ))
    } else if content.contains("\"expo\"") {
        Some(("expo", 0.9, "expo dependency in package.json".to_string()))
    } else if content.contains("\"express\"") {
        Some((
            "express",
            0.9,
            "express dependency in package.json".to_string(),
        ))
    } else if content.contains("\"react\"") {
        Some(("react", 0.7, "react dependency in package.json".to_string()))
    } else {
        None
    }
}

fn detect_framework_conventions(snapshot: &ProjectSnapshot, builder: &mut ProfileBuilder) {
    let conventions: &[(&[&str], &str, &str, f32, &str)] = &[
        (
            &["next.config.js", "next.config.mjs", "next.config.ts"],
            "framework",
            "next.js",
            0.8,
            "Next.js config convention",
        ),
        (
            &["manage.py"],
            "framework",
            "django",
            0.8,
            "Django manage.py convention",
        ),
        (
            &["Program.cs", "Startup.cs"],
            "framework",
            "aspnetcore",
            0.6,
            "ASP.NET Core entrypoint convention",
        ),
        (
            &["main.py", "app.py"],
            "framework",
            "fastapi",
            0.5,
            "Python app entrypoint convention",
        ),
        (
            &["src/main.rs"],
            "language",
            "rust",
            0.6,
            "Rust binary entrypoint convention",
        ),
    ];
    for (names, field, value, confidence, detail) in conventions {
        let mut found = snapshot.files_named_anywhere(names);
        found.sort_by(|a, b| a.0.cmp(b.0));
        if let Some((path, _)) = found.first() {
            // Entrypoint-only hints corroborate; they never override a
            // manifest-backed finding at higher confidence.
            let current_confidence = match *field {
                "language" => builder.language.confidence,
                _ => builder.framework.confidence,
            };
            if current_confidence < *confidence {
                builder.set_field(
                    field,
                    value,
                    Evidence::new(
                        path.to_string(),
                        "framework_convention",
                        (*detail).to_string(),
                        *confidence,
                    ),
                    *confidence,
                );
            } else {
                builder.push_evidence(Evidence::new(
                    path.to_string(),
                    "framework_convention",
                    (*detail).to_string(),
                    *confidence,
                ));
            }
        }
    }
}

/// Deep Tier 3 framework evidence: settings/modules, route and migration
/// discovery that corroborates or confines adapter claims through the
/// existing confidence/unknown machinery. Confidences stay at or below
/// the manifest stage (0.9) except for fully-resolved layouts (0.95),
/// so deep evidence corroborates manifest findings and only rival
/// high-confidence claims collapse to explicit unknowns.
fn detect_deep_framework_conventions(snapshot: &ProjectSnapshot, builder: &mut ProfileBuilder) {
    let convention = |source: &str, detail: &str, confidence: f32| {
        Evidence::new(
            source.to_string(),
            "framework_convention",
            detail.to_string(),
            confidence,
        )
    };
    // Django: settings module discovery corroborates the manifest claim;
    // manage.py plus settings is a fully-resolved layout.
    let mut settings = snapshot.files_named_anywhere(&["settings.py"]);
    settings.sort_by(|a, b| a.0.cmp(b.0));
    if let Some((path, _)) = settings.first() {
        builder.set_field(
            "framework",
            "django",
            convention(path, "Django settings module discovered", 0.85),
            0.85,
        );
    }
    if snapshot.has_file("manage.py") {
        if !settings.is_empty() && snapshot.has_dir_anywhere("migrations") {
            builder.set_field(
                "framework",
                "django",
                convention(
                    "manage.py",
                    "Django layout fully resolved (manage.py, settings, migrations)",
                    0.95,
                ),
                0.95,
            );
        }
        builder.push_fact(
            "migration_runner",
            "manage.py migrate".to_string(),
            convention("manage.py", "Django migration runner convention", 0.9),
        );
        builder.push_fact(
            "test_runner",
            "manage.py test".to_string(),
            convention("manage.py", "Django test runner convention", 0.8),
        );
    }
    // FastAPI: entry-point content plus runner tooling.
    let mut entries = snapshot.files_named_anywhere(&["main.py", "app.py"]);
    entries.sort_by(|a, b| a.0.cmp(b.0));
    if let Some((path, _)) = entries
        .iter()
        .find(|(_, content)| content.contains("FastAPI"))
    {
        builder.set_field(
            "framework",
            "fastapi",
            convention(path, "FastAPI application entry discovered", 0.85),
            0.85,
        );
    }
    if snapshot.has_file("alembic.ini") || snapshot.has_dir_anywhere("alembic") {
        builder.push_fact(
            "migration_runner",
            "alembic upgrade head".to_string(),
            convention("alembic.ini", "Alembic migration runner convention", 0.9),
        );
    }
    if snapshot.has_file("pytest.ini")
        || snapshot.has_dir_anywhere("tests")
        || snapshot
            .any_content_contains_insensitive("pytest")
            .is_some()
    {
        builder.push_fact(
            "test_runner",
            "pytest".to_string(),
            convention("pytest", "pytest test runner convention", 0.8),
        );
    }
    // Axum: binary entry using axum plus migration tooling.
    let mut mains = snapshot.files_with_name("main.rs");
    mains.sort_by(|a, b| a.0.cmp(b.0));
    if let Some((path, _)) = mains.iter().find(|(_, c)| c.contains("axum")) {
        builder.set_field(
            "framework",
            "axum",
            convention(path, "Axum application entry discovered", 0.85),
            0.85,
        );
    }
    let cargo = snapshot.get("Cargo.toml").unwrap_or("");
    if cargo.contains("sqlx") {
        builder.push_fact(
            "migration_runner",
            "sqlx migrate run".to_string(),
            convention("Cargo.toml", "sqlx migration runner convention", 0.9),
        );
    } else if cargo.contains("diesel") || snapshot.has_file("diesel.toml") {
        builder.push_fact(
            "migration_runner",
            "diesel migration run".to_string(),
            convention("Cargo.toml", "diesel migration runner convention", 0.9),
        );
    } else if snapshot.has_dir_anywhere("migrations") && snapshot.has_file("Cargo.toml") {
        builder.push_fact(
            "migration_runner",
            "cargo migration runner (unresolved tool)".to_string(),
            convention(
                "migrations/",
                "migrations layout without a resolved tool",
                0.6,
            ),
        );
    }
    if snapshot.has_file("Cargo.toml") && cargo.contains("[workspace]") {
        builder.push_fact(
            "workspace",
            "cargo workspace".to_string(),
            convention("Cargo.toml", "Cargo workspace layout", 0.9),
        );
    }
    // Expo: build profiles and file-based routing beyond bare markers.
    if snapshot.has_file("eas.json") {
        builder.push_fact(
            "build_profile",
            "eas".to_string(),
            convention("eas.json", "Expo Application Services build profiles", 0.9),
        );
    }
    if snapshot.has_dir_anywhere("app") && snapshot.has_file("app.json") {
        builder.set_field(
            "framework",
            "expo",
            convention("app/", "Expo file-based routing discovered", 0.85),
            0.85,
        );
    }
}

fn detect_configuration(snapshot: &ProjectSnapshot, builder: &mut ProfileBuilder) {
    // Existing auth integrations are preserved as KEEP candidates later.
    if let Some((path, _)) = snapshot.any_content_contains("GOOGLE_CLIENT_ID") {
        builder.push_fact(
            "auth_provider",
            "google-oauth".to_string(),
            Evidence::new(
                path.to_string(),
                "configuration",
                "Google OAuth client reference configured",
                0.9,
            ),
        );
    } else if let Some((path, _)) = snapshot
        .any_content_contains_insensitive("accounts.google.com")
        .filter(|_| snapshot.any_content_contains_insensitive("oauth").is_some())
    {
        builder.push_fact(
            "auth_provider",
            "google-oauth".to_string(),
            Evidence::new(
                path.to_string(),
                "configuration",
                "Google OAuth endpoint configured",
                0.8,
            ),
        );
    }
    if let Some((path, _)) = snapshot.any_content_contains("GITHUB_CLIENT_ID") {
        builder.push_fact(
            "auth_provider",
            "github-oauth".to_string(),
            Evidence::new(
                path.to_string(),
                "configuration",
                "GitHub OAuth client reference configured",
                0.9,
            ),
        );
    }
    for name in [
        ".env",
        "appsettings.json",
        "application.yml",
        "application.yaml",
        "config.yaml",
    ] {
        let mut found = snapshot.files_with_name(name);
        if name == ".env" {
            found.extend(snapshot.files_with_extension(".env"));
            let mut dotted: Vec<(&str, &str)> = snapshot
                .files_named_anywhere(&[])
                .into_iter()
                .filter(|(p, _)| file_name(p).starts_with(".env."))
                .collect();
            found.append(&mut dotted);
        }
        found.sort_by(|a, b| a.0.cmp(b.0));
        found.dedup_by(|a, b| a.0 == b.0);
        if let Some((path, _)) = found.first() {
            builder.push_fact(
                "configuration",
                file_name(path).to_string(),
                Evidence::new(
                    path.to_string(),
                    "configuration",
                    format!("{} configuration present", file_name(path)),
                    0.8,
                ),
            );
        }
    }
}

fn detect_ci(snapshot: &ProjectSnapshot, builder: &mut ProfileBuilder) {
    let mut ci: Vec<(&str, &str, &str)> = Vec::new();
    if snapshot.has_dir(".github/workflows") {
        if let Some((path, _)) = snapshot
            .files
            .iter()
            .filter(|(p, _)| p.starts_with(".github/workflows/"))
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .min_by(|a, b| a.0.cmp(b.0))
        {
            ci.push((path, "github-actions", "GitHub Actions workflow present"));
        }
    }
    for (name, value, detail) in [
        (
            ".gitlab-ci.yml",
            "gitlab-ci",
            "GitLab CI configuration present",
        ),
        (
            "azure-pipelines.yml",
            "azure-pipelines",
            "Azure Pipelines configuration present",
        ),
        ("Jenkinsfile", "jenkins", "Jenkinsfile present"),
        (
            ".circleci/config.yml",
            "circleci",
            "CircleCI configuration present",
        ),
    ] {
        if snapshot.has_file(name) {
            ci.push((name, value, detail));
        }
    }
    for (path, value, detail) in ci {
        builder.push_fact(
            "ci",
            value.to_string(),
            Evidence::new(path.to_string(), "ci", detail.to_string(), 0.9),
        );
    }
}

fn detect_source_fallback(snapshot: &ProjectSnapshot, builder: &mut ProfileBuilder) {
    // Weak source-extension hints never set a language on their own; they are
    // recorded as candidates so the field stays explicitly unknown.
    if builder.language.value.is_some() {
        return;
    }
    let hints: &[(&str, &str)] = &[
        (".ts", "node"),
        (".tsx", "node"),
        (".js", "node"),
        (".jsx", "node"),
        (".cs", "dotnet"),
        (".py", "python"),
        (".rs", "rust"),
    ];
    for (extension, language) in hints {
        let mut found = snapshot.files_with_extension(extension);
        found.sort_by(|a, b| a.0.cmp(b.0));
        if let Some((path, _)) = found.first() {
            let evidence = Evidence::new(
                path.to_string(),
                "source",
                format!("{extension} source file without a recognized manifest"),
                0.3,
            );
            builder
                .language
                .candidates
                .push(((*language).to_string(), evidence.clone(), 0.3));
            builder.push_evidence(evidence);
        }
    }
}

fn apply_plugin(
    plugin: &dyn InspectorPlugin,
    snapshot: &ProjectSnapshot,
    builder: &mut ProfileBuilder,
) {
    for finding in plugin.detect(snapshot) {
        let mut evidence = finding.evidence;
        evidence.stage = "plugin".to_string();
        evidence.source = format!("plugin:{}:{}", plugin.name(), evidence.source);
        let confidence = finding.confidence.clamp(0.0, 1.0);
        match finding.field.as_str() {
            "language" | "framework" => {
                let (current, current_confidence): (Option<String>, f32) =
                    match finding.field.as_str() {
                        "language" => (builder.language.value.clone(), builder.language.confidence),
                        _ => (
                            builder.framework.value.clone(),
                            builder.framework.confidence,
                        ),
                    };
                match current {
                    None => {
                        // Inference alone (confidence < 1.0) never invents a
                        // value: record the candidate and keep the unknown.
                        let target = match finding.field.as_str() {
                            "language" => &mut builder.language,
                            _ => &mut builder.framework,
                        };
                        target.candidates.push((
                            finding.value.clone(),
                            evidence.clone(),
                            confidence,
                        ));
                        builder.push_evidence(evidence);
                    }
                    Some(current_value) if current_value.eq_ignore_ascii_case(&finding.value) => {
                        builder.push_evidence(evidence);
                    }
                    Some(current_value) => {
                        if confidence >= current_confidence {
                            builder.unknowns.push(Unknown {
                                field: finding.field.clone(),
                                reason: format!(
                                    "conflicting evidence: '{current_value}' vs plugin '{}' hypothesis '{}'; recorded as unknown",
                                    plugin.name(),
                                    finding.value
                                ),
                                evidence_considered: vec![evidence.source.clone()],
                            });
                            let target = match finding.field.as_str() {
                                "language" => &mut builder.language,
                                _ => &mut builder.framework,
                            };
                            target.value = None;
                        } else {
                            builder.push_fact(
                                &format!("candidate_{}", finding.field),
                                finding.value.clone(),
                                evidence,
                            );
                            continue;
                        }
                        builder.push_evidence(Evidence::new(
                            format!("plugin:{}", plugin.name()),
                            "plugin",
                            format!("conflicting hypothesis '{}' recorded", finding.value),
                            confidence,
                        ));
                    }
                }
            }
            other => {
                let name = other.strip_prefix("fact:").unwrap_or(other);
                builder.push_fact(name, finding.value.clone(), evidence);
            }
        }
    }
}

/// Classification of one adoption item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum AdoptionItemKind {
    Keep,
    Adopt,
    OptionalMigration,
}

/// One reviewable recommendation. Generating a plan never applies it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptionItem {
    pub kind: AdoptionItemKind,
    pub subject: String,
    pub rationale: String,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
}

/// Incremental adoption plan for an inspected project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptionPlan {
    #[serde(default)]
    pub items: Vec<AdoptionItem>,
    /// Deterministic digest of the profile this plan was generated from.
    pub profile_digest: String,
    #[serde(default)]
    pub applied: bool,
}

impl AdoptionPlan {
    /// Generates KEEP / ADOPT / OPTIONAL MIGRATION items without touching the
    /// project. Existing integrations become KEEP; platform capabilities
    /// become ADOPT proposals; replacements become OPTIONAL MIGRATION.
    pub fn generate(profile: &ApplicationProfile) -> Self {
        let mut items = Vec::new();
        fn push_item(
            items: &mut Vec<AdoptionItem>,
            kind: AdoptionItemKind,
            subject: &str,
            rationale: &str,
            evidence: Vec<Evidence>,
        ) {
            items.push(AdoptionItem {
                kind,
                subject: subject.to_string(),
                rationale: rationale.to_string(),
                evidence,
            });
        }
        for fact in &profile.facts {
            match fact.name.as_str() {
                "auth_provider" => push_item(
                    &mut items,
                    AdoptionItemKind::Keep,
                    &format!("auth integration ({})", fact.value),
                    "Existing auth is preserved; the platform must not rewrite it.",
                    vec![fact.evidence.clone()],
                ),
                "ci" => push_item(
                    &mut items,
                    AdoptionItemKind::Keep,
                    &format!("CI configuration ({})", fact.value),
                    "Existing CI keeps running; adoption must not replace it silently.",
                    vec![fact.evidence.clone()],
                ),
                "compose" => push_item(
                    &mut items,
                    AdoptionItemKind::Keep,
                    "compose definition",
                    "Existing compose topology is preserved as declared infrastructure.",
                    vec![fact.evidence.clone()],
                ),
                "git" => push_item(
                    &mut items,
                    AdoptionItemKind::Keep,
                    "git history and remotes",
                    "Version history stays authoritative in the source repository.",
                    vec![fact.evidence.clone()],
                ),
                _ => {}
            }
        }
        if profile.container_compatible {
            let evidence = profile
                .evidence
                .iter()
                .find(|e| e.stage == "dockerfile")
                .cloned()
                .into_iter()
                .collect();
            push_item(
                &mut items,
                AdoptionItemKind::Keep,
                "Dockerfile / container definition",
                "Existing container definition is preserved as the runtime contract.",
                evidence,
            );
        }

        for subject in [
            "secrets management",
            "preview environments",
            "deployment pipeline",
            "log aggregation",
        ] {
            push_item(
                &mut items,
                AdoptionItemKind::Adopt,
                subject,
                "Proposed platform capability; requires review and never applies automatically.",
                Vec::new(),
            );
        }

        // Tier 3 framework runners are proposed, never auto-run: migration
        // and test commands execute only through reviewed jobs under
        // approval gates.
        for fact in &profile.facts {
            match fact.name.as_str() {
                "migration_runner" => push_item(
                    &mut items,
                    AdoptionItemKind::Adopt,
                    &format!("database migrations via {}", fact.value),
                    "Proposed migration command from detected framework layout; requires review and never runs automatically.",
                    vec![fact.evidence.clone()],
                ),
                "test_runner" => push_item(
                    &mut items,
                    AdoptionItemKind::Adopt,
                    &format!("test suite via {}", fact.value),
                    "Proposed test command from detected framework layout; requires review and never runs automatically.",
                    vec![fact.evidence.clone()],
                ),
                _ => {}
            }
        }

        items.push(AdoptionItem {
            kind: AdoptionItemKind::OptionalMigration,
            subject: "storage replacement / framework migration".to_string(),
            rationale: "Presented with affected evidence; never applied without explicit approval."
                .to_string(),
            evidence: profile.evidence.clone(),
        });

        Self {
            items,
            profile_digest: profile_digest(profile),
            applied: false,
        }
    }

    /// Explicit, incremental application. Fails without approval; even with
    /// approval it only marks the plan record — the project itself is changed
    /// through reviewed follow-up actions, never by generation.
    pub fn apply(&mut self, approval: &ExplicitApproval) -> Result<()> {
        if !approval.approved {
            return Err(CoreError::ApprovalRequired(
                "adoption plan requires explicit approval before apply".to_string(),
            ));
        }
        if approval.approver.trim().is_empty() {
            return Err(CoreError::ApprovalRequired(
                "adoption plan approval must name an approver".to_string(),
            ));
        }
        self.applied = true;
        Ok(())
    }

    /// Canonical JSON serialization.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(CoreError::from)
    }

    /// Strict JSON deserialization.
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(CoreError::from)
    }

    /// Items of one kind, in plan order.
    pub fn items_of_kind(&self, kind: AdoptionItemKind) -> Vec<&AdoptionItem> {
        self.items.iter().filter(|i| i.kind == kind).collect()
    }
}

/// Explicit human approval for applying a proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplicitApproval {
    pub approved: bool,
    pub approver: String,
    pub proposal_summary: String,
}

impl ExplicitApproval {
    pub fn granted(approver: impl Into<String>, proposal_summary: impl Into<String>) -> Self {
        Self {
            approved: true,
            approver: approver.into(),
            proposal_summary: proposal_summary.into(),
        }
    }

    pub fn denied(approver: impl Into<String>, proposal_summary: impl Into<String>) -> Self {
        Self {
            approved: false,
            approver: approver.into(),
            proposal_summary: proposal_summary.into(),
        }
    }
}

/// Required inspect → understand → model → propose → apply sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ChangeStep {
    Inspect,
    Understand,
    Model,
    Propose,
    Apply,
}

impl ChangeStep {
    fn predecessors(self) -> &'static [ChangeStep] {
        match self {
            ChangeStep::Inspect => &[],
            ChangeStep::Understand => &[ChangeStep::Inspect],
            ChangeStep::Model => &[ChangeStep::Inspect, ChangeStep::Understand],
            ChangeStep::Propose => &[
                ChangeStep::Inspect,
                ChangeStep::Understand,
                ChangeStep::Model,
            ],
            ChangeStep::Apply => &[
                ChangeStep::Inspect,
                ChangeStep::Understand,
                ChangeStep::Model,
                ChangeStep::Propose,
            ],
        }
    }
}

/// Gate enforcing the broad-change sequence for imported projects.
pub struct BroadChangeGate;

impl BroadChangeGate {
    /// Fails with [`CoreError::SequenceViolation`] when any predecessor of
    /// `requested` is missing from `completed`.
    pub fn check(completed: &[ChangeStep], requested: ChangeStep) -> Result<()> {
        let missing: Vec<ChangeStep> = requested
            .predecessors()
            .iter()
            .copied()
            .filter(|step| !completed.contains(step))
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(CoreError::SequenceViolation(format!(
                "broad changes require inspect → understand → model → propose → apply; missing {missing:?} before {requested:?}"
            )))
        }
    }
}

/// Deterministic FNV-1a digest over the profile's determining fields.
fn profile_digest(profile: &ApplicationProfile) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut mix = |bytes: &[u8]| {
        for b in bytes {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    mix(profile.container_compatible.to_string().as_bytes());
    mix(profile.language.as_deref().unwrap_or("?").as_bytes());
    mix(profile.framework.as_deref().unwrap_or("?").as_bytes());
    let mut facts: Vec<(&str, &str)> = profile
        .facts
        .iter()
        .map(|f| (f.name.as_str(), f.value.as_str()))
        .collect();
    facts.sort();
    for (name, value) in facts {
        mix(name.as_bytes());
        mix(value.as_bytes());
    }
    format!("{hash:016x}")
}
