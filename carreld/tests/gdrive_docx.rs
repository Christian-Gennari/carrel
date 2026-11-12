//! The real `.docx` converter from WP6 in the Google Drive mirror, against the
//! local mock of Google's OAuth and Drive APIs. No test talks to Google.

mod support;

use std::fs;
use std::io::Read;
use std::sync::Arc;

use carreld::export::PandocDocxConverter;
use carreld::gdrive::api::DOCX_MIME;
use carreld::gdrive::{config::OAUTH_CONFIG_ENV, DriveSyncRunner, GdriveConfig};
use carreld::GdriveRunner;

fn docx_text(bytes: &[u8]) -> String {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("the .docx copy is a zip");
    let mut document = archive
        .by_name("word/document.xml")
        .expect("word/document.xml is in the .docx copy");
    let mut xml = String::new();
    document
        .read_to_string(&mut xml)
        .expect("read the document XML");
    xml
}

#[tokio::test]
async fn the_mirror_uploads_a_real_docx_copy_when_the_setting_is_on() {
    let dir = tempfile::tempdir().unwrap();
    let writing = dir.path().join("writing");
    let state = dir.path().join("state");
    fs::create_dir_all(writing.join("notes")).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::write(
        writing.join("notes/a.md"),
        "# Hello\n\nA sentence with *emphasis*.\n",
    )
    .unwrap();
    GdriveConfig {
        refresh_token: "refresh-1".into(),
        docx_copy: true,
    }
    .save(&state)
    .unwrap();

    let mock = support::MockGoogle::start().await;
    let oauth = dir.path().join("oauth.toml");
    fs::write(
        &oauth,
        "[google]\nclient_id = \"test-client\"\nclient_secret = \"test-secret\"\n",
    )
    .unwrap();

    // The runner reads its client and endpoints from the environment, the same
    // hooks the daemon uses for the mocked-Drive tests.
    std::env::set_var(OAUTH_CONFIG_ENV, &oauth);
    std::env::set_var("CARREL_GDRIVE_BASE_URL", mock.base_url());
    let runner = DriveSyncRunner::with_converter(Arc::new(PandocDocxConverter));
    let report = runner
        .sync(&writing, &state)
        .await
        .expect("mirror with .docx copies");

    assert_eq!(report.uploaded, 2, "Markdown and .docx: {report:?}");
    assert_eq!(
        mock.content("notes/a.md").as_deref(),
        Some("# Hello\n\nA sentence with *emphasis*.\n")
    );
    let docx = mock
        .file("notes/a.docx")
        .expect("the .docx copy is in Drive");
    assert_eq!(docx.mime_type, DOCX_MIME);
    assert!(
        docx.content.starts_with(b"PK\x03\x04"),
        "not a Word document: {:?}",
        &docx.content[..8.min(docx.content.len())]
    );
    let text = docx_text(&docx.content);
    assert!(text.contains("Hello"), "{text}");
    assert!(text.contains("sentence"), "{text}");

    // Nothing is re-uploaded, including the .docx copy.
    let second = runner.sync(&writing, &state).await.unwrap();
    assert!(second.is_noop(), "{second:?}");

    // Turning the setting off returns to a Markdown-only mirror; the existing
    // .docx copy stays in Drive, untouched.
    GdriveConfig {
        refresh_token: "refresh-1".into(),
        docx_copy: false,
    }
    .save(&state)
    .unwrap();
    let markdown_only = runner.sync(&writing, &state).await.unwrap();
    assert!(markdown_only.is_noop(), "{markdown_only:?}");
    assert_eq!(mock.uploads(), 2, "the two first-sync uploads only");

    std::env::remove_var(OAUTH_CONFIG_ENV);
    std::env::remove_var("CARREL_GDRIVE_BASE_URL");
}
