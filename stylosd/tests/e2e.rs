//! End-to-end tests for WP3: a real `stylos-server` on localhost with TLS and
//! certificate pinning, driven by the real `stylosd` client code.
//!
//! Covered: pairing approved over HTTP, push, pull, conflicts on both sides,
//! delete with `base_rev`, resuming interrupted pulls and pushes, and an
//! idempotent second sync that transfers nothing.

use std::collections::BTreeMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use base64::Engine;
use stylos_protocol::endpoints;
use stylos_protocol::{ConflictResponse, FileEntry, FileList, PairStatus};
use stylos_server::{start, Config, RunningServer};
use stylosd::http::HttpClient;
use stylosd::tls::TlsMode;
use stylosd::{
    generate_device_pubkey, sync_once, PairingSession, SyncConfig, SyncError, SyncOptions,
    SyncReport,
};
use tempfile::TempDir;

const ADMIN_PASSWORD: &str = "admin-secret";

struct TestServer {
    dir: TempDir,
    running: RunningServer,
}

impl TestServer {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::new(dir.path().join("data"));
        config.listen = "127.0.0.1:0".parse().unwrap();
        config.mdns = false;
        config.admin_password = Some(ADMIN_PASSWORD.into());
        config.label = Some("test-server".into());
        let running = start(config).await.unwrap();
        Self { dir, running }
    }

    fn url(&self) -> String {
        self.running.local_url()
    }

    fn data_dir(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn cert_sha256(&self) -> &str {
        &self.running.cert_sha256
    }

    async fn shutdown(self) {
        let TestServer { running, .. } = self;
        running.shutdown().await;
    }
}

struct TestClient {
    _dir: TempDir,
    writing: PathBuf,
    state: PathBuf,
    config: SyncConfig,
}

impl TestClient {
    async fn pair(server: &TestServer, name: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let writing = dir.path().join("writing");
        let state = dir.path().join("state");
        fs::create_dir_all(&writing).unwrap();
        let config = pair_client_at(&server.url(), &state, name).await;
        Self {
            _dir: dir,
            writing,
            state,
            config,
        }
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.writing.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn remove(&self, relative: &str) {
        fs::remove_file(self.writing.join(relative)).unwrap();
    }

    fn read(&self, relative: &str) -> Option<String> {
        fs::read_to_string(self.writing.join(relative)).ok()
    }

    fn files(&self) -> BTreeMap<String, String> {
        read_markdown_files(&self.writing)
    }

    async fn sync(&self) -> SyncReport {
        sync_once(&self.writing, &self.state, &SyncOptions::default())
            .await
            .unwrap()
    }

    async fn try_sync(&self, options: &SyncOptions) -> Result<SyncReport> {
        sync_once(&self.writing, &self.state, options).await
    }
}

/// Pair a client, approving the code through the real HTTP page.
async fn pair_client_at(server_url: &str, state_dir: &Path, name: &str) -> SyncConfig {
    let pubkey = generate_device_pubkey().unwrap();
    let session = PairingSession::start(server_url, name, &pubkey)
        .await
        .unwrap();
    let code = session.code().to_string();
    approve_via_http_at(server_url, &code).await;
    let credential = session
        .wait(Duration::from_millis(25), Duration::from_secs(5))
        .await
        .unwrap();
    let config = credential.to_sync_config();
    config.save(state_dir).unwrap();
    config
}

fn basic_auth(password: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(format!("admin:{password}"));
    format!("Basic {encoded}")
}

async fn approve_via_http_at(server_url: &str, code: &str) {
    let client = HttpClient::new(server_url, TlsMode::AcceptAny)
        .unwrap()
        .with_authorization_header(basic_auth(ADMIN_PASSWORD));
    let page = client.get("/approve").await.unwrap();
    assert_eq!(page.status, 200, "approval page: {}", page.error_message());
    let html = page.text().unwrap().to_string();
    let csrf = extract_csrf(&html);
    let body = format!("csrf={csrf}&code={code}");
    let response = client
        .request(
            "POST",
            "/approve",
            Some("application/x-www-form-urlencoded"),
            Some(body.as_bytes()),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status,
        200,
        "approval failed: {}",
        response.error_message()
    );
}

fn extract_csrf(html: &str) -> String {
    let marker = "name=\"csrf\" value=\"";
    let start = html.find(marker).expect("approval page has a csrf field") + marker.len();
    let end = html[start..].find('"').expect("csrf value is quoted") + start;
    html[start..end].to_string()
}

fn pinned_client(config: &SyncConfig) -> HttpClient {
    HttpClient::new(&config.server_url, config.tls_mode())
        .unwrap()
        .with_token(config.token.clone())
}

async fn server_files(config: &SyncConfig) -> Vec<FileEntry> {
    let client = pinned_client(config);
    let list: FileList = client.get_json("/files?since=0").await.unwrap();
    list.files
}

fn read_markdown_files(root: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    walk_markdown(root, root, &mut files);
    files
}

fn walk_markdown(root: &Path, directory: &Path, files: &mut BTreeMap<String, String>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            walk_markdown(root, &path, files);
        } else if name.ends_with(".md") {
            if let Ok(relative) = path.strip_prefix(root) {
                let relative = relative.to_string_lossy().replace('\\', "/");
                files.insert(relative, fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
}

fn conflict_copies(files: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    files
        .iter()
        .filter(|(path, _)| {
            let name = Path::new(path)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            name.contains(" (conflict ") && name.ends_with(").md")
        })
        .map(|(path, content)| (path.clone(), content.clone()))
        .collect()
}

fn assert_conflict_copy_name(path: &str, original_stem: &str, source: &str) {
    let name = Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .expect("path has a file name");
    let marker = " (conflict ";
    let start = name
        .find(marker)
        .unwrap_or_else(|| panic!("no conflict marker in {name:?}"));
    assert_eq!(&name[..start], original_stem, "conflict copy {name:?}");
    let rest = &name[start + marker.len()..];
    let (timestamp, tail) = rest
        .split_once(" from ")
        .unwrap_or_else(|| panic!("no source in {name:?}"));
    assert_eq!(tail, format!("{source}).md"), "conflict copy {name:?}");
    assert_eq!(timestamp.len(), 15, "timestamp in {name:?}");
    let bytes = timestamp.as_bytes();
    assert_eq!(bytes[4], b'-');
    assert_eq!(bytes[7], b'-');
    assert_eq!(bytes[10], b' ');
    assert!(bytes[11..].iter().all(u8::is_ascii_digit));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pair_push_pull_delete_and_idempotent_sync() {
    let server = TestServer::start().await;
    let a = TestClient::pair(&server, "laptop-a").await;
    assert_eq!(a.config.cert_sha256.as_deref(), Some(server.cert_sha256()));
    assert_eq!(a.config.server_name.as_deref(), Some("test-server"));

    a.write("notes/one.md", "First chapter\n");
    a.write("two.md", "Second\n");
    let report = a.sync().await;
    assert_eq!(report.pushed, 2, "{report:?}");
    assert_eq!(report.pulled, 0);
    assert_eq!(
        fs::read_to_string(server.data_dir().join("notes/one.md")).unwrap(),
        "First chapter\n"
    );
    assert_eq!(
        fs::read_to_string(server.data_dir().join("two.md")).unwrap(),
        "Second\n"
    );

    let b = TestClient::pair(&server, "laptop-b").await;
    let report = b.sync().await;
    assert_eq!(report.pulled, 2, "{report:?}");
    assert_eq!(report.pushed, 0);
    assert_eq!(b.read("notes/one.md").as_deref(), Some("First chapter\n"));
    assert_eq!(b.read("two.md").as_deref(), Some("Second\n"));

    // A second sync transfers nothing.
    let before = server_files(&a.config).await;
    let report = a.sync().await;
    assert!(report.is_noop(), "{report:?}");
    assert_eq!(report.transferred(), 0);
    let after = server_files(&a.config).await;
    assert_eq!(before, after);
    assert!(b.sync().await.is_noop());

    // Edit on B, pull on A.
    b.write("notes/one.md", "B's edit\n");
    let report = b.sync().await;
    assert_eq!(report.pushed, 1, "{report:?}");
    let report = a.sync().await;
    assert_eq!(report.pulled, 1, "{report:?}");
    assert_eq!(a.read("notes/one.md").as_deref(), Some("B's edit\n"));
    assert!(a.sync().await.is_noop());

    // Delete on A, pull the tombstone on B.
    a.remove("two.md");
    let report = a.sync().await;
    assert_eq!(report.deleted_remote, 1, "{report:?}");
    assert!(!server.data_dir().join("two.md").exists());
    let report = b.sync().await;
    assert_eq!(report.deleted_local, 1, "{report:?}");
    assert_eq!(b.read("two.md"), None);
    assert!(b.sync().await.is_noop());

    let server_files = read_markdown_files(&server.data_dir());
    assert_eq!(
        server_files.get("notes/one.md").map(String::as_str),
        Some("B's edit\n")
    );
    assert!(!server_files.contains_key("two.md"));

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_edits_keep_both_versions() {
    let server = TestServer::start().await;
    let a = TestClient::pair(&server, "laptop-a").await;
    let b = TestClient::pair(&server, "laptop-b").await;

    a.write("note.md", "version A\n");
    a.sync().await;
    b.sync().await;
    assert_eq!(b.read("note.md").as_deref(), Some("version A\n"));

    a.write("note.md", "version A edited\n");
    a.sync().await;

    b.write("note.md", "version B edited\n");
    let report = b.sync().await;
    assert_eq!(report.conflicts, 1, "{report:?}");
    assert_eq!(report.pushed, 2, "local file and conflict copy: {report:?}");
    assert_eq!(b.read("note.md").as_deref(), Some("version B edited\n"));

    let copies = conflict_copies(&b.files());
    assert_eq!(copies.len(), 1, "copy of the other version: {copies:?}");
    let (copy_path, copy_content) = copies.iter().next().unwrap();
    assert_eq!(copy_content, "version A edited\n");
    assert_conflict_copy_name(copy_path, "note", "test-server");

    // A sees B's version at note.md and pulls its own old edit back as a
    // conflict copy. Nothing is lost.
    let report = a.sync().await;
    assert_eq!(report.pulled, 2, "{report:?}");
    assert_eq!(a.read("note.md").as_deref(), Some("version B edited\n"));
    let copies = conflict_copies(&a.files());
    assert_eq!(copies.len(), 1, "{copies:?}");
    assert_eq!(copies.values().next().unwrap(), "version A edited\n");

    // Everyone converges and a second sync transfers nothing.
    assert!(a.sync().await.is_noop());
    assert!(b.sync().await.is_noop());

    let server_files = read_markdown_files(&server.data_dir());
    assert_eq!(server_files.len(), 2, "{server_files:?}");
    assert_eq!(
        server_files.get("note.md").map(String::as_str),
        Some("version B edited\n")
    );
    let server_copies = conflict_copies(&server_files);
    assert_eq!(server_copies.len(), 1, "{server_files:?}");
    assert_eq!(server_copies.values().next().unwrap(), "version A edited\n");

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_with_base_rev_and_conflict_preserves_remote_text() {
    let server = TestServer::start().await;
    let a = TestClient::pair(&server, "laptop-a").await;
    let b = TestClient::pair(&server, "laptop-b").await;

    a.write("note.md", "base\n");
    a.sync().await;
    b.sync().await;
    b.write("note.md", "from B\n");
    b.sync().await;

    // A stale DELETE with an old base_rev is rejected and changes nothing.
    let entries = server_files(&a.config).await;
    let entry = entries
        .iter()
        .find(|entry| entry.path == "note.md")
        .expect("note exists on the server");
    assert_eq!(entry.rev, 2);
    let client = pinned_client(&a.config);
    let response = client
        .delete(&endpoints::file_delete(entry.file_id, 1))
        .await
        .unwrap();
    assert_eq!(response.status, 409);
    let conflict: ConflictResponse = response.json().unwrap();
    assert_eq!(conflict.conflict.server_rev, 2);
    assert_eq!(
        fs::read_to_string(server.data_dir().join("note.md")).unwrap(),
        "from B\n"
    );

    // A deletes locally. Its pull sees B's newer text; that version must be
    // kept as a conflict copy before the delete is pushed.
    a.remove("note.md");
    let report = a.sync().await;
    assert_eq!(report.conflicts, 1, "{report:?}");
    assert_eq!(report.deleted_remote, 1, "{report:?}");
    assert_eq!(a.read("note.md"), None);
    let a_copies = conflict_copies(&a.files());
    assert_eq!(a_copies.len(), 1, "{a_copies:?}");
    assert_eq!(a_copies.values().next().unwrap(), "from B\n");
    assert!(!server.data_dir().join("note.md").exists());

    // B pulls the tombstone and the preserved copy; its text survives.
    let report = b.sync().await;
    assert_eq!(report.deleted_local, 1, "{report:?}");
    assert_eq!(b.read("note.md"), None);
    let b_copies = conflict_copies(&b.files());
    assert_eq!(b_copies.len(), 1, "{b_copies:?}");
    assert_eq!(b_copies.values().next().unwrap(), "from B\n");
    assert!(b.sync().await.is_noop());

    let server_files = read_markdown_files(&server.data_dir());
    assert!(!server_files.contains_key("note.md"));
    assert_eq!(server_files.len(), 1, "{server_files:?}");
    assert_eq!(
        conflict_copies(&server_files).values().next().unwrap(),
        "from B\n"
    );

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_syncs_resume_safely() {
    // Pull side: a sync stops after the first file is applied.
    let server = TestServer::start().await;
    let a = TestClient::pair(&server, "laptop-a").await;
    a.write("a.md", "one\n");
    a.write("b.md", "two\n");
    a.write("c.md", "three\n");
    assert_eq!(a.sync().await.pushed, 3);

    let b = TestClient::pair(&server, "laptop-b").await;
    let error = b
        .try_sync(&SyncOptions {
            interrupt_after: Some(1),
        })
        .await
        .unwrap_err();
    assert!(
        error.downcast_ref::<SyncError>().is_some(),
        "expected an interrupted sync, got {error:#}"
    );
    assert_eq!(
        b.files().len(),
        1,
        "exactly one file applied before the error"
    );
    let state = stylosd::state::SyncState::from_dir(&b.state).unwrap();
    assert_eq!(state.cursor().unwrap(), "0", "cursor not saved");

    let report = b.sync().await;
    assert_eq!(report.pulled, 2, "only the remaining files: {report:?}");
    assert_eq!(b.files().len(), 3);
    assert_eq!(b.read("a.md").as_deref(), Some("one\n"));
    assert_eq!(b.read("b.md").as_deref(), Some("two\n"));
    assert_eq!(b.read("c.md").as_deref(), Some("three\n"));
    assert!(b.sync().await.is_noop());
    server.shutdown().await;

    // Push side: a sync stops after the first upload.
    let server = TestServer::start().await;
    let c = TestClient::pair(&server, "laptop-c").await;
    c.write("a.md", "one\n");
    c.write("b.md", "two\n");
    c.write("c.md", "three\n");
    let error = c
        .try_sync(&SyncOptions {
            interrupt_after: Some(1),
        })
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<SyncError>().is_some(), "{error:#}");
    assert_eq!(read_markdown_files(&server.data_dir()).len(), 1);

    let report = c.sync().await;
    assert_eq!(report.pushed, 2, "only the remaining uploads: {report:?}");
    let server_files = read_markdown_files(&server.data_dir());
    assert_eq!(server_files.len(), 3, "{server_files:?}");
    assert_eq!(server_files.get("a.md").map(String::as_str), Some("one\n"));
    assert_eq!(server_files.get("b.md").map(String::as_str), Some("two\n"));
    assert_eq!(
        server_files.get("c.md").map(String::as_str),
        Some("three\n")
    );
    assert!(c.sync().await.is_noop());

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tls_pinning_rejects_a_different_certificate() {
    let server = TestServer::start().await;
    let pin = server.cert_sha256().to_string();

    let trusted = HttpClient::new(&server.url(), TlsMode::Pinned(pin.clone())).unwrap();
    let response = trusted.get("/healthz").await.unwrap();
    assert_eq!(response.status, 200);
    let health: serde_json::Value = response.json().unwrap();
    assert_eq!(health["status"], "ok");

    let observed_client = HttpClient::new(&server.url(), TlsMode::AcceptAny).unwrap();
    observed_client.get("/healthz").await.unwrap();
    assert_eq!(
        observed_client.observed_cert_sha256().as_deref(),
        Some(pin.as_str())
    );

    let mut wrong = pin.clone();
    let replacement = if wrong.starts_with('0') { "1" } else { "0" };
    wrong.replace_range(0..1, replacement);
    let untrusted = HttpClient::new(&server.url(), TlsMode::Pinned(wrong)).unwrap();
    assert!(
        untrusted.get("/healthz").await.is_err(),
        "a different certificate must not be accepted"
    );

    // Pairing stores exactly the observed fingerprint.
    let a = TestClient::pair(&server, "laptop-a").await;
    assert_eq!(a.config.cert_sha256.as_deref(), Some(pin.as_str()));

    // A tampered pin makes sync fail before any file moves.
    let mut config = a.config.clone();
    config.cert_sha256 = Some("00".repeat(32));
    config.save(&a.state).unwrap();
    a.write("note.md", "text\n");
    let error = a.try_sync(&SyncOptions::default()).await.unwrap_err();
    assert!(
        format!("{error:#}").to_lowercase().contains("tls")
            || format!("{error:#}").to_lowercase().contains("certificate"),
        "unexpected error: {error:#}"
    );
    assert!(!server.data_dir().join("note.md").exists());

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_conflict_sync_does_not_duplicate_copies() {
    let server = TestServer::start().await;
    let a = TestClient::pair(&server, "laptop-a").await;
    let b = TestClient::pair(&server, "laptop-b").await;

    a.write("note.md", "base\n");
    a.sync().await;
    b.sync().await;
    a.write("note.md", "from A\n");
    a.sync().await;
    b.write("note.md", "from B\n");

    let error = b
        .try_sync(&SyncOptions {
            interrupt_after: Some(1),
        })
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<SyncError>().is_some(), "{error:#}");
    assert_eq!(conflict_copies(&b.files()).len(), 1);
    assert_eq!(
        stylosd::state::SyncState::from_dir(&b.state)
            .unwrap()
            .cursor()
            .unwrap(),
        "1",
        "the cursor must not be saved before the pull completes"
    );

    let report = b.sync().await;
    assert_eq!(report.pushed, 2, "note plus its conflict copy: {report:?}");
    let copies = conflict_copies(&b.files());
    assert_eq!(copies.len(), 1, "the copy was not duplicated: {copies:?}");
    assert_eq!(copies.values().next().unwrap(), "from A\n");
    assert_eq!(b.read("note.md").as_deref(), Some("from B\n"));
    assert!(b.sync().await.is_noop());

    a.sync().await;
    let copies = conflict_copies(&a.files());
    assert_eq!(copies.len(), 1, "{copies:?}");
    assert_eq!(copies.values().next().unwrap(), "from A\n");
    assert_eq!(a.read("note.md").as_deref(), Some("from B\n"));

    let server_files = read_markdown_files(&server.data_dir());
    assert_eq!(server_files.len(), 2, "{server_files:?}");
    assert_eq!(
        server_files.get("note.md").map(String::as_str),
        Some("from B\n")
    );
    assert_eq!(
        conflict_copies(&server_files).values().next().unwrap(),
        "from A\n"
    );

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_keeps_certificate_and_files() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let config = |port: u16| {
        let mut config = Config::new(&data);
        config.listen = SocketAddr::from(([127, 0, 0, 1], port));
        config.mdns = false;
        config.admin_password = Some(ADMIN_PASSWORD.into());
        config.label = Some("test-server".into());
        config
    };

    let first = start(config(0)).await.unwrap();
    let pin = first.cert_sha256.clone();
    let url = first.local_url();
    let port = first.addr.port();

    let client_dir = tempfile::tempdir().unwrap();
    let writing = client_dir.path().join("writing");
    let state = client_dir.path().join("state");
    fs::create_dir_all(&writing).unwrap();
    let paired = pair_client_at(&url, &state, "laptop-a").await;
    assert_eq!(paired.cert_sha256.as_deref(), Some(pin.as_str()));
    fs::write(writing.join("kept.md"), "persisted\n").unwrap();
    assert_eq!(
        sync_once(&writing, &state, &SyncOptions::default())
            .await
            .unwrap()
            .pushed,
        1
    );
    first.shutdown().await;

    // Same data directory and port: the certificate and the index must survive.
    let second = start(config(port)).await.unwrap();
    assert_eq!(second.cert_sha256, pin);
    assert_eq!(second.local_url(), url);
    let report = sync_once(&writing, &state, &SyncOptions::default())
        .await
        .unwrap();
    assert!(report.is_noop(), "{report:?}");

    let other_state = client_dir.path().join("other-state");
    let _other = pair_client_at(&second.local_url(), &other_state, "laptop-b").await;
    let other_writing = client_dir.path().join("other-writing");
    fs::create_dir_all(&other_writing).unwrap();
    let report = sync_once(&other_writing, &other_state, &SyncOptions::default())
        .await
        .unwrap();
    assert_eq!(report.pulled, 1, "{report:?}");
    assert_eq!(
        fs::read_to_string(other_writing.join("kept.md")).unwrap(),
        "persisted\n"
    );

    second.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_http_rules() {
    let server = TestServer::start().await;

    let anonymous = HttpClient::new(&server.url(), TlsMode::AcceptAny).unwrap();
    assert_eq!(anonymous.get("/healthz").await.unwrap().status, 200);
    assert_eq!(anonymous.get("/files").await.unwrap().status, 401);
    assert_eq!(anonymous.get("/approve").await.unwrap().status, 401);

    let wrong_password = HttpClient::new(&server.url(), TlsMode::AcceptAny)
        .unwrap()
        .with_authorization_header(basic_auth("wrong"));
    assert_eq!(wrong_password.get("/approve").await.unwrap().status, 401);

    let admin = HttpClient::new(&server.url(), TlsMode::AcceptAny)
        .unwrap()
        .with_authorization_header(basic_auth(ADMIN_PASSWORD));
    assert_eq!(admin.get("/approve").await.unwrap().status, 200);

    // Start a pairing and check the page lists it.
    let pubkey = generate_device_pubkey().unwrap();
    let session = PairingSession::start(&server.url(), "phone", &pubkey)
        .await
        .unwrap();
    let status = session.poll().await.unwrap();
    assert_eq!(status.status, PairStatus::Pending);
    assert!(status.token.is_none());
    assert_eq!(
        status.server_cert_sha256.as_deref(),
        Some(server.cert_sha256())
    );

    let page = admin.get("/approve").await.unwrap();
    let html = page.text().unwrap();
    assert!(html.contains(session.code()), "page: {html}");
    assert!(html.contains("phone"));

    // A form without the CSRF token is rejected.
    let csrf = extract_csrf(html);
    let response = admin
        .request(
            "POST",
            "/approve",
            Some("application/x-www-form-urlencoded"),
            Some(format!("csrf=wrong&code={}", session.code()).as_bytes()),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 403);

    // Approve by typing the code, then poll again.
    let response = admin
        .request(
            "POST",
            "/approve",
            Some("application/x-www-form-urlencoded"),
            Some(format!("csrf={csrf}&code={}", session.code().to_lowercase()).as_bytes()),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 200, "{}", response.error_message());
    let status = session.poll().await.unwrap();
    assert_eq!(status.status, PairStatus::Approved);
    let token = status.token.expect("approved pairing has a token");

    // Bearer auth works and the library endpoint answers 404 as specified.
    let client = HttpClient::new(&server.url(), TlsMode::Pinned(server.cert_sha256().into()))
        .unwrap()
        .with_token(token.clone());
    assert_eq!(client.get("/files").await.unwrap().status, 200);
    let library = client.get("/library").await.unwrap();
    assert_eq!(library.status, 404);
    assert!(library.error_message().contains("Nostos"));

    let bogus = HttpClient::new(&server.url(), TlsMode::Pinned(server.cert_sha256().into()))
        .unwrap()
        .with_token("not-a-token");
    assert_eq!(bogus.get("/files").await.unwrap().status, 401);

    // Invalid paths and oversized bodies are rejected.
    let response = client
        .put_json(
            &endpoints::file(uuid::Uuid::now_v7()),
            &stylos_protocol::PutFileRequest {
                path: "../escape.md".into(),
                content: "no".into(),
                base_rev: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(response.status, 400);

    server.shutdown().await;
}
