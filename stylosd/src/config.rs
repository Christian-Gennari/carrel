//! The paired-server configuration at `<state>/sync.toml`.
//!
//! Contains the bearer token, so the file is created with mode 0600. The token
//! is never printed in logs or `Debug` output.

use std::fmt;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::tls::{validate_fingerprint, TlsMode};

/// Everything needed to sync with a paired server.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncConfig {
    pub server_url: String,
    pub token: String,
    /// SHA-256 of the server certificate, when the server asked to be pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_sha256: Option<String>,
    /// Human server name reported during pairing, used in conflict names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    /// Ed25519 public key generated at pairing time.
    #[serde(default)]
    pub device_pubkey: String,
}

impl fmt::Debug for SyncConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncConfig")
            .field("server_url", &self.server_url)
            .field("token", &"<redacted>")
            .field("cert_sha256", &self.cert_sha256)
            .field("server_name", &self.server_name)
            .field("device_pubkey", &self.device_pubkey)
            .finish()
    }
}

impl SyncConfig {
    /// `<state>/sync.toml`.
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("sync.toml")
    }

    /// Load the configuration, with clear errors for a missing or broken file.
    pub fn load(state_dir: &Path) -> Result<Self> {
        let path = Self::path(state_dir);
        let text = fs::read_to_string(&path).with_context(|| {
            format!(
                "read {}: the device is not paired (run `stylosd pair --url …`)",
                path.display()
            )
        })?;
        let config: Self =
            toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        if config.server_url.trim().is_empty() {
            bail!("{} has an empty server_url", path.display());
        }
        if config.token.trim().is_empty() {
            bail!("{} has an empty token", path.display());
        }
        if let Some(fingerprint) = &config.cert_sha256 {
            validate_fingerprint(fingerprint)
                .with_context(|| format!("{} has an invalid certificate pin", path.display()))?;
        }
        Ok(config)
    }

    /// Write the file with mode 0600 (token inside).
    pub fn save(&self, state_dir: &Path) -> Result<()> {
        fs::create_dir_all(state_dir).with_context(|| format!("create {}", state_dir.display()))?;
        fs::set_permissions(state_dir, fs::Permissions::from_mode(0o700)).ok();
        let path = Self::path(state_dir);
        let text = toml::to_string_pretty(self).context("serialize sync.toml")?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("write {}", path.display()))?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).ok();
        Ok(())
    }

    /// Label used in conflict-copy names: the server name, or its host.
    pub fn source_label(&self) -> String {
        if let Some(name) = &self.server_name {
            let name = name.trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
        Url::parse(&self.server_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .filter(|host| !host.is_empty())
            .unwrap_or_else(|| "server".to_string())
    }

    /// TLS trust decision for this server.
    pub fn tls_mode(&self) -> TlsMode {
        match &self.cert_sha256 {
            Some(fingerprint) => TlsMode::Pinned(fingerprint.clone()),
            None => TlsMode::SystemRoots,
        }
    }
}

/// Path of the sync state database inside the state directory.
pub fn state_database(state_dir: &Path) -> PathBuf {
    state_dir.join("sync-state.sqlite")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SyncConfig {
        SyncConfig {
            server_url: "https://127.0.0.1:8443".into(),
            token: "secret-token".into(),
            cert_sha256: Some("ab".repeat(32)),
            server_name: Some("Study".into()),
            device_pubkey: "cd".repeat(32),
        }
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let config = sample();
        config.save(dir.path()).unwrap();
        let loaded = SyncConfig::load(dir.path()).unwrap();
        assert_eq!(config, loaded);
        let mode = fs::metadata(SyncConfig::path(dir.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn debug_redacts_the_token() {
        let text = format!("{:?}", sample());
        assert!(!text.contains("secret-token"));
        assert!(text.contains("Study"));
    }

    #[test]
    fn missing_file_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let error = SyncConfig::load(dir.path()).unwrap_err().to_string();
        assert!(error.contains("not paired"), "{error}");
    }

    #[test]
    fn invalid_pin_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = sample();
        config.cert_sha256 = Some("nope".into());
        config.save(dir.path()).unwrap();
        assert!(SyncConfig::load(dir.path()).is_err());
    }

    #[test]
    fn source_label_prefers_server_name() {
        let mut config = sample();
        assert_eq!(config.source_label(), "Study");
        config.server_name = None;
        assert_eq!(config.source_label(), "127.0.0.1");
    }

    #[test]
    fn tls_mode_uses_the_pin() {
        let config = sample();
        assert!(matches!(config.tls_mode(), TlsMode::Pinned(_)));
        let mut unpinned = sample();
        unpinned.cert_sha256 = None;
        assert!(matches!(unpinned.tls_mode(), TlsMode::SystemRoots));
    }
}
