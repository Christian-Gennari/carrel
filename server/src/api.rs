//! HTTP API and pairing approval handlers.
//!
//! All responses are JSON except `GET /approve`, which serves the one small
//! HTML page. Every `/files` request requires the bearer token issued at
//! pairing time.

use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use serde::Deserialize;
use carrel_protocol::{
    endpoints, now_unix, validate_path, ConflictResponse, DeleteResponse, ErrorResponse, FileList,
    PairStartRequest, PairStartResponse, PairStatus, PairStatusResponse, PutFileRequest,
    PutFileResponse,
};
use uuid::Uuid;

use crate::approve::render_page;
use crate::store::{random_token, ApproveOutcome, DeleteOutcome, PutOutcome, Store};

/// Shared server state passed to axum handlers.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    store: Mutex<Store>,
    admin_password: String,
    csrf: String,
    cert_sha256: String,
    label: String,
    max_file_bytes: usize,
}

impl AppState {
    pub fn new(
        store: Store,
        admin_password: String,
        csrf: String,
        cert_sha256: String,
        label: String,
        max_file_bytes: usize,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                store: Mutex::new(store),
                admin_password,
                csrf,
                cert_sha256,
                label,
                max_file_bytes,
            }),
        }
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        self.inner
            .store
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn label(&self) -> &str {
        &self.inner.label
    }

    fn cert_sha256(&self) -> &str {
        &self.inner.cert_sha256
    }

    fn csrf(&self) -> &str {
        &self.inner.csrf
    }

    fn admin_password(&self) -> &str {
        &self.inner.admin_password
    }

    fn max_file_bytes(&self) -> usize {
        self.inner.max_file_bytes
    }
}

/// Build the full application router.
pub fn router(state: AppState) -> Router {
    // JSON escaping can make a body larger than the Markdown file itself.
    let body_limit = state
        .max_file_bytes()
        .saturating_mul(2)
        .saturating_add(64 * 1024);
    Router::new()
        .route(endpoints::HEALTH, get(health))
        .route(endpoints::PAIR_START, post(pair_start))
        .route("/pair/{pair_id}", get(pair_status))
        .route(endpoints::APPROVE, get(approve_page).post(approve_submit))
        .route(endpoints::FILES, get(list_files))
        .route(
            "/files/{file_id}",
            get(get_file).put(put_file).delete(delete_file),
        )
        .route(endpoints::LIBRARY, get(library))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(body_limit))
        .with_state(state)
}

/// An error that renders as the protocol's JSON error body.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
    www_authenticate: Option<&'static str>,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            www_authenticate: None,
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn unauthorized() -> Self {
        let mut error = Self::new(StatusCode::UNAUTHORIZED, "missing or invalid token");
        error.www_authenticate = Some("Bearer");
        error
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(error = %error, "request failed");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(ErrorResponse {
            error: self.message,
        });
        let mut response = (self.status, body).into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        if let Some(scheme) = self.www_authenticate {
            if let Ok(value) = HeaderValue::from_str(scheme) {
                response
                    .headers_mut()
                    .insert(header::WWW_AUTHENTICATE, value);
            }
        }
        response
    }
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "protocol": carrel_protocol::PROTOCOL_VERSION,
        "name": state.label(),
    }))
}

async fn pair_start(
    State(state): State<AppState>,
    Json(request): Json<PairStartRequest>,
) -> Result<Json<PairStartResponse>, ApiError> {
    let device_name = request.device_name.trim();
    if device_name.is_empty() || device_name.chars().count() > 100 {
        return Err(ApiError::bad_request(
            "device_name must be 1-100 characters",
        ));
    }
    let device_pubkey = request.device_pubkey.trim();
    if device_pubkey.is_empty() || device_pubkey.len() > 256 {
        return Err(ApiError::bad_request(
            "device_pubkey must be 1-256 characters",
        ));
    }

    let now = now_unix();
    let mut store = state.store();
    for _ in 0..8 {
        let mut bytes = [0u8; 6];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut bytes);
        match store.create_pairing(device_name, device_pubkey, &bytes, now) {
            Ok(Some((pair_id, code, expires_at))) => {
                return Ok(Json(PairStartResponse {
                    pair_id,
                    code,
                    expires_at,
                }))
            }
            Ok(None) => continue,
            Err(error) => return Err(ApiError::internal(error)),
        }
    }
    Err(ApiError::internal("could not allocate a pairing code"))
}

async fn pair_status(
    State(state): State<AppState>,
    Path(pair_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let now = now_unix();
    let record = state
        .store()
        .pairing(pair_id, now)
        .map_err(ApiError::internal)?;
    let Some(record) = record else {
        return Err(ApiError::not_found("unknown pairing"));
    };
    let response = PairStatusResponse {
        status: record.status,
        token: if record.status == PairStatus::Approved {
            record.token
        } else {
            None
        },
        server_cert_sha256: Some(state.cert_sha256().to_string()),
        server_name: Some(state.label().to_string()),
    };
    let mut response = Json(response).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[derive(Deserialize)]
struct ListQuery {
    since: Option<String>,
}

async fn list_files(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<FileList>, ApiError> {
    require_bearer(&state, &headers)?;
    let since = match query.since.as_deref() {
        None | Some("") => 0,
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| ApiError::bad_request("invalid cursor"))?,
    };
    let (files, cursor) = state
        .store()
        .changes_since(since)
        .map_err(ApiError::internal)?;
    Ok(Json(FileList { files, cursor }))
}

async fn get_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(file_id): Path<Uuid>,
) -> Result<Json<carrel_protocol::FileContent>, ApiError> {
    require_bearer(&state, &headers)?;
    let content = state
        .store()
        .file_content(file_id)
        .map_err(ApiError::internal)?;
    content
        .map(Json)
        .ok_or_else(|| ApiError::not_found("file not found"))
}

async fn put_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(file_id): Path<Uuid>,
    Json(request): Json<PutFileRequest>,
) -> Result<Response, ApiError> {
    require_bearer(&state, &headers)?;
    validate_path(&request.path).map_err(|error| ApiError::bad_request(error.to_string()))?;
    if request.content.len() > state.max_file_bytes() {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("file is larger than {} bytes", state.max_file_bytes()),
        ));
    }
    let outcome = state
        .store()
        .put(
            file_id,
            &request.path,
            &request.content,
            request.base_rev,
            now_unix(),
        )
        .map_err(ApiError::internal)?;
    match outcome {
        PutOutcome::Success { rev, hash } => {
            Ok(Json(PutFileResponse { rev, hash }).into_response())
        }
        PutOutcome::Conflict(conflict) => {
            Ok((StatusCode::CONFLICT, Json(ConflictResponse { conflict })).into_response())
        }
    }
}

#[derive(Deserialize)]
struct DeleteQuery {
    base_rev: Option<u64>,
}

async fn delete_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(file_id): Path<Uuid>,
    Query(query): Query<DeleteQuery>,
) -> Result<Response, ApiError> {
    require_bearer(&state, &headers)?;
    let Some(base_rev) = query.base_rev else {
        return Err(ApiError::bad_request("base_rev is required"));
    };
    let outcome = state
        .store()
        .delete(file_id, base_rev, now_unix())
        .map_err(ApiError::internal)?;
    match outcome {
        DeleteOutcome::Success { rev } => Ok(Json(DeleteResponse { rev }).into_response()),
        DeleteOutcome::Conflict(conflict) => {
            Ok((StatusCode::CONFLICT, Json(ConflictResponse { conflict })).into_response())
        }
    }
}

async fn library() -> ApiError {
    ApiError::not_found("library requires a Nostos account")
}

async fn not_found() -> ApiError {
    ApiError::not_found("not found")
}

async fn approve_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !has_admin_auth(&headers, state.admin_password()) {
        return basic_unauthorized();
    }
    let now = now_unix();
    let pending = match state.store().pending_pairings(now) {
        Ok(pending) => pending,
        Err(error) => return ApiError::internal(error).into_response(),
    };
    html_response(render_page(
        state.label(),
        state.cert_sha256(),
        &pending,
        state.csrf(),
        None,
        now,
    ))
}

#[derive(Deserialize)]
struct ApproveForm {
    csrf: String,
    pair_id: Option<String>,
    code: Option<String>,
}

async fn approve_submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Form(form): axum::Form<ApproveForm>,
) -> Response {
    if !has_admin_auth(&headers, state.admin_password()) {
        return basic_unauthorized();
    }
    if !constant_time_eq(form.csrf.as_bytes(), state.csrf().as_bytes()) {
        return ApiError::new(StatusCode::FORBIDDEN, "invalid form token").into_response();
    }
    let pair_id = match form.pair_id.as_deref() {
        Some(value) if !value.is_empty() => match Uuid::parse_str(value) {
            Ok(pair_id) => Some(pair_id),
            Err(_) => return ApiError::bad_request("invalid pair_id").into_response(),
        },
        _ => None,
    };
    let now = now_unix();
    let mut store = state.store();
    let outcome = match store.approve(pair_id, form.code.as_deref(), Some(random_token()), now) {
        Ok(outcome) => outcome,
        Err(error) => return ApiError::internal(error).into_response(),
    };
    let message = match &outcome {
        ApproveOutcome::Approved { device_name, .. } => {
            format!("Approved pairing for {device_name}.")
        }
        ApproveOutcome::AlreadyApproved => "That pairing is already approved.".to_string(),
        ApproveOutcome::Expired => "That code has expired. Start pairing again.".to_string(),
        ApproveOutcome::NotFound => "No pending pairing with that code.".to_string(),
    };
    let pending = match store.pending_pairings(now) {
        Ok(pending) => pending,
        Err(error) => return ApiError::internal(error).into_response(),
    };
    drop(store);
    let status = if matches!(outcome, ApproveOutcome::Approved { .. }) {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    let mut response = html_response_with_status(
        render_page(
            state.label(),
            state.cert_sha256(),
            &pending,
            state.csrf(),
            Some(&message),
            now,
        ),
        status,
    );
    if matches!(outcome, ApproveOutcome::Approved { .. }) {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

fn require_bearer(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(ApiError::unauthorized());
    };
    let mut parts = value.splitn(2, ' ');
    let scheme = parts.next().unwrap_or_default();
    let token = parts.next().unwrap_or_default().trim();
    if !scheme.eq_ignore_ascii_case("Bearer") || token.is_empty() {
        return Err(ApiError::unauthorized());
    }
    let valid = state
        .store()
        .token_valid(token)
        .map_err(ApiError::internal)?;
    if valid {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}

fn has_admin_auth(headers: &HeaderMap, password: &str) -> bool {
    if password.is_empty() {
        return false;
    }
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let mut parts = value.splitn(2, ' ');
    let scheme = parts.next().unwrap_or_default();
    let encoded = parts.next().unwrap_or_default().trim();
    if !scheme.eq_ignore_ascii_case("Basic") || encoded.is_empty() {
        return false;
    }
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return false;
    };
    let Ok(text) = String::from_utf8(decoded) else {
        return false;
    };
    let Some((_user, supplied)) = text.split_once(':') else {
        return false;
    };
    constant_time_eq(supplied.as_bytes(), password.as_bytes())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right) {
        difference |= a ^ b;
    }
    difference == 0
}

fn basic_unauthorized() -> Response {
    let mut response =
        ApiError::new(StatusCode::UNAUTHORIZED, "admin password required").into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"Carrel server\", charset=\"UTF-8\""),
    );
    response
}

fn html_response(body: String) -> Response {
    html_response_with_status(body, StatusCode::OK)
}

fn html_response_with_status(body: String, status: StatusCode) -> Response {
    let mut response = (status, Html(body)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; base-uri 'none'",
        ),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
}
