use std::fs;

use stylos_export::{import, Error};

#[test]
fn imports_markdown_and_text_without_overwriting() {
    let directory = tempfile::tempdir().unwrap();
    let usb = directory.path().join("usb");
    let writing = directory.path().join("writing");
    fs::create_dir_all(&usb).unwrap();
    fs::create_dir_all(&writing).unwrap();
    fs::write(usb.join("essay.md"), "# Essay\n").unwrap();
    fs::write(usb.join("Notes.txt"), "plain notes\n").unwrap();
    fs::write(usb.join("scan.pdf"), b"%PDF-1.7").unwrap();
    fs::write(usb.join("Über Mich.md"), "unicode").unwrap();

    let report = import(
        &[
            usb.join("essay.md"),
            usb.join("Notes.txt"),
            usb.join("scan.pdf"),
            usb.join("Über Mich.md"),
        ],
        &writing,
    )
    .unwrap();

    assert_eq!(report.destination, writing.join("Imported"));
    assert_eq!(report.files.len(), 3);
    assert_eq!(report.skipped, vec![usb.join("scan.pdf")]);
    assert_eq!(
        fs::read_to_string(report.destination.join("essay.md")).unwrap(),
        "# Essay\n"
    );
    // `.txt` imports as Markdown, with the bytes unchanged.
    assert_eq!(
        fs::read_to_string(report.destination.join("Notes.md")).unwrap(),
        "plain notes\n"
    );
    assert_eq!(
        fs::read_to_string(report.destination.join("Über Mich.md")).unwrap(),
        "unicode"
    );

    let second = import(&[usb.join("essay.md")], &writing).unwrap();
    assert_eq!(
        second.files[0].destination,
        report.destination.join("essay (2).md")
    );
    assert_eq!(
        fs::read_to_string(&second.files[0].destination).unwrap(),
        "# Essay\n"
    );
    assert_eq!(
        fs::read_to_string(report.destination.join("essay.md")).unwrap(),
        "# Essay\n"
    );
}

#[test]
fn imports_a_folder_with_its_structure() {
    let directory = tempfile::tempdir().unwrap();
    let usb = directory.path().join("usb");
    let writing = directory.path().join("writing");
    fs::create_dir_all(usb.join("Notes/sub")).unwrap();
    fs::create_dir_all(&writing).unwrap();
    fs::write(usb.join("Notes/top.md"), "top").unwrap();
    fs::write(usb.join("Notes/sub/deep.txt"), "deep").unwrap();
    fs::write(usb.join("Notes/photo.jpg"), b"jpg").unwrap();

    let report = import(&[usb.join("Notes")], &writing).unwrap();

    assert_eq!(report.files.len(), 2);
    assert!(report.destination.join("top.md").is_file());
    assert_eq!(
        fs::read_to_string(report.destination.join("sub/deep.md")).unwrap(),
        "deep"
    );
    assert!(report
        .skipped
        .iter()
        .any(|path| path.ends_with("photo.jpg")));
    assert!(!report.destination.join("photo.jpg").exists());
}

#[test]
fn missing_import_sources_are_an_error() {
    let directory = tempfile::tempdir().unwrap();
    let writing = directory.path().join("writing");
    fs::create_dir_all(&writing).unwrap();

    let error = import(&[directory.path().join("missing.md")], &writing).unwrap_err();
    assert!(matches!(error, Error::NotFound(_)), "{error:?}");
}

#[test]
fn import_into_a_missing_writing_root_is_an_error() {
    let directory = tempfile::tempdir().unwrap();
    let usb = directory.path().join("usb");
    fs::create_dir_all(&usb).unwrap();
    fs::write(usb.join("a.md"), "a").unwrap();

    let error = import(&[usb.join("a.md")], &directory.path().join("absent")).unwrap_err();
    assert!(matches!(error, Error::NotFound(_)), "{error:?}");
}
