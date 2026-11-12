//! Self-signed TLS material for the server.
//!
//! A certificate is generated on first run (rcgen, ring backend) and reused
//! afterwards, so the SHA-256 fingerprint devices pin does not change on every
//! restart. The fingerprint is read from the DER certificate, exactly what a
//! client sees in the TLS handshake.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use rcgen::{CertificateParams, DnType, KeyPair};
use rustls::pki_types::PrivateKeyDer;
use stylos_protocol::sha256_hex;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

/// Parsed certificate, key and fingerprint.
pub struct TlsMaterial {
    cert_der: Vec<u8>,
    key_der: PrivateKeyDer<'static>,
    /// Lowercase hex SHA-256 of `cert_der`.
    pub fingerprint_hex: String,
    pub certificate_pem: String,
}

/// Load the certificate below `dir`, generating it on first run.
pub fn load_or_generate(dir: &Path, label: &str) -> Result<TlsMaterial> {
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    if !cert_path.exists() || !key_path.exists() {
        generate(dir, label)?;
    }
    load(&cert_path, &key_path)
}

fn generate(dir: &Path, label: &str) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).ok();

    let mut names = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];
    let hostname = host_name();
    if !hostname.is_empty() && !names.contains(&hostname) {
        names.push(hostname);
    }
    let dns_label = dns_label(label);
    if dns_label != "stylos-server" && !names.contains(&dns_label) {
        names.push(dns_label);
    }

    let mut params = CertificateParams::new(names).context("build certificate parameters")?;
    params
        .distinguished_name
        .push(DnType::CommonName, "Stylos Server");
    params.is_ca = rcgen::IsCa::NoCa;

    let key_pair = KeyPair::generate().context("generate TLS key")?;
    let certificate = params
        .self_signed(&key_pair)
        .context("self-sign TLS certificate")?;

    write_private(&dir.join("key.pem"), key_pair.serialize_pem().as_bytes())?;
    write_private(&dir.join("cert.pem"), certificate.pem().as_bytes())?;
    Ok(())
}

fn load(cert_path: &Path, key_path: &Path) -> Result<TlsMaterial> {
    let cert_pem = fs::read(cert_path).with_context(|| format!("read {}", cert_path.display()))?;
    let key_pem = fs::read(key_path).with_context(|| format!("read {}", key_path.display()))?;

    let certs: Vec<_> = rustls_pemfile::certs(&mut cert_pem.as_slice())
        .collect::<io::Result<Vec<_>>>()
        .context("parse certificate PEM")?;
    let Some(cert_der) = certs.into_iter().next() else {
        bail!("{} contains no certificate", cert_path.display());
    };
    let key_der = rustls_pemfile::private_key(&mut key_pem.as_slice())
        .context("parse private key PEM")?
        .ok_or_else(|| anyhow::anyhow!("{} contains no private key", key_path.display()))?;

    let fingerprint_hex = sha256_hex(cert_der.as_ref());
    let certificate_pem = String::from_utf8(cert_pem).context("certificate PEM is not UTF-8")?;
    Ok(TlsMaterial {
        cert_der: cert_der.as_ref().to_vec(),
        key_der,
        fingerprint_hex,
        certificate_pem,
    })
}

/// Build a rustls acceptor from the material, using the ring provider so the
/// server does not need cmake or aws-lc.
pub fn acceptor(material: &TlsMaterial) -> Result<TlsAcceptor> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("rustls protocol versions")?
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(
                material.cert_der.clone(),
            )],
            material.key_der.clone_key(),
        )
        .context("rustls server certificate")?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

fn write_private(path: &PathBuf, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).ok();
    Ok(())
}

fn host_name() -> String {
    if let Ok(name) = std::env::var("HOSTNAME") {
        let name = name.trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    fs::read_to_string("/etc/hostname")
        .map(|name| name.trim().to_string())
        .unwrap_or_default()
}

/// Turn a human label into a DNS-safe single label, for the certificate SAN
/// and the mDNS host name.
pub fn dns_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for character in label.chars() {
        if character.is_ascii_alphanumeric() {
            out.push(character.to_ascii_lowercase());
        } else if character == '-' || character == ' ' || character == '_' {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "stylos-server".to_string()
    } else {
        trimmed
    }
}

/// A TCP listener that completes a TLS handshake before handing the stream to
/// axum. Failed handshakes (scanners, wrong pins) are logged and skipped: they
/// must not take the server down.
pub struct TlsListener {
    inner: TcpListener,
    acceptor: TlsAcceptor,
}

impl TlsListener {
    pub fn new(inner: TcpListener, acceptor: TlsAcceptor) -> Self {
        Self { inner, acceptor }
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<TcpStream>;
    type Addr = std::net::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.inner.accept().await {
                Ok((stream, addr)) => match self.acceptor.accept(stream).await {
                    Ok(tls) => return (tls, addr),
                    Err(error) => {
                        tracing::debug!(%error, %addr, "TLS handshake failed");
                    }
                },
                Err(error) => {
                    tracing::warn!(%error, "TCP accept failed");
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_and_reuses_a_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_generate(dir.path(), "Test Server").unwrap();
        assert_eq!(first.fingerprint_hex.len(), 64);
        assert!(dir.path().join("cert.pem").exists());
        assert!(dir.path().join("key.pem").exists());
        let second = load_or_generate(dir.path(), "Test Server").unwrap();
        assert_eq!(first.fingerprint_hex, second.fingerprint_hex);
        acceptor(&second).unwrap();
        let metadata = fs::metadata(dir.path().join("key.pem")).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn dns_labels_are_safe() {
        assert_eq!(dns_label("My Study Server"), "my-study-server");
        assert_eq!(dns_label("___"), "stylos-server");
        assert_eq!(dns_label(""), "stylos-server");
        assert_eq!(dns_label("a.b/c"), "abc");
    }
}
