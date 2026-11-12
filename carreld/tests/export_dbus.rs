//! End-to-end export and import over D-Bus with the fake drive.
//!
//! Starts `carreld run` on a private bus with `CARREL_FAKE_DRIVES_DIR`, then
//! calls the real `ListDrives`, `Export` and `Import` methods: everything as
//! Markdown, everything as PDF, and a `.txt` file imported from the drive.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use futures_util::StreamExt;
use carreld::{ExportProgress, UsbDrive};

/// Kills a child process when the test ends, even on panic.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A minimal bus configuration. `dbus-daemon --session` needs a passwd entry
/// for the current uid, which a container does not have; this configuration
/// only needs the socket and EXTERNAL auth.
const BUS_CONFIG: &str = r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
 <busconfig>
   <type>session</type>
   <listen>unix:tmpdir=/tmp</listen>
   <auth>ANONYMOUS</auth>
   <allow_anonymous/>
   <policy context="default">
     <allow send_destination="*" eavesdrop="true"/>
     <allow eavesdrop="true"/>
     <allow own="*"/>
   </policy>
 </busconfig>
"#;

fn start_bus(dir: &Path) -> (KillOnDrop, String) {
    let config = dir.join("bus.conf");
    std::fs::write(&config, BUS_CONFIG).expect("write the private bus configuration");
    let mut child = Command::new("dbus-daemon")
        .arg(format!("--config-file={}", config.display()))
        .args(["--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn dbus-daemon (the dev container installs the dbus package)");
    let stdout = child.stdout.take().expect("dbus-daemon stdout");
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .expect("read the private bus address");
    let address = line.trim().to_string();
    assert!(
        address.starts_with("unix:"),
        "dbus-daemon printed an unexpected address {address:?}"
    );
    (KillOnDrop(child), address)
}

fn start_daemon(
    writing: &Path,
    state: &Path,
    rfkill: &Path,
    drives: &Path,
    address: &str,
) -> KillOnDrop {
    let child = Command::new(env!("CARGO_BIN_EXE_carreld"))
        .arg("run")
        .env("CARREL_WRITING_DIR", writing)
        .env("CARREL_STATE_DIR", state)
        .env("CARREL_DBUS_ADDRESS", address)
        .env("CARREL_DBUS_ANONYMOUS", "1")
        // No /sys in the test: an empty rfkill directory means "no radio".
        .env("CARREL_RFKILL_DIR", rfkill)
        // The one fake drive mounts at `<drives>/drive-0`.
        .env("CARREL_FAKE_DRIVES_DIR", drives)
        .env("RUST_LOG", "carreld=warn")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start carreld run");
    KillOnDrop(child)
}

async fn connect(address: &str) -> zbus::Connection {
    zbus::connection::Builder::address(address)
        .expect("parse bus address")
        .auth_mechanism(zbus::connection::AuthMechanism::Anonymous)
        .build()
        .await
        .expect("connect to the private bus")
}

async fn wait_for_name(connection: &zbus::Connection) {
    let dbus = zbus::fdo::DBusProxy::new(connection)
        .await
        .expect("DBus proxy");
    for _ in 0..200 {
        let name: zbus::names::BusName<'_> =
            "page.nostos.Carrel.Daemon1".try_into().expect("valid bus name");
        if dbus.name_has_owner(name).await.unwrap_or(false) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("carreld did not take page.nostos.Carrel.Daemon1 on the private bus");
}

async fn call<B>(proxy: &zbus::Proxy<'_>, method: &str, body: &B) -> zbus::Message
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    proxy
        .call_method(method, body)
        .await
        .unwrap_or_else(|error| panic!("{method} failed: {error}"))
}

/// Wait for the final `ExportProgress` of one export.
async fn wait_for_done(signals: &mut zbus::proxy::SignalStream<'_>) -> ExportProgress {
    loop {
        let signal = tokio::time::timeout(Duration::from_secs(120), signals.next())
            .await
            .expect("ExportProgress signal did not arrive")
            .expect("ExportProgress stream ended");
        let progress: ExportProgress = signal.body().deserialize().expect("ExportProgress body");
        if progress.state != "exporting" {
            return progress;
        }
    }
}

fn drive_mount(drives: &Path) -> PathBuf {
    drives.join("drive-0")
}

#[tokio::test]
async fn daemon_lists_drives_exports_markdown_and_pdf_and_imports_a_text_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let writing = dir.path().join("writing");
    let state = dir.path().join("state");
    let rfkill = dir.path().join("rfkill");
    let drives = dir.path().join("drives");
    std::fs::create_dir_all(&writing).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&rfkill).unwrap();
    std::fs::create_dir_all(&drives).unwrap();

    let (_bus, address) = start_bus(dir.path());
    let _daemon = start_daemon(&writing, &state, &rfkill, &drives, &address);

    let connection = connect(&address).await;
    wait_for_name(&connection).await;
    let proxy = zbus::Proxy::new(
        &connection,
        "page.nostos.Carrel.Daemon1",
        "/page/nostos/Carrel/Daemon1",
        "page.nostos.Carrel.Daemon1",
    )
    .await
    .expect("daemon proxy");

    // A fresh daemon lists the fake stick and has not mounted it.
    let reply = call(&proxy, "ListDrives", &()).await;
    let listed: Vec<UsbDrive> = reply.body().deserialize().expect("drive list");
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].name, "Fake USB drive");
    assert!(!listed[0].mounted, "{listed:?}");
    let drive_id = listed[0].id.clone();

    std::fs::write(
        writing.join("Chapter one.md"),
        "# One\n\nHello from Carrel.\n",
    )
    .unwrap();
    std::fs::create_dir_all(writing.join("Notes")).unwrap();
    std::fs::write(writing.join("Notes/second.md"), "Second note.\n").unwrap();

    let mut signals = proxy
        .receive_signal("ExportProgress")
        .await
        .expect("subscribe to ExportProgress");

    // Everything as Markdown, copied byte for byte into the dated folder.
    let reply = call(
        &proxy,
        "Export",
        &(Vec::<String>::new(), "markdown", drive_id.as_str()),
    )
    .await;
    let markdown_destination: String = reply.body().deserialize().expect("export path");
    let markdown_destination = PathBuf::from(markdown_destination);
    assert!(
        markdown_destination.starts_with(drive_mount(&drives)),
        "{markdown_destination:?}"
    );
    assert_eq!(
        std::fs::read_to_string(markdown_destination.join("Chapter one.md")).unwrap(),
        "# One\n\nHello from Carrel.\n"
    );
    assert_eq!(
        std::fs::read_to_string(markdown_destination.join("Notes/second.md")).unwrap(),
        "Second note.\n"
    );
    let done = wait_for_done(&mut signals).await;
    assert_eq!(done.state, "done", "{done:?}");
    assert_eq!(done.total, 2, "{done:?}");
    assert!(done.message.contains("Safe to remove"), "{done:?}");
    assert_eq!(done.destination, markdown_destination.display().to_string());

    // Everything as PDF through pandoc + typst.
    let reply = call(
        &proxy,
        "Export",
        &(Vec::<String>::new(), "pdf", drive_id.as_str()),
    )
    .await;
    let pdf_destination: String = reply.body().deserialize().expect("export path");
    let pdf_destination = PathBuf::from(pdf_destination);
    assert!(
        pdf_destination.starts_with(drive_mount(&drives)),
        "{pdf_destination:?}"
    );
    assert_ne!(pdf_destination, markdown_destination);
    let pdf = std::fs::read(pdf_destination.join("Chapter one.pdf")).expect("a PDF file");
    assert!(
        pdf.starts_with(b"%PDF"),
        "not a PDF: {:?}",
        &pdf[..8.min(pdf.len())]
    );
    assert!(pdf.len() > 1000, "suspiciously small PDF: {}", pdf.len());
    let done = wait_for_done(&mut signals).await;
    assert_eq!(done.state, "done", "{done:?}");
    assert!(done.message.contains("Safe to remove"), "{done:?}");

    // The drive was unmounted and powered off after both exports.
    let reply = call(&proxy, "ListDrives", &()).await;
    let listed: Vec<UsbDrive> = reply.body().deserialize().expect("drive list");
    assert!(!listed[0].mounted, "{listed:?}");

    // Import a .txt file from the drive: it becomes Imported/letter.md.
    let inbox = drive_mount(&drives).join("Inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::write(inbox.join("letter.txt"), "From the stick.\n").unwrap();
    let reply = call(
        &proxy,
        "Import",
        &(drive_id.as_str(), vec!["Inbox".to_string()]),
    )
    .await;
    let imported: Vec<String> = reply.body().deserialize().expect("imported paths");
    assert_eq!(imported.len(), 1, "{imported:?}");
    let imported = PathBuf::from(&imported[0]);
    assert_eq!(imported.file_name().unwrap(), "letter.md");
    assert_eq!(
        std::fs::read_to_string(&imported).unwrap(),
        "From the stick.\n"
    );
    assert!(writing.join("Imported/letter.md").is_file());

    let reply = call(&proxy, "ListDrives", &()).await;
    let listed: Vec<UsbDrive> = reply.body().deserialize().expect("drive list");
    assert!(!listed[0].mounted, "{listed:?}");
}
