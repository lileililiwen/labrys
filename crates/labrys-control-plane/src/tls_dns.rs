//! Real DNS verification and TLS issuance behind [`DomainDeliveryAdapter`].
//!
//! [`ResolvingDns`] verifies a hostname through the real system resolver
//! (`tokio::net::lookup_host`): a name that resolves is `Propagated`, a name
//! that does not is `Pending` with the diagnostic (unproven, retryable —
//! never a simulated propagation). [`OpensslTlsIssuer`] issues real
//! self-signed X.509 certificates through the `openssl` CLI with structured
//! arguments (no shell anywhere on the path) into an approved directory with
//! `0600` key files. Persisted certificate evidence carries the SHA-256
//! fingerprint, subject, and expiry; key material never leaves the filesystem
//! and never enters events, logs, or evidence rows.
//!
//! Scope notes (see `docs/release.md`): disposable scope covers
//! resolver-verified DNS plus locally issued certificates. Public ACME trust
//! and authoritative DNS management stay out of scope; issuance failures
//! report the environment blocker with recovery and attach no traffic.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use sha2::{Digest, Sha256};

use labrys_core::{
    CertificateState, Deployment, DnsState, Domain, DomainDelivery, ExplicitApproval,
};

use crate::error::{ControlPlaneError, Result};
use crate::providers::DomainDeliveryAdapter;
use crate::redact;

/// Verifies hostnames through the real system resolver.
#[derive(Debug, Clone, Copy, Default)]
pub struct ResolvingDns;

impl ResolvingDns {
    /// Returns `Propagated` when the hostname resolves to at least one
    /// socket address, `Pending` otherwise.
    pub async fn ensure(&self, hostname: &str) -> DnsState {
        let hostname = hostname.trim();
        if hostname.is_empty() || hostname.chars().any(char::is_whitespace) {
            return DnsState::Failed {
                reason: "hostname is empty or contains whitespace".to_string(),
            };
        }
        // Port 443 is the lookup key only; no connection is opened. The
        // timeout keeps an unresponsive resolver from stalling dispatch.
        match tokio::time::timeout(
            Duration::from_secs(10),
            tokio::net::lookup_host((hostname, 443)),
        )
        .await
        {
            Ok(Ok(mut addresses)) => match addresses.next() {
                Some(_) => DnsState::Propagated,
                None => DnsState::Pending,
            },
            Ok(Err(_)) | Err(_) => DnsState::Pending,
        }
    }
}

/// Real self-signed X.509 issuer through the `openssl` CLI.
#[derive(Debug, Clone)]
pub struct OpensslTlsIssuer {
    openssl_bin: PathBuf,
    dir: PathBuf,
}

impl OpensslTlsIssuer {
    pub fn new(openssl_bin: PathBuf, dir: PathBuf) -> Result<Self> {
        if dir.as_os_str().is_empty() {
            return Err(ControlPlaneError::Execution(
                "TLS issuer requires a non-empty directory".to_string(),
            ));
        }
        Ok(Self { openssl_bin, dir })
    }

    pub fn from_path(dir: PathBuf) -> Result<Self> {
        Self::new(PathBuf::from("openssl"), dir)
    }

    fn file_stem(hostname: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(hostname.as_bytes());
        let hex: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("tls_{}", hex.chars().take(16).collect::<String>())
    }

    pub fn cert_path(&self, hostname: &str) -> PathBuf {
        self.dir.join(format!("{}.crt", Self::file_stem(hostname)))
    }

    pub fn key_path(&self, hostname: &str) -> PathBuf {
        self.dir.join(format!("{}.key", Self::file_stem(hostname)))
    }

    async fn run_openssl(&self, args: &[&str], timeout_secs: u64) -> Result<std::process::Output> {
        let mut cmd = tokio::process::Command::new(&self.openssl_bin);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = cmd.spawn().map_err(|err| {
            ControlPlaneError::Execution(format!(
                "TLS issuer could not execute openssl ({}): {err}. {}",
                self.openssl_bin.display(),
                labrys_core::CERTIFICATE_FAILURE_RECOVERY,
            ))
        })?;
        tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
            .await
            .map_err(|_| {
                ControlPlaneError::Execution(format!(
                    "openssl {action} timed out after {timeout_secs}s; {recovery}",
                    action = args.first().unwrap_or(&"<unknown>"),
                    recovery = labrys_core::CERTIFICATE_FAILURE_RECOVERY,
                ))
            })?
            .map_err(ControlPlaneError::Io)
    }

    /// Issues (or reuses) a self-signed certificate for `hostname`, returning
    /// `Issued`. Key files are created `0600`; existing files are reused so
    /// issuance is idempotent per hostname.
    pub async fn issue(&self, hostname: &str) -> Result<CertificateState> {
        let hostname = hostname.trim();
        if hostname.is_empty() || hostname.chars().any(char::is_whitespace) {
            return Ok(CertificateState::Failed {
                reason: "hostname is empty or contains whitespace".to_string(),
            });
        }
        tokio::fs::create_dir_all(&self.dir).await?;
        let cert = self.cert_path(hostname);
        let key = self.key_path(hostname);
        if cert.exists() && key.exists() {
            return Ok(CertificateState::Issued);
        }
        let cert_arg = cert.to_string_lossy().to_string();
        let key_arg = key.to_string_lossy().to_string();
        let output = self
            .run_openssl(
                &[
                    "req",
                    "-x509",
                    "-newkey",
                    "rsa:2048",
                    "-nodes",
                    "-keyout",
                    &key_arg,
                    "-out",
                    &cert_arg,
                    "-days",
                    "90",
                    "-subj",
                    &format!("/CN={hostname}"),
                ],
                60,
            )
            .await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Ok(CertificateState::Failed {
                reason: format!(
                    "openssl issuance failed for '{hostname}': {}. {}",
                    stderr.chars().take(200).collect::<String>(),
                    labrys_core::CERTIFICATE_FAILURE_RECOVERY,
                ),
            });
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).await?;
            tokio::fs::set_permissions(&cert, std::fs::Permissions::from_mode(0o644)).await?;
        }
        Ok(CertificateState::Issued)
    }

    /// Persistable evidence for an issued hostname certificate: fingerprint,
    /// subject, and expiry. Reads the certificate file; key material is never
    /// loaded, let alone returned.
    pub async fn evidence(&self, hostname: &str) -> Option<String> {
        let cert = self.cert_path(hostname);
        if !cert.exists() {
            return None;
        }
        let cert_arg = cert.to_string_lossy().to_string();
        let fingerprint = self
            .run_openssl(
                &[
                    "x509",
                    "-noout",
                    "-fingerprint",
                    "-sha256",
                    "-in",
                    &cert_arg,
                ],
                15,
            )
            .await
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())?;
        let enddate = self
            .run_openssl(&["x509", "-noout", "-enddate", "-in", &cert_arg], 15)
            .await
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_default();
        Some(format!(
            "certificate for '{hostname}': {fingerprint} {enddate} (self-signed, 90d)"
        ))
    }
}

/// Real domain delivery: resolver-verified DNS, openssl-issued TLS, and the
/// core approval-gated traffic attachment.
#[derive(Debug, Clone)]
pub struct RealDomainDelivery {
    dns: ResolvingDns,
    tls: OpensslTlsIssuer,
}

impl RealDomainDelivery {
    pub fn new(tls: OpensslTlsIssuer) -> Self {
        Self {
            dns: ResolvingDns,
            tls,
        }
    }

    pub fn dns(&self) -> ResolvingDns {
        self.dns
    }

    pub fn tls(&self) -> &OpensslTlsIssuer {
        &self.tls
    }
}

#[async_trait::async_trait]
impl DomainDeliveryAdapter for RealDomainDelivery {
    async fn ensure_dns(&self, hostname: &str) -> Result<DnsState> {
        Ok(self.dns.ensure(hostname).await)
    }

    async fn issue_certificate(&self, hostname: &str) -> Result<CertificateState> {
        self.tls.issue(hostname).await.map_err(|err| {
            ControlPlaneError::Domain(redact::redact(
                &format!("certificate issuance failed for '{hostname}': {err}"),
                &[],
            ))
        })
    }

    async fn attach_traffic(
        &self,
        delivery: &mut DomainDelivery,
        domain: &Domain,
        deployment: &Deployment,
        approval: &ExplicitApproval,
    ) -> Result<()> {
        labrys_core::plan_traffic_attachment(delivery, domain, deployment, approval)
            .map_err(|err| ControlPlaneError::Domain(err.to_string()))
    }

    async fn certificate_evidence(&self, hostname: &str) -> Option<String> {
        self.tls.evidence(hostname).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn localhost_resolves_through_the_system_resolver() {
        assert_eq!(ResolvingDns.ensure("localhost").await, DnsState::Propagated);
    }

    #[tokio::test]
    async fn unresolvable_names_stay_pending_never_propagated() {
        // Empty labels never leave the resolver stub, so this holds even
        // behind wildcard DNS.
        let state = ResolvingDns.ensure("unresolvable..labrys-test").await;
        assert!(
            matches!(state, DnsState::Pending),
            "unproven DNS must stay pending, got {state:?}"
        );
    }

    #[tokio::test]
    async fn blank_hostnames_fail_fast() {
        assert!(matches!(
            ResolvingDns.ensure("  ").await,
            DnsState::Failed { .. }
        ));
    }

    #[tokio::test]
    async fn missing_openssl_reports_an_environment_blocker() {
        let issuer = OpensslTlsIssuer::new(
            PathBuf::from("/nonexistent-labrys-openssl"),
            std::env::temp_dir(),
        )
        .unwrap();
        let err = issuer
            .run_openssl(&["version"], 10)
            .await
            .expect_err("missing openssl must fail");
        assert!(
            err.to_string().contains("could not execute openssl"),
            "{err}"
        );
    }
}
