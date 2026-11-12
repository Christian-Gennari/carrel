//! OAuth 2.0 device authorization flow for Google Drive.
//!
//! Google's "TV and limited input devices" client lets the device show a short
//! code and a URL; the user approves on a phone. The scope is exactly
//! `drive.file`, so Stylos can only see the files it created and Google does
//! not require a verification review.
//!
//! The refresh token is the only long-lived credential. Access tokens are
//! fetched at the start of every sync and never stored.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::gdrive::config::OAuthApp;

/// The only scope Stylos requests.
pub const DRIVE_FILE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";

/// The device grant type from RFC 8628.
pub const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// RFC 8628 says to add five seconds after `slow_down`.
pub const SLOW_DOWN_PENALTY: Duration = Duration::from_secs(5);

/// Never poll faster than this, even if a server sends `interval = 0`.
const MIN_POLL: Duration = Duration::from_millis(100);

/// OAuth endpoints. API base URLs are injectable so tests can use a local
/// mock instead of Google.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthEndpoints {
    pub device_code: String,
    pub token: String,
    pub revoke: String,
}

impl Default for OAuthEndpoints {
    fn default() -> Self {
        Self {
            device_code: "https://oauth2.googleapis.com/device/code".into(),
            token: "https://oauth2.googleapis.com/token".into(),
            revoke: "https://oauth2.googleapis.com/revoke".into(),
        }
    }
}

impl OAuthEndpoints {
    /// Endpoints under one mock base URL, used by tests:
    /// `<base>/oauth/device/code`, `<base>/oauth/token`, `<base>/oauth/revoke`.
    pub fn for_base_url(base: &str) -> Self {
        let base = base.trim_end_matches('/');
        Self {
            device_code: format!("{base}/oauth/device/code"),
            token: format!("{base}/oauth/token"),
            revoke: format!("{base}/oauth/revoke"),
        }
    }

    /// Google's endpoints, or overrides from the environment for tests:
    /// `STYLOS_GDRIVE_BASE_URL` or the individual `*_URL` variables.
    pub fn from_env_or_default() -> Self {
        let base = std::env::var("STYLOS_GDRIVE_BASE_URL")
            .ok()
            .map(|base| base.trim_end_matches('/').to_string())
            .filter(|base| !base.is_empty());
        let pick = |name: &str, suffix: &str, default: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .or_else(|| base.as_ref().map(|base| format!("{base}{suffix}")))
                .unwrap_or_else(|| default.to_string())
        };
        let default = Self::default();
        Self {
            device_code: pick(
                "STYLOS_GDRIVE_DEVICE_CODE_URL",
                "/oauth/device/code",
                &default.device_code,
            ),
            token: pick("STYLOS_GDRIVE_TOKEN_URL", "/oauth/token", &default.token),
            revoke: pick("STYLOS_GDRIVE_REVOKE_URL", "/oauth/revoke", &default.revoke),
        }
    }
}

/// The device code response from Google.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    #[serde(alias = "verification_uri")]
    pub verification_url: String,
    pub expires_in: i64,
    #[serde(default)]
    pub interval: u64,
}

/// A successful token response. `refresh_token` is present on the first
/// device-flow exchange, not on later refreshes.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
}

/// The meaningful failures of a device authorization.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("the user has not approved the code yet")]
    Pending,
    #[error("Google asked the device to poll more slowly")]
    SlowDown,
    #[error("the user denied access to Google Drive")]
    AccessDenied,
    #[error("the Google Drive code expired before it was approved")]
    Expired,
    #[error("{0}")]
    Rejected(String),
}

/// A small OAuth client bound to one Google client ID.
pub struct OAuthClient {
    app: OAuthApp,
    endpoints: OAuthEndpoints,
    http: reqwest::Client,
    slow_down_penalty: Duration,
}

impl OAuthClient {
    pub fn new(app: OAuthApp, endpoints: OAuthEndpoints) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(format!("stylosd/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .context("build the OAuth HTTP client")?;
        Ok(Self {
            app,
            endpoints,
            http,
            slow_down_penalty: SLOW_DOWN_PENALTY,
        })
    }

    /// Tests shorten the RFC 8628 penalty to keep the suite fast.
    pub fn with_slow_down_penalty(mut self, penalty: Duration) -> Self {
        self.slow_down_penalty = penalty;
        self
    }

    /// `POST <device_code>`: ask Google for a code to show the user.
    pub async fn start_device_flow(&self) -> Result<DeviceCode> {
        let response = self
            .http
            .post(&self.endpoints.device_code)
            .form(&[
                ("client_id", self.app.client_id.as_str()),
                ("scope", DRIVE_FILE_SCOPE),
            ])
            .send()
            .await
            .context("ask Google for a device code")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("read the device code reply")?;
        if !status.is_success() {
            bail!(
                "Google refused the device code request: HTTP {}: {}",
                status.as_u16(),
                describe(&body)
            );
        }
        let code: DeviceCode =
            serde_json::from_str(&body).context("decode the Google device code reply")?;
        if code.device_code.trim().is_empty()
            || code.user_code.trim().is_empty()
            || code.verification_url.trim().is_empty()
        {
            bail!("Google returned an incomplete device code");
        }
        Ok(code)
    }

    /// One poll of `POST <token>` with the device grant.
    pub async fn poll_token(&self, device_code: &str) -> Result<TokenResponse, OAuthError> {
        let response = self
            .http
            .post(&self.endpoints.token)
            .form(&[
                ("client_id", self.app.client_id.as_str()),
                ("client_secret", self.app.client_secret.as_str()),
                ("device_code", device_code),
                ("grant_type", DEVICE_CODE_GRANT),
            ])
            .send()
            .await
            .map_err(|error| OAuthError::Rejected(format!("token request failed: {error}")))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| OAuthError::Rejected(format!("read token reply: {error}")))?;
        if !status.is_success() {
            return Err(device_error(&body));
        }
        let tokens: TokenResponse = serde_json::from_str(&body)
            .map_err(|error| OAuthError::Rejected(format!("decode token reply: {error}")))?;
        if tokens.access_token.trim().is_empty() {
            return Err(OAuthError::Rejected(
                "Google returned an empty access token".into(),
            ));
        }
        Ok(tokens)
    }

    /// Poll until approval, denial, expiry or `timeout`.
    ///
    /// `interval` is honoured; `slow_down` adds [`SLOW_DOWN_PENALTY`] to every
    /// later wait, as RFC 8628 requires.
    pub async fn wait_for_approval(
        &self,
        device: &DeviceCode,
        timeout: Duration,
    ) -> Result<TokenResponse> {
        let deadline = Instant::now() + timeout;
        let mut interval = Duration::from_secs(device.interval).max(MIN_POLL);
        loop {
            match self.poll_token(&device.device_code).await {
                Ok(tokens) => return Ok(tokens),
                Err(OAuthError::Pending) => {}
                Err(OAuthError::SlowDown) => interval += self.slow_down_penalty,
                Err(error) => bail!(error),
            }
            let now = Instant::now();
            if now >= deadline {
                bail!("timed out waiting for the Google Drive code to be approved");
            }
            tokio::time::sleep(interval.min(deadline.saturating_duration_since(now))).await;
        }
    }

    /// Exchange the stored refresh token for a fresh access token.
    pub async fn refresh(&self, refresh_token: &str) -> Result<String> {
        if refresh_token.trim().is_empty() {
            bail!("the stored Google Drive refresh token is empty");
        }
        let response = self
            .http
            .post(&self.endpoints.token)
            .form(&[
                ("client_id", self.app.client_id.as_str()),
                ("client_secret", self.app.client_secret.as_str()),
                ("refresh_token", refresh_token),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .context("refresh the Google Drive access token")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("read the token refresh reply")?;
        if !status.is_success() {
            let error = device_error(&body);
            if matches!(error, OAuthError::Rejected(_)) && body.contains("invalid_grant") {
                bail!("Google Drive access was revoked or expired; pair Google Drive again");
            }
            bail!(error);
        }
        let tokens: TokenResponse =
            serde_json::from_str(&body).context("decode the token refresh reply")?;
        if tokens.access_token.trim().is_empty() {
            bail!("Google returned an empty access token");
        }
        Ok(tokens.access_token)
    }

    /// Revoke the refresh token, so Unpair really disconnects the account.
    /// A token Google already considers invalid is fine.
    pub async fn revoke(&self, token: &str) -> Result<()> {
        let response = self
            .http
            .post(&self.endpoints.revoke)
            .timeout(Duration::from_secs(10))
            .form(&[("token", token)])
            .send()
            .await
            .context("revoke the Google Drive token")?;
        let status = response.status();
        if status.is_success() || status.as_u16() == 400 {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        bail!(
            "revoking the Google Drive token failed: HTTP {}: {}",
            status.as_u16(),
            describe(&body)
        );
    }
}

/// Map an OAuth error body to the error the caller acts on.
fn device_error(body: &str) -> OAuthError {
    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(value) => value,
        Err(_) => return OAuthError::Rejected(describe(body)),
    };
    let code = value
        .get("error")
        .and_then(|error| error.as_str())
        .unwrap_or_default();
    let description = value
        .get("error_description")
        .and_then(|text| text.as_str())
        .filter(|text| !text.is_empty());
    match code {
        "authorization_pending" => OAuthError::Pending,
        "slow_down" => OAuthError::SlowDown,
        "access_denied" => OAuthError::AccessDenied,
        "expired_token" => OAuthError::Expired,
        other => OAuthError::Rejected(match description {
            Some(description) => format!("Google rejected the request ({other}): {description}"),
            None => format!("Google rejected the request: {other}"),
        }),
    }
}

/// A short, safe description of an error body for one-line messages.
fn describe(body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(message) = value
            .get("error_description")
            .and_then(|text| text.as_str())
            .or_else(|| {
                value
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(|text| text.as_str())
            })
        {
            return message.chars().take(300).collect();
        }
        if let Some(code) = value.get("error").and_then(|error| error.as_str()) {
            return code.to_string();
        }
    }
    body.trim().chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_can_be_built_from_one_base_url() {
        let endpoints = OAuthEndpoints::for_base_url("http://127.0.0.1:1234/");
        assert_eq!(
            endpoints.device_code,
            "http://127.0.0.1:1234/oauth/device/code"
        );
        assert_eq!(endpoints.token, "http://127.0.0.1:1234/oauth/token");
        assert_eq!(endpoints.revoke, "http://127.0.0.1:1234/oauth/revoke");
    }

    #[test]
    fn recognizes_the_device_flow_errors() {
        assert!(matches!(
            device_error(r#"{"error":"authorization_pending"}"#),
            OAuthError::Pending
        ));
        assert!(matches!(
            device_error(r#"{"error":"slow_down"}"#),
            OAuthError::SlowDown
        ));
        assert!(matches!(
            device_error(r#"{"error":"access_denied","error_description":"no"}"#),
            OAuthError::AccessDenied
        ));
        assert!(matches!(
            device_error(r#"{"error":"expired_token"}"#),
            OAuthError::Expired
        ));
        assert!(matches!(
            device_error(r#"{"error":"invalid_client"}"#),
            OAuthError::Rejected(_)
        ));
        assert!(matches!(device_error("not json"), OAuthError::Rejected(_)));
    }

    #[test]
    fn describes_error_bodies_in_one_line() {
        assert_eq!(
            describe(r#"{"error":"invalid_client","error_description":"bad id"}"#),
            "bad id"
        );
        assert_eq!(
            describe(r#"{"error":{"message":"quota exceeded"}}"#),
            "quota exceeded"
        );
        assert_eq!(describe("plain text"), "plain text");
    }
}
