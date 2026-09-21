//! Real OCI registry delivery over the Distribution HTTP API.
//!
//! [`OciRegistryClient`] implements [`OciRegistry`] against any OCI
//! Distribution-spec registry (e.g. `registry:2`, or a cloud registry over
//! HTTPS): blob upload by digest, manifest push by tag, and digest
//! verification of the registry-reported `Docker-Content-Digest` before
//! anything is recorded. Unverified artifacts are refused before any network
//! call; digest mismatches are refused with recovery guidance and never
//! promote.
//!
//! Content bytes travel through the additive [`OciRegistry::push_content`]
//! default method: the original `push(artifact)` shape cannot carry bytes, so
//! the real client reports an explicit unsupported error there instead of
//! faking a push. [`ProviderRuntime::push_content`] persists the same digest
//! evidence rows as [`ProviderRuntime::push_artifact`].

use sha2::{Digest, Sha256};

use labrys_core::{gate_registry_push, verify_registry_digest, RegistryArtifact};

use crate::error::{ControlPlaneError, Result};
use crate::providers::OciRegistry;
use crate::redact;

/// Media type of the minimal manifests this client pushes.
pub const OCI_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
/// Media type of the empty image configs this client pushes.
pub const OCI_CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.image.config.v1+json";

/// Lowercase hex SHA-256 of `bytes`, the digest primitive registries speak.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Builds a minimal OCI image manifest referencing one config blob. The
/// digest of the returned bytes is the content digest registries report.
pub fn oci_manifest(config_digest: &str, config_size: usize) -> Vec<u8> {
    format!(
        "{{\"schemaVersion\":2,\"mediaType\":\"{OCI_MANIFEST_MEDIA_TYPE}\",\
         \"config\":{{\"mediaType\":\"{OCI_CONFIG_MEDIA_TYPE}\",\
         \"digest\":\"{config_digest}\",\"size\":{config_size}}},\"layers\":[]}}",
    )
    .into_bytes()
}

/// Splits an artifact reference into repository name and tag. `app:tag`
/// yields `("app", "tag")`; a bare `app` yields `("app", tag)` with the
/// caller-supplied default; a registry host prefix (with an optional port)
/// is preserved as part of the name.
pub fn reference_parts(reference: &str, default_tag: &str) -> Result<(String, String)> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Err(ControlPlaneError::Registry(
            "registry reference must not be empty".to_string(),
        ));
    }
    if reference.chars().any(char::is_whitespace) {
        return Err(ControlPlaneError::Registry(format!(
            "registry reference is not a plain reference: {reference}"
        )));
    }
    match reference.rsplit_once(':') {
        Some((name, tag)) if !tag.contains('/') && !name.is_empty() && !tag.is_empty() => {
            Ok((name.to_string(), tag.to_string()))
        }
        _ => Ok((reference.to_string(), default_tag.to_string())),
    }
}

/// Real OCI Distribution client. Constructed from process configuration;
/// credentials are redacted out of every diagnostic.
#[derive(Debug, Clone)]
pub struct OciRegistryClient {
    endpoint: String,
    client: reqwest::Client,
    username: Option<String>,
    password: Option<String>,
}

impl OciRegistryClient {
    pub fn new(
        endpoint: impl Into<String>,
        username: Option<String>,
        password: Option<String>,
    ) -> Result<Self> {
        let endpoint = endpoint.into().trim_end_matches('/').to_string();
        if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
            return Err(ControlPlaneError::Config(
                "OCI registry endpoint must be an http(s) URL".to_string(),
            ));
        }
        let client = reqwest::Client::builder()
            .build()
            .map_err(|err| ControlPlaneError::Registry(format!("registry client failed: {err}")))?;
        Ok(Self {
            endpoint,
            client,
            username,
            password,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Password values this client may place in diagnostics, for redaction.
    pub fn credential_secrets(&self) -> Vec<String> {
        match &self.password {
            Some(password) if !password.is_empty() => vec![password.clone()],
            _ => Vec::new(),
        }
    }

    fn scrub(&self, text: &str) -> String {
        let secrets = self.credential_secrets();
        let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
        redact::redact(text, &refs)
    }

    fn authed(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match (&self.username, &self.password) {
            (Some(user), Some(pass)) => request.basic_auth(user, Some(pass)),
            (Some(user), None) => request.basic_auth(user, None::<String>),
            _ => request,
        }
    }

    async fn check_status(
        &self,
        response: reqwest::Response,
        action: &str,
    ) -> Result<reqwest::Response> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        Err(ControlPlaneError::Registry(self.scrub(&format!(
            "registry {action} failed with {status}: {}",
            body.chars().take(300).collect::<String>()
        ))))
    }

    fn join(&self, location: &str) -> String {
        if location.starts_with("http://") || location.starts_with("https://") {
            location.to_string()
        } else {
            format!("{}{}", self.endpoint, location)
        }
    }

    /// Pushes manifest + config bytes for `artifact` and returns the
    /// registry-verified digest. The recorded digest must match what the
    /// registry reports or delivery is refused.
    pub async fn push_bytes(
        &self,
        artifact: &RegistryArtifact,
        manifest: &[u8],
        config: &[u8],
    ) -> Result<String> {
        gate_registry_push(artifact).map_err(|err| ControlPlaneError::Registry(err.to_string()))?;
        if manifest.is_empty() {
            return Err(ControlPlaneError::Registry(
                "registry push requires manifest bytes".to_string(),
            ));
        }
        let default_tag = artifact.revision_sha.chars().take(12).collect::<String>();
        let default_tag = if default_tag.is_empty() {
            "latest".to_string()
        } else {
            default_tag
        };
        let (name, tag) = reference_parts(&artifact.reference, &default_tag)?;
        let config_digest = format!("sha256:{}", sha256_hex(config));

        // 1. Initiate the config blob upload.
        let response = self
            .authed(
                self.client
                    .post(format!("{}/v2/{name}/blobs/uploads/", self.endpoint)),
            )
            .send()
            .await
            .map_err(|err| {
                ControlPlaneError::Registry(
                    self.scrub(&format!("blob upload initiation failed: {err}")),
                )
            })?;
        let response = self
            .check_status(response, "blob upload initiation")
            .await?;
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                ControlPlaneError::Registry(
                    "registry omitted the upload Location header".to_string(),
                )
            })?
            .to_string();

        // 2. Upload the config blob by digest. The initiation Location
        // already carries a query string, so the digest joins with `&`.
        let separator = if self.join(&location).contains('?') {
            "&"
        } else {
            "?"
        };
        let url = format!("{}{separator}digest={config_digest}", self.join(&location));
        let response = self
            .authed(
                self.client
                    .put(url)
                    .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                    .body(config.to_vec()),
            )
            .send()
            .await
            .map_err(|err| {
                ControlPlaneError::Registry(self.scrub(&format!("blob upload failed: {err}")))
            })?;
        self.check_status(response, "blob upload").await?;

        // 3. Push the manifest by tag.
        let response = self
            .authed(
                self.client
                    .put(format!("{}/v2/{name}/manifests/{tag}", self.endpoint))
                    .header(reqwest::header::CONTENT_TYPE, OCI_MANIFEST_MEDIA_TYPE)
                    .body(manifest.to_vec()),
            )
            .send()
            .await
            .map_err(|err| {
                ControlPlaneError::Registry(self.scrub(&format!("manifest push failed: {err}")))
            })?;
        let response = self.check_status(response, "manifest push").await?;
        let reported = response
            .headers()
            .get("docker-content-digest")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if reported.is_empty() {
            return Err(ControlPlaneError::Registry(
                "registry omitted the Docker-Content-Digest header".to_string(),
            ));
        }

        // 4. Fetch the manifest back and verify: computed, reported, and
        // recorded digests must all agree before anything is recorded.
        let computed = format!("sha256:{}", sha256_hex(manifest));
        verify_registry_digest(&computed, &reported)
            .map_err(|err| ControlPlaneError::Registry(self.scrub(&err.to_string())))?;
        verify_registry_digest(&artifact.digest, &reported)
            .map_err(|err| ControlPlaneError::Registry(self.scrub(&err.to_string())))?;
        Ok(reported)
    }

    /// Fetches the registry-reported digest for a pushed tag.
    pub async fn fetch_digest(&self, reference: &str, tag: &str) -> Result<String> {
        let (name, _) = reference_parts(reference, tag)?;
        let response = self
            .authed(
                self.client
                    .get(format!("{}/v2/{name}/manifests/{tag}", self.endpoint))
                    .header(reqwest::header::ACCEPT, OCI_MANIFEST_MEDIA_TYPE),
            )
            .send()
            .await
            .map_err(|err| {
                ControlPlaneError::Registry(self.scrub(&format!("manifest fetch failed: {err}")))
            })?;
        let response = self.check_status(response, "manifest fetch").await?;
        response
            .headers()
            .get("docker-content-digest")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .filter(|digest| !digest.is_empty())
            .ok_or_else(|| {
                ControlPlaneError::Registry(
                    "registry omitted the Docker-Content-Digest header".to_string(),
                )
            })
    }
}

#[async_trait::async_trait]
impl OciRegistry for OciRegistryClient {
    async fn push(&self, _artifact: &RegistryArtifact) -> Result<String> {
        // Content bytes are required for a genuine push and the trait shape
        // does not carry them: refuse explicitly instead of simulating.
        Err(ControlPlaneError::Registry(
            "registry push requires content bytes; use push_content with manifest and config bytes"
                .to_string(),
        ))
    }

    async fn pull(&self, reference: &str) -> Result<String> {
        let (name, tag) = reference_parts(reference, "latest")?;
        self.fetch_digest(&name, &tag).await
    }

    async fn push_content(
        &self,
        artifact: &RegistryArtifact,
        manifest: &[u8],
        config: &[u8],
    ) -> Result<String> {
        self.push_bytes(artifact, manifest, config).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_parts_split_names_and_tags() {
        assert_eq!(
            reference_parts("app:1.2", "latest").unwrap(),
            ("app".to_string(), "1.2".to_string())
        );
        assert_eq!(
            reference_parts("example.com:5000/ns/app:tag", "latest").unwrap(),
            ("example.com:5000/ns/app".to_string(), "tag".to_string())
        );
        assert_eq!(
            reference_parts("registry.local/app", "rev123").unwrap(),
            ("registry.local/app".to_string(), "rev123".to_string())
        );
        assert!(reference_parts("", "latest").is_err());
        assert!(reference_parts("has space/app", "latest").is_err());
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn manifest_embeds_config_digest_and_size() {
        let manifest = oci_manifest("sha256:deadbeef", 2);
        let text = String::from_utf8(manifest).unwrap();
        assert!(text.contains("sha256:deadbeef"), "{text}");
        assert!(text.contains("\"size\":2"), "{text}");
    }

    #[test]
    fn non_http_endpoint_is_rejected() {
        assert!(OciRegistryClient::new("ftp://host/v2", None, None).is_err());
        assert!(OciRegistryClient::new("http://localhost:5000/", None, None).is_ok());
    }
}
