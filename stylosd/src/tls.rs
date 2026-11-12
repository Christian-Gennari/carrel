//! TLS for the client.
//!
//! Three modes:
//! - `Pinned`: accept exactly the certificate whose DER SHA-256 matches the
//!   fingerprint obtained at pairing. This is what a self-hosted server uses;
//!   hostnames and CA chains are irrelevant because the certificate is pinned.
//! - `AcceptAny`: used only while pairing, before a fingerprint exists. The
//!   observed fingerprint is recorded and must match the one the server
//!   reports, so a man-in-the-middle cannot silently swap certificates.
//! - `SystemRoots`: normal CA verification for hosted services such as Nostos.

use std::fmt;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error, RootCertStore, SignatureScheme};
use stylos_protocol::{decode_sha256_hex, sha256_hex};

/// How the client decides whether to trust the server certificate.
#[derive(Debug, Clone)]
pub enum TlsMode {
    /// Trust only this SHA-256 fingerprint (lowercase or uppercase hex).
    Pinned(String),
    /// Accept the certificate and record its fingerprint. Pairing only.
    AcceptAny,
    /// Normal web PKI roots (hosted servers).
    SystemRoots,
}

/// Fingerprint observed during the TLS handshake, if recording was requested.
#[derive(Clone, Default)]
pub struct ObservedCert {
    inner: Arc<Mutex<Option<String>>>,
}

impl ObservedCert {
    /// Lowercase hex SHA-256 of the leaf certificate seen on the wire.
    pub fn get(&self) -> Option<String> {
        self.inner.lock().ok().and_then(|value| value.clone())
    }

    fn record(&self, fingerprint: String) {
        if let Ok(mut value) = self.inner.lock() {
            *value = Some(fingerprint);
        }
    }
}

/// Build a rustls client config for the given mode, plus the observation slot.
pub fn client_config(mode: &TlsMode) -> Result<(rustls::ClientConfig, ObservedCert)> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let observed = ObservedCert::default();
    let verifier: Arc<dyn ServerCertVerifier> = match mode {
        TlsMode::Pinned(fingerprint) => {
            let pin = decode_sha256_hex(fingerprint)
                .with_context(|| format!("invalid certificate fingerprint {fingerprint:?}"))?;
            Arc::new(PinnedVerifier {
                pin,
                provider: provider.clone(),
            })
        }
        TlsMode::AcceptAny => Arc::new(RecordingVerifier {
            provider: provider.clone(),
            observed: observed.clone(),
        }),
        TlsMode::SystemRoots => {
            let mut roots = RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let inner =
                WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
                    .build()
                    .context("build web PKI verifier")?;
            Arc::new(SystemVerifier { inner })
        }
    };

    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("rustls protocol versions")?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok((config, observed))
}

fn verify_signature(
    provider: &CryptoProvider,
    message: &[u8],
    certificate: &CertificateDer<'_>,
    signature: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, Error> {
    rustls::crypto::verify_tls13_signature(
        message,
        certificate,
        signature,
        &provider.signature_verification_algorithms,
    )
}

fn verify_signature_tls12(
    provider: &CryptoProvider,
    message: &[u8],
    certificate: &CertificateDer<'_>,
    signature: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, Error> {
    rustls::crypto::verify_tls12_signature(
        message,
        certificate,
        signature,
        &provider.signature_verification_algorithms,
    )
}

struct PinnedVerifier {
    pin: [u8; 32],
    provider: Arc<CryptoProvider>,
}

impl fmt::Debug for PinnedVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PinnedVerifier")
    }
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let observed = decode_hex_digest(end_entity.as_ref());
        if observed == self.pin {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(Error::General(
                "server certificate does not match the pinned fingerprint".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_signature_tls12(&self.provider, message, certificate, signature)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_signature(&self.provider, message, certificate, signature)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

struct RecordingVerifier {
    provider: Arc<CryptoProvider>,
    observed: ObservedCert,
}

impl fmt::Debug for RecordingVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecordingVerifier")
    }
}

impl ServerCertVerifier for RecordingVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        self.observed.record(sha256_hex(end_entity.as_ref()));
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_signature_tls12(&self.provider, message, certificate, signature)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_signature(&self.provider, message, certificate, signature)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

struct SystemVerifier {
    inner: Arc<WebPkiServerVerifier>,
}

impl fmt::Debug for SystemVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SystemVerifier")
    }
}

impl ServerCertVerifier for SystemVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        self.inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner
            .verify_tls12_signature(message, certificate, signature)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner
            .verify_tls13_signature(message, certificate, signature)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

fn decode_hex_digest(bytes: &[u8]) -> [u8; 32] {
    let hex = sha256_hex(bytes);
    decode_sha256_hex(&hex).expect("sha256_hex always produces a 32-byte digest")
}

/// Verify that a fingerprint string is a SHA-256 digest; used before saving.
pub fn validate_fingerprint(fingerprint: &str) -> Result<()> {
    if decode_sha256_hex(fingerprint).is_err() {
        bail!("{fingerprint:?} is not a SHA-256 certificate fingerprint");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_config_rejects_bad_hex() {
        assert!(client_config(&TlsMode::Pinned("zz".into())).is_err());
        assert!(client_config(&TlsMode::Pinned("00".repeat(32))).is_ok());
    }

    #[test]
    fn system_and_recording_configs_build() {
        assert!(client_config(&TlsMode::SystemRoots).is_ok());
        let (_, observed) = client_config(&TlsMode::AcceptAny).unwrap();
        assert!(observed.get().is_none());
    }

    #[test]
    fn fingerprint_validation() {
        assert!(validate_fingerprint(&"ab".repeat(32)).is_ok());
        assert!(validate_fingerprint("nope").is_err());
    }
}
