//! The small part of the Google Drive v3 API that the mirror needs.
//!
//! Only files created by Stylos are visible (`drive.file` scope), so the
//! mirror can list everything and rebuild its picture of the tree after a lost
//! state database. Uploads create the metadata first and then the content, so
//! a retry after a network error can reuse the file that was already created.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// MIME type of a Drive folder.
pub const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
/// MIME type Stylos uploads Markdown as.
pub const MARKDOWN_MIME: &str = "text/markdown";
/// MIME type of the optional `.docx` copy.
pub const DOCX_MIME: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

/// Fields requested from the API. Listing everything the app can see is
/// cheap (only Stylos files) and makes the mirror recover from a lost state
/// database without creating duplicates.
const FILE_FIELDS: &str = "id,name,mimeType,parents,appProperties,trashed,sha256Checksum";

/// Drive API base URLs, injectable for tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveEndpoints {
    pub api: String,
    pub upload: String,
}

impl Default for DriveEndpoints {
    fn default() -> Self {
        Self {
            api: "https://www.googleapis.com/drive/v3".into(),
            upload: "https://www.googleapis.com/upload/drive/v3".into(),
        }
    }
}

impl DriveEndpoints {
    /// `<base>/drive/v3` and `<base>/upload/drive/v3`, for a local mock.
    pub fn for_base_url(base: &str) -> Self {
        let base = base.trim_end_matches('/');
        Self {
            api: format!("{base}/drive/v3"),
            upload: format!("{base}/upload/drive/v3"),
        }
    }

    /// Google's endpoints, or `STYLOS_GDRIVE_BASE_URL` /
    /// `STYLOS_GDRIVE_API_URL` / `STYLOS_GDRIVE_UPLOAD_URL`.
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
            api: pick("STYLOS_GDRIVE_API_URL", "/drive/v3", &default.api),
            upload: pick(
                "STYLOS_GDRIVE_UPLOAD_URL",
                "/upload/drive/v3",
                &default.upload,
            ),
        }
    }
}

/// One Drive file or folder.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DriveFile {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub app_properties: BTreeMap<String, String>,
    #[serde(default)]
    pub trashed: bool,
    #[serde(default)]
    pub sha256_checksum: Option<String>,
}

impl DriveFile {
    pub fn is_folder(&self) -> bool {
        self.mime_type == FOLDER_MIME
    }

    /// First parent, if any.
    pub fn parent(&self) -> Option<&str> {
        self.parents.first().map(String::as_str)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileList {
    #[serde(default)]
    files: Vec<DriveFile>,
    #[serde(default)]
    next_page_token: Option<String>,
}

/// An error from the Drive API. `status` stays available so callers can treat
/// "already gone" specially.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct DriveError {
    pub status: Option<u16>,
    pub message: String,
}

impl DriveError {
    pub fn is_not_found(&self) -> bool {
        self.status == Some(404)
    }
}

/// A Drive v3 client with one access token.
pub struct DriveClient {
    http: reqwest::Client,
    endpoints: DriveEndpoints,
    access_token: String,
}

impl DriveClient {
    pub fn new(endpoints: DriveEndpoints, access_token: impl Into<String>) -> Result<Self> {
        let access_token = access_token.into();
        if access_token.trim().is_empty() {
            bail!("the Google Drive access token is empty");
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .user_agent(format!("stylosd/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .context("build the Google Drive HTTP client")?;
        Ok(Self {
            http,
            endpoints,
            access_token,
        })
    }

    /// Every non-trashed file this app can see.
    pub async fn list_all(&self) -> Result<Vec<DriveFile>> {
        let mut files = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut request = self
                .http
                .get(format!("{}/files", self.endpoints.api))
                .query(&[
                    ("q", "trashed=false".to_string()),
                    ("pageSize", "1000".to_string()),
                    ("fields", format!("nextPageToken,files({FILE_FIELDS})")),
                ])
                .bearer_auth(&self.access_token);
            if let Some(token) = &page_token {
                request = request.query(&[("pageToken", token.clone())]);
            }
            let reply: FileList = self
                .send_json(request, "list files")
                .await?
                .context("decode the Drive file list")?;
            files.extend(reply.files);
            match reply.next_page_token {
                Some(token) if !token.is_empty() => page_token = Some(token),
                _ => break,
            }
        }
        Ok(files)
    }

    /// Find a folder Stylos created, marked with `appProperties`.
    pub async fn find_by_app_property(&self, key: &str, value: &str) -> Result<Option<DriveFile>> {
        let query =
            format!("appProperties has {{key='{key}' and value='{value}'}} and trashed=false");
        let reply: FileList = self
            .send_json(
                self.http
                    .get(format!("{}/files", self.endpoints.api))
                    .query(&[
                        ("q", query),
                        ("pageSize", "10".to_string()),
                        ("fields", format!("files({FILE_FIELDS})")),
                    ])
                    .bearer_auth(&self.access_token),
                "find the Stylos folder",
            )
            .await?
            .context("decode the Drive search reply")?;
        Ok(reply.files.into_iter().next())
    }

    /// Create a folder. Folders are marked with one `appProperties` pair so a
    /// user rename does not hide them from Stylos.
    pub async fn create_folder(
        &self,
        name: &str,
        parent: Option<&str>,
        property: (&str, &str),
    ) -> Result<DriveFile> {
        let mut body = serde_json::json!({
            "name": name,
            "mimeType": FOLDER_MIME,
            "appProperties": { property.0: property.1 },
        });
        if let Some(parent) = parent {
            body["parents"] = serde_json::json!([parent]);
        }
        self.create(&body, "create a Drive folder").await
    }

    /// Create a file (metadata only). The caller then uploads its content with
    /// [`DriveClient::update_content`].
    pub async fn create_file(
        &self,
        name: &str,
        parent: &str,
        mime_type: &str,
    ) -> Result<DriveFile> {
        let body = serde_json::json!({
            "name": name,
            "mimeType": mime_type,
            "parents": [parent],
        });
        self.create(&body, "create a Drive file").await
    }

    async fn create(&self, body: &serde_json::Value, action: &str) -> Result<DriveFile> {
        let reply: DriveFile = self
            .send_json(
                self.http
                    .post(format!("{}/files", self.endpoints.api))
                    .query(&[("fields", FILE_FIELDS)])
                    .bearer_auth(&self.access_token)
                    .json(body),
                action,
            )
            .await?
            .context("decode the created Drive file")?;
        Ok(reply)
    }

    /// Replace the content of an existing file.
    pub async fn update_content(&self, file_id: &str, mime_type: &str, bytes: &[u8]) -> Result<()> {
        let request = self
            .http
            .patch(format!("{}/files/{file_id}", self.endpoints.upload))
            .query(&[("uploadType", "media")])
            .bearer_auth(&self.access_token)
            .header(reqwest::header::CONTENT_TYPE, mime_type)
            .body(bytes.to_vec());
        self.send_empty(request, "upload file content").await
    }

    /// Rename a file.
    pub async fn rename_file(&self, file_id: &str, name: &str) -> Result<()> {
        self.send_json::<serde_json::Value>(
            self.http
                .patch(format!("{}/files/{file_id}", self.endpoints.api))
                .query(&[("fields", FILE_FIELDS)])
                .bearer_auth(&self.access_token)
                .json(&serde_json::json!({ "name": name })),
            "rename a Drive file",
        )
        .await?;
        Ok(())
    }

    /// Move a file into another folder without renaming it.
    pub async fn move_file(
        &self,
        file_id: &str,
        add_parent: &str,
        remove_parent: Option<&str>,
    ) -> Result<()> {
        let mut request = self
            .http
            .patch(format!("{}/files/{file_id}", self.endpoints.api))
            .query(&[("addParents", add_parent.to_string())])
            .bearer_auth(&self.access_token);
        if let Some(remove_parent) = remove_parent {
            request = request.query(&[("removeParents", remove_parent.to_string())]);
        }
        self.send_json::<serde_json::Value>(
            request.json(&serde_json::json!({})),
            "move a Drive file",
        )
        .await?;
        Ok(())
    }

    /// Send a request with a JSON reply and map HTTP errors to [`DriveError`].
    async fn send_json<T: for<'de> Deserialize<'de>>(
        &self,
        request: reqwest::RequestBuilder,
        action: &str,
    ) -> Result<Option<T>> {
        let response = request
            .send()
            .await
            .with_context(|| format!("{action}: Google Drive is unreachable"))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .with_context(|| format!("{action}: read the Drive reply"))?;
        if !status.is_success() {
            return Err(DriveError {
                status: Some(status.as_u16()),
                message: format!(
                    "{action} failed: HTTP {}: {}",
                    status.as_u16(),
                    drive_error_message(&body)
                ),
            }
            .into());
        }
        if body.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&body).with_context(|| {
            format!("{action}: decode the Drive reply")
        })?))
    }

    async fn send_empty(&self, request: reqwest::RequestBuilder, action: &str) -> Result<()> {
        let response = request
            .send()
            .await
            .with_context(|| format!("{action}: Google Drive is unreachable"))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        Err(DriveError {
            status: Some(status.as_u16()),
            message: format!(
                "{action} failed: HTTP {}: {}",
                status.as_u16(),
                drive_error_message(&body)
            ),
        }
        .into())
    }
}

/// Extract Google's one-line message from an error body.
fn drive_error_message(body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(message) = value
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(|message| message.as_str())
        {
            return message.chars().take(300).collect();
        }
        if let Some(message) = value
            .get("error_description")
            .and_then(|text| text.as_str())
        {
            return message.chars().take(300).collect();
        }
    }
    body.trim().chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_can_be_built_from_one_base_url() {
        let endpoints = DriveEndpoints::for_base_url("http://127.0.0.1:9/");
        assert_eq!(endpoints.api, "http://127.0.0.1:9/drive/v3");
        assert_eq!(endpoints.upload, "http://127.0.0.1:9/upload/drive/v3");
    }

    #[test]
    fn parses_google_field_names() {
        let file: DriveFile = serde_json::from_str(
            r#"{"id":"f1","name":"one.md","mimeType":"text/markdown","parents":["p1"],
                "appProperties":{"stylos":"root"},"trashed":false,
                "sha256Checksum":"ab"}"#,
        )
        .unwrap();
        assert_eq!(file.name, "one.md");
        assert!(!file.is_folder());
        assert_eq!(file.parent(), Some("p1"));
        assert_eq!(file.app_properties.get("stylos").unwrap(), "root");
        assert_eq!(file.sha256_checksum.as_deref(), Some("ab"));
    }

    #[test]
    fn extracts_drive_error_messages() {
        assert_eq!(
            drive_error_message(r#"{"error":{"code":404,"message":"File not found"}}"#),
            "File not found"
        );
        assert_eq!(drive_error_message("plain"), "plain");
    }

    #[test]
    fn rejects_an_empty_token() {
        assert!(DriveClient::new(DriveEndpoints::default(), " ").is_err());
    }
}
