//! End-to-end tests for the `/library` half of a sync: a mock Nostos server
//! over TLS serves paged books, items and tombstones; the real `carreld` sync
//! fetches every page, applies the deletions, keeps the cursor and treats a
//! `404` as "this backend has no library".

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use carrel_protocol::{
    FileList, LibraryBook, LibraryCitation, LibraryItem, LibraryItemKind, LibraryPage,
};
use carreld::library::LibraryCache;
use carreld::{sync_once, SyncConfig, SyncOptions};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

#[derive(Clone, Default)]
struct MockState {
    /// Response keyed by the requested `since` cursor.
    pages: Arc<Mutex<HashMap<String, LibraryPage>>>,
    requests: Arc<Mutex<Vec<String>>>,
    not_found: Arc<Mutex<bool>>,
}

struct MockNostos {
    url: String,
    fingerprint: String,
    state: MockState,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl MockNostos {
    async fn start() -> Self {
        let dir = tempfile::tempdir().expect("tls tempdir");
        let material =
            carrel_server::tls::load_or_generate(dir.path(), "mock-nostos").expect("tls material");
        let fingerprint = material.fingerprint_hex.clone();
        let acceptor = carrel_server::tls::acceptor(&material).expect("tls acceptor");
        drop(material);
        drop(dir);

        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind mock");
        let addr = listener.local_addr().expect("mock address");
        let state = MockState::default();
        let app = Router::new()
            .route("/files", get(files))
            .route("/library", get(library))
            .with_state(state.clone());
        let (shutdown, receiver) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = axum::serve(
                carrel_server::tls::TlsListener::new(listener, acceptor),
                app,
            )
            .with_graceful_shutdown(async {
                receiver.await.ok();
            })
            .await;
        });
        Self {
            url: format!("https://127.0.0.1:{}", addr.port()),
            fingerprint,
            state,
            shutdown: Some(shutdown),
            task,
        }
    }

    fn set_page(&self, since: &str, page: LibraryPage) {
        self.state
            .pages
            .lock()
            .unwrap()
            .insert(since.to_string(), page);
    }

    fn set_not_found(&self, not_found: bool) {
        *self.state.not_found.lock().unwrap() = not_found;
    }

    fn library_requests(&self) -> Vec<String> {
        self.state.requests.lock().unwrap().clone()
    }

    fn config(&self) -> SyncConfig {
        SyncConfig {
            server_url: self.url.clone(),
            token: "test-token".into(),
            cert_sha256: Some(self.fingerprint.clone()),
            server_name: Some("Mock Nostos".into()),
            device_pubkey: String::new(),
        }
    }
}

impl Drop for MockNostos {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.abort();
    }
}

async fn files() -> Json<FileList> {
    Json(FileList {
        files: Vec::new(),
        cursor: "0".into(),
    })
}

async fn library(
    State(state): State<MockState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let since = query.get("since").cloned().unwrap_or_default();
    state.requests.lock().unwrap().push(since.clone());
    if *state.not_found.lock().unwrap() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "library requires a Nostos account"})),
        )
            .into_response();
    }
    let page = state
        .pages
        .lock()
        .unwrap()
        .get(&since)
        .cloned()
        .unwrap_or(LibraryPage {
            cursor: since,
            ..LibraryPage::default()
        });
    Json(page).into_response()
}

fn book(id: &str, title: &str, author: &str, topics: &[&str]) -> LibraryBook {
    LibraryBook {
        id: id.into(),
        title: title.into(),
        author: author.into(),
        topics: topics.iter().map(|topic| topic.to_string()).collect(),
    }
}

fn item(
    id: &str,
    book_id: &str,
    kind: LibraryItemKind,
    text: &str,
    topics: &[&str],
) -> LibraryItem {
    LibraryItem {
        id: id.into(),
        kind,
        book_id: book_id.into(),
        text: text.into(),
        note: String::new(),
        location: "p. 41".into(),
        topics: topics.iter().map(|topic| topic.to_string()).collect(),
        citation: Some(LibraryCitation {
            markdown: format!("[The Quiet Hours, p. 41](nostos:cite/{id})"),
            url: format!("https://nostos.example/library/{id}"),
        }),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_fetches_every_page_and_applies_library_deletions() {
    let mock = MockNostos::start().await;
    mock.set_page(
        "0",
        LibraryPage {
            books: vec![book("b1", "The Quiet Hours", "Mara Ellison", &["writing"])],
            items: vec![
                item(
                    "q1",
                    "b1",
                    LibraryItemKind::Highlight,
                    "A sentence is a small room.",
                    &["writing"],
                ),
                item(
                    "q2",
                    "b1",
                    LibraryItemKind::Passage,
                    "The kettle clicks off at six.",
                    &["writing"],
                ),
            ],
            deleted: Vec::new(),
            cursor: "1".into(),
        },
    );
    mock.set_page(
        "1",
        LibraryPage {
            books: vec![book(
                "b1",
                "The Quiet Hours",
                "Mara Ellison",
                &["writing", "attention"],
            )],
            items: vec![item(
                "q3",
                "b1",
                LibraryItemKind::Note,
                "Write before the house wakes.",
                &["attention"],
            )],
            deleted: vec!["q1".into()],
            cursor: "2".into(),
        },
    );

    let dir = tempfile::tempdir().unwrap();
    let writing = dir.path().join("writing");
    let state = dir.path().join("state");
    std::fs::create_dir_all(&writing).unwrap();
    mock.config().save(&state).unwrap();

    let report = sync_once(&writing, &state, &SyncOptions::default())
        .await
        .expect("library sync");
    assert_eq!(report.library_items, 3, "{report:?}");
    assert_eq!(report.library_deleted, 1, "{report:?}");
    assert!(!report.library_unavailable, "{report:?}");
    assert_eq!(mock.library_requests(), vec!["0", "1", "2"]);

    let cache = LibraryCache::from_state_dir(&state).unwrap();
    assert_eq!(cache.cursor().unwrap(), "2");
    assert!(!cache.is_unavailable().unwrap());
    assert_eq!(cache.count().unwrap(), 2);
    assert_eq!(cache.search("kettle", &HashMap::new()).unwrap()[0].id, "q2");
    let hits = cache.search("sentence", &HashMap::new()).unwrap();
    assert!(hits.is_empty(), "q1 was deleted: {hits:?}");
    let hits = cache.search("wakes", &HashMap::new()).unwrap();
    assert_eq!(hits[0].id, "q3");
    assert_eq!(hits[0].book_title, "The Quiet Hours");

    // A second sync asks from the saved cursor, gets an empty page and changes
    // nothing: the library cursor is incremental, not a full rescan.
    let again = sync_once(&writing, &state, &SyncOptions::default())
        .await
        .unwrap();
    assert!(again.is_noop(), "{again:?}");
    assert_eq!(mock.library_requests(), vec!["0", "1", "2", "2"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_404_marks_the_library_unavailable_without_failing_the_sync() {
    let mock = MockNostos::start().await;
    mock.set_not_found(true);

    let dir = tempfile::tempdir().unwrap();
    let writing = dir.path().join("writing");
    let state = dir.path().join("state");
    std::fs::create_dir_all(&writing).unwrap();
    mock.config().save(&state).unwrap();

    let report = sync_once(&writing, &state, &SyncOptions::default())
        .await
        .expect("a self-hosted server's 404 is not a sync error");
    assert!(report.library_unavailable, "{report:?}");
    assert_eq!(report.library_items, 0);
    let cache = LibraryCache::from_state_dir(&state).unwrap();
    assert!(cache.is_unavailable().unwrap());
    assert_eq!(
        cache.cursor().unwrap(),
        "0",
        "404 must not advance the cursor"
    );

    // Pairing with Nostos later (same URL in the test) retries from zero and
    // clears the unavailable mark.
    mock.set_not_found(false);
    mock.set_page(
        "0",
        LibraryPage {
            books: vec![book("b1", "The Quiet Hours", "Mara Ellison", &[])],
            items: vec![item(
                "q1",
                "b1",
                LibraryItemKind::Highlight,
                "A sentence is a small room.",
                &[],
            )],
            deleted: Vec::new(),
            cursor: "1".into(),
        },
    );
    let report = sync_once(&writing, &state, &SyncOptions::default())
        .await
        .unwrap();
    assert!(!report.library_unavailable, "{report:?}");
    assert_eq!(report.library_items, 1, "{report:?}");
    let cache = LibraryCache::from_state_dir(&state).unwrap();
    assert!(!cache.is_unavailable().unwrap());
    assert_eq!(cache.cursor().unwrap(), "1");
    assert_eq!(cache.count().unwrap(), 1);
    assert_eq!(mock.library_requests(), vec!["0", "0", "1"]);
}
