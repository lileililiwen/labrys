//! Durable capabilities, providers, bindings, resources, and secrets.
//!
//! Covers requirement `capability-platform`: Capability (what an application
//! needs), Provider (who supplies it), and Binding (how it is connected) are
//! separate versioned objects; every capability offers a generic binding when a
//! portable contract is possible; managed, external, and adopted modes are
//! explicit with auditable, reversible transitions; secret values are sealed at
//! rest, redacted from events and logs, and injected only into authorized
//! operations.
//!
//! This module is pure and deterministic. [`SecretStore`] seals values with
//! ChaCha20-Poly1305 under an external [`MasterKey`] (key material arrives
//! from process configuration and never sits beside ciphertext, mirroring
//! encrypted PostgreSQL with the key outside the database); authentication
//! failures surface as verification failures with recovery, never plaintext.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Nonce,
};
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::agent::SecretReference;
use crate::error::{CoreError, Result};
use crate::ids::{ApplicationId, BindingId, ResourceId};
use crate::inspector::ExplicitApproval;

/// Version of the capability contract vocabulary this crate implements.
pub const CAPABILITY_CONTRACT_VERSION: u32 = 1;

/// How a portable environment value is sourced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum EnvSource {
    /// Fixed literal value.
    Literal(String),
    /// Value comes from a sealed secret under this reference id.
    SecretRef(String),
    /// Value comes from a field of the bound resource.
    ResourceField(String),
}

/// Portable environment contract of a binding: variable name to source.
pub type EnvContract = BTreeMap<String, EnvSource>;

/// Stable product contract an application needs, e.g. `database.postgres`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub key: String,
    pub version: u32,
    /// Keys of capabilities that must also be bound for this one to resolve.
    #[serde(default)]
    pub dependencies: Vec<String>,
}

impl Capability {
    pub fn new(key: impl Into<String>, version: u32) -> Self {
        Self {
            key: key.into(),
            version,
            dependencies: Vec::new(),
        }
    }

    pub fn with_dependencies(mut self, dependencies: Vec<String>) -> Self {
        self.dependencies = dependencies;
        self
    }
}

/// Who supplies a capability, e.g. `postgres.managed`. Provider identity is
/// independent of any framework integration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub key: String,
    /// Capability key this provider supplies.
    pub capability: String,
    pub version: u32,
    /// True when the platform can hand a resource back to its original owner
    /// (adopted → external). Adoption of providers without reversal is
    /// permanent until manually migrated.
    pub supports_reversal: bool,
    /// Modes this provider can operate in.
    #[serde(default)]
    pub modes: Vec<CapabilityMode>,
}

impl Provider {
    pub fn new(
        key: impl Into<String>,
        capability: impl Into<String>,
        version: u32,
        supports_reversal: bool,
        modes: Vec<CapabilityMode>,
    ) -> Self {
        Self {
            key: key.into(),
            capability: capability.into(),
            version,
            supports_reversal,
            modes,
        }
    }

    pub fn supplies(&self, capability_key: &str) -> bool {
        self.capability == capability_key
    }

    pub fn supports_mode(&self, mode: &CapabilityMode) -> bool {
        self.modes.contains(mode)
    }
}

/// Explicit operating mode of a bound capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum CapabilityMode {
    /// Platform creates and operates the resource.
    Managed,
    /// Customer-owned resource; platform only connects to it.
    External,
    /// Pre-existing resource the platform now operates without recreating.
    Adopted,
}

/// How a binding connects a capability/provider pair to a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum BindingKind {
    /// Framework-aware deep binding, e.g. EF Core or SQLx.
    Framework { framework: String },
    /// Portable environment contract; the mandatory fallback.
    Generic,
}

/// Connects a capability to a provider for one project. Separate versioned
/// object from both the capability and the provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub key: String,
    pub capability: String,
    pub provider: String,
    pub version: u32,
    pub kind: BindingKind,
    pub env: EnvContract,
}

impl Binding {
    pub fn new(
        key: impl Into<String>,
        capability: impl Into<String>,
        provider: impl Into<String>,
        version: u32,
        kind: BindingKind,
        env: EnvContract,
    ) -> Self {
        Self {
            key: key.into(),
            capability: capability.into(),
            provider: provider.into(),
            version,
            kind,
            env,
        }
    }

    pub fn is_generic(&self) -> bool {
        self.kind == BindingKind::Generic
    }

    pub fn framework(&self) -> Option<&str> {
        match &self.kind {
            BindingKind::Framework { framework } => Some(framework),
            BindingKind::Generic => None,
        }
    }

    /// Secret reference ids this binding injects into the runtime.
    pub fn secret_refs(&self) -> Vec<&str> {
        self.env
            .values()
            .filter_map(|source| match source {
                EnvSource::SecretRef(id) => Some(id.as_str()),
                _ => None,
            })
            .collect()
    }
}

/// Operational entity a binding points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceRef {
    pub id: ResourceId,
    pub capability: String,
    pub provider: String,
    /// Opaque handle owned by the provider (connection string host, bucket
    /// name, ...). Adoption never recreates or deletes it.
    pub external_ref: String,
}

impl ResourceRef {
    pub fn new(
        capability: impl Into<String>,
        provider: impl Into<String>,
        external_ref: impl Into<String>,
    ) -> Self {
        Self {
            id: ResourceId::new(),
            capability: capability.into(),
            provider: provider.into(),
            external_ref: external_ref.into(),
        }
    }
}

/// Selects the binding that connects `capability` through `provider` for a
/// project using `framework`.
///
/// A framework binding matching `framework` wins; otherwise the generic
/// binding is the fallback, so a project is never blocked by the absence of a
/// deep binding. Fails with [`CoreError::CapabilityBinding`] on provider
/// mismatch, missing bindings, or binding conflicts.
pub fn resolve_binding<'a>(
    bindings: &'a [Binding],
    providers: &'a [Provider],
    capability_key: &str,
    provider_key: &str,
    framework: Option<&str>,
) -> Result<&'a Binding> {
    let provider = providers
        .iter()
        .find(|p| p.key == provider_key)
        .ok_or_else(|| {
            CoreError::CapabilityBinding(format!("unknown provider '{provider_key}'"))
        })?;
    if !provider.supplies(capability_key) {
        return Err(CoreError::CapabilityBinding(format!(
            "provider '{provider_key}' supplies '{}', not '{capability_key}'",
            provider.capability
        )));
    }
    let candidates: Vec<&Binding> = bindings
        .iter()
        .filter(|b| b.capability == capability_key && b.provider == provider_key)
        .collect();
    if candidates.is_empty() {
        return Err(CoreError::CapabilityBinding(format!(
            "no binding connects capability '{capability_key}' to provider '{provider_key}'"
        )));
    }
    if let Some(framework) = framework {
        let deep: Vec<&Binding> = candidates
            .iter()
            .copied()
            .filter(|b| b.framework() == Some(framework))
            .collect();
        match deep.len() {
            0 => {}
            1 => return Ok(deep[0]),
            _ => {
                return Err(CoreError::CapabilityBinding(format!(
                    "binding conflict: {} bindings for framework '{framework}' on '{capability_key}/{provider_key}'",
                    deep.len()
                )))
            }
        }
    }
    let generic: Vec<&Binding> = candidates
        .iter()
        .copied()
        .filter(|b| b.is_generic())
        .collect();
    match generic.len() {
        1 => Ok(generic[0]),
        0 => Err(CoreError::CapabilityBinding(format!(
            "no generic binding for '{capability_key}/{provider_key}' and no framework binding matches {framework:?}"
        ))),
        _ => Err(CoreError::CapabilityBinding(format!(
            "binding conflict: {} generic bindings for '{capability_key}/{provider_key}'",
            generic.len()
        ))),
    }
}

/// Portable generic binding contract for PostgreSQL: one `DATABASE_URL`
/// secret reference.
pub fn generic_postgres_binding(provider_key: &str) -> Binding {
    let mut env = EnvContract::new();
    env.insert(
        "DATABASE_URL".to_string(),
        EnvSource::SecretRef(format!("{provider_key}.database_url")),
    );
    Binding::new(
        "postgres.generic",
        "database.postgres",
        provider_key,
        1,
        BindingKind::Generic,
        env,
    )
}

/// Portable generic binding contract for object storage.
pub fn generic_object_storage_binding(provider_key: &str) -> Binding {
    let mut env = EnvContract::new();
    env.insert(
        "OBJECT_STORAGE_URL".to_string(),
        EnvSource::ResourceField("endpoint".to_string()),
    );
    env.insert(
        "OBJECT_STORAGE_BUCKET".to_string(),
        EnvSource::ResourceField("bucket".to_string()),
    );
    env.insert(
        "OBJECT_STORAGE_ACCESS_KEY".to_string(),
        EnvSource::SecretRef(format!("{provider_key}.access_key")),
    );
    env.insert(
        "OBJECT_STORAGE_SECRET_KEY".to_string(),
        EnvSource::SecretRef(format!("{provider_key}.secret_key")),
    );
    Binding::new(
        "object-storage.generic",
        "storage.object",
        provider_key,
        1,
        BindingKind::Generic,
        env,
    )
}

/// Portable generic binding contract for auth.
pub fn generic_auth_binding(provider_key: &str) -> Binding {
    let mut env = EnvContract::new();
    env.insert(
        "AUTH_URL".to_string(),
        EnvSource::ResourceField("issuer".to_string()),
    );
    env.insert(
        "AUTH_SECRET_KEY".to_string(),
        EnvSource::SecretRef(format!("{provider_key}.signing_key")),
    );
    Binding::new(
        "auth.generic",
        "auth",
        provider_key,
        1,
        BindingKind::Generic,
        env,
    )
}

/// Portable generic binding contract for file storage.
pub fn generic_file_storage_binding(provider_key: &str) -> Binding {
    let mut env = EnvContract::new();
    env.insert(
        "FILE_STORAGE_URL".to_string(),
        EnvSource::ResourceField("root".to_string()),
    );
    env.insert(
        "FILE_STORAGE_TOKEN".to_string(),
        EnvSource::SecretRef(format!("{provider_key}.token")),
    );
    Binding::new(
        "file-storage.generic",
        "storage.file",
        provider_key,
        1,
        BindingKind::Generic,
        env,
    )
}

/// One auditable mode transition on a bound capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModeTransition {
    pub from: CapabilityMode,
    pub to: CapabilityMode,
    /// Named human approver; required for every transition.
    pub approved_by: String,
    pub at: DateTime<Utc>,
    /// True when this transition can be reversed by the provider.
    pub reversible: bool,
    /// Adoption source description (e.g. `external:prod-db`). Recorded only
    /// when entering Adopted mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl ModeTransition {
    /// Renders the audit/event detail. Contains no secret values.
    pub fn to_event_detail(&self) -> String {
        let source = self
            .source
            .as_deref()
            .map(|s| format!(" source: {s}"))
            .unwrap_or_default();
        format!(
            "mode {:?} -> {:?} approved by {}{} reversible: {}",
            self.from, self.to, self.approved_by, source, self.reversible
        )
    }
}

/// A capability bound to one application environment: the durable record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundCapability {
    pub id: BindingId,
    pub application_id: ApplicationId,
    pub environment_name: String,
    pub capability: String,
    pub provider: String,
    pub binding: String,
    pub mode: CapabilityMode,
    pub resource: ResourceRef,
    /// Full transition history, oldest first.
    #[serde(default)]
    pub history: Vec<ModeTransition>,
}

impl BoundCapability {
    /// Binds a capability in its initial mode. Every mode change goes through
    /// [`BoundCapability::transition`], so the initial record is the only
    /// un-audited entry and names its author.
    #[allow(clippy::too_many_arguments)]
    pub fn bind(
        application_id: ApplicationId,
        environment_name: impl Into<String>,
        binding: &Binding,
        provider: &Provider,
        mode: CapabilityMode,
        resource: ResourceRef,
        actor: impl Into<String>,
        at: DateTime<Utc>,
    ) -> Result<Self> {
        if binding.capability != resource.capability || binding.provider != resource.provider {
            return Err(CoreError::CapabilityBinding(format!(
                "resource {} does not match binding '{}/{}'",
                resource.id, binding.capability, binding.provider
            )));
        }
        if !provider.supplies(&binding.capability) || provider.key != binding.provider {
            return Err(CoreError::CapabilityBinding(format!(
                "provider '{}' does not supply binding '{}/{}'",
                provider.key, binding.capability, binding.provider
            )));
        }
        if !provider.supports_mode(&mode) {
            return Err(CoreError::CapabilityBinding(format!(
                "provider '{}' does not operate in mode {:?}",
                provider.key, mode
            )));
        }
        Ok(Self {
            id: BindingId::new(),
            application_id,
            environment_name: environment_name.into(),
            capability: binding.capability.clone(),
            provider: binding.provider.clone(),
            binding: binding.key.clone(),
            mode,
            resource,
            history: vec![ModeTransition {
                from: mode,
                to: mode,
                approved_by: actor.into(),
                at,
                reversible: provider.supports_reversal,
                source: None,
            }],
        })
    }

    /// Moves to `next` mode. Requires a granted, named human approval, records
    /// the transition, and rejects reversals the provider cannot support.
    /// Entering Adopted keeps the existing external resource untouched and
    /// records its source.
    pub fn transition(
        &mut self,
        next: CapabilityMode,
        provider: &Provider,
        approval: &ExplicitApproval,
        at: DateTime<Utc>,
    ) -> Result<&ModeTransition> {
        if !approval.approved {
            return Err(CoreError::ApprovalRequired(format!(
                "capability '{}' mode change to {:?} requires explicit approval",
                self.capability, next
            )));
        }
        if approval.approver.trim().is_empty() {
            return Err(CoreError::ApprovalRequired(
                "mode transition approval must name an approver".to_string(),
            ));
        }
        if next == self.mode {
            return Err(CoreError::CapabilityBinding(format!(
                "capability '{}' is already in mode {:?}",
                self.capability, next
            )));
        }
        if !provider.supports_mode(&next) {
            return Err(CoreError::CapabilityBinding(format!(
                "provider '{}' cannot operate in mode {:?}",
                provider.key, next
            )));
        }
        let reversing = self.mode == CapabilityMode::Adopted && next != CapabilityMode::Adopted;
        if reversing && !provider.supports_reversal {
            return Err(CoreError::CapabilityBinding(format!(
                "provider '{}' does not support reversal out of Adopted",
                provider.key
            )));
        }
        let source = if next == CapabilityMode::Adopted {
            Some(format!("{}:{}", self.provider, self.resource.external_ref))
        } else {
            None
        };
        self.mode = next;
        self.history.push(ModeTransition {
            from: self.history.last().expect("history is never empty").to,
            to: next,
            approved_by: approval.approver.clone(),
            at,
            reversible: provider.supports_reversal,
            source,
        });
        Ok(self.history.last().expect("transition was just pushed"))
    }

    /// Renders the durable record for events/logs. Never contains secret
    /// values; secrets stay behind [`EnvSource::SecretRef`] ids.
    pub fn to_event_detail(&self) -> String {
        format!(
            "capability '{}' via provider '{}' binding '{}' in mode {:?} (resource {})",
            self.capability, self.provider, self.binding, self.mode, self.resource.id
        )
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(CoreError::from)
    }

    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(CoreError::from)
    }
}

/// Versioned cipher envelope vocabulary for sealed secrets.
///
/// `0` is the retired deterministic XOR stand-in: readers still dispatch on
/// it so pre-hardening envelopes stay openable until their reference is
/// rotated, but writers never mint it. `1` is the current ChaCha20-Poly1305
/// envelope with a fresh random nonce per seal.
pub const SECRET_ENVELOPE_LEGACY_XOR: u32 = 0;
/// Current cipher envelope version minted by [`SecretStore::put`].
pub const SECRET_ENVELOPE_VERSION: u32 = 1;

/// Master key for the [`SecretStore`]. Held outside the store, mirroring the
/// MVP rule that the key lives outside the encrypted database.
///
/// The raw material arrives from process configuration (environment handle
/// or file, see [`MasterKey::from_env`]/[`MasterKey::from_file`]) and is
/// zeroized on drop; it is never serialized, logged, or placed in events.
/// The AEAD key is derived per operation as `SHA256(material)` so arbitrary
/// handle lengths work while the cipher always gets 32 bytes.
pub struct MasterKey(Vec<u8>);

impl Clone for MasterKey {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl PartialEq for MasterKey {
    fn eq(&self, other: &Self) -> bool {
        // Fixed-length derived-key comparison so equality checks do not
        // short-circuit on the raw material's prefix.
        self.aead_key() == other.aead_key()
    }
}

impl Eq for MasterKey {}

impl Drop for MasterKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl MasterKey {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Result<Self> {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return Err(CoreError::CapabilityBinding(
                "master key must not be empty".to_string(),
            ));
        }
        Ok(Self(bytes))
    }

    /// Loads key material from an environment variable. The variable must be
    /// set and non-empty; only its name (never its value) appears in errors.
    pub fn from_env(var: &str) -> Result<Self> {
        let mut value = std::env::var(var).map_err(|_| {
            CoreError::CapabilityBinding(format!(
                "master key environment variable '{var}' is not set; export it from process configuration and retry"
            ))
        })?;
        if value.trim().is_empty() {
            value.zeroize();
            return Err(CoreError::CapabilityBinding(format!(
                "master key environment variable '{var}' is empty; provide key material from process configuration and retry"
            )));
        }
        let key = Self(value.into_bytes());
        Ok(key)
    }

    /// Loads key material from a file (raw bytes, trailing newline trimmed).
    /// Only the path (never the content) appears in errors.
    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let mut bytes = std::fs::read(path).map_err(|e| {
            CoreError::CapabilityBinding(format!(
                "cannot read master key file '{}': {e}; check the path and file permissions, then retry",
                path.display()
            ))
        })?;
        while bytes.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
            bytes.pop();
        }
        if bytes.is_empty() {
            bytes.zeroize();
            return Err(CoreError::CapabilityBinding(format!(
                "master key file '{}' holds no key material; write key material from process configuration and retry",
                path.display()
            )));
        }
        Ok(Self(bytes))
    }

    /// Generates a fresh 256-bit key from the OS RNG for rotation.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let key = Self(bytes.to_vec());
        bytes.zeroize();
        key
    }

    /// Derives the 32-byte AEAD key as `SHA256(material)`.
    fn aead_key(&self) -> [u8; 32] {
        let digest = Sha256::digest(&self.0);
        let mut key = [0u8; 32];
        key.copy_from_slice(&digest);
        key
    }
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MasterKey")
            .field("bytes", &"[redacted]")
            .finish()
    }
}

/// One sealed secret at rest: a versioned AEAD envelope.
///
/// `cipher_version` selects the envelope (`0` legacy XOR, `1`
/// ChaCha20-Poly1305); `nonce` is fresh randomness per seal for v1 and
/// empty for v0. Readers dispatch on `cipher_version` so pre-rotation
/// envelopes stay readable until their reference is rotated.
#[derive(Clone, PartialEq, Eq)]
pub struct SealedSecret {
    pub ref_id: String,
    pub version: u64,
    /// Application environment this value may be injected into.
    pub scope: InjectionGrant,
    pub cipher_version: u32,
    nonce: [u8; 12],
    ciphertext: Vec<u8>,
}

impl std::fmt::Debug for SealedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedSecret")
            .field("ref_id", &self.ref_id)
            .field("version", &self.version)
            .field("scope", &self.scope)
            .field("ciphertext", &"[redacted]")
            .finish()
    }
}

impl SealedSecret {
    /// Public reference safe for agent context.
    pub fn reference(&self) -> SecretReference {
        SecretReference::new(self.ref_id.clone()).with_hint(format!("v{}", self.version))
    }
}

/// Who is authorized to receive injected secret values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InjectionGrant {
    pub application_id: ApplicationId,
    pub environment_name: String,
    /// True for production runtimes; replacement of production secrets then
    /// requires human approval.
    pub production: bool,
}

impl InjectionGrant {
    pub fn new(
        application_id: ApplicationId,
        environment_name: impl Into<String>,
        production: bool,
    ) -> Self {
        Self {
            application_id,
            environment_name: environment_name.into(),
            production,
        }
    }
}

/// Audit record for secret lifecycle events. Never contains values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretAudit {
    pub ref_id: String,
    pub action: SecretAction,
    pub version: u64,
    pub actor: String,
    pub at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
}

/// Secret lifecycle actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SecretAction {
    Store,
    Rotate,
    Replace,
    Delete,
}

impl SecretAudit {
    /// Renders the audit/event detail; contains only reference ids.
    pub fn to_event_detail(&self) -> String {
        let approver = self
            .approved_by
            .as_deref()
            .map(|a| format!(" approved by {a}"))
            .unwrap_or_default();
        format!(
            "secret '{}' {:?} v{} by {}{approver}",
            self.ref_id, self.action, self.version, self.actor
        )
    }
}

/// Sealed-at-rest secret store with redaction, injection, rotation, and
/// replacement policy.
///
/// Values are held as ciphertext under a [`MasterKey`] that is never stored
/// inside the store (MVP: encrypted PostgreSQL, master key outside the
/// database). [`SecretStore::redact`] strips any known plaintext from event
/// or log text, and [`SecretStore::inject`] reveals values only for a grant
/// naming the authorized application environment.
pub struct SecretStore {
    master_key: MasterKey,
    secrets: BTreeMap<String, SealedSecret>,
    audit: Vec<SecretAudit>,
}

impl std::fmt::Debug for SecretStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretStore")
            .field("master_key", &self.master_key)
            .field("secrets", &self.secrets)
            .field("audit", &self.audit)
            .finish()
    }
}

impl SecretStore {
    pub fn new(master_key: MasterKey) -> Self {
        Self {
            master_key,
            secrets: BTreeMap::new(),
            audit: Vec::new(),
        }
    }

    pub fn audit(&self) -> &[SecretAudit] {
        &self.audit
    }

    pub fn contains(&self, ref_id: &str) -> bool {
        self.secrets.contains_key(ref_id)
    }

    pub fn sealed(&self, ref_id: &str) -> Option<&SealedSecret> {
        self.secrets.get(ref_id)
    }

    /// Stores (or replaces) a value under `ref_id`, sealed at rest and scoped
    /// to the grant's application environment.
    pub fn put(
        &mut self,
        ref_id: impl Into<String>,
        value: &str,
        scope: &InjectionGrant,
        actor: impl Into<String>,
        at: DateTime<Utc>,
    ) -> Result<SealedSecret> {
        let ref_id = ref_id.into();
        if ref_id.trim().is_empty() {
            return Err(CoreError::SecretNotFound(
                "secret reference id must not be empty".to_string(),
            ));
        }
        let version = self.secrets.get(&ref_id).map_or(1, |s| s.version + 1);
        let (nonce, ciphertext) = seal(&self.master_key, &ref_id, version, value);
        let sealed = SealedSecret {
            ref_id: ref_id.clone(),
            version,
            scope: scope.clone(),
            cipher_version: SECRET_ENVELOPE_VERSION,
            nonce,
            ciphertext,
        };
        self.secrets.insert(ref_id.clone(), sealed.clone());
        self.audit.push(SecretAudit {
            ref_id,
            action: SecretAction::Store,
            version,
            actor: actor.into(),
            at,
            approved_by: None,
        });
        Ok(sealed)
    }

    /// Reveals a value for an authorized runtime operation. The grant must
    /// match the environment the secret is scoped to. The plaintext leaves the
    /// store only here; callers must not log the result. Authentication
    /// failures (tampered envelope or wrong key) surface as verification
    /// failures carrying recovery, never plaintext.
    pub fn inject(&self, grant: &InjectionGrant, ref_id: &str) -> Result<String> {
        let sealed = self
            .secrets
            .get(ref_id)
            .ok_or_else(|| CoreError::SecretNotFound(format!("secret '{ref_id}' not found")))?;
        if sealed.scope.application_id != grant.application_id
            || sealed.scope.environment_name != grant.environment_name
        {
            return Err(CoreError::CapabilityBinding(format!(
                "secret '{ref_id}' is not authorized for environment '{}'",
                grant.environment_name
            )));
        }
        self.open_envelope(sealed)
    }

    /// Opens any envelope the store holds (including a pre-rotation clone
    /// kept by the caller) under the store's active key. Readers dispatch on
    /// the envelope version; tampered envelopes and wrong-key opens fail
    /// authentication and return a verification failure with recovery.
    pub fn open_envelope(&self, sealed: &SealedSecret) -> Result<String> {
        open(&self.master_key, sealed).ok_or_else(|| {
            CoreError::Verification(format!(
                "secret '{}' failed authentication (tampered envelope or wrong key); no plaintext was exposed — re-seal from a trusted source and rotate",
                sealed.ref_id
            ))
        })
    }

    /// Injects every secret referenced by a binding into the grant's runtime.
    pub fn inject_binding(
        &self,
        grant: &InjectionGrant,
        binding: &Binding,
    ) -> Result<BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        for (name, source) in &binding.env {
            if let EnvSource::SecretRef(ref_id) = source {
                out.insert(name.clone(), self.inject(grant, ref_id)?);
            }
        }
        Ok(out)
    }

    /// Re-seals an existing value under a new version with a fresh nonce
    /// (rotation). The value itself is unchanged; consumers must pick up the
    /// new reference. Rotating a production-scoped secret requires a granted,
    /// named human approval, denied before any re-seal; the denial carries
    /// recovery. Neither old nor new values enter the audit record.
    pub fn rotate(
        &mut self,
        ref_id: &str,
        approval: &ExplicitApproval,
        actor: impl Into<String>,
        at: DateTime<Utc>,
    ) -> Result<u64> {
        let sealed = self
            .secrets
            .get(ref_id)
            .ok_or_else(|| CoreError::SecretNotFound(format!("secret '{ref_id}' not found")))?;
        let approved = approval.approved && !approval.approver.trim().is_empty();
        if sealed.scope.production && !approved {
            return Err(CoreError::ApprovalRequired(format!(
                "production rotation of secret '{ref_id}' requires explicit human approval; request a granted named approval and retry"
            )));
        }
        let value = self.open_envelope(sealed)?;
        let version = sealed.version + 1;
        let scope = sealed.scope.clone();
        let (nonce, ciphertext) = seal(&self.master_key, ref_id, version, &value);
        self.secrets.insert(
            ref_id.to_string(),
            SealedSecret {
                ref_id: ref_id.to_string(),
                version,
                scope,
                cipher_version: SECRET_ENVELOPE_VERSION,
                nonce,
                ciphertext,
            },
        );
        self.audit.push(SecretAudit {
            ref_id: ref_id.to_string(),
            action: SecretAction::Rotate,
            version,
            actor: actor.into(),
            at,
            approved_by: approved.then(|| approval.approver.clone()),
        });
        Ok(version)
    }

    /// Re-seals every live reference under `new_key` (key rotation) with
    /// fresh nonces, then adopts `new_key` as the store's active key. When
    /// any live reference is production-scoped, a granted, named human
    /// approval is required and the rotation is denied before any re-seal.
    /// The audit trail records one `Rotate` entry per reference carrying
    /// only reference id, version lineage, actor, and approver — values
    /// never enter it by construction. Returns the number of re-sealed
    /// references.
    pub fn rotate_key(
        &mut self,
        new_key: MasterKey,
        approval: &ExplicitApproval,
        actor: impl Into<String>,
        at: DateTime<Utc>,
    ) -> Result<usize> {
        let approved = approval.approved && !approval.approver.trim().is_empty();
        let production_refs: Vec<String> = self
            .secrets
            .iter()
            .filter(|(_, s)| s.scope.production)
            .map(|(id, _)| id.clone())
            .collect();
        if !production_refs.is_empty() && !approved {
            return Err(CoreError::ApprovalRequired(format!(
                "key rotation over production secrets ({}) requires explicit human approval; request a granted named approval and retry",
                production_refs.join(", ")
            )));
        }
        let actor = actor.into();
        // Open every value under the OLD key first so a tampered envelope
        // aborts the rotation before any state changes.
        let mut plaintexts: Vec<(String, u64, InjectionGrant, String)> = Vec::new();
        for (ref_id, sealed) in &self.secrets {
            let value = self.open_envelope(sealed)?;
            plaintexts.push((
                ref_id.clone(),
                sealed.version + 1,
                sealed.scope.clone(),
                value,
            ));
        }
        let mut rotated = 0;
        for (ref_id, version, scope, value) in &plaintexts {
            let (nonce, ciphertext) = seal(&new_key, ref_id, *version, value);
            self.secrets.insert(
                ref_id.clone(),
                SealedSecret {
                    ref_id: ref_id.clone(),
                    version: *version,
                    scope: scope.clone(),
                    cipher_version: SECRET_ENVELOPE_VERSION,
                    nonce,
                    ciphertext,
                },
            );
            self.audit.push(SecretAudit {
                ref_id: ref_id.clone(),
                action: SecretAction::Rotate,
                version: *version,
                actor: actor.clone(),
                at,
                approved_by: approved.then(|| approval.approver.clone()),
            });
            rotated += 1;
        }
        self.master_key = new_key;
        // Plaintexts drop here without entering logs, events, or audit.
        for (_, _, _, value) in plaintexts {
            let mut value = value;
            value.zeroize();
        }
        Ok(rotated)
    }

    /// Replaces a secret value. Production replacement always requires a
    /// granted, named human approval; neither old nor new values appear in
    /// the returned audit record.
    pub fn replace(
        &mut self,
        ref_id: &str,
        new_value: &str,
        grant: &InjectionGrant,
        approval: &ExplicitApproval,
        actor: impl Into<String>,
        at: DateTime<Utc>,
    ) -> Result<SecretAudit> {
        let approved = approval.approved && !approval.approver.trim().is_empty();
        if grant.production && !approved {
            return Err(CoreError::ApprovalRequired(format!(
                "production replacement of secret '{ref_id}' requires explicit human approval"
            )));
        }
        if !self.contains(ref_id) {
            return Err(CoreError::SecretNotFound(format!(
                "cannot replace unknown secret '{ref_id}'"
            )));
        }
        let actor = actor.into();
        self.put(ref_id, new_value, grant, actor.clone(), at)?;
        let audit = SecretAudit {
            ref_id: ref_id.to_string(),
            action: SecretAction::Replace,
            version: self.sealed(ref_id).map(|s| s.version).expect("just stored"),
            actor,
            at,
            approved_by: approved.then(|| approval.approver.clone()),
        };
        self.audit.push(audit.clone());
        Ok(audit)
    }

    /// Deletes a sealed secret.
    pub fn delete(
        &mut self,
        ref_id: &str,
        actor: impl Into<String>,
        at: DateTime<Utc>,
    ) -> Result<()> {
        if self.secrets.remove(ref_id).is_none() {
            return Err(CoreError::SecretNotFound(format!(
                "cannot delete unknown secret '{ref_id}'"
            )));
        }
        self.audit.push(SecretAudit {
            ref_id: ref_id.to_string(),
            action: SecretAction::Delete,
            version: 0,
            actor: actor.into(),
            at,
            approved_by: None,
        });
        Ok(())
    }

    /// Replaces every known plaintext occurrence in `text` with a redaction
    /// marker. Applied to events and logs before they reach any transcript.
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for sealed in self.secrets.values() {
            if let Some(value) = open(&self.master_key, sealed) {
                if !value.is_empty() && out.contains(&value) {
                    out = out.replace(&value, &format!("[redacted:{}]", sealed.ref_id));
                }
                let mut value = value;
                value.zeroize();
            }
        }
        out
    }

    /// Fail-closed boundary guard for any payload about to be persisted or
    /// serialized (event, log, evidence, API response, dashboard payload):
    /// redacts known plaintext, then rejects the payload when any known
    /// value survives redaction. The error names the field only, never the
    /// value. Startup and periodic sweeps use this to prove no secret value
    /// reaches events, logs, evidence, API responses, or dashboard payloads.
    pub fn guard_text(&self, field: &str, text: &str) -> Result<String> {
        let redacted = self.redact(text);
        for sealed in self.secrets.values() {
            if let Some(value) = open(&self.master_key, sealed) {
                let leaked = !value.is_empty() && redacted.contains(&value);
                let mut value = value;
                value.zeroize();
                if leaked {
                    return Err(CoreError::SecretLeak(format!(
                        "secret value rejected before persistence in field '{field}'"
                    )));
                }
            }
        }
        Ok(redacted)
    }
}

/// Seals `plaintext` into the current ChaCha20-Poly1305 envelope with a
/// fresh random nonce per seal. The reference id and secret version are
/// bound as associated data so re-targeting a sealed value at another
/// reference or version fails authentication.
fn seal(
    master_key: &MasterKey,
    ref_id: &str,
    version: u64,
    plaintext: &str,
) -> ([u8; 12], Vec<u8>) {
    let cipher = ChaCha20Poly1305::new_from_slice(&master_key.aead_key())
        .expect("SHA256 output is always a valid 256-bit key");
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let aad = envelope_aad(ref_id, version);
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext.as_bytes(),
                aad: &aad,
            },
        )
        .expect("ChaCha20-Poly1305 encryption cannot fail");
    (nonce_bytes, ciphertext)
}

/// Opens an envelope, dispatching on its cipher version: v1 verifies the
/// AEAD tag and associated data, v0 falls back to the retired XOR
/// stand-in so pre-hardening envelopes stay readable until rotated.
/// Returns `None` on authentication failure without exposing plaintext.
fn open(master_key: &MasterKey, sealed: &SealedSecret) -> Option<String> {
    match sealed.cipher_version {
        SECRET_ENVELOPE_VERSION => {
            let cipher = ChaCha20Poly1305::new_from_slice(&master_key.aead_key()).ok()?;
            let nonce = Nonce::from_slice(&sealed.nonce);
            let aad = envelope_aad(&sealed.ref_id, sealed.version);
            let plain = cipher
                .decrypt(
                    nonce,
                    Payload {
                        msg: &sealed.ciphertext,
                        aad: &aad,
                    },
                )
                .ok()?;
            String::from_utf8(plain).ok()
        }
        SECRET_ENVELOPE_LEGACY_XOR => open_legacy(master_key, sealed),
        _ => None,
    }
}

/// Associated data binding a sealed value to its reference id and version.
fn envelope_aad(ref_id: &str, version: u64) -> Vec<u8> {
    format!("labrys-secret v{SECRET_ENVELOPE_VERSION}:{ref_id}:v{version}").into_bytes()
}

/// Retired deterministic keyed seal (XOR stream). Writers never mint it;
/// kept so readers can open pre-hardening envelopes until rotation.
#[allow(dead_code)]
fn seal_legacy(master_key: &MasterKey, ref_id: &str, plaintext: &str) -> Vec<u8> {
    let stream = keystream(master_key, ref_id, plaintext.len());
    plaintext
        .as_bytes()
        .iter()
        .zip(stream.iter())
        .map(|(b, k)| b ^ k)
        .collect()
}

fn open_legacy(master_key: &MasterKey, sealed: &SealedSecret) -> Option<String> {
    let stream = keystream(master_key, &sealed.ref_id, sealed.ciphertext.len());
    let plain: Vec<u8> = sealed
        .ciphertext
        .iter()
        .zip(stream.iter())
        .map(|(b, k)| b ^ k)
        .collect();
    String::from_utf8(plain).ok()
}

fn keystream(master_key: &MasterKey, ref_id: &str, len: usize) -> Vec<u8> {
    fn mix(hash: &mut u64, bytes: &[u8]) {
        for b in bytes {
            *hash ^= u64::from(*b);
            *hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    let mut out = Vec::with_capacity(len);
    let mut hash: u64 = 0xcbf29ce484222325;
    mix(&mut hash, &master_key.0);
    mix(&mut hash, ref_id.as_bytes());
    let mut counter: u64 = 0;
    while out.len() < len {
        mix(&mut hash, &counter.to_le_bytes());
        out.extend_from_slice(&hash.to_le_bytes());
        counter += 1;
    }
    out.truncate(len);
    out
}

#[cfg(test)]
mod secret_cipher_tests {
    use super::*;
    use chrono::TimeZone;

    fn test_key() -> MasterKey {
        MasterKey::new(vec![7u8; 32]).unwrap()
    }

    fn legacy_envelope(key: &MasterKey, ref_id: &str, version: u64, value: &str) -> SealedSecret {
        SealedSecret {
            ref_id: ref_id.to_string(),
            version,
            scope: InjectionGrant::new(ApplicationId::new(), "production", true),
            cipher_version: SECRET_ENVELOPE_LEGACY_XOR,
            nonce: [0u8; 12],
            ciphertext: seal_legacy(key, ref_id, value),
        }
    }

    #[test]
    fn aead_round_trips_and_tamper_fails_closed_without_plaintext() {
        let key = test_key();
        let (nonce, ciphertext) = seal(&key, "db.url", 1, "s3cr3t-value");
        let sealed = SealedSecret {
            ref_id: "db.url".to_string(),
            version: 1,
            scope: InjectionGrant::new(ApplicationId::new(), "production", true),
            cipher_version: SECRET_ENVELOPE_VERSION,
            nonce,
            ciphertext,
        };
        assert_eq!(open(&key, &sealed).as_deref(), Some("s3cr3t-value"));

        // Ciphertext at rest carries no plaintext structure.
        assert!(!sealed.ciphertext.windows(6).any(|w| w == b"s3cr3t"));

        // Flipping any byte of the ciphertext fails authentication.
        let mut tampered = sealed.clone();
        tampered.ciphertext[0] ^= 0x01;
        assert!(open(&key, &tampered).is_none());

        // A swapped nonce fails authentication.
        let mut bad_nonce = sealed.clone();
        bad_nonce.nonce[0] ^= 0x01;
        assert!(open(&key, &bad_nonce).is_none());

        // The wrong key fails authentication.
        let wrong = MasterKey::new(vec![9u8; 32]).unwrap();
        assert!(open(&wrong, &sealed).is_none());

        // Unknown envelope versions never open.
        let mut unknown = sealed.clone();
        unknown.cipher_version = 99;
        assert!(open(&key, &unknown).is_none());
    }

    #[test]
    fn associated_data_binds_reference_and_version() {
        let key = test_key();
        let (nonce, ciphertext) = seal(&key, "db.url", 1, "s3cr3t-value");
        let sealed = SealedSecret {
            ref_id: "db.url".to_string(),
            version: 1,
            scope: InjectionGrant::new(ApplicationId::new(), "production", true),
            cipher_version: SECRET_ENVELOPE_VERSION,
            nonce,
            ciphertext,
        };
        // Re-targeting the envelope at another reference fails.
        let mut retargeted = sealed.clone();
        retargeted.ref_id = "other.url".to_string();
        assert!(open(&key, &retargeted).is_none());
        // Re-targeting at another version fails.
        let mut reversioned = sealed.clone();
        reversioned.version = 2;
        assert!(open(&key, &reversioned).is_none());
    }

    #[test]
    fn nonces_are_unique_per_seal() {
        let key = test_key();
        let (n1, c1) = seal(&key, "db.url", 1, "same-value");
        let (n2, c2) = seal(&key, "db.url", 1, "same-value");
        assert_ne!(n1, n2);
        assert_ne!(c1, c2);
    }

    #[test]
    fn legacy_v0_envelope_opens_until_rotated() {
        let key = test_key();
        // A pre-hardening envelope dispatches on its version and stays
        // readable under the same key material.
        let legacy = legacy_envelope(&key, "db.url", 1, "old-s3cr3t");
        assert_eq!(legacy.cipher_version, SECRET_ENVELOPE_LEGACY_XOR);
        assert_eq!(open(&key, &legacy).as_deref(), Some("old-s3cr3t"));

        // New seals mint only the current envelope.
        let store_key = test_key();
        let mut store = SecretStore::new(store_key);
        let grant = InjectionGrant::new(ApplicationId::new(), "production", true);
        let at = chrono::Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap();
        let sealed = store
            .put("db.url", "new-s3cr3t", &grant, "alice", at)
            .unwrap();
        assert_eq!(sealed.cipher_version, SECRET_ENVELOPE_VERSION);
        // The legacy envelope still opens beside the rotated reference.
        assert_eq!(open(&key, &legacy).as_deref(), Some("old-s3cr3t"));
    }
}
