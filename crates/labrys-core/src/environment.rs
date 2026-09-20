use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{ApplicationId, EnvironmentId};

/// Operational environment kind. Every application supports development,
/// preview instances, and production.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum EnvironmentKind {
    Development,
    /// An isolated preview instance; `name` distinguishes parallel previews.
    Preview {
        name: String,
    },
    Production,
}

impl EnvironmentKind {
    pub fn is_preview(&self) -> bool {
        matches!(self, Self::Preview { .. })
    }
}

/// Environment-scoped references.
///
/// Later changes (runtime, capability, deployment) resolve these identifiers.
/// The foundation guarantees isolation: each environment owns its maps, so a
/// preview binding change cannot mutate production.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentRefs {
    #[serde(default)]
    pub runtime: BTreeMap<String, String>,
    #[serde(default)]
    pub capabilities: BTreeMap<String, String>,
    #[serde(default)]
    pub resources: BTreeMap<String, String>,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    #[serde(default)]
    pub deployments: BTreeMap<String, String>,
    #[serde(default)]
    pub domains: BTreeMap<String, String>,
}

/// One operational environment of an application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub id: EnvironmentId,
    pub application_id: ApplicationId,
    pub kind: EnvironmentKind,
    /// Human-readable name (`development`, `production`, or preview name).
    pub name: String,
    #[serde(default)]
    pub refs: EnvironmentRefs,
}

impl Environment {
    pub fn new(
        application_id: ApplicationId,
        kind: EnvironmentKind,
        name: impl Into<String>,
    ) -> Self {
        Self {
            id: EnvironmentId::new(),
            application_id,
            kind,
            name: name.into(),
            refs: EnvironmentRefs::default(),
        }
    }

    pub fn development(application_id: ApplicationId) -> Self {
        Self::new(application_id, EnvironmentKind::Development, "development")
    }

    pub fn production(application_id: ApplicationId) -> Self {
        Self::new(application_id, EnvironmentKind::Production, "production")
    }

    pub fn preview(application_id: ApplicationId, name: impl Into<String>) -> Self {
        let name = name.into();
        Self::new(
            application_id,
            EnvironmentKind::Preview { name: name.clone() },
            name,
        )
    }
}
