//! Managed PostgreSQL provisioning behind [`ProviderAdapter`].
//!
//! [`PostgresProvisioner`] executes real server-side provisioning against the
//! admin connection URL from process configuration
//! (`LABRYS_PROVIDER_POSTGRES_URL`): least-privilege role creation, database
//! creation owned by that role, least-privilege grants, catalog-backed
//! readiness checks, and approval-gated teardown. Credential values (the admin
//! password, minted role passwords) live only in process memory and in the
//! adapter's in-memory credential cache; they are never stored in the
//! database, never logged, and never embedded in persisted operation details.
//! Every free-text string the adapter returns is scrubbed against the cached
//! passwords before it leaves the adapter.
//!
//! Scope notes (see `docs/release.md`):
//! - The credential cache is process-lifetime: a daemon restart empties it.
//!   Readiness after a restart verifies role/database existence from the
//!   catalog and reports degraded housekeeping (re-provision to mint a fresh
//!   key) rather than pretending to know a forgotten password.
//! - Server version/extension management, backups, and replication are out of
//!   scope; the provisioner manages roles, databases, and grants only.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sqlx::PgPool;

use labrys_core::{ProviderAction, ProviderKind, ProviderOperation};

use crate::error::{ControlPlaneError, Result};
use crate::providers::{ProviderAdapter, ProviderExecution};
use crate::redact;

/// In-memory credential for one provisioned role. Process memory only: it is
/// never serialized, never persisted, and never embedded in an outcome detail.
#[derive(Debug, Clone)]
pub struct ProvisionedCredential {
    pub username: String,
    pub password: String,
    pub database: String,
    pub host: String,
    pub port: u16,
}

/// Real PostgreSQL provisioner over an admin connection.
///
/// Connects lazily: construction validates the URL shape, and the first
/// operation surfaces an unreachable server as a retryable failure with
/// recovery guidance, never a pass.
#[derive(Debug, Clone)]
pub struct PostgresProvisioner {
    admin_pool: PgPool,
    admin_password: Option<String>,
    admin_host: String,
    admin_port: u16,
    credentials: Arc<Mutex<HashMap<String, ProvisionedCredential>>>,
}

impl PostgresProvisioner {
    pub fn new(admin_url: &str) -> Result<Self> {
        if !(admin_url.starts_with("postgres://") || admin_url.starts_with("postgresql://")) {
            return Err(ControlPlaneError::Config(
                "managed postgres provisioner requires a postgresql:// admin URL".to_string(),
            ));
        }
        let (host, port) = admin_host_port(admin_url);
        let admin_password = admin_password_of(admin_url);
        let admin_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect_lazy(admin_url)
            .map_err(|err| {
                ControlPlaneError::Config(format!("managed postgres URL is unusable: {err}"))
            })?;
        Ok(Self {
            admin_pool,
            admin_password,
            admin_host: host,
            admin_port: port,
            credentials: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Password values this provisioner may place in diagnostics, for
    /// redaction registration. The admin URL itself is never exposed.
    pub fn credential_secrets(&self) -> Vec<String> {
        let mut secrets = Vec::new();
        if let Some(password) = self.admin_password.clone() {
            if !password.is_empty() {
                secrets.push(password);
            }
        }
        for credential in self.credentials.lock().expect("lock").values() {
            secrets.push(credential.password.clone());
        }
        secrets
    }

    /// In-memory credential for a provisioned resource, if this process
    /// minted it. Never persisted; used by tests and in-process binding only.
    pub fn credential_for(
        &self,
        resource: &labrys_core::ResourceId,
    ) -> Option<ProvisionedCredential> {
        self.credentials
            .lock()
            .expect("lock")
            .get(&resource.to_string())
            .cloned()
    }

    fn scrub(&self, text: &str) -> String {
        let secrets = self.credential_secrets();
        let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
        redact::redact(text, &refs)
    }

    fn names(operation: &ProviderOperation) -> Result<(String, String)> {
        let stem = operation.resource_id.to_string().replace('-', "");
        let stem = stem.chars().take(16).collect::<String>();
        let role = format!("labrys_r_{stem}");
        let database = format!("labrys_db_{stem}");
        validate_identifier(&role)?;
        validate_identifier(&database)?;
        Ok((role, database))
    }

    async fn role_exists(&self, role: &str) -> Result<bool> {
        let row = sqlx::query("SELECT 1 AS one FROM pg_roles WHERE rolname = $1")
            .bind(role)
            .fetch_optional(&self.admin_pool)
            .await
            .map_err(|err| {
                ControlPlaneError::Provider(self.scrub(&format!("role lookup failed: {err}")))
            })?;
        Ok(row.is_some())
    }

    async fn database_exists(&self, database: &str) -> Result<bool> {
        let row = sqlx::query("SELECT 1 AS one FROM pg_database WHERE datname = $1")
            .bind(database)
            .fetch_optional(&self.admin_pool)
            .await
            .map_err(|err| {
                ControlPlaneError::Provider(self.scrub(&format!("database lookup failed: {err}")))
            })?;
        Ok(row.is_some())
    }

    async fn provision(&self, operation: &ProviderOperation) -> Result<ProviderExecution> {
        let (role, database) = Self::names(operation)?;
        if self.role_exists(&role).await? && self.database_exists(&database).await? {
            return Ok(ProviderExecution::Ready {
                detail: format!(
                    "managed postgres role {} with database {} on {}:{} already present (least-privilege: LOGIN only)",
                    quoted(&role),
                    quoted(&database),
                    self.admin_host,
                    self.admin_port,
                ),
            });
        }
        // Hex uuid: no quotes possible, but escape defensively anyway.
        let password = uuid::Uuid::new_v4().simple().to_string();
        let password_literal = format!("'{}'", password.replace('\'', "''"));
        sqlx::query(&format!(
            "CREATE ROLE {} WITH LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD {password_literal}",
            quoted(&role),
        ))
        .execute(&self.admin_pool)
        .await
        .map_err(|err| {
            ControlPlaneError::Provider(self.scrub(&format!(
                "role creation failed for {}: {err}",
                quoted(&role)
            )))
        })?;
        sqlx::query(&format!(
            "CREATE DATABASE {} OWNER {}",
            quoted(&database),
            quoted(&role)
        ))
        .execute(&self.admin_pool)
        .await
        .map_err(|err| {
            ControlPlaneError::Provider(self.scrub(&format!(
                "database creation failed for {}: {err}",
                quoted(&database)
            )))
        })?;
        // Least privilege: connect only; schema use comes from ownership.
        sqlx::query(&format!(
            "GRANT CONNECT ON DATABASE {} TO {}",
            quoted(&database),
            quoted(&role)
        ))
        .execute(&self.admin_pool)
        .await
        .map_err(|err| {
            ControlPlaneError::Provider(self.scrub(&format!(
                "connect grant failed for {}: {err}",
                quoted(&role)
            )))
        })?;
        self.credentials.lock().expect("lock").insert(
            operation.resource_id.to_string(),
            ProvisionedCredential {
                username: role.clone(),
                password,
                database: database.clone(),
                host: self.admin_host.clone(),
                port: self.admin_port,
            },
        );
        Ok(ProviderExecution::Ready {
            detail: format!(
                "managed postgres role {} with database {} on {}:{} (least-privilege: LOGIN only; credential held in process memory under resource {})",
                quoted(&role),
                quoted(&database),
                self.admin_host,
                self.admin_port,
                operation.resource_id,
            ),
        })
    }

    async fn readiness(&self, operation: &ProviderOperation) -> Result<ProviderExecution> {
        let (role, database) = Self::names(operation)?;
        let has_role = self.role_exists(&role).await?;
        let has_db = self.database_exists(&database).await?;
        if has_role && has_db {
            let cached = self
                .credentials
                .lock()
                .expect("lock")
                .contains_key(&operation.resource_id.to_string());
            let housekeeping = if cached {
                "credential cached in process memory"
            } else {
                "credential cache empty after restart; re-provision to mint a fresh key"
            };
            return Ok(ProviderExecution::Ready {
                detail: format!(
                    "managed postgres role {} with database {} present on {}:{} ({housekeeping})",
                    quoted(&role),
                    quoted(&database),
                    self.admin_host,
                    self.admin_port,
                ),
            });
        }
        Ok(ProviderExecution::Failed {
            reason: format!(
                "managed postgres role {} present: {has_role}, database {} present: {has_db}",
                quoted(&role),
                quoted(&database),
            ),
            recovery:
                "re-run provision; if the failure persists, inspect redacted provider diagnostics"
                    .to_string(),
        })
    }

    async fn adopt(&self, operation: &ProviderOperation) -> Result<ProviderExecution> {
        let (role, database) = Self::names(operation)?;
        if self.database_exists(&database).await? {
            // Ensure the deterministic least-privilege role exists and can
            // connect, then record the adoption.
            if !self.role_exists(&role).await? {
                return Ok(ProviderExecution::Failed {
                    reason: format!(
                        "database {} exists but managed role {} is missing; adoption refuses to take over an unmanaged role layout",
                        quoted(&database),
                        quoted(&role),
                    ),
                    recovery: "provision a fresh managed database or align the role layout first".to_string(),
                });
            }
            return Ok(ProviderExecution::Ready {
                detail: format!(
                    "adopted managed postgres database {} with role {} on {}:{}",
                    quoted(&database),
                    quoted(&role),
                    self.admin_host,
                    self.admin_port,
                ),
            });
        }
        Ok(ProviderExecution::Failed {
            reason: format!(
                "adoption found no database {} on {}:{}",
                quoted(&database),
                self.admin_host,
                self.admin_port,
            ),
            recovery: "provision a fresh managed database instead".to_string(),
        })
    }

    async fn delete(&self, operation: &ProviderOperation) -> Result<ProviderExecution> {
        let (role, database) = Self::names(operation)?;
        // Approval is enforced upstream by `ProviderRuntime`; reaching here
        // means a granted, named approval exists.
        if self.database_exists(&database).await? {
            sqlx::query("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = $1 AND pid <> pg_backend_pid()")
                .bind(&database)
                .execute(&self.admin_pool)
                .await
                .map_err(|err| {
                    ControlPlaneError::Provider(self.scrub(&format!(
                        "backend termination failed for {}: {err}",
                        quoted(&database)
                    )))
                })?;
            sqlx::query(&format!("DROP DATABASE {}", quoted(&database)))
                .execute(&self.admin_pool)
                .await
                .map_err(|err| {
                    ControlPlaneError::Provider(self.scrub(&format!(
                        "database drop failed for {}: {err}",
                        quoted(&database)
                    )))
                })?;
        }
        if self.role_exists(&role).await? {
            sqlx::query(&format!("DROP ROLE {}", quoted(&role)))
                .execute(&self.admin_pool)
                .await
                .map_err(|err| {
                    ControlPlaneError::Provider(
                        self.scrub(&format!("role drop failed for {}: {err}", quoted(&role))),
                    )
                })?;
        }
        self.credentials
            .lock()
            .expect("lock")
            .remove(&operation.resource_id.to_string());
        Ok(ProviderExecution::Ready {
            detail: format!(
                "managed postgres database {} and role {} deleted on {}:{}",
                quoted(&database),
                quoted(&role),
                self.admin_host,
                self.admin_port,
            ),
        })
    }

    fn unsupported(action: ProviderAction) -> ProviderExecution {
        ProviderExecution::Failed {
            reason: format!(
                "managed postgres provisioner does not support {action:?}; provision, readiness-check, adopt, and delete are the executable actions"
            ),
            recovery: "use provision for new databases, adopt for existing ones, delete with approval for teardown".to_string(),
        }
    }
}

#[async_trait]
impl ProviderAdapter for PostgresProvisioner {
    fn key(&self) -> &str {
        "postgres.managed"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Postgres
    }

    async fn execute(&self, operation: &ProviderOperation) -> Result<ProviderExecution> {
        if operation.kind != ProviderKind::Postgres {
            return Err(ControlPlaneError::Provider(format!(
                "postgres.managed does not supply capability '{}'",
                operation.capability
            )));
        }
        let outcome = match operation.action {
            ProviderAction::Provision => self.provision(operation).await?,
            ProviderAction::ReadinessCheck => self.readiness(operation).await?,
            ProviderAction::Adopt => self.adopt(operation).await?,
            ProviderAction::Delete => self.delete(operation).await?,
            other => Self::unsupported(other),
        };
        // Belt and braces: outcomes never carry credential values even though
        // details are built from refs only.
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

fn validate_identifier(name: &str) -> Result<()> {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() || first == '_' => {}
        _ => {
            return Err(ControlPlaneError::Execution(format!(
                "postgres identifier must start with a lowercase letter or underscore: {name}"
            )));
        }
    }
    if name.len() > 48
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(ControlPlaneError::Execution(format!(
            "postgres identifier must be <48 chars of [a-z0-9_]: {name}"
        )));
    }
    Ok(())
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn admin_host_port(url: &str) -> (String, u16) {
    let after_scheme = url.split("://").nth(1).unwrap_or("");
    let authority_host = after_scheme.split('@').next_back().unwrap_or("");
    let host_port = authority_host.split('/').next().unwrap_or("");
    let mut parts = host_port.split(':');
    let host = parts.next().unwrap_or("localhost").to_string();
    let port = parts.next().and_then(|p| p.parse().ok()).unwrap_or(5432);
    (host, port)
}

fn admin_password_of(url: &str) -> Option<String> {
    let after_scheme = url.split("://").nth(1)?;
    let authority = after_scheme.split('@').next()?;
    let password = authority.split(':').nth(1)?;
    if password.is_empty() {
        None
    } else {
        Some(password.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_validation_rejects_injection() {
        assert!(validate_identifier("labrys_r_abc123").is_ok());
        assert!(validate_identifier("evil\"; DROP TABLE x;--").is_err());
        assert!(validate_identifier("HasUpper").is_err());
        assert!(validate_identifier("9starts-with-digit").is_err());
        assert!(validate_identifier(&"a".repeat(49)).is_err());
    }

    #[test]
    fn quoting_escapes_double_quotes() {
        assert_eq!(quoted("a\"b"), "\"a\"\"b\"");
    }

    #[tokio::test]
    async fn admin_url_parsing_extracts_host_port_password() {
        let provisioner =
            PostgresProvisioner::new("postgres://admin:s3cret@db-host:5544/app").unwrap();
        assert_eq!(provisioner.admin_host, "db-host");
        assert_eq!(provisioner.admin_port, 5544);
        assert_eq!(provisioner.credential_secrets(), vec!["s3cret".to_string()]);
    }

    #[test]
    fn non_postgres_admin_url_is_rejected() {
        assert!(PostgresProvisioner::new("mysql://h/db").is_err());
    }
}
