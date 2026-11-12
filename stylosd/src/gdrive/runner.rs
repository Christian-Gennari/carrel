//! Backends for Google Drive: pairing uses [`OAuthClient`], syncing uses
//! [`DriveSyncRunner`] and the real API clients. The trait keeps the daemon
//! testable without network access.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;

use crate::gdrive::api::{DriveClient, DriveEndpoints};
use crate::gdrive::config::{GdriveConfig, OAuthApp};
use crate::gdrive::mirror::{mirror_once, DocxConverter, MirrorOptions};
use crate::gdrive::oauth::{OAuthClient, OAuthEndpoints};
use crate::gdrive::state::GdriveState;

/// What the daemon needs from the Google Drive backend.
#[async_trait]
pub trait GdriveRunner: Send + Sync {
    /// One mirror run inside the daemon's existing radio window.
    async fn sync(
        &self,
        writing_dir: &Path,
        state_dir: &Path,
    ) -> Result<crate::gdrive::GdriveReport>;

    /// Best-effort token revoke on Unpair. Not being able to reach Google must
    /// not keep the user paired locally.
    async fn revoke(&self, state_dir: &Path) -> Result<()> {
        let _ = state_dir;
        Ok(())
    }
}

/// The real backend: refresh the access token, then mirror.
#[derive(Default)]
pub struct DriveSyncRunner {
    /// Set by the export work package once the real converter exists.
    converter: Option<Arc<dyn DocxConverter>>,
    /// Explicit OAuth client credentials (the `--oauth` CLI flag). `None`
    /// keeps the image/default lookup of [`OAuthApp::load`].
    app: Option<OAuthApp>,
}

impl DriveSyncRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Install the real `.docx` converter. Until then, `docx_copy` is skipped.
    pub fn with_converter(converter: Arc<dyn DocxConverter>) -> Self {
        Self {
            converter: Some(converter),
            app: None,
        }
    }

    /// Use explicit OAuth client credentials instead of [`OAuthApp::load`].
    pub fn with_app(mut self, app: OAuthApp) -> Self {
        self.app = Some(app);
        self
    }

    fn oauth_client(&self) -> Result<OAuthClient> {
        let app = match &self.app {
            Some(app) => app.clone(),
            None => OAuthApp::load()?,
        };
        OAuthClient::new(app, OAuthEndpoints::from_env_or_default())
    }
}

#[async_trait]
impl GdriveRunner for DriveSyncRunner {
    async fn sync(
        &self,
        writing_dir: &Path,
        state_dir: &Path,
    ) -> Result<crate::gdrive::GdriveReport> {
        let config =
            GdriveConfig::load(state_dir)?.context("Google Drive is not paired on this device")?;
        let oauth = self.oauth_client()?;
        let access_token = oauth.refresh(&config.refresh_token).await?;
        let api = DriveClient::new(DriveEndpoints::from_env_or_default(), access_token)?;
        let mut state = GdriveState::from_dir(state_dir)?;
        let options = MirrorOptions {
            docx_copy: config.docx_copy,
        };
        mirror_once(
            &api,
            writing_dir,
            &mut state,
            &options,
            self.converter.as_deref(),
        )
        .await
    }

    async fn revoke(&self, state_dir: &Path) -> Result<()> {
        let Some(config) = GdriveConfig::load(state_dir)? else {
            return Ok(());
        };
        let oauth = self.oauth_client()?;
        oauth.revoke(&config.refresh_token).await
    }
}
