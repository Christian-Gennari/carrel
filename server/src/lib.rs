//! `carrel-server`: a self-hosted Carrel sync server.
//!
//! - Files are plain Markdown below the data directory; the SQLite index only
//!   stores revisions, hashes, tombstones and the cursor.
//! - Pairing happens on a minimal HTML page at `/approve`, protected by an
//!   admin password.
//! - TLS is self-signed, generated on first run; the certificate fingerprint is
//!   returned during pairing so devices can pin it.
//! - The service is announced on the LAN with mDNS (`_carrel._tcp`).

pub mod api;
pub mod approve;
pub mod store;
pub mod tls;

use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use carrel_protocol::{MAX_FILE_BYTES, MDNS_SERVICE_TYPE};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::api::AppState;
use crate::store::{random_csrf_token, Store};
use crate::tls::TlsListener;

/// Configuration for a server instance.
///
/// Everything has a sane default; `Config::new` is enough to run a server on
/// `/data` and `0.0.0.0:8443` with mDNS and a generated admin password.
#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub listen: SocketAddr,
    /// Password for the `/approve` page. When `None`, the password is read
    /// from `<data_dir>/.carrel/admin-password`, or generated and stored there.
    pub admin_password: Option<String>,
    /// Human name announced over mDNS and used in conflict-copy names.
    pub label: Option<String>,
    pub mdns: bool,
    pub max_file_bytes: usize,
}

impl Config {
    /// Config for `data_dir` with the defaults documented on the type.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            listen: SocketAddr::from(([0, 0, 0, 0], 8443)),
            admin_password: None,
            label: None,
            mdns: true,
            max_file_bytes: MAX_FILE_BYTES,
        }
    }
}

/// A running server. Hold it for as long as the server should run; call
/// [`RunningServer::shutdown`] to stop it cleanly.
pub struct RunningServer {
    pub addr: SocketAddr,
    /// Lowercase hex SHA-256 of the TLS certificate's DER encoding.
    pub cert_sha256: String,
    pub admin_password: String,
    pub label: String,
    handle: Option<JoinHandle<()>>,
    shutdown: Option<oneshot::Sender<()>>,
    mdns: Option<ServiceDaemon>,
}

impl RunningServer {
    /// URL to reach this server from the same machine.
    pub fn local_url(&self) -> String {
        let ip = if self.addr.ip().is_unspecified() {
            std::net::IpAddr::from([127, 0, 0, 1])
        } else {
            self.addr.ip()
        };
        format!("https://{ip}:{}", self.addr.port())
    }

    /// URL of the approval page on this server.
    pub fn approve_url(&self) -> String {
        format!("{}/approve", self.local_url())
    }

    /// Stop accepting connections and wait for the server task to finish.
    pub async fn shutdown(mut self) {
        self.stop();
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }

    fn stop(&mut self) {
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(());
        }
        if let Some(daemon) = self.mdns.take() {
            let _ = daemon.shutdown();
        }
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Bind, generate/load TLS, register mDNS and start serving.
///
/// Returns as soon as the listener is bound, so tests can use port `0` and
/// read the assigned port from [`RunningServer::addr`].
pub async fn start(config: Config) -> Result<RunningServer> {
    fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("create data directory {}", config.data_dir.display()))?;
    let meta_dir = config.data_dir.join(".carrel");
    fs::create_dir_all(&meta_dir)?;
    fs::set_permissions(&meta_dir, fs::Permissions::from_mode(0o700)).ok();

    let label = config.label.clone().unwrap_or_else(default_label);
    let admin_password =
        resolve_admin_password(&config.data_dir, config.admin_password.as_deref())?;

    let material = tls::load_or_generate(&meta_dir.join("tls"), &label)?;
    let cert_sha256 = material.fingerprint_hex.clone();
    let acceptor = tls::acceptor(&material)?;
    drop(material);

    let store = Store::open(&config.data_dir)?;
    let state = AppState::new(
        store,
        admin_password.clone(),
        random_csrf_token(),
        cert_sha256.clone(),
        label.clone(),
        config.max_file_bytes,
    );
    let app = api::router(state);

    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("bind {}", config.listen))?;
    let addr = listener.local_addr()?;

    let mdns = if config.mdns {
        match register_mdns(&label, addr.port()) {
            Ok(daemon) => Some(daemon),
            Err(error) => {
                tracing::warn!(%error, "mDNS announcement failed; continuing without it");
                None
            }
        }
    } else {
        None
    };

    let tls_listener = TlsListener::new(listener, acceptor);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        let result = axum::serve(tls_listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await;
        if let Err(error) = result {
            tracing::error!(%error, "server stopped with an error");
        }
    });

    tracing::info!(%addr, cert = %cert_sha256, "carrel-server listening");
    Ok(RunningServer {
        addr,
        cert_sha256,
        admin_password,
        label,
        handle: Some(handle),
        shutdown: Some(shutdown_tx),
        mdns,
    })
}

fn register_mdns(label: &str, port: u16) -> Result<ServiceDaemon> {
    let daemon = ServiceDaemon::new().context("start mDNS daemon")?;
    let host_name = format!("{}.local.", tls::dns_label(label));
    let properties = HashMap::from([
        (
            "v".to_string(),
            carrel_protocol::PROTOCOL_VERSION.to_string(),
        ),
        ("name".to_string(), label.to_string()),
    ]);
    let info = ServiceInfo::new(MDNS_SERVICE_TYPE, label, &host_name, (), port, properties)
        .context("build mDNS service info")?
        .enable_addr_auto();
    daemon.register(info).context("register mDNS service")?;
    Ok(daemon)
}

fn default_label() -> String {
    if let Ok(label) = std::env::var("CARREL_LABEL") {
        let label = label.trim();
        if !label.is_empty() {
            return label.to_string();
        }
    }
    if let Ok(name) = std::env::var("HOSTNAME") {
        let name = name
            .trim()
            .trim_end_matches(".local")
            .trim_end_matches(".local.");
        if !name.is_empty() {
            return name.to_string();
        }
    }
    if let Ok(name) = fs::read_to_string("/etc/hostname") {
        let name = name.trim();
        if !name.is_empty() {
            return name.trim_end_matches(".local").to_string();
        }
    }
    "carrel-server".to_string()
}

fn resolve_admin_password(data_dir: &Path, provided: Option<&str>) -> Result<String> {
    if let Some(password) = provided {
        let password = password.trim();
        if !password.is_empty() {
            return Ok(password.to_string());
        }
    }
    let path = data_dir.join(".carrel").join("admin-password");
    if let Ok(existing) = fs::read_to_string(&path) {
        let existing = existing.trim();
        if !existing.is_empty() {
            return Ok(existing.to_string());
        }
    }
    let password = store::random_token()[..16].to_string();
    fs::write(&path, format!("{password}\n"))
        .with_context(|| format!("write {}", path.display()))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).ok();
    Ok(password)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn server_starts_and_exposes_a_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::new(dir.path());
        config.listen = SocketAddr::from(([127, 0, 0, 1], 0));
        config.mdns = false;
        config.admin_password = Some("secret".into());
        config.label = Some("test-server".into());
        let server = start(config).await.unwrap();
        assert_eq!(server.cert_sha256.len(), 64);
        assert_eq!(server.admin_password, "secret");
        assert_eq!(server.label, "test-server");
        assert!(server.approve_url().ends_with("/approve"));
        server.shutdown().await;
    }

    #[test]
    fn admin_password_is_generated_once() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".carrel")).unwrap();
        let first = resolve_admin_password(dir.path(), None).unwrap();
        let second = resolve_admin_password(dir.path(), None).unwrap();
        assert_eq!(first, second);
        assert!(!first.is_empty());
        let provided = resolve_admin_password(dir.path(), Some("other")).unwrap();
        assert_eq!(provided, "other");
    }

    #[test]
    fn labels_are_non_empty() {
        assert!(!default_label().is_empty());
    }
}
