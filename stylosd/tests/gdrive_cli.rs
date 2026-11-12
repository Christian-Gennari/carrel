//! The `stylosd gdrive-pair` / `gdrive-sync` commands from WP6b, end to end
//! against the local mock of Google's OAuth and Drive APIs.

mod support;

use std::path::Path;
use std::process::{Command, Output};

use support::MockGoogle;

/// Run the daemon binary without blocking the mock's runtime.
async fn run_stylosd(args: Vec<String>, base_url: &str) -> Output {
    let base_url = base_url.to_string();
    tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_stylosd"))
            .args(args)
            .env("STYLOS_GDRIVE_BASE_URL", base_url)
            .env("RUST_LOG", "warn")
            .output()
            .expect("run stylosd")
    })
    .await
    .expect("join the stylosd command")
}

fn write_oauth(path: &Path) {
    std::fs::write(
        path,
        "[google]\nclient_id = \"test-client\"\nclient_secret = \"test-secret\"\n",
    )
    .expect("write oauth.toml");
}

#[tokio::test]
async fn gdrive_pair_and_sync_work_against_the_mock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let writing = dir.path().join("writing");
    let state = dir.path().join("state");
    let oauth = dir.path().join("oauth.toml");
    std::fs::create_dir_all(&writing).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    write_oauth(&oauth);
    let mock = MockGoogle::start().await;

    // Pair: prints the URL and code, waits, stores the refresh token.
    let output = run_stylosd(
        vec![
            "gdrive-pair".into(),
            "--state".into(),
            state.display().to_string(),
            "--oauth".into(),
            oauth.display().to_string(),
        ],
        mock.base_url(),
    )
    .await;
    assert!(
        output.status.success(),
        "gdrive-pair failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("TEST-CODE"), "{stdout}");
    assert!(stdout.contains("https://example.test/device"), "{stdout}");
    let config = std::fs::read_to_string(state.join("sync.toml")).expect("sync.toml");
    assert!(config.contains("refresh-1"), "{config}");

    // Sync with --docx: Markdown plus a real Word copy in the mock Drive.
    std::fs::write(writing.join("note.md"), "# Note\n\nHello.\n").unwrap();
    let output = run_stylosd(
        vec![
            "gdrive-sync".into(),
            "--dir".into(),
            writing.display().to_string(),
            "--state".into(),
            state.display().to_string(),
            "--oauth".into(),
            oauth.display().to_string(),
            "--docx".into(),
        ],
        mock.base_url(),
    )
    .await;
    assert!(
        output.status.success(),
        "gdrive-sync failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        mock.content("note.md").as_deref(),
        Some("# Note\n\nHello.\n")
    );
    let docx = mock.file("note.docx").expect("the .docx copy");
    assert!(docx.content.starts_with(b"PK\x03\x04"));
}
