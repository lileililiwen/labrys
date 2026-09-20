use serde::{Deserialize, Serialize};

use crate::application::Application;
use crate::environment::Environment;
use crate::error::{CoreError, Result};
use crate::origin::Origin;
use crate::state::DesiredState;

/// Version of the portable manifest schema implemented by this crate.
pub const MANIFEST_VERSION: u32 = 1;

/// Portable `labrys.yaml` manifest.
///
/// Optional: imported applications operate without this file in their
/// repository; the platform database remains authoritative and can later
/// export a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub manifest_version: u32,
    pub name: String,
    pub origin: Origin,
    #[serde(default)]
    pub environments: Vec<ManifestEnvironment>,
    #[serde(default)]
    pub desired_state: Option<DesiredState>,
}

/// Portable environment entry inside a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestEnvironment {
    pub name: String,
    #[serde(default = "default_kind")]
    pub kind: String,
}

fn default_kind() -> String {
    "development".to_string()
}

impl Manifest {
    /// Exports an application to its portable manifest form.
    pub fn from_application(app: &Application) -> Self {
        Self {
            manifest_version: MANIFEST_VERSION,
            name: app.name.clone(),
            origin: app.origin.clone(),
            environments: app
                .environments
                .iter()
                .map(|env| ManifestEnvironment {
                    name: env.name.clone(),
                    kind: match &env.kind {
                        crate::environment::EnvironmentKind::Development => {
                            "development".to_string()
                        }
                        crate::environment::EnvironmentKind::Production => "production".to_string(),
                        crate::environment::EnvironmentKind::Preview { .. } => {
                            "preview".to_string()
                        }
                    },
                })
                .collect(),
            desired_state: Some(app.desired_state.clone()),
        }
    }

    /// Serializes the manifest to `labrys.yaml` YAML text.
    pub fn to_yaml(&self) -> Result<String> {
        serde_yaml::to_string(self).map_err(CoreError::from)
    }

    /// Parses `labrys.yaml` YAML text with strict schema and version checks.
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        let manifest: Self = serde_yaml::from_str(yaml).map_err(CoreError::from)?;
        if manifest.manifest_version != MANIFEST_VERSION {
            return Err(CoreError::UnsupportedVersion {
                found: manifest.manifest_version,
                expected: MANIFEST_VERSION,
            });
        }
        if manifest.name.trim().is_empty() {
            return Err(CoreError::InvalidManifest(
                "manifest name must not be empty".to_string(),
            ));
        }
        Ok(manifest)
    }

    /// Builds an application from an optional manifest.
    ///
    /// `None` (no `labrys.yaml` in the repository) yields a default
    /// application whose database state is the source of truth.
    pub fn import_application(
        manifest: Option<&Manifest>,
        fallback_name: impl Into<String>,
        fallback_origin: Origin,
        created_by: impl Into<String>,
    ) -> Application {
        let created_by = created_by.into();
        match manifest {
            None => Application::new(fallback_name, fallback_origin, created_by),
            Some(m) => {
                let mut app = Application::new(m.name.clone(), m.origin.clone(), created_by);
                if let Some(desired) = &m.desired_state {
                    // Imported desired state keeps its version history.
                    app.desired_state = desired.clone();
                    app.observed_state =
                        crate::state::ObservedState::unknown(app.desired_state.version);
                }
                for entry in &m.environments {
                    if app.environment_by_name(&entry.name).is_none() {
                        let kind = match entry.kind.as_str() {
                            "production" => crate::environment::EnvironmentKind::Production,
                            "preview" => crate::environment::EnvironmentKind::Preview {
                                name: entry.name.clone(),
                            },
                            _ => crate::environment::EnvironmentKind::Development,
                        };
                        let env = Environment::new(app.id, kind, entry.name.clone());
                        app.environments.push(env);
                    }
                }
                app
            }
        }
    }
}

impl Application {
    /// Exports this application as `labrys.yaml` YAML text.
    pub fn export_manifest_yaml(&self) -> Result<String> {
        Manifest::from_application(self).to_yaml()
    }

    /// Imports an application from optional `labrys.yaml` YAML text.
    ///
    /// `None` means the repository has no manifest; the database state is
    /// the source of truth.
    pub fn import_manifest_yaml(
        yaml: Option<&str>,
        fallback_name: impl Into<String>,
        fallback_origin: Origin,
        created_by: impl Into<String>,
    ) -> Result<Self> {
        match yaml {
            None => Ok(Manifest::import_application(
                None,
                fallback_name,
                fallback_origin,
                created_by,
            )),
            Some(text) => {
                let manifest = Manifest::from_yaml(text)?;
                Ok(Manifest::import_application(
                    Some(&manifest),
                    fallback_name,
                    fallback_origin,
                    created_by,
                ))
            }
        }
    }
}
