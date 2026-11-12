//! Pairing a device with a server.
//!
//! The flow follows the spec: start a pairing, show the code, poll every two
//! seconds until the user approves it on the server's page, then store the
//! token and the pinned certificate fingerprint.
//!
//! While pairing, TLS accepts the server's self-signed certificate but records
//! the fingerprint it saw. The fingerprint the server reports in the approval
//! response must match the observed one, so a man-in-the-middle cannot swap the
//! certificate unnoticed.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use carrel_protocol::{
    endpoints, hex_encode, normalize_pairing_code, PairStartRequest, PairStartResponse, PairStatus,
    PairStatusResponse,
};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use url::Url;

use crate::config::SyncConfig;
use crate::http::HttpClient;
use crate::tls::TlsMode;

/// Result of a successful pairing.
#[derive(Debug, Clone)]
pub struct PairCredential {
    pub server_url: String,
    pub token: String,
    /// Set for self-hosted servers that ask to be pinned.
    pub cert_sha256: Option<String>,
    pub server_name: Option<String>,
    pub device_pubkey: String,
}

impl PairCredential {
    /// Build the `sync.toml` contents from this credential.
    pub fn to_sync_config(&self) -> SyncConfig {
        SyncConfig {
            server_url: self.server_url.clone(),
            token: self.token.clone(),
            cert_sha256: self.cert_sha256.clone(),
            server_name: self.server_name.clone(),
            device_pubkey: self.device_pubkey.clone(),
        }
    }
}

/// An in-progress pairing.
pub struct PairingSession {
    client: HttpClient,
    server_url: String,
    start: PairStartResponse,
    device_pubkey: String,
}

impl PairingSession {
    /// `POST /pair/start`.
    pub async fn start(server_url: &str, device_name: &str, device_pubkey: &str) -> Result<Self> {
        let device_name = device_name.trim();
        if device_name.is_empty() || device_name.chars().count() > 100 {
            bail!("device name must be 1-100 characters");
        }
        if device_pubkey.trim().is_empty() {
            bail!("device public key is empty");
        }
        let client = HttpClient::new(server_url, TlsMode::AcceptAny)?;
        let request = PairStartRequest {
            device_name: device_name.to_string(),
            device_pubkey: device_pubkey.to_string(),
        };
        let response = client.post_json(endpoints::PAIR_START, &request).await?;
        if !response.is_success() {
            bail!(
                "server refused to start pairing: HTTP {}: {}",
                response.status,
                response.error_message()
            );
        }
        let start: PairStartResponse = response.json()?;
        if normalize_pairing_code(&start.code).is_none() {
            bail!("server returned an invalid pairing code {:?}", start.code);
        }
        Ok(Self {
            client,
            server_url: server_url.to_string(),
            start,
            device_pubkey: device_pubkey.to_string(),
        })
    }

    /// The code the user types or approves on the server page.
    pub fn code(&self) -> &str {
        &self.start.code
    }

    pub fn pair_id(&self) -> uuid::Uuid {
        self.start.pair_id
    }

    pub fn expires_at(&self) -> i64 {
        self.start.expires_at
    }

    /// URL of the server's approval page, for the device's QR code.
    pub fn approval_url(&self) -> String {
        Url::parse(&self.server_url)
            .map(|url| format!("{}/approve", url.origin().ascii_serialization()))
            .unwrap_or_else(|_| format!("{}/approve", self.server_url.trim_end_matches('/')))
    }

    /// Poll until approved, expired or timed out.
    ///
    /// The spec says every 2 seconds for at most 10 minutes; tests pass shorter
    /// values. The device only talks to the network during pairing.
    pub async fn wait(&self, interval: Duration, timeout: Duration) -> Result<PairCredential> {
        let interval = interval.max(Duration::from_millis(25));
        let deadline = Instant::now() + timeout;
        loop {
            let response = self
                .client
                .get(&endpoints::pair_status(self.start.pair_id))
                .await?;
            if response.status == 404 {
                bail!("the server no longer knows this pairing");
            }
            if !response.is_success() {
                bail!(
                    "pairing poll failed: HTTP {}: {}",
                    response.status,
                    response.error_message()
                );
            }
            let status: PairStatusResponse = response.json()?;
            match status.status {
                PairStatus::Approved => {
                    let observed = self
                        .client
                        .observed_cert_sha256()
                        .context("TLS handshake did not expose a certificate fingerprint")?;
                    let pin = match status.server_cert_sha256.as_deref() {
                        Some(reported) => {
                            if !reported.eq_ignore_ascii_case(&observed) {
                                bail!(
                                    "certificate fingerprint mismatch: server reported {reported}, \
                                     connection presented {observed}"
                                );
                            }
                            Some(observed)
                        }
                        // No fingerprint: the server expects public-CA
                        // verification for every later request (TlsMode::SystemRoots).
                        // Refuse now with a clear message if that would fail,
                        // instead of pairing and then failing every sync.
                        None if self.client.observed_cert_publicly_trusted() => None,
                        None => bail!(
                            "the server reported no certificate fingerprint, but its \
                             certificate is not trusted by a public certificate authority; \
                             give the server a publicly trusted certificate (for example \
                             through a TLS proxy or `tailscale cert`)"
                        ),
                    };
                    let token = status
                        .token
                        .context("server approved the pairing but returned no token")?;
                    if token.trim().is_empty() {
                        bail!("server approved the pairing but returned an empty token");
                    }
                    return Ok(PairCredential {
                        server_url: self.server_url.clone(),
                        token,
                        cert_sha256: pin,
                        server_name: status.server_name,
                        device_pubkey: self.device_pubkey.clone(),
                    });
                }
                PairStatus::Expired => {
                    bail!("the pairing code expired before it was approved")
                }
                PairStatus::Pending => {
                    let now = Instant::now();
                    if now >= deadline {
                        bail!("timed out waiting for the pairing to be approved");
                    }
                    let remaining = deadline.saturating_duration_since(now);
                    tokio::time::sleep(interval.min(remaining)).await;
                }
            }
        }
    }

    /// Single poll, used by tests and by future daemon status reporting.
    pub async fn poll(&self) -> Result<PairStatusResponse> {
        self.client
            .get_json(&endpoints::pair_status(self.start.pair_id))
            .await
    }
}

/// Generate a fresh Ed25519 public key (32 bytes, hex encoded) for
/// `POST /pair/start`.
///
/// Protocol v1 uses the token for authentication; the public key is recorded by
/// the server for future device identification.
pub fn generate_device_pubkey() -> Result<String> {
    let rng = SystemRandom::new();
    let document = Ed25519KeyPair::generate_pkcs8(&rng)
        .map_err(|_| anyhow::anyhow!("could not generate an Ed25519 key"))?;
    let key_pair = Ed25519KeyPair::from_pkcs8(document.as_ref())
        .map_err(|_| anyhow::anyhow!("could not load the generated Ed25519 key"))?;
    Ok(hex_encode(key_pair.public_key().as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_keys_are_32_byte_hex() {
        let key = generate_device_pubkey().unwrap();
        assert_eq!(key.len(), 64);
        assert!(key.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(key, generate_device_pubkey().unwrap());
    }

    #[test]
    fn approval_url_uses_the_server_origin() {
        let session = PairingSession {
            client: HttpClient::new("https://127.0.0.1:8443", TlsMode::AcceptAny).unwrap(),
            server_url: "https://127.0.0.1:8443/some/prefix".into(),
            start: PairStartResponse {
                pair_id: carrel_protocol::new_pair_id(),
                code: "K7Q-4MZ".into(),
                expires_at: 0,
            },
            device_pubkey: "ab".repeat(32),
        };
        assert_eq!(session.approval_url(), "https://127.0.0.1:8443/approve");
    }
}
