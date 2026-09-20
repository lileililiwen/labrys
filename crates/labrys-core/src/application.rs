use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::environment::Environment;
use crate::ids::ApplicationId;
use crate::origin::Origin;
use crate::state::{DesiredState, ObservedState};

/// Lifecycle status of an application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ApplicationStatus {
    Draft,
    Active,
    Archived,
}

/// Who created / last modified the record. Never contains secret values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditMetadata {
    pub created_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
    /// Stable audit event identifiers, newest last.
    #[serde(default)]
    pub audit_event_ids: Vec<String>,
}

/// Stable identifiers for lifecycle collections owned by the application.
///
/// The foundation tracks identity only; later changes define the payloads
/// (workspaces, sessions, capabilities, resources, providers, secrets,
/// previews, deployments, domains, jobs, artifacts, logs, audit events).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleCollections {
    #[serde(default)]
    pub workspaces: Vec<String>,
    #[serde(default)]
    pub agent_sessions: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub resources: Vec<String>,
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub previews: Vec<String>,
    #[serde(default)]
    pub deployments: Vec<String>,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub jobs: Vec<String>,
    #[serde(default)]
    pub artifacts: Vec<String>,
    #[serde(default)]
    pub logs: Vec<String>,
    #[serde(default)]
    pub audit_events: Vec<String>,
}

/// Canonical application record: the aggregate root of the platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Application {
    pub id: ApplicationId,
    pub name: String,
    pub origin: Origin,
    pub status: ApplicationStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub audit: AuditMetadata,
    #[serde(default)]
    pub environments: Vec<Environment>,
    pub desired_state: DesiredState,
    pub observed_state: ObservedState,
    #[serde(default)]
    pub collections: LifecycleCollections,
}

impl Application {
    /// Creates an application with development and production environments.
    ///
    /// Works without any `labrys.yaml` manifest; the database record is the
    /// source of truth for imported projects.
    pub fn new(name: impl Into<String>, origin: Origin, created_by: impl Into<String>) -> Self {
        let id = ApplicationId::new();
        let now = Utc::now();
        let desired_state = DesiredState::initial();
        Self {
            id,
            name: name.into(),
            origin,
            status: ApplicationStatus::Draft,
            created_at: now,
            updated_at: now,
            audit: AuditMetadata {
                created_by: created_by.into(),
                updated_by: None,
                audit_event_ids: Vec::new(),
            },
            environments: vec![Environment::development(id), Environment::production(id)],
            observed_state: ObservedState::unknown(desired_state.version),
            desired_state,
            collections: LifecycleCollections::default(),
        }
    }

    /// Records a desired-state update and refreshes lifecycle timestamps.
    pub fn update_desired_state(&mut self, next: DesiredState, updated_by: impl Into<String>) {
        self.desired_state = next;
        self.touch(updated_by);
    }

    /// Records an observation without mutating desired state.
    pub fn record_observation(&mut self, observed: ObservedState) {
        self.observed_state = observed;
        self.updated_at = Utc::now();
    }

    /// Adds an isolated preview environment.
    pub fn add_preview_environment(&mut self, name: impl Into<String>) -> &Environment {
        let env = Environment::preview(self.id, name);
        self.environments.push(env);
        let now = Utc::now();
        self.updated_at = now;
        self.environments.last().expect("just pushed")
    }

    /// Finds an environment by its human-readable name.
    pub fn environment_by_name(&self, name: &str) -> Option<&Environment> {
        self.environments.iter().find(|env| env.name == name)
    }

    /// Finds a mutable environment by its human-readable name.
    pub fn environment_by_name_mut(&mut self, name: &str) -> Option<&mut Environment> {
        self.environments.iter_mut().find(|env| env.name == name)
    }

    /// Serializes to canonical JSON.
    pub fn to_json(&self) -> Result<String, crate::CoreError> {
        serde_json::to_string_pretty(self).map_err(crate::CoreError::from)
    }

    /// Deserializes canonical JSON with strict schema enforcement.
    pub fn from_json(json: &str) -> Result<Self, crate::CoreError> {
        serde_json::from_str(json).map_err(crate::CoreError::from)
    }

    fn touch(&mut self, updated_by: impl Into<String>) {
        self.updated_at = Utc::now();
        self.audit.updated_by = Some(updated_by.into());
    }
}
