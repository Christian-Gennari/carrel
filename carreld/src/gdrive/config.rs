//! Google OAuth application credentials and the Google Drive part of
//! `<state>/sync.toml`.
//!
//! The client ID/secret belong to the image, not to the user, so they live in
//! `oauth.toml` (outside git). The refresh token is a user credential, so it
//! is stored in the existing `sync.toml` (mode 0600) under `[gdrive]`, next to
//! the self-hosted server settings.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{read_document, write_private, SyncConfig};

/// Environment override for [`OAuthApp::load`]. Tests point it at a temporary
/// file; self-builders use it to keep their own client next to the state.
pub const OAUTH_CONFIG_ENV: &str = "CARREL_OAUTH_CONFIG";

/// User-provided credentials win over the copy built into the image.
pub const DATA_OAUTH_CONFIG: &str = "/data/.carrel/oauth.toml";
/// The copy built into the Carrel image.
pub const IMAGE_OAUTH_CONFIG: &str = "/usr/share/carrel/oauth.toml";

/// The `[gdrive]` table in `sync.toml`.
const GDRIVE_SECTION: &str = "gdrive";

/// A Google OAuth client ("TVs and Limited Input devices").
#[derive(Clone, PartialEq, Eq)]
pub struct OAuthApp {
    pub client_id: String,
    pub client_secret: String,
}

impl fmt::Debug for OAuthApp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthApp")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .finish()
    }
}

/// Shape of `oauth.toml`:
///
/// ```toml
/// [google]
/// client_id = "…apps.googleusercontent.com"
/// client_secret = "…"
/// ```
#[derive(Debug, Deserialize)]
struct OAuthFile {
    google: OAuthFileGoogle,
}

#[derive(Debug, Deserialize)]
struct OAuthFileGoogle {
    client_id: String,
    client_secret: String,
}

impl OAuthApp {
    /// Load credentials from `$CARREL_OAUTH_CONFIG`, then
    /// [`DATA_OAUTH_CONFIG`], then [`IMAGE_OAUTH_CONFIG`].
    pub fn load() -> Result<Self> {
        if let Some(path) = std::env::var_os(OAUTH_CONFIG_ENV) {
            return Self::load_from(Path::new(&path)).with_context(|| {
                format!(
                    "load the Google OAuth client from {} ({OAUTH_CONFIG_ENV})",
                    Path::new(&path).display()
                )
            });
        }
        for path in [DATA_OAUTH_CONFIG, IMAGE_OAUTH_CONFIG] {
            let path = Path::new(path);
            if path.exists() {
                return Self::load_from(path);
            }
        }
        bail!(
            "Google Drive is not set up on this device: no oauth.toml at \
             {DATA_OAUTH_CONFIG} or {IMAGE_OAUTH_CONFIG}"
        )
    }

    /// Load one explicit `oauth.toml`.
    pub fn load_from(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let file: OAuthFile =
            toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        let client_id = file.google.client_id.trim().to_string();
        let client_secret = file.google.client_secret.trim().to_string();
        if client_id.is_empty() || client_secret.is_empty() {
            bail!("{} has an empty Google client ID or secret", path.display());
        }
        Ok(Self {
            client_id,
            client_secret,
        })
    }
}

/// The Google Drive part of `sync.toml`.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GdriveConfig {
    /// Long-lived token from the device authorization flow.
    pub refresh_token: String,
    /// Also upload a `.docx` copy of every file, when a converter is installed.
    #[serde(default)]
    pub docx_copy: bool,
}

impl fmt::Debug for GdriveConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GdriveConfig")
            .field("refresh_token", &"<redacted>")
            .field("docx_copy", &self.docx_copy)
            .finish()
    }
}

impl GdriveConfig {
    /// Read `[gdrive]` from `<state>/sync.toml`, or `None` when Google Drive is
    /// not paired. A missing file is not an error.
    pub fn load(state_dir: &Path) -> Result<Option<Self>> {
        let path = SyncConfig::path(state_dir);
        let document = read_document(&path)?;
        let Some(section) = document.get(GDRIVE_SECTION) else {
            return Ok(None);
        };
        let config: Self = section
            .clone()
            .try_into()
            .with_context(|| format!("parse [{GDRIVE_SECTION}] in {}", path.display()))?;
        if config.refresh_token.trim().is_empty() {
            bail!(
                "[{GDRIVE_SECTION}] in {} has an empty refresh token",
                path.display()
            );
        }
        Ok(Some(config))
    }

    /// True when Google Drive is paired.
    pub fn is_configured(state_dir: &Path) -> Result<bool> {
        Ok(Self::load(state_dir)?.is_some())
    }

    /// Store `[gdrive]` without touching the self-hosted server settings.
    pub fn save(&self, state_dir: &Path) -> Result<()> {
        if self.refresh_token.trim().is_empty() {
            bail!("refusing to store an empty Google refresh token");
        }
        std::fs::create_dir_all(state_dir)
            .with_context(|| format!("create {}", state_dir.display()))?;
        let path = SyncConfig::path(state_dir);
        let mut document = read_document(&path)?;
        let value = toml::Value::try_from(self).context("serialize [gdrive]")?;
        document.insert(GDRIVE_SECTION.to_string(), value);
        let text =
            toml::to_string_pretty(&toml::Value::Table(document)).context("serialize sync.toml")?;
        write_private(&path, &text)
    }

    /// Path of the file that stores this section. Used in messages.
    pub fn path(state_dir: &Path) -> PathBuf {
        SyncConfig::path(state_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_oauth(dir: &Path, client_id: &str) -> PathBuf {
        let path = dir.join("oauth.toml");
        std::fs::write(
            &path,
            format!("[google]\nclient_id = \"{client_id}\"\nclient_secret = \"secret\"\n"),
        )
        .unwrap();
        path
    }

    #[test]
    fn loads_the_google_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_oauth(dir.path(), "id-1");
        let app = OAuthApp::load_from(&path).unwrap();
        assert_eq!(app.client_id, "id-1");
        assert_eq!(app.client_secret, "secret");
    }

    #[test]
    fn rejects_an_empty_client() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth.toml");
        std::fs::write(&path, "[google]\nclient_id = \"\"\nclient_secret = \"x\"\n").unwrap();
        assert!(OAuthApp::load_from(&path).is_err());
    }

    #[test]
    fn gdrive_section_round_trips_with_0600() {
        let dir = tempfile::tempdir().unwrap();
        assert!(GdriveConfig::load(dir.path()).unwrap().is_none());
        let config = GdriveConfig {
            refresh_token: "refresh-1".into(),
            docx_copy: true,
        };
        config.save(dir.path()).unwrap();
        assert_eq!(GdriveConfig::load(dir.path()).unwrap(), Some(config));
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(GdriveConfig::path(dir.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn saving_gdrive_keeps_the_server_section() {
        let dir = tempfile::tempdir().unwrap();
        let server = SyncConfig {
            server_url: "https://127.0.0.1:8443".into(),
            token: "server-token".into(),
            cert_sha256: None,
            server_name: Some("Study".into()),
            device_pubkey: "ab".repeat(32),
        };
        server.save(dir.path()).unwrap();
        GdriveConfig {
            refresh_token: "refresh-1".into(),
            docx_copy: false,
        }
        .save(dir.path())
        .unwrap();
        assert_eq!(SyncConfig::load(dir.path()).unwrap(), server);

        // And saving the server again keeps Google Drive paired.
        server.save(dir.path()).unwrap();
        assert_eq!(
            GdriveConfig::load(dir.path())
                .unwrap()
                .unwrap()
                .refresh_token,
            "refresh-1"
        );
    }

    #[test]
    fn debug_redacts_credentials() {
        let app = OAuthApp {
            client_id: "id".into(),
            client_secret: "super-secret".into(),
        };
        assert!(!format!("{app:?}").contains("super-secret"));
        let config = GdriveConfig {
            refresh_token: "refresh-secret".into(),
            docx_copy: true,
        };
        let text = format!("{config:?}");
        assert!(!text.contains("refresh-secret"), "{text}");
        assert!(text.contains("docx_copy"), "{text}");
    }

    #[test]
    fn rejects_an_empty_refresh_token() {
        let dir = tempfile::tempdir().unwrap();
        let config = GdriveConfig {
            refresh_token: "  ".into(),
            docx_copy: false,
        };
        assert!(config.save(dir.path()).is_err());
    }
}
