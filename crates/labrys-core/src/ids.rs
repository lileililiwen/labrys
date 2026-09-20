use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identifier for an [`crate::Application`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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

/// Stable identifier for a [`crate::capability::BoundCapability`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BindingId(Uuid);

impl BindingId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for BindingId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for BindingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bnd_{}", self.0.simple())
    }
}

impl FromStr for BindingId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s
            .strip_prefix("bnd_")
            .ok_or_else(|| format!("invalid binding id: {s}"))?;
        Uuid::parse_str(hex)
            .map(Self)
            .map_err(|_| format!("invalid binding id: {s}"))
    }
}

/// Stable identifier for a [`crate::capability::ResourceRef`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResourceId(Uuid);

impl ResourceId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for ResourceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "res_{}", self.0.simple())
    }
}

impl FromStr for ResourceId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s
            .strip_prefix("res_")
            .ok_or_else(|| format!("invalid resource id: {s}"))?;
        Uuid::parse_str(hex)
            .map(Self)
            .map_err(|_| format!("invalid resource id: {s}"))
    }
}

/// Stable identifier for a [`crate::controller::Job`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(Uuid);

impl JobId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "job_{}", self.0.simple())
    }
}

impl FromStr for JobId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s
            .strip_prefix("job_")
            .ok_or_else(|| format!("invalid job id: {s}"))?;
        Uuid::parse_str(hex)
            .map(Self)
            .map_err(|_| format!("invalid job id: {s}"))
    }
}

/// Stable identifier for a [`crate::deployment::Deployment`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeploymentId(Uuid);

impl DeploymentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for DeploymentId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for DeploymentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "dep_{}", self.0.simple())
    }
}

impl FromStr for DeploymentId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s
            .strip_prefix("dep_")
            .ok_or_else(|| format!("invalid deployment id: {s}"))?;
        Uuid::parse_str(hex)
            .map(Self)
            .map_err(|_| format!("invalid deployment id: {s}"))
    }
}

/// Stable identifier for a [`crate::deployment::Domain`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DomainId(Uuid);

impl DomainId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for DomainId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for DomainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "dom_{}", self.0.simple())
    }
}

impl FromStr for DomainId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s
            .strip_prefix("dom_")
            .ok_or_else(|| format!("invalid domain id: {s}"))?;
        Uuid::parse_str(hex)
            .map(Self)
            .map_err(|_| format!("invalid domain id: {s}"))
    }
}
