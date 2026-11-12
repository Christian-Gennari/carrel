//! Wire types for every endpoint in Stylos Sync Protocol v1.
//!
//! Serialization is plain JSON. Field names are exactly the names in
//! `spec/02-sync-protocol.md` so other implementations can follow the document
//! without reading this crate.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Convenience constructors for the protocol's URL paths.
pub mod endpoints {
    use super::Uuid;

    pub const PAIR_START: &str = "/pair/start";
    pub const APPROVE: &str = "/approve";
    pub const FILES: &str = "/files";
    pub const LIBRARY: &str = "/library";
    pub const HEALTH: &str = "/healthz";

    pub fn pair_status(pair_id: Uuid) -> String {
        format!("/pair/{pair_id}")
    }

    /// `GET /files?since=<cursor>`. Cursors are opaque strings, but this
    /// implementation issues decimal integers, so no escaping is needed.
    pub fn files_since(cursor: &str) -> String {
        format!("/files?since={cursor}")
    }

    pub fn file(file_id: Uuid) -> String {
        format!("/files/{file_id}")
    }

    pub fn file_delete(file_id: Uuid, base_rev: u64) -> String {
        format!("/files/{file_id}?base_rev={base_rev}")
    }

    /// `GET /library?since=<cursor>`. The cursor is opaque, exactly like the
    /// file cursor; servers must URL-encode it if it contains reserved
    /// characters, and clients pass it back unchanged.
    pub fn library_since(cursor: &str) -> String {
        format!("/library?since={cursor}")
    }
}

/// `POST /pair/start` request body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairStartRequest {
    pub device_name: String,
    pub device_pubkey: String,
}

/// `POST /pair/start` response body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairStartResponse {
    pub pair_id: Uuid,
    /// Human code in `XXX-XXX` form.
    pub code: String,
    /// Unix seconds after which the code can no longer be approved.
    pub expires_at: i64,
}

/// Pairing status reported by `GET /pair/{pair_id}`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PairStatus {
    Pending,
    Approved,
    Expired,
}

impl PairStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PairStatus::Pending => "pending",
            PairStatus::Approved => "approved",
            PairStatus::Expired => "expired",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(PairStatus::Pending),
            "approved" => Some(PairStatus::Approved),
            "expired" => Some(PairStatus::Expired),
            _ => None,
        }
    }
}

/// `GET /pair/{pair_id}` response body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairStatusResponse {
    pub status: PairStatus,
    /// Present once the status is `approved`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// SHA-256 of the server certificate's DER encoding, lowercase hex.
    /// Self-hosted servers return it so the device can pin the certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_cert_sha256: Option<String>,
    /// Human label of the server, used in conflict-copy names. Additive field:
    /// clients fall back to the URL's host when it is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
}

/// One file in a change-list response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    pub file_id: Uuid,
    pub path: String,
    /// Content hash of the last version (still set on tombstones).
    pub hash: String,
    pub rev: u64,
    pub deleted: bool,
    /// Unix seconds of the server-side change.
    pub modified_at: i64,
}

/// `GET /files?since=<cursor>` response body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileList {
    pub files: Vec<FileEntry>,
    pub cursor: String,
}

/// `GET /files/{file_id}` response body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileContent {
    pub file_id: Uuid,
    pub path: String,
    pub hash: String,
    pub rev: u64,
    pub content: String,
}

/// `PUT /files/{file_id}` request body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PutFileRequest {
    pub path: String,
    pub content: String,
    /// The server rev this edit is based on. `null` creates a new file.
    pub base_rev: Option<u64>,
}

/// Successful `PUT` response body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PutFileResponse {
    pub rev: u64,
    pub hash: String,
}

/// Successful `DELETE` response body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeleteResponse {
    pub rev: u64,
}

/// The server's current version, sent with `409 Conflict`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConflictInfo {
    pub server_rev: u64,
    pub server_hash: String,
    /// Additive: the file currently occupying `path` when it is not the file
    /// being written (a rename collision). Clients preserve its content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_file_id: Option<Uuid>,
}

/// `409 Conflict` response body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConflictResponse {
    pub conflict: ConflictInfo,
}

/// Generic error body used for 400/401/404/500 responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorResponse {
    pub error: String,
}

// ---------------------------------------------------------------------------
// Library (Nostos only)
// ---------------------------------------------------------------------------

/// What a library item is: a highlighted passage, a note the writer made, or a
/// passage the writer collected.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LibraryItemKind {
    Highlight,
    Note,
    Passage,
}

impl LibraryItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LibraryItemKind::Highlight => "highlight",
            LibraryItemKind::Note => "note",
            LibraryItemKind::Passage => "passage",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "highlight" => Some(LibraryItemKind::Highlight),
            "note" => Some(LibraryItemKind::Note),
            "passage" => Some(LibraryItemKind::Passage),
            _ => None,
        }
    }
}

/// Deserialize `null` as the type's default. Servers may send `null` for an
/// optional library field (for example `"location": null` when the location is
/// unknown); a missing field is handled by `#[serde(default)]`.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// How to cite an item: ready-to-insert Markdown plus the canonical URL.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryCitation {
    #[serde(default, deserialize_with = "null_as_default")]
    pub markdown: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub url: String,
}

/// One book in the writer's library. `author` and `topics` may be absent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryBook {
    pub id: String,
    pub title: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub author: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub topics: Vec<String>,
}

/// One highlight, note or passage.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryItem {
    pub id: String,
    pub kind: LibraryItemKind,
    pub book_id: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub text: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub note: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub location: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub topics: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation: Option<LibraryCitation>,
}

/// `GET /library?since=<cursor>` response body.
///
/// A page may add or update books and items and remove item ids. The server
/// returns pages until one is empty; the last page's cursor is the new
/// incremental position.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryPage {
    #[serde(default, deserialize_with = "null_as_default")]
    pub books: Vec<LibraryBook>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub items: Vec<LibraryItem>,
    /// Ids of items deleted since the requested cursor.
    #[serde(default, deserialize_with = "null_as_default")]
    pub deleted: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub cursor: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn id() -> Uuid {
        Uuid::parse_str("0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b").unwrap()
    }

    #[test]
    fn pair_start_shape() {
        let request = PairStartRequest {
            device_name: "X250".into(),
            device_pubkey: "ab".repeat(32),
        };
        let value: Value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["device_name"], "X250");
        assert!(value["device_pubkey"].is_string());

        let response = PairStartResponse {
            pair_id: id(),
            code: "K7Q-4MZ".into(),
            expires_at: 1_800_000_000,
        };
        let value: Value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["pair_id"], id().to_string());
        assert_eq!(value["code"], "K7Q-4MZ");
        assert_eq!(value["expires_at"], 1_800_000_000);
    }

    #[test]
    fn pair_status_uses_lowercase_strings() {
        assert_eq!(
            serde_json::to_value(PairStatus::Pending).unwrap(),
            "pending"
        );
        assert_eq!(
            serde_json::to_value(PairStatus::Approved).unwrap(),
            "approved"
        );
        assert_eq!(
            serde_json::to_value(PairStatus::Expired).unwrap(),
            "expired"
        );
        assert_eq!(PairStatus::parse("approved"), Some(PairStatus::Approved));
        assert_eq!(PairStatus::parse("APPROVED"), None);
    }

    #[test]
    fn pending_status_omits_secrets() {
        let response = PairStatusResponse {
            status: PairStatus::Pending,
            token: None,
            server_cert_sha256: None,
            server_name: Some("study".into()),
        };
        let value: Value = serde_json::to_value(&response).unwrap();
        assert!(value.get("token").is_none());
        assert!(value.get("server_cert_sha256").is_none());
        assert_eq!(value["server_name"], "study");
    }

    #[test]
    fn file_list_shape_matches_spec() {
        let list = FileList {
            files: vec![FileEntry {
                file_id: id(),
                path: "notes/one.md".into(),
                hash: "abc".into(),
                rev: 2,
                deleted: false,
                modified_at: 1_800_000_000,
            }],
            cursor: "7".into(),
        };
        let value: Value = serde_json::to_value(&list).unwrap();
        assert_eq!(
            value,
            json!({
                "files": [{
                    "file_id": id(),
                    "path": "notes/one.md",
                    "hash": "abc",
                    "rev": 2,
                    "deleted": false,
                    "modified_at": 1_800_000_000
                }],
                "cursor": "7"
            })
        );
    }

    #[test]
    fn conflict_shape_matches_spec() {
        let response = ConflictResponse {
            conflict: ConflictInfo {
                server_rev: 3,
                server_hash: "def".into(),
                server_file_id: None,
            },
        };
        let value: Value = serde_json::to_value(&response).unwrap();
        assert_eq!(
            value,
            json!({"conflict": {"server_rev": 3, "server_hash": "def"}})
        );
    }

    #[test]
    fn put_request_accepts_null_base_rev() {
        let request: PutFileRequest =
            serde_json::from_value(json!({"path": "a.md", "content": "x", "base_rev": null}))
                .unwrap();
        assert_eq!(request.base_rev, None);
        let request: PutFileRequest =
            serde_json::from_value(json!({"path": "a.md", "content": "x", "base_rev": 4})).unwrap();
        assert_eq!(request.base_rev, Some(4));
    }

    #[test]
    fn endpoint_builders() {
        let id = id();
        assert_eq!(endpoints::PAIR_START, "/pair/start");
        assert_eq!(endpoints::files_since("12"), "/files?since=12");
        assert_eq!(endpoints::file(id), format!("/files/{id}"));
        assert_eq!(
            endpoints::file_delete(id, 3),
            format!("/files/{id}?base_rev=3")
        );
        assert_eq!(endpoints::pair_status(id), format!("/pair/{id}"));
        assert_eq!(endpoints::library_since("40"), "/library?since=40");
    }

    #[test]
    fn library_page_shape_matches_the_spec() {
        let page = LibraryPage {
            books: vec![LibraryBook {
                id: "book-1".into(),
                title: "The Quiet Hours".into(),
                author: "Mara Ellison".into(),
                topics: vec!["writing".into()],
            }],
            items: vec![LibraryItem {
                id: "quote-1".into(),
                kind: LibraryItemKind::Highlight,
                book_id: "book-1".into(),
                text: "A sentence is a small room.".into(),
                note: String::new(),
                location: "p. 41".into(),
                topics: vec!["writing".into()],
                citation: Some(LibraryCitation {
                    markdown: "[The Quiet Hours, p. 41](nostos:cite/quote-1)".into(),
                    url: "https://nostos.app/library/quote-1".into(),
                }),
            }],
            deleted: vec!["quote-0".into()],
            cursor: "7".into(),
        };
        let value: Value = serde_json::to_value(&page).unwrap();
        assert_eq!(
            value,
            json!({
                "books": [{
                    "id": "book-1",
                    "title": "The Quiet Hours",
                    "author": "Mara Ellison",
                    "topics": ["writing"]
                }],
                "items": [{
                    "id": "quote-1",
                    "kind": "highlight",
                    "book_id": "book-1",
                    "text": "A sentence is a small room.",
                    "note": "",
                    "location": "p. 41",
                    "topics": ["writing"],
                    "citation": {
                        "markdown": "[The Quiet Hours, p. 41](nostos:cite/quote-1)",
                        "url": "https://nostos.app/library/quote-1"
                    }
                }],
                "deleted": ["quote-0"],
                "cursor": "7"
            })
        );
    }

    #[test]
    fn library_items_tolerate_optional_fields() {
        let page: LibraryPage = serde_json::from_value(json!({
            "items": [{
                "id": "n1",
                "kind": "note",
                "book_id": "b1"
            }],
            "cursor": "1"
        }))
        .unwrap();
        assert!(page.books.is_empty());
        assert!(page.deleted.is_empty());
        let item = &page.items[0];
        assert_eq!(item.kind, LibraryItemKind::Note);
        assert!(item.text.is_empty());
        assert!(item.citation.is_none());
        assert!(item.topics.is_empty());

        for (kind, text) in [
            (LibraryItemKind::Highlight, "highlight"),
            (LibraryItemKind::Note, "note"),
            (LibraryItemKind::Passage, "passage"),
        ] {
            assert_eq!(kind.as_str(), text);
            assert_eq!(LibraryItemKind::parse(text), Some(kind));
        }
        assert_eq!(LibraryItemKind::parse("unknown"), None);
    }

    #[test]
    fn library_accepts_null_optional_fields() {
        // Shape sent by Nostos for a paper-book highlight without a location.
        let page: LibraryPage = serde_json::from_value(json!({
            "books": [{"id": "b1", "title": "Gilead", "author": null, "topics": null}],
            "items": [{
                "id": "n1", "kind": "highlight", "book_id": "b1",
                "text": "ember", "note": null, "location": null, "topics": null,
                "citation": {"markdown": "Marilynne Robinson, *Gilead*", "url": null}
            }],
            "deleted": null,
            "cursor": "1"
        }))
        .unwrap();
        assert_eq!(page.books[0].author, "");
        assert_eq!(page.items[0].location, "");
        assert_eq!(page.items[0].note, "");
        assert!(page.items[0].topics.is_empty());
        assert_eq!(page.items[0].citation.as_ref().unwrap().url, "");
        assert!(page.deleted.is_empty());
    }
}
