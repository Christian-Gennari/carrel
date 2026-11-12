//! End-to-end D-Bus tests: start `stylosd run` on a private bus and call the
//! real `Commit`, `History`, `Restore`, `Pair`, `Sync` and `Unpair` methods
//! from a client.
//!
//! `dbus-daemon --session` acts as the system bus; `STYLOS_DBUS_ADDRESS` makes
//! the daemon connect to it instead of `/run/dbus/system_bus_socket`.

mod support;

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use futures_util::StreamExt;
use stylosd::history::HistoryEntry;

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

fn start_daemon(writing: &Path, state: &Path, rfkill: &Path, address: &str) -> KillOnDrop {
    start_daemon_with(writing, state, rfkill, address, &[])
}

fn start_daemon_with(
    writing: &Path,
    state: &Path,
    rfkill: &Path,
    address: &str,
    extra_env: &[(&str, String)],
) -> KillOnDrop {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stylosd"));
    command
        .arg("run")
        .env("STYLOS_WRITING_DIR", writing)
        .env("STYLOS_STATE_DIR", state)
        .env("STYLOS_DBUS_ADDRESS", address)
        .env("STYLOS_DBUS_ANONYMOUS", "1")
        // No /sys in the test: an empty rfkill directory means "no radio".
        .env("STYLOS_RFKILL_DIR", rfkill)
        .env("RUST_LOG", "stylosd=warn")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let child = command.spawn().expect("start stylosd run");
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
            "org.stylos.Daemon1".try_into().expect("valid bus name");
        if dbus.name_has_owner(name).await.unwrap_or(false) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("stylosd did not take org.stylos.Daemon1 on the private bus");
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

#[tokio::test]
async fn daemon_serves_commit_history_and_restore() {
    let dir = tempfile::tempdir().expect("tempdir");
    let writing = dir.path().join("writing");
    let state = dir.path().join("state");
    let rfkill = dir.path().join("rfkill");
    std::fs::create_dir_all(&writing).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(rfkill.join("rfkill0")).unwrap();
    // A fake Wi-Fi radio. The daemon blocks it at startup and around syncs.
    std::fs::write(rfkill.join("rfkill0/type"), "wlan\n").unwrap();
    std::fs::write(rfkill.join("rfkill0/soft"), "1").unwrap();

    let (_bus, address) = start_bus(dir.path());
    let _daemon = start_daemon(&writing, &state, &rfkill, &address);

    let connection = connect(&address).await;
    wait_for_name(&connection).await;
    let proxy = zbus::Proxy::new(
        &connection,
        "org.stylos.Daemon1",
        "/org/stylos/Daemon1",
        "org.stylos.Daemon1",
    )
    .await
    .expect("daemon proxy");

    // First version.
    std::fs::write(writing.join("note.md"), "first version").unwrap();
    call(&proxy, "Commit", &()).await;

    let reply = call(&proxy, "History", &("note.md",)).await;
    let versions: Vec<HistoryEntry> = reply.body().deserialize().expect("history entries");
    assert_eq!(versions.len(), 1, "{versions:?}");
    assert_eq!(versions[0].words, 2);
    assert_eq!(versions[0].delta, 2);
    let first_rev = versions[0].rev.clone();

    // Second version.
    std::fs::write(writing.join("note.md"), "first version with more words").unwrap();
    call(&proxy, "Commit", &()).await;
    let reply = call(&proxy, "History", &("note.md",)).await;
    let versions: Vec<HistoryEntry> = reply.body().deserialize().expect("history entries");
    assert_eq!(versions.len(), 2, "{versions:?}");
    assert_eq!(versions[0].words, 5);
    assert_eq!(versions[0].delta, 3);
    assert_eq!(versions[1].rev, first_rev);

    // Restore returns the old text and leaves the file and history alone.
    let reply = call(&proxy, "Restore", &("note.md", first_rev.as_str())).await;
    let text: String = reply.body().deserialize().expect("restored text");
    assert_eq!(text, "first version");
    assert_eq!(
        std::fs::read_to_string(writing.join("note.md")).unwrap(),
        "first version with more words"
    );
    let reply = call(&proxy, "History", &("note.md",)).await;
    let versions: Vec<HistoryEntry> = reply.body().deserialize().unwrap();
    assert_eq!(versions.len(), 2, "Restore must not add a version");

    // The stubs answer with a clear "not implemented" D-Bus error.
    let error = proxy
        .call_method(
            "SearchLibrary",
            &("quote", HashMap::<String, String>::new()),
        )
        .await
        .expect_err("SearchLibrary is a stub");
    let text = format!("{error}");
    assert!(text.contains("not implemented"), "{text}");
    assert!(text.contains("WP9"), "{text}");

    // UpdateStatus is a working stub: there is never an update in WP4.
    let reply = call(&proxy, "UpdateStatus", &()).await;
    let status: String = reply.body().deserialize().unwrap();
    assert_eq!(status, "none");

    // A sync with no paired server fails, reports it through SyncStatus, and
    // blocks the radio again on the error path.
    let mut signals = proxy
        .receive_signal("SyncStatus")
        .await
        .expect("subscribe to SyncStatus");
    call(&proxy, "Sync", &()).await;
    let mut last_state = String::new();
    let mut last_message = String::new();
    for _ in 0..3 {
        let signal = tokio::time::timeout(Duration::from_secs(20), signals.next())
            .await
            .expect("SyncStatus signal did not arrive")
            .expect("SyncStatus stream ended");
        let (state, message, last_sync): (String, String, i64) =
            signal.body().deserialize().expect("SyncStatus body");
        assert!(last_sync >= 0);
        // The first signal is "syncing"; wait for the final state.
        last_state = state;
        last_message = message;
        if last_state != "syncing" {
            break;
        }
    }
    assert_eq!(last_state, "error", "{last_message}");
    assert!(last_message.contains("not paired"), "{last_message}");
    let soft = std::fs::read_to_string(rfkill.join("rfkill0/soft")).unwrap();
    assert_eq!(soft, "1", "the radio must be blocked after a failed sync");

    // The SyncInfo property tells the Sync screen the same thing.
    let properties = zbus::Proxy::new(
        &connection,
        "org.stylos.Daemon1",
        "/org/stylos/Daemon1",
        "org.freedesktop.DBus.Properties",
    )
    .await
    .unwrap();
    let reply = properties
        .call_method("Get", &("org.stylos.Daemon1", "SyncInfo"))
        .await
        .expect("read the SyncInfo property");
    let value: zbus::zvariant::OwnedValue = reply.body().deserialize().unwrap();
    let info = zbus::zvariant::Structure::try_from(value).expect("SyncInfo is a struct");
    let fields = info.fields();
    assert_eq!(fields.len(), 3);
    assert_eq!(fields[0], zbus::zvariant::Value::from("error"));
}

/// `Pair("gdrive", "")` starts the device flow and returns the code; the
/// daemon then polls the mocked Google in the background, stores the refresh
/// token, mirrors the writing directory on `Sync()` and revokes on `Unpair()`.
#[tokio::test]
async fn daemon_pairs_with_google_drive_and_syncs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let writing = dir.path().join("writing");
    let state = dir.path().join("state");
    let rfkill = dir.path().join("rfkill");
    std::fs::create_dir_all(&writing).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(rfkill.join("rfkill0")).unwrap();
    std::fs::write(rfkill.join("rfkill0/type"), "wlan\n").unwrap();
    std::fs::write(rfkill.join("rfkill0/soft"), "1").unwrap();

    let mock = support::MockGoogle::start().await;
    let oauth = dir.path().join("oauth.toml");
    std::fs::write(
        &oauth,
        "[google]\nclient_id = \"test-client\"\nclient_secret = \"test-secret\"\n",
    )
    .unwrap();

    let (_bus, address) = start_bus(dir.path());
    let _daemon = start_daemon_with(
        &writing,
        &state,
        &rfkill,
        &address,
        &[
            ("STYLOS_OAUTH_CONFIG", oauth.display().to_string()),
            ("STYLOS_GDRIVE_BASE_URL", mock.base_url().to_string()),
        ],
    );

    let connection = connect(&address).await;
    wait_for_name(&connection).await;
    let proxy = zbus::Proxy::new(
        &connection,
        "org.stylos.Daemon1",
        "/org/stylos/Daemon1",
        "org.stylos.Daemon1",
    )
    .await
    .expect("daemon proxy");

    let mut pair_signals = proxy
        .receive_signal("PairStatus")
        .await
        .expect("subscribe to PairStatus");
    let reply = call(&proxy, "Pair", &("gdrive", "")).await;
    let info: stylosd::PairingInfo = reply.body().deserialize().expect("pairing info");
    assert_eq!(info.backend, "gdrive");
    assert_eq!(info.code, "TEST-CODE");
    assert!(
        info.approval_url.starts_with("https://"),
        "{:?}",
        info.approval_url
    );
    assert!(info.expires_at > 0);

    let signal = tokio::time::timeout(Duration::from_secs(30), pair_signals.next())
        .await
        .expect("PairStatus signal did not arrive")
        .expect("PairStatus stream ended");
    let (pair_state, code, message): (String, String, String) =
        signal.body().deserialize().expect("PairStatus body");
    assert_eq!(pair_state, "approved", "{message}");
    assert_eq!(code, "TEST-CODE");
    assert_eq!(
        std::fs::read_to_string(rfkill.join("rfkill0/soft")).unwrap(),
        "1",
        "the radio must be blocked again after pairing"
    );
    let stored = std::fs::read_to_string(state.join("sync.toml")).expect("sync.toml was written");
    assert!(stored.contains("refresh-1"), "{stored}");

    std::fs::write(writing.join("note.md"), "hello drive").unwrap();
    let mut sync_signals = proxy
        .receive_signal("SyncStatus")
        .await
        .expect("subscribe to SyncStatus");
    call(&proxy, "Sync", &()).await;
    let mut last_state = String::new();
    let mut last_message = String::new();
    for _ in 0..3 {
        let signal = tokio::time::timeout(Duration::from_secs(30), sync_signals.next())
            .await
            .expect("SyncStatus signal did not arrive")
            .expect("SyncStatus stream ended");
        let (state, message, last_sync): (String, String, i64) =
            signal.body().deserialize().expect("SyncStatus body");
        assert!(last_sync >= 0);
        last_state = state;
        last_message = message;
        if last_state != "syncing" {
            break;
        }
    }
    assert_eq!(last_state, "idle", "{last_message}");
    assert!(last_message.contains("Google Drive"), "{last_message}");
    assert_eq!(mock.refresh_calls(), 1, "sync refreshes the access token");
    assert_eq!(mock.content("note.md").as_deref(), Some("hello drive"));
    assert_eq!(
        std::fs::read_to_string(rfkill.join("rfkill0/soft")).unwrap(),
        "1",
        "the radio must be blocked again after the sync"
    );

    call(&proxy, "Unpair", &()).await;
    assert_eq!(mock.revoke_calls(), 1, "Unpair revokes the Google token");
    assert!(!state.join("sync.toml").exists());
    assert_eq!(
        std::fs::read_to_string(rfkill.join("rfkill0/soft")).unwrap(),
        "1",
        "the radio must be blocked again after Unpair"
    );
}
