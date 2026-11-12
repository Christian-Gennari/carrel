//! End-to-end test: the shell's zbus client talks to a real `stylosd` on a
//! private bus.
//!
//! The helper test at the bottom of this file is the daemon process. The main
//! test re-executes this test binary with `STYLOS_TEST_DAEMON_HELPER=1` so the
//! child runs only that test, which calls the daemon's `run()` and stays
//! alive; this is how a test can start the real service without a second
//! binary. `dbus-daemon --session` acts as the system bus, exactly as in
//! `stylosd/tests/daemon_dbus.rs`.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use stylos_shell::daemon::{DaemonClient, ZbusDaemon};

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

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
    let exe = std::env::current_exe().expect("the test binary");
    let child = Command::new(exe)
        .args(["--exact", "daemon_helper", "--nocapture"])
        .env("STYLOS_TEST_DAEMON_HELPER", "1")
        .env("STYLOS_WRITING_DIR", writing)
        .env("STYLOS_STATE_DIR", state)
        .env("STYLOS_DBUS_ADDRESS", address)
        .env("STYLOS_DBUS_ANONYMOUS", "1")
        // No /sys in the test: an empty rfkill directory means "no radio".
        .env("STYLOS_RFKILL_DIR", rfkill)
        .env("RUST_LOG", "stylosd=warn")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start the helper test as stylosd");
    KillOnDrop(child)
}

#[test]
fn the_shell_client_commits_and_reads_history_through_the_real_daemon() {
    let dir = tempfile::tempdir().expect("tempdir");
    let writing = dir.path().join("writing");
    let state = dir.path().join("state");
    let rfkill = dir.path().join("rfkill");
    std::fs::create_dir_all(&writing).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(rfkill.join("rfkill0")).unwrap();
    std::fs::write(rfkill.join("rfkill0/type"), "wlan\n").unwrap();
    std::fs::write(rfkill.join("rfkill0/soft"), "1").unwrap();

    let (_bus, address) = start_bus(dir.path());
    let _daemon = start_daemon(&writing, &state, &rfkill, &address);

    std::fs::write(writing.join("note.md"), "first version").unwrap();

    let daemon = ZbusDaemon::with_address(&address, true);
    // The daemon takes the bus name asynchronously; retry the first commit.
    let mut committed = false;
    for _ in 0..200 {
        if daemon.commit().is_ok() {
            committed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(committed, "stylosd never accepted Commit on the private bus");

    let versions = daemon.history("note.md").expect("History over D-Bus");
    assert_eq!(versions.len(), 1, "{versions:?}");
    assert_eq!(versions[0].words, 2);
    assert_eq!(versions[0].delta, 2);

    std::fs::write(writing.join("note.md"), "first version with more words").unwrap();
    daemon.commit().unwrap();
    let versions = daemon.history("note.md").expect("History over D-Bus");
    assert_eq!(versions.len(), 2, "{versions:?}");
    assert!(versions[0].words > versions[1].words);

    // The read-only Restore preview and the SyncInfo property also come back
    // through the shell's client with the daemon's exact shapes.
    let old = daemon
        .restore("note.md", &versions[1].rev)
        .expect("Restore preview over D-Bus");
    assert_eq!(old, "first version");
    assert_eq!(
        std::fs::read_to_string(writing.join("note.md")).unwrap(),
        "first version with more words",
        "Restore must stay read-only"
    );
    let info = daemon.sync_info().expect("SyncInfo property");
    assert_eq!(info.state, "idle");
    assert_eq!(info.last_sync, 0);
}

/// Runs only in the child process started by the test above.
#[test]
fn daemon_helper() {
    if std::env::var_os("STYLOS_TEST_DAEMON_HELPER").is_none() {
        return;
    }
    let writing = std::env::var("STYLOS_WRITING_DIR").expect("STYLOS_WRITING_DIR");
    let state = std::env::var("STYLOS_STATE_DIR").expect("STYLOS_STATE_DIR");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime
        .block_on(async {
            stylosd::dbus::run(stylosd::dbus::RunConfig {
                writing_dir: writing.into(),
                state_dir: state.into(),
            })
            .await
        })
        .expect("run stylosd");
}
