use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identifier for an [`crate::Application`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApplicationId(Uuid);

/// Stable identifier for an [`crate::Environment`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EnvironmentId(Uuid);

impl ApplicationId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for ApplicationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ApplicationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "app_{}", self.0.simple())
    }
}

impl FromStr for ApplicationId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s
            .strip_prefix("app_")
            .ok_or_else(|| format!("invalid application id: {s}"))?;
        Uuid::parse_str(hex)
            .map(Self)
            .map_err(|_| format!("invalid application id: {s}"))
    }
}

/// Stable identifier for an agent [`crate::agent::AgentSession`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentSessionId(Uuid);

impl AgentSessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for AgentSessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for AgentSessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "agt_{}", self.0.simple())
    }
}

impl FromStr for AgentSessionId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s
            .strip_prefix("agt_")
            .ok_or_else(|| format!("invalid agent session id: {s}"))?;
        Uuid::parse_str(hex)
            .map(Self)
            .map_err(|_| format!("invalid agent session id: {s}"))
    }
}

impl EnvironmentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for EnvironmentId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for EnvironmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "env_{}", self.0.simple())
    }
}

impl FromStr for EnvironmentId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s
            .strip_prefix("env_")
            .ok_or_else(|| format!("invalid environment id: {s}"))?;
        Uuid::parse_str(hex)
            .map(Self)
            .map_err(|_| format!("invalid environment id: {s}"))
    }
}

/// Stable identifier for a session [`crate::workspace::SessionWorkspace`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkspaceId(Uuid);

impl WorkspaceId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for WorkspaceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ws_{}", self.0.simple())
    }
}

impl FromStr for WorkspaceId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s
            .strip_prefix("ws_")
            .ok_or_else(|| format!("invalid workspace id: {s}"))?;
        Uuid::parse_str(hex)
            .map(Self)
            .map_err(|_| format!("invalid workspace id: {s}"))
    }
}

/// Stable identifier for a [`crate::workspace::WebPreview`] or
/// [`crate::workspace::ExpoShare`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PreviewId(Uuid);

impl PreviewId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for PreviewId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PreviewId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "prv_{}", self.0.simple())
    }
}

impl FromStr for PreviewId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s
            .strip_prefix("prv_")
            .ok_or_else(|| format!("invalid preview id: {s}"))?;
        Uuid::parse_str(hex)
            .map(Self)
            .map_err(|_| format!("invalid preview id: {s}"))
    }
}

/// Stable identifier for a [`crate::workspace::Feedback`] record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FeedbackId(Uuid);

impl FeedbackId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for FeedbackId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for FeedbackId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fb_{}", self.0.simple())
    }
}

impl FromStr for FeedbackId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s
            .strip_prefix("fb_")
            .ok_or_else(|| format!("invalid feedback id: {s}"))?;
        Uuid::parse_str(hex)
            .map(Self)
            .map_err(|_| format!("invalid feedback id: {s}"))
    }
}
