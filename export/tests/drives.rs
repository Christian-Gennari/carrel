use std::fs;

use stylos_export::{export, DriveCall, DriveInfo, Drives, FakeDrives, Format, Selection};

#[test]
fn fake_drive_mounts_only_on_request_and_exports_to_it() {
    let directory = tempfile::tempdir().unwrap();
    let writing = directory.path().join("writing");
    fs::create_dir_all(&writing).unwrap();
    fs::write(writing.join("chapter.md"), "# Chapter\n").unwrap();

    let id = "/org/freedesktop/UDisks2/block_devices/sdb1".to_string();
    let fake = FakeDrives::new(
        directory.path().join("media"),
        vec![DriveInfo {
            id: id.clone(),
            name: "STYLOS USB".into(),
            size: 16_000_000_000,
            mount_points: Vec::new(),
        }],
    );

    // Listing shows the drive but must not mount it.
    let drives = fake.list().unwrap();
    assert_eq!(drives.len(), 1);
    assert_eq!(drives[0].name, "STYLOS USB");
    assert!(!drives[0].is_mounted());
    assert_eq!(fake.calls(), vec![DriveCall::List]);
    assert!(!directory.path().join("media").exists());

    let mount_point = fake.mount(&id).unwrap();
    assert!(mount_point.is_dir());
    assert!(fake.is_mounted(&id));
    assert!(fake.list().unwrap()[0].is_mounted());

    let report = export(
        Selection::Everything,
        Format::Markdown,
        &writing,
        &mount_point,
        &(),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.count(), 1);
    assert!(report.files[0].destination.is_file());

    fake.unmount(&id).unwrap();
    assert!(!fake.is_mounted(&id));
    fake.power_off(&id).unwrap();

    assert_eq!(
        fake.calls(),
        vec![
            DriveCall::List,
            DriveCall::Mount(id.clone()),
            DriveCall::List,
            DriveCall::Unmount(id.clone()),
            DriveCall::PowerOff(id),
        ]
    );
}

#[test]
fn unknown_drives_are_reported() {
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeDrives::new(directory.path().join("media"), Vec::new());
    let error = fake
        .mount("/org/freedesktop/UDisks2/block_devices/nope")
        .unwrap_err();
    assert!(error.to_string().contains("no drive"), "{error}");
}
