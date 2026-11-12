//! A local stand-in for Google's OAuth and Drive APIs.
//!
//! Tests point `OAuthEndpoints::for_base_url` / `DriveEndpoints::for_base_url`
//! (or, for the daemon binary, `CARREL_GDRIVE_BASE_URL`) at it. It models just
//! enough of the two APIs: the device flow with scripted pending/slow_down/
//! denied replies, token refresh and revoke, and a flat Drive file store with
//! folders, `appProperties`, metadata patches and media uploads.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

type Shared = Arc<Mutex<MockState>>;

/// One file in the mock Drive.
#[derive(Debug, Clone)]
pub struct MockFile {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub parents: Vec<String>,
    pub app_properties: BTreeMap<String, String>,
    pub trashed: bool,
    pub content: Vec<u8>,
}

impl MockFile {
    pub fn is_folder(&self) -> bool {
        self.mime_type == "application/vnd.google-apps.folder"
    }
}

/// Scriptable state shared by all mock handlers.
pub struct MockState {
    pending_polls: usize,
    slow_down_polls: usize,
    denied: bool,
    expired: bool,
    interval: u64,
    last_scope: String,
    refresh_token: String,
    valid_tokens: BTreeSet<String>,
    files: BTreeMap<String, MockFile>,
    next_id: usize,
    revoke_calls: usize,
    device_requests: usize,
    polls: usize,
    refresh_calls: usize,
    uploads: usize,
    fail_uploads: usize,
}

impl Default for MockState {
    fn default() -> Self {
        let mut valid_tokens = BTreeSet::new();
        valid_tokens.insert("access-1".to_string());
        Self {
            pending_polls: 0,
            slow_down_polls: 0,
            denied: false,
            expired: false,
            interval: 0,
            last_scope: String::new(),
            refresh_token: "refresh-1".to_string(),
            valid_tokens,
            files: BTreeMap::new(),
            next_id: 1,
            revoke_calls: 0,
            device_requests: 0,
            polls: 0,
            refresh_calls: 0,
            uploads: 0,
            fail_uploads: 0,
        }
    }
}

/// The running mock server. Dropping it shuts the HTTP server down.
pub struct MockGoogle {
    base_url: String,
    shared: Shared,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl MockGoogle {
    pub async fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind mock");
        let addr = listener.local_addr().expect("mock address");
        let shared: Shared = Arc::new(Mutex::new(MockState::default()));
        let app = Router::new()
            .route("/oauth/device/code", post(device_code))
            .route("/oauth/token", post(token))
            .route("/oauth/revoke", post(revoke))
            .route("/drive/v3/files", get(list_files).post(create_file))
            .route("/drive/v3/files/{id}", patch(patch_file))
            .route("/upload/drive/v3/files/{id}", patch(upload_content))
            .with_state(shared.clone());
        let (shutdown, receiver) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    receiver.await.ok();
                })
                .await;
        });
        Self {
            base_url: format!("http://{addr}"),
            shared,
            shutdown: Some(shutdown),
            task,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn set_pending(&self, polls: usize) {
        self.state().pending_polls = polls;
    }

    pub fn set_slow_down(&self, polls: usize) {
        self.state().slow_down_polls = polls;
    }

    pub fn set_denied(&self) {
        self.state().denied = true;
    }

    pub fn set_expired(&self) {
        self.state().expired = true;
    }

    pub fn set_interval(&self, seconds: u64) {
        self.state().interval = seconds;
    }

    pub fn fail_uploads(&self, count: usize) {
        self.state().fail_uploads = count;
    }

    pub fn device_requests(&self) -> usize {
        self.state().device_requests
    }

    /// The `scope` field of the last device code request.
    pub fn requested_scope(&self) -> String {
        self.state().last_scope.clone()
    }

    pub fn polls(&self) -> usize {
        self.state().polls
    }

    pub fn refresh_calls(&self) -> usize {
        self.state().refresh_calls
    }

    pub fn uploads(&self) -> usize {
        self.state().uploads
    }

    pub fn revoke_calls(&self) -> usize {
        self.state().revoke_calls
    }

    pub fn file_count(&self) -> usize {
        self.state().files.len()
    }

    /// How many files in the mock have this name (folders included). Used to
    /// prove a retried upload did not create a duplicate.
    pub fn count_named(&self, name: &str) -> usize {
        self.state()
            .files
            .values()
            .filter(|file| file.name == name)
            .count()
    }

    /// Every non-trashed file under the `Carrel` root, keyed by path relative
    /// to that root (`note.md`, `notes/a.md`, `.deleted/old.md`).
    pub fn paths(&self) -> BTreeMap<String, MockFile> {
        let state = self.state();
        let Some(root) = state
            .files
            .values()
            .find(|file| file.app_properties.get("carrel").map(String::as_str) == Some("root"))
            .map(|file| file.id.clone())
        else {
            return BTreeMap::new();
        };
        let mut paths = BTreeMap::new();
        for file in state.files.values() {
            if file.trashed || file.id == root {
                continue;
            }
            if let Some(path) = relative_path(file, &state.files, &root) {
                paths.insert(path, file.clone());
            }
        }
        paths
    }

    pub fn file(&self, path: &str) -> Option<MockFile> {
        self.paths().remove(path)
    }

    pub fn content(&self, path: &str) -> Option<String> {
        self.file(path)
            .map(|file| String::from_utf8_lossy(&file.content).to_string())
    }

    fn state(&self) -> std::sync::MutexGuard<'_, MockState> {
        self.shared.lock().expect("mock state poisoned")
    }
}

impl Drop for MockGoogle {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.abort();
    }
}

/// Path of `file` below `root`, or `None` when it is not a descendant.
fn relative_path(
    file: &MockFile,
    files: &BTreeMap<String, MockFile>,
    root: &str,
) -> Option<String> {
    let mut segments = vec![file.name.clone()];
    let mut current = file;
    for _ in 0..64 {
        let parent = current.parents.first()?;
        if parent == root {
            segments.reverse();
            return Some(segments.join("/"));
        }
        let parent_file = files.get(parent)?;
        if !parent_file.is_folder() {
            return None;
        }
        segments.push(parent_file.name.clone());
        current = parent_file;
    }
    None
}

async fn device_code(State(state): State<Shared>, body: Bytes) -> Response {
    let form = parse_form(&body);
    let mut state = state.lock().unwrap();
    state.device_requests += 1;
    state.last_scope = form.get("scope").cloned().unwrap_or_default();
    Json(json!({
        "device_code": "device-1",
        "user_code": "TEST-CODE",
        "verification_url": "https://example.test/device",
        "expires_in": 600,
        "interval": state.interval,
    }))
    .into_response()
}

async fn token(State(state): State<Shared>, body: Bytes) -> Response {
    let form = parse_form(&body);
    let mut state = state.lock().unwrap();
    match form.get("grant_type").map(String::as_str) {
        Some("urn:ietf:params:oauth:grant-type:device_code") => {
            state.polls += 1;
            if state.denied {
                return oauth_error("access_denied", "the user said no");
            }
            if state.expired {
                return oauth_error("expired_token", "the code expired");
            }
            if state.pending_polls > 0 {
                state.pending_polls -= 1;
                return oauth_error("authorization_pending", "waiting for approval");
            }
            if state.slow_down_polls > 0 {
                state.slow_down_polls -= 1;
                return oauth_error("slow_down", "poll more slowly");
            }
            Json(json!({
                "access_token": "access-1",
                "refresh_token": state.refresh_token,
                "expires_in": 3600,
                "token_type": "Bearer",
            }))
            .into_response()
        }
        Some("refresh_token") => {
            state.refresh_calls += 1;
            let matches = form.get("refresh_token") == Some(&state.refresh_token);
            if !matches {
                return oauth_error("invalid_grant", "token revoked");
            }
            let token = format!("access-refreshed-{}", state.refresh_calls);
            state.valid_tokens.insert(token.clone());
            Json(json!({
                "access_token": token,
                "expires_in": 3600,
                "token_type": "Bearer",
            }))
            .into_response()
        }
        _ => oauth_error("unsupported_grant_type", "unknown grant"),
    }
}

async fn revoke(State(state): State<Shared>, body: Bytes) -> Response {
    let form = parse_form(&body);
    let mut state = state.lock().unwrap();
    state.revoke_calls += 1;
    if let Some(token) = form.get("token") {
        state.valid_tokens.remove(token);
    }
    Json(json!({})).into_response()
}

async fn list_files(
    State(state): State<Shared>,
    headers: HeaderMap,
    Query(params): Query<BTreeMap<String, String>>,
) -> Response {
    let state = state.lock().unwrap();
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    let query = params.get("q").map(String::as_str).unwrap_or("");
    let files: Vec<Value> = state
        .files
        .values()
        .filter(|file| matches_query(file, query))
        .map(file_json)
        .collect();
    Json(json!({ "files": files, "kind": "drive#fileList" })).into_response()
}

async fn create_file(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let mut state = state.lock().unwrap();
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    let id = format!("file-{}", state.next_id);
    state.next_id += 1;
    let file = MockFile {
        id: id.clone(),
        name: body["name"].as_str().unwrap_or_default().to_string(),
        mime_type: body["mimeType"]
            .as_str()
            .unwrap_or("application/octet-stream")
            .to_string(),
        parents: body["parents"]
            .as_array()
            .map(|parents| {
                parents
                    .iter()
                    .filter_map(|parent| parent.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        app_properties: body["appProperties"]
            .as_object()
            .map(|properties| {
                properties
                    .iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|value| (key.clone(), value.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        trashed: false,
        content: Vec::new(),
    };
    let reply = file_json(&file);
    state.files.insert(id, file);
    Json(reply).into_response()
}

async fn patch_file(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<BTreeMap<String, String>>,
    Json(body): Json<Value>,
) -> Response {
    let mut state = state.lock().unwrap();
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    let Some(file) = state.files.get_mut(&id) else {
        return drive_error(StatusCode::NOT_FOUND, "File not found");
    };
    if let Some(name) = body["name"].as_str() {
        file.name = name.to_string();
    }
    if let Some(add) = params.get("addParents") {
        if !file.parents.iter().any(|parent| parent == add) {
            file.parents.push(add.clone());
        }
    }
    if let Some(remove) = params.get("removeParents") {
        file.parents.retain(|parent| parent != remove);
    }
    let reply = file_json(file);
    Json(reply).into_response()
}

async fn upload_content(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let mut state = state.lock().unwrap();
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    if state.fail_uploads > 0 {
        state.fail_uploads -= 1;
        return drive_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "temporary upload failure",
        );
    }
    let reply = {
        let Some(file) = state.files.get_mut(&id) else {
            return drive_error(StatusCode::NOT_FOUND, "File not found");
        };
        file.content = body.to_vec();
        file_json(file)
    };
    state.uploads += 1;
    Json(reply).into_response()
}

fn authorized(state: &MockState, headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| state.valid_tokens.contains(token))
}

fn matches_query(file: &MockFile, query: &str) -> bool {
    if file.trashed {
        return false;
    }
    if let Some(rest) = query.split("appProperties has").nth(1) {
        let key = between(rest, "key='", "'").unwrap_or_default();
        let value = between(rest, "value='", "'").unwrap_or_default();
        if file.app_properties.get(key).map(String::as_str) != Some(value) {
            return false;
        }
    }
    true
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let start = text.find(start)? + start.len();
    let rest = &text[start..];
    let end = rest.find(end)?;
    Some(&rest[..end])
}

fn parse_form(body: &[u8]) -> BTreeMap<String, String> {
    url::form_urlencoded::parse(body).into_owned().collect()
}

fn file_json(file: &MockFile) -> Value {
    json!({
        "id": file.id,
        "name": file.name,
        "mimeType": file.mime_type,
        "parents": file.parents,
        "appProperties": file.app_properties,
        "trashed": file.trashed,
    })
}

fn unauthorized() -> Response {
    drive_error(StatusCode::UNAUTHORIZED, "Invalid credentials")
}

fn oauth_error(code: &str, description: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": code, "error_description": description })),
    )
        .into_response()
}

fn drive_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({ "error": { "code": status.as_u16(), "message": message } })),
    )
        .into_response()
}
