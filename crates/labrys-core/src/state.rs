use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Versioned description of what the application should look like.
///
/// Stored separately from [`ObservedState`] so future controllers can
/// reconcile without letting an agent claim an operation completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesiredState {
    /// Monotonic version; incremented on every desired-state update.
    pub version: u64,
    /// Portable profile overlays keyed by profile name (e.g. `development`).
    #[serde(default)]
    pub profiles: BTreeMap<String, serde_json::Value>,
    /// Opaque desired configuration payload.
    #[serde(default)]
    pub config: serde_json::Value,
}

/// Independently observed actual state of the application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedState {
    /// Desired version this observation was taken against.
    pub desired_version: u64,
    pub status: ObservedStatus,
    pub last_observed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Health of the observed state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservedStatus {
    Unknown,
    Healthy,
    Degraded,
    Unhealthy,
}

impl DesiredState {
    pub fn initial() -> Self {
        Self {
            version: 1,
            profiles: BTreeMap::new(),
            config: serde_json::Value::Null,
        }
    }

    /// Returns the next version with `config` replaced.
    pub fn with_config(&self, config: serde_json::Value) -> Self {
        Self {
            version: self.version + 1,
            profiles: self.profiles.clone(),
            config,
        }
    }
}

impl ObservedState {
    pub fn unknown(desired_version: u64) -> Self {
        Self {
            desired_version,
            status: ObservedStatus::Unknown,
            last_observed_at: Utc::now(),
            detail: None,
        }
    }

    /// True when the observation lags behind the current desired version.
    pub fn is_stale(&self, desired: &DesiredState) -> bool {
        self.desired_version != desired.version
    }
}
