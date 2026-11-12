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
    }
}
