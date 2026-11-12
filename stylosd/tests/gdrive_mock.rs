//! Tests for the Google Drive backend against a local mock of Google's OAuth
//! and Drive APIs. No test talks to Google.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use stylosd::gdrive::{
    mirror_once, DocxConverter, DriveClient, DriveEndpoints, GdriveReport, GdriveState,
    MirrorOptions, OAuthApp, OAuthClient, OAuthEndpoints,
};
use tempfile::TempDir;

use support::MockGoogle;

fn oauth_app() -> OAuthApp {
    OAuthApp {
        client_id: "test-client".into(),
        client_secret: "test-secret".into(),
    }
}

fn oauth(mock: &MockGoogle) -> OAuthClient {
    OAuthClient::new(oauth_app(), OAuthEndpoints::for_base_url(mock.base_url())).unwrap()
}

/// One writing directory, one state directory and a mock Google.
struct Env {
    mock: MockGoogle,
    _dir: TempDir,
    writing: PathBuf,
    state: PathBuf,
}

impl Env {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let writing = dir.path().join("writing");
        let state = dir.path().join("state");
        fs::create_dir_all(&writing).unwrap();
        fs::create_dir_all(&state).unwrap();
        let mock = MockGoogle::start().await;
        Self {
            mock,
            _dir: dir,
            writing,
            state,
        }
    }

    fn write(&self, path: &str, text: &str) {
        let target = self.writing.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, text).unwrap();
    }

    fn remove(&self, path: &str) {
        fs::remove_file(self.writing.join(path)).unwrap();
    }

    async fn sync(
        &self,
        options: &MirrorOptions,
        converter: Option<&dyn DocxConverter>,
    ) -> Result<GdriveReport> {
        let access = self.oauth_client().refresh("refresh-1").await?;
        let api =
            DriveClient::new(DriveEndpoints::for_base_url(self.mock.base_url()), access).unwrap();
        let mut state = GdriveState::from_dir(&self.state).unwrap();
        mirror_once(&api, &self.writing, &mut state, options, converter).await
    }

    fn oauth_client(&self) -> OAuthClient {
        oauth(&self.mock)
    }
}

fn default_options() -> MirrorOptions {
    MirrorOptions::default()
}

#[tokio::test]
async fn device_flow_waits_for_pending_and_slow_down() {
    let mock = MockGoogle::start().await;
    mock.set_pending(1);
    mock.set_slow_down(1);
    let client = oauth(&mock).with_slow_down_penalty(Duration::from_millis(50));

    let device = client.start_device_flow().await.unwrap();
    assert_eq!(
        mock.requested_scope(),
        "https://www.googleapis.com/auth/drive.file",
        "Google Drive must only get the drive.file scope"
    );
    assert_eq!(device.user_code, "TEST-CODE");
    assert_eq!(device.verification_url, "https://example.test/device");
    assert_eq!(device.expires_in, 600);

    let tokens = client
        .wait_for_approval(&device, Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(tokens.access_token, "access-1");
    assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-1"));
    // pending, slow_down, success.
    assert_eq!(mock.polls(), 3);
}

#[tokio::test]
async fn device_flow_reports_denied_and_expired() {
    let denied = MockGoogle::start().await;
    denied.set_denied();
    let client = oauth(&denied);
    let device = client.start_device_flow().await.unwrap();
    let error = client
        .wait_for_approval(&device, Duration::from_secs(5))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("denied"), "{error}");

    let expired = MockGoogle::start().await;
    expired.set_expired();
    let client = oauth(&expired);
    let device = client.start_device_flow().await.unwrap();
    let error = client
        .wait_for_approval(&device, Duration::from_secs(5))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("expired"), "{error}");
}

#[tokio::test]
async fn refreshes_the_access_token_and_revokes_the_refresh_token() {
    let mock = MockGoogle::start().await;
    let client = oauth(&mock);

    let access = client.refresh("refresh-1").await.unwrap();
    assert_eq!(access, "access-refreshed-1");
    assert_eq!(mock.refresh_calls(), 1);

    // The refreshed token is what the Drive client uses.
    let api = DriveClient::new(DriveEndpoints::for_base_url(mock.base_url()), access).unwrap();
    assert!(api.list_all().await.unwrap().is_empty());

    let error = client.refresh("wrong-token").await.unwrap_err().to_string();
    assert!(error.contains("revoked or expired"), "{error}");

    client.revoke("refresh-1").await.unwrap();
    assert_eq!(mock.revoke_calls(), 1);
}

#[tokio::test]
async fn first_mirror_uploads_folders_and_files_then_second_is_a_noop() {
    let env = Env::new().await;
    env.write("root.md", "root");
    env.write("notes/a.md", "alpha");
    env.write("notes/deep/b.md", "beta");
    env.write("notes/ignored.txt", "not markdown");

    let report = env.sync(&default_options(), None).await.unwrap();
    assert_eq!(report.uploaded, 3, "{report:?}");
    assert_eq!(report.folders, 2, "{report:?}");
    assert_eq!(report.skipped, 0, "{report:?}");

    let paths = env.mock.paths();
    assert!(paths.contains_key("notes"), "{paths:?}");
    assert!(paths["notes"].is_folder());
    assert!(paths.contains_key("notes/deep"), "{paths:?}");
    assert!(paths["notes/deep"].is_folder());
    assert_eq!(env.mock.content("root.md").as_deref(), Some("root"));
    assert_eq!(env.mock.content("notes/a.md").as_deref(), Some("alpha"));
    assert_eq!(env.mock.content("notes/deep/b.md").as_deref(), Some("beta"));

    let uploads = env.mock.uploads();
    let second = env.sync(&default_options(), None).await.unwrap();
    assert_eq!(second.uploaded, 0, "{second:?}");
    assert_eq!(second.unchanged, 3, "{second:?}");
    assert!(second.is_noop());
    assert_eq!(env.mock.uploads(), uploads, "nothing was re-uploaded");
}

#[tokio::test]
async fn editing_one_file_uploads_exactly_one() {
    let env = Env::new().await;
    env.write("a.md", "one");
    env.write("b.md", "two");
    env.sync(&default_options(), None).await.unwrap();
    let uploads = env.mock.uploads();

    env.write("b.md", "two, edited");
    let report = env.sync(&default_options(), None).await.unwrap();
    assert_eq!(report.uploaded, 1, "{report:?}");
    assert_eq!(report.unchanged, 1, "{report:?}");
    assert_eq!(env.mock.uploads(), uploads + 1);
    assert_eq!(env.mock.content("a.md").as_deref(), Some("one"));
    assert_eq!(env.mock.content("b.md").as_deref(), Some("two, edited"));
}

#[tokio::test]
async fn deleting_a_file_moves_it_to_the_deleted_folder() {
    let env = Env::new().await;
    env.write("keep.md", "keep");
    env.write("notes/gone.md", "gone");
    env.sync(&default_options(), None).await.unwrap();
    let id = env.mock.file("notes/gone.md").unwrap().id;

    env.remove("notes/gone.md");
    let report = env.sync(&default_options(), None).await.unwrap();
    assert_eq!(report.deleted, 1, "{report:?}");
    assert_eq!(report.uploaded, 0, "{report:?}");
    assert!(env.mock.file("notes/gone.md").is_none());
    assert_eq!(
        env.mock.content(".deleted/gone.md").as_deref(),
        Some("gone")
    );
    assert_eq!(env.mock.file(".deleted/gone.md").unwrap().id, id);
    assert_eq!(env.mock.content("keep.md").as_deref(), Some("keep"));

    // The move is remembered; the next sync transfers nothing.
    let again = env.sync(&default_options(), None).await.unwrap();
    assert!(again.is_noop(), "{again:?}");
}

#[tokio::test]
async fn an_api_error_mid_sync_is_safe_to_retry() {
    let env = Env::new().await;
    env.write("a.md", "alpha");
    env.write("b.md", "beta");

    env.mock.fail_uploads(1);
    let error = env.sync(&default_options(), None).await.unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("500"), "{message}");

    let retry = env.sync(&default_options(), None).await.unwrap();
    assert_eq!(retry.uploaded, 2, "{retry:?}");
    assert_eq!(env.mock.content("a.md").as_deref(), Some("alpha"));
    assert_eq!(env.mock.content("b.md").as_deref(), Some("beta"));
    assert_eq!(env.mock.count_named("a.md"), 1, "no duplicate after retry");
    assert_eq!(env.mock.count_named("b.md"), 1, "no duplicate after retry");

    let settled = env.sync(&default_options(), None).await.unwrap();
    assert!(settled.is_noop(), "{settled:?}");
}

struct FakeConverter;

impl DocxConverter for FakeConverter {
    fn convert(&self, path: &str, markdown: &str) -> Result<Vec<u8>> {
        Ok(format!("DOCX of {path}: {markdown}").into_bytes())
    }
}

#[tokio::test]
async fn the_optional_docx_copy_follows_the_setting() {
    let env = Env::new().await;
    env.write("notes/a.md", "hello");

    let markdown_only = env
        .sync(&default_options(), Some(&FakeConverter))
        .await
        .unwrap();
    assert_eq!(markdown_only.uploaded, 1, "{markdown_only:?}");
    assert!(env.mock.file("notes/a.docx").is_none());

    let with_docx = MirrorOptions { docx_copy: true };
    let report = env.sync(&with_docx, Some(&FakeConverter)).await.unwrap();
    assert_eq!(report.uploaded, 1, "{report:?}");
    assert_eq!(
        env.mock.content("notes/a.docx").as_deref(),
        Some("DOCX of notes/a.md: hello")
    );

    let again = env.sync(&with_docx, Some(&FakeConverter)).await.unwrap();
    assert!(again.is_noop(), "{again:?}");

    env.remove("notes/a.md");
    let deleted = env.sync(&with_docx, Some(&FakeConverter)).await.unwrap();
    assert_eq!(deleted.deleted, 2, "{deleted:?}");
    assert!(env.mock.file(".deleted/a.docx").is_some());
}

#[tokio::test]
async fn a_lost_state_reuses_drive_files_without_duplicates() {
    let env = Env::new().await;
    env.write("notes/a.md", "alpha");
    env.sync(&default_options(), None).await.unwrap();

    // Simulate a lost state database. Without the stored hash the mirror
    // cannot know the file is unchanged, so it uploads once more, but it must
    // reuse the existing Drive file instead of creating a duplicate.
    for suffix in ["", "-wal", "-shm"] {
        let path = env.state.join(format!("sync-state.sqlite{suffix}"));
        let _ = fs::remove_file(path);
    }
    let report = env.sync(&default_options(), None).await.unwrap();
    assert_eq!(report.uploaded, 1, "{report:?}");
    assert_eq!(env.mock.count_named("a.md"), 1);

    let settled = env.sync(&default_options(), None).await.unwrap();
    assert!(settled.is_noop(), "{settled:?}");
}

#[test]
fn a_missing_oauth_file_is_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let error = OAuthApp::load_from(Path::new(&dir.path().join("missing.toml")))
        .unwrap_err()
        .to_string();
    assert!(error.contains("missing.toml"), "{error}");
}
