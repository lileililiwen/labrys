//! Filesystem-backed object/file storage provisioning.
//!
//! [`FsBucketBackend`] provisions real buckets as `0700` directories under an
//! approved root and mints real scoped access keys. Key secrets live only in
//! the backend's process-memory registry: persisted operation details carry
//! the key id, scope, and expiry (references), never the secret. Deletion
//! revokes the key and removes the bucket directory. A daemon restart empties
//! the registry; readiness then reports the bucket present but the key gone,
//! with recovery (re-provision), instead of pretending continuity.
//!
//! [`ObjectStorageProvisioner`] (`storage.object`) and
//! [`FileStorageProvisioner`] (`storage.file`) share one backend with
//! distinct bucket prefixes and scopes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

use labrys_core::{ProviderAction, ProviderKind, ProviderOperation};

use crate::error::{ControlPlaneError, Result};
use crate::providers::{ProviderAdapter, ProviderExecution};
use crate::redact;

/// A minted scoped key. The secret stays in process memory; only the id,
/// scope, bucket, and expiry may be persisted.
#[derive(Debug, Clone)]
pub struct ScopedKey {
    pub key_id: String,
    pub secret: String,
    pub scope: String,
    pub bucket: String,
    pub expires_at: DateTime<Utc>,
}

/// Default key lifetime: 24 hours.
pub const DEFAULT_KEY_TTL_SECS: i64 = 86_400;

/// Real bucket backend over one approved filesystem root.
#[derive(Debug, Clone)]
pub struct FsBucketBackend {
    root: PathBuf,
    key_ttl: Duration,
    keys: Arc<Mutex<HashMap<String, ScopedKey>>>,
}

impl FsBucketBackend {
    pub fn new(root: PathBuf) -> Result<Self> {
        Self::with_ttl(root, Duration::seconds(DEFAULT_KEY_TTL_SECS))
    }

    pub fn with_ttl(root: PathBuf, key_ttl: Duration) -> Result<Self> {
        if root.as_os_str().is_empty() {
            return Err(ControlPlaneError::Execution(
                "storage backend requires a non-empty root".to_string(),
            ));
        }
        Ok(Self {
            root,
            key_ttl,
            keys: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Secrets this backend may have placed in diagnostics, for redaction.
    pub fn credential_secrets(&self) -> Vec<String> {
        self.keys
            .lock()
            .expect("lock")
            .values()
            .map(|key| key.secret.clone())
            .collect()
    }

    /// In-memory key for a bucket, if this process minted it.
    pub fn key_for(&self, bucket: &str) -> Option<ScopedKey> {
        self.keys.lock().expect("lock").get(bucket).cloned()
    }

    pub fn bucket_path(&self, bucket: &str) -> Result<PathBuf> {
        if !bucket_under_root(&self.root, bucket) {
            return Err(ControlPlaneError::Execution(
                "bucket path escapes the approved storage root".to_string(),
            ));
        }
        Ok(self.root.join(bucket))
    }

    fn scrub(&self, text: &str) -> String {
        let secrets = self.credential_secrets();
        let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
        redact::redact(text, &refs)
    }

    fn bucket_name(prefix: &str, operation: &ProviderOperation) -> Result<String> {
        Self::bucket_name_for(prefix, &operation.resource_id)
    }

    /// Derives the deterministic bucket name for a resource. Public so
    /// callers (and tests) resolve the same name the backend provisions.
    pub fn bucket_name_for(prefix: &str, resource: &labrys_core::ResourceId) -> Result<String> {
        // Resource ids carry a `res_` prefix and dashes; strip both so the
        // bucket stays in `[a-z0-9-]` scope.
        let stem = resource
            .to_string()
            .replace(['-', '_'], "")
            .chars()
            .take(16)
            .collect::<String>();
        let bucket = format!("{prefix}-{stem}");
        if bucket.len() > 48
            || !bucket
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(ControlPlaneError::Execution(format!(
                "bucket name out of scope: {bucket}"
            )));
        }
        Ok(bucket)
    }

    async fn provision(
        &self,
        prefix: &str,
        scope: &str,
        operation: &ProviderOperation,
    ) -> Result<ProviderExecution> {
        let bucket = Self::bucket_name(prefix, operation)?;
        let path = self.bucket_path(&bucket)?;
        let key = self.key_for(&bucket);
        let live = key.as_ref().is_some_and(|k| k.expires_at > Utc::now());
        if path.exists() && live {
            let key = key.expect("live key");
            return Ok(ProviderExecution::Ready {
                detail: format!(
                    "bucket {} present under the approved root with live scoped key {} (scope {scope}, expires {})",
                    bucket,
                    key.key_id,
                    key.expires_at.to_rfc3339(),
                ),
            });
        }
        tokio::fs::create_dir_all(&path).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).await?;
        }
        let now = Utc::now();
        let minted = ScopedKey {
            key_id: format!("key_{}", uuid::Uuid::new_v4().simple()),
            secret: uuid::Uuid::new_v4().simple().to_string()
                + &uuid::Uuid::new_v4().simple().to_string(),
            scope: scope.to_string(),
            bucket: bucket.clone(),
            expires_at: now + self.key_ttl,
        };
        let detail = format!(
            "bucket {} provisioned under the approved root with scoped key {} (scope {scope}, expires {}); secret held in process memory only",
            bucket,
            minted.key_id,
            minted.expires_at.to_rfc3339(),
        );
        self.keys.lock().expect("lock").insert(bucket, minted);
        Ok(ProviderExecution::Ready { detail })
    }

    async fn readiness(
        &self,
        prefix: &str,
        operation: &ProviderOperation,
    ) -> Result<ProviderExecution> {
        let bucket = Self::bucket_name(prefix, operation)?;
        let path = self.bucket_path(&bucket)?;
        if !path.exists() {
            return Ok(ProviderExecution::Failed {
                reason: format!("bucket {bucket} is missing under the approved root"),
                recovery: "re-run provision to recreate the bucket".to_string(),
            });
        }
        match self.key_for(&bucket) {
            Some(key) if key.expires_at > Utc::now() => Ok(ProviderExecution::Ready {
                detail: format!(
                    "bucket {bucket} present with live scoped key {} (expires {})",
                    key.key_id,
                    key.expires_at.to_rfc3339(),
                ),
            }),
            _ => Ok(ProviderExecution::Failed {
                reason: format!(
                    "bucket {bucket} present but no live key is cached (restart empties the process registry)"
                ),
                recovery: "re-run provision to mint a fresh scoped key".to_string(),
            }),
        }
    }

    async fn delete(
        &self,
        prefix: &str,
        operation: &ProviderOperation,
    ) -> Result<ProviderExecution> {
        let bucket = Self::bucket_name(prefix, operation)?;
        let path = self.bucket_path(&bucket)?;
        self.keys.lock().expect("lock").remove(&bucket);
        if path.exists() {
            tokio::fs::remove_dir_all(&path).await?;
        }
        Ok(ProviderExecution::Ready {
            detail: format!("bucket {bucket} deleted and its scoped key revoked"),
        })
    }

    async fn execute_scoped(
        &self,
        prefix: &str,
        scope: &str,
        operation: &ProviderOperation,
    ) -> Result<ProviderExecution> {
        if operation.kind != kind_for_prefix(prefix) {
            return Err(ControlPlaneError::Provider(format!(
                "{prefix} backend does not supply capability '{}'",
                operation.capability
            )));
        }
        let outcome = match operation.action {
            ProviderAction::Provision => self.provision(prefix, scope, operation).await?,
            ProviderAction::ReadinessCheck => self.readiness(prefix, operation).await?,
            ProviderAction::Adopt => self.readiness(prefix, operation).await?,
            ProviderAction::Delete => self.delete(prefix, operation).await?,
            other => ProviderExecution::Failed {
                reason: format!(
                    "filesystem storage backend does not support {other:?}; provision, readiness-check, adopt, and delete are the executable actions"
                ),
                recovery: "use provision for new buckets and delete with approval for teardown".to_string(),
            },
        };
        Ok(match outcome {
            ProviderExecution::Accepted { detail } => ProviderExecution::Accepted {
                detail: self.scrub(&detail),
            },
            ProviderExecution::Ready { detail } => ProviderExecution::Ready {
                detail: self.scrub(&detail),
            },
            ProviderExecution::Degraded { reason } => ProviderExecution::Degraded {
                reason: self.scrub(&reason),
            },
            ProviderExecution::Failed { reason, recovery } => ProviderExecution::Failed {
                reason: self.scrub(&reason),
                recovery: self.scrub(&recovery),
            },
        })
    }
}

fn kind_for_prefix(prefix: &str) -> ProviderKind {
    match prefix {
        "obj" => ProviderKind::ObjectStorage,
        _ => ProviderKind::FileStorage,
    }
}

/// Real object-storage provisioner (`storage.object`) over a filesystem
/// backend with scoped keys.
#[derive(Debug, Clone)]
pub struct ObjectStorageProvisioner {
    backend: FsBucketBackend,
}

impl ObjectStorageProvisioner {
    pub fn new(root: PathBuf) -> Result<Self> {
        Ok(Self {
            backend: FsBucketBackend::new(root)?,
        })
    }

    pub fn with_backend(backend: FsBucketBackend) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> &FsBucketBackend {
        &self.backend
    }
}

#[async_trait]
impl ProviderAdapter for ObjectStorageProvisioner {
    fn key(&self) -> &str {
        "objectstore.fs"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::ObjectStorage
    }

    async fn execute(&self, operation: &ProviderOperation) -> Result<ProviderExecution> {
        self.backend
            .execute_scoped("obj", "storage.object:read-write", operation)
            .await
    }
}

/// Real file-storage provisioner (`storage.file`) over a filesystem backend
/// with scoped keys.
#[derive(Debug, Clone)]
pub struct FileStorageProvisioner {
    backend: FsBucketBackend,
}

impl FileStorageProvisioner {
    pub fn new(root: PathBuf) -> Result<Self> {
        Ok(Self {
            backend: FsBucketBackend::new(root)?,
        })
    }

    pub fn with_backend(backend: FsBucketBackend) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> &FsBucketBackend {
        &self.backend
    }
}

#[async_trait]
impl ProviderAdapter for FileStorageProvisioner {
    fn key(&self) -> &str {
        "filestore.fs"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::FileStorage
    }

    async fn execute(&self, operation: &ProviderOperation) -> Result<ProviderExecution> {
        self.backend
            .execute_scoped("file", "storage.file:read-write", operation)
            .await
    }
}

/// True when `bucket` resolves under `root`. Component-wise: any root,
/// prefix, parent-dir, or current-dir component is rejected, because a purely
/// lexical prefix check lets `..` escape.
pub fn bucket_under_root(root: &Path, bucket: &str) -> bool {
    use std::path::Component;
    let relative = Path::new(bucket);
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return false;
    }
    root.join(relative).starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_paths_cannot_escape_the_root() {
        let root = PathBuf::from("/tmp/labrys-storage-test");
        assert!(bucket_under_root(&root, "obj-abc123"));
        assert!(!bucket_under_root(&root, "../escape"));
    }

    #[test]
    fn empty_root_is_rejected() {
        assert!(FsBucketBackend::new(PathBuf::new()).is_err());
    }

    #[tokio::test]
    async fn provision_creates_bucket_and_mints_unique_keys() {
        let root = std::env::temp_dir().join(format!(
            "labrys-storage-unit-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let backend = FsBucketBackend::new(root.clone()).unwrap();
        let op_a = labrys_core::ProviderOperation::plan(
            labrys_core::ApplicationId::new(),
            None,
            labrys_core::ResourceId::new(),
            "storage.object",
            "objectstore.fs",
            ProviderKind::ObjectStorage,
            ProviderAction::Provision,
            "trace-a",
        )
        .unwrap();
        let detail = match backend
            .execute_scoped("obj", "storage.object:read-write", &op_a)
            .await
            .unwrap()
        {
            ProviderExecution::Ready { detail } => detail,
            other => panic!("expected ready, got {other:?}"),
        };
        assert!(detail.contains("obj-"), "{detail}");
        let bucket = FsBucketBackend::bucket_name_for("obj", &op_a.resource_id).unwrap();
        assert!(root.join(&bucket).exists());
        let key = backend.key_for(&bucket).expect("key cached in memory");
        assert_eq!(key.secret.len(), 64);
        // Second provision is idempotent: same live key, no rotation.
        let again = backend
            .execute_scoped("obj", "storage.object:read-write", &op_a)
            .await
            .unwrap();
        match again {
            ProviderExecution::Ready { detail } => {
                assert!(detail.contains(&key.key_id), "{detail}")
            }
            other => panic!("expected ready, got {other:?}"),
        }
        // Ready detail never carries the secret.
        assert!(!detail.contains(&key.secret));
        tokio::fs::remove_dir_all(&root).await.ok();
    }
}
