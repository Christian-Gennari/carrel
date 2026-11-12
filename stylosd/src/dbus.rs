//! The `org.stylos.Daemon1` D-Bus service.
//!
//! The object lives on the system bus at `/org/stylos/Daemon1`. Only the
//! `writer` user and root may talk to it (see
//! `stylosd/dbus/org.stylos.Daemon1.conf`). The service translates D-Bus calls
//! into [`DaemonCore`] operations and [`DaemonEvent`]s into signals; the policy
//! itself lives in [`crate::daemon`] so it is testable without a bus.
//!
//! `SearchLibrary`, `Export`/`Import` and `ApplyUpdate` are deliberately
//! stubs: they belong to WP9, WP6 and WP7. They return a D-Bus
//! `NotSupported` error that says so.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use zbus::fdo;
use zbus::object_server::SignalEmitter;
use zbus::Connection;

use crate::daemon::{
    DaemonCore, DaemonDeps, DaemonEvent, DaemonTasks, EventSink, HttpSyncRunner, PairingInfo,
    SyncInfo,
};
use crate::gdrive::DriveSyncRunner;
use crate::history::HistoryEntry;
use crate::network::{NetworkManager, WifiNetwork};
use crate::power::{BatteryStatus, LogindSession, UPowerPower};
use crate::radio::{RadioManager, SysfsRfkill};

/// Well-known bus name promised by `stylosd.service` (`Type=dbus`).
pub const BUS_NAME: &str = "org.stylos.Daemon1";
/// Object path of the daemon interface.
pub const OBJECT_PATH: &str = "/org/stylos/Daemon1";
/// Interface name.
pub const INTERFACE: &str = "org.stylos.Daemon1";

/// Environment variable that points the daemon at a private test bus.
pub const BUS_ADDRESS_ENV: &str = "STYLOS_DBUS_ADDRESS";
/// Test-only: authenticate with `ANONYMOUS` instead of `EXTERNAL`.
///
/// The dev container runs with a uid that has no passwd entry, so
/// `dbus-daemon` cannot resolve it for `EXTERNAL` authentication. The real
/// system bus only offers `EXTERNAL`, so setting this there simply fails to
/// connect; it cannot weaken a device.
pub const BUS_ANONYMOUS_ENV: &str = "STYLOS_DBUS_ANONYMOUS";

/// Start the daemon until SIGTERM/SIGINT.
pub async fn run(config: RunConfig) -> Result<()> {
    let connection = connect_bus().await?;
    let (sender, mut receiver) = mpsc::unbounded_channel();

    let radio = Arc::new(RadioManager::new(Arc::new(SysfsRfkill::new())));
    let core = Arc::new(DaemonCore::new(DaemonDeps {
        writing_dir: config.writing_dir,
        state_dir: config.state_dir,
        radio: radio.clone(),
        network: Arc::new(NetworkManager::new()),
        power: Arc::new(UPowerPower::new()),
        session: Arc::new(LogindSession::new()),
        runner: Arc::new(HttpSyncRunner),
        gdrive: Some(Arc::new(DriveSyncRunner::new())),
        events: Arc::new(BusSink { sender }),
    })?);

    // Fail closed: whatever the radio did before, start with Wi-Fi blocked.
    if let Err(error) = core.radio().force_block() {
        tracing::error!(%error, "could not block the Wi-Fi radio at startup");
    }

    connection
        .object_server()
        .at(
            OBJECT_PATH,
            Daemon1 {
                core: Arc::clone(&core),
            },
        )
        .await
        .context("export the daemon object")?;
    connection
        .request_name(BUS_NAME)
        .await
        .context("own the org.stylos.Daemon1 bus name")?;

    let emitter: SignalEmitter<'static> = SignalEmitter::new(&connection, OBJECT_PATH)?;
    tokio::spawn(async move {
        while let Some(event) = receiver.recv().await {
            if let Err(error) = emit_event(&emitter, event).await {
                tracing::warn!(%error, "could not emit a D-Bus signal");
            }
        }
    });

    DaemonTasks::new(Arc::clone(&core)).spawn();
    tracing::info!(writing_dir = %core.writing_dir.display(), "stylosd is running");

    wait_for_shutdown().await;
    core.shutdown();
    tracing::info!("stylosd stopped");
    Ok(())
}

/// Where the daemon keeps writing and state. Mirrors the CLI flags.
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub writing_dir: PathBuf,
    pub state_dir: PathBuf,
}

async fn connect_bus() -> Result<Connection> {
    let address = std::env::var(BUS_ADDRESS_ENV)
        .ok()
        .map(|address| address.trim().to_string())
        .filter(|address| !address.is_empty());
    match address {
        Some(address) => {
            let mut builder = zbus::connection::Builder::address(address.as_str())?;
            if std::env::var_os(BUS_ANONYMOUS_ENV).is_some() {
                builder = builder.auth_mechanism(zbus::connection::AuthMechanism::Anonymous);
            }
            builder
                .build()
                .await
                .with_context(|| format!("connect to the bus at {address}"))
        }
        None => Connection::system()
            .await
            .context("connect to the system bus (is dbus.service running?)"),
    }
}

async fn wait_for_shutdown() {
    let mut terminate =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(error) => {
                tracing::warn!(%error, "cannot watch SIGTERM; stopping on SIGINT only");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

/// Signals are sent through an unbounded channel so the core never waits on
/// D-Bus; one task keeps them in order.
struct BusSink {
    sender: mpsc::UnboundedSender<DaemonEvent>,
}

impl EventSink for BusSink {
    fn emit(&self, event: DaemonEvent) {
        let _ = self.sender.send(event);
    }
}

async fn emit_event(emitter: &SignalEmitter<'static>, event: DaemonEvent) -> zbus::Result<()> {
    match event {
        DaemonEvent::BatteryChanged(status) => Daemon1::battery_changed(emitter, status).await,
        DaemonEvent::SyncStatus(info) => Daemon1::sync_status(emitter, info).await,
        DaemonEvent::PairStatus {
            state,
            code,
            message,
        } => Daemon1::pair_status(emitter, state, code, message).await,
    }
}

/// The D-Bus object. Every method is a thin wrapper over [`DaemonCore`].
struct Daemon1 {
    core: Arc<DaemonCore>,
}

#[zbus::interface(name = "org.stylos.Daemon1")]
impl Daemon1 {
    // ----- history ---------------------------------------------------------

    /// Snapshot the writing directory into local history now.
    #[zbus(name = "Commit")]
    async fn commit(&self) -> fdo::Result<()> {
        self.core.commit("on demand").map(|_| ()).map_err(failed)
    }

    /// Versions of one file, newest first.
    #[zbus(name = "History")]
    async fn history(&self, path: String) -> fdo::Result<Vec<HistoryEntry>> {
        self.core.history(&path).map_err(failed)
    }

    /// Old text of `path` at `rev`. Read-only: the caller applies it as a new
    /// edit and history is never rewritten.
    #[zbus(name = "Restore")]
    async fn restore(&self, path: String, rev: String) -> fdo::Result<String> {
        self.core.restore(&path, &rev).map_err(failed)
    }

    // ----- sync ------------------------------------------------------------

    /// Start a sync. Returns at once; progress arrives as `SyncStatus`.
    #[zbus(name = "Sync")]
    async fn sync(&self) -> fdo::Result<()> {
        self.core.trigger_sync().map_err(failed)
    }

    /// The Sync screen's one progress line and last-sync time.
    ///
    /// Returned as `(state, message, last_sync_unix)` because a struct would
    /// need an extra conversion for D-Bus properties.
    #[zbus(property, name = "SyncInfo")]
    fn sync_info(&self) -> (String, String, i64) {
        let info = self.core.sync_info();
        (info.state, info.message, info.last_sync)
    }

    /// Start pairing. Returns the code to show; approval arrives as
    /// `PairStatus`.
    #[zbus(name = "Pair")]
    async fn pair(&self, backend: String, url: String) -> fdo::Result<PairingInfo> {
        self.core.pair(&backend, &url).await.map_err(failed)
    }

    /// Forget every paired backend.
    #[zbus(name = "Unpair")]
    async fn unpair(&self) -> fdo::Result<()> {
        self.core.unpair().await.map_err(failed)
    }

    // ----- Wi-Fi -----------------------------------------------------------

    /// Scan for networks. Opens the two-minute radio window of the Wi-Fi screen.
    #[zbus(name = "WifiScan")]
    async fn wifi_scan(&self) -> fdo::Result<Vec<WifiNetwork>> {
        self.core.wifi_scan().await.map_err(failed)
    }

    /// Connect to a network. A captive portal is an error with the message the
    /// Wi-Fi screen shows.
    #[zbus(name = "WifiConnect")]
    async fn wifi_connect(&self, ssid: String, password: String) -> fdo::Result<()> {
        self.core
            .wifi_connect(&ssid, &password)
            .await
            .map_err(failed)
    }

    /// Forget a saved network.
    #[zbus(name = "WifiForget")]
    async fn wifi_forget(&self, ssid: String) -> fdo::Result<()> {
        self.core.wifi_forget(&ssid).await.map_err(failed)
    }

    // ----- battery and updates --------------------------------------------

    /// Current battery reading.
    #[zbus(name = "Battery")]
    async fn battery(&self) -> fdo::Result<BatteryStatus> {
        self.core.battery().await.map_err(failed)
    }

    /// Update state. WP7 brings sysupdate; until then there is never an update.
    #[zbus(name = "UpdateStatus")]
    async fn update_status(&self) -> fdo::Result<String> {
        Ok("none".to_string())
    }

    // ----- stubs owned by later work packages ------------------------------

    /// TODO(WP9): SQLite FTS5 library cache. Stub on purpose.
    #[zbus(name = "SearchLibrary")]
    async fn search_library(
        &self,
        query: String,
        filters: HashMap<String, String>,
    ) -> fdo::Result<Vec<String>> {
        let _ = (query, filters);
        Err(not_implemented("SearchLibrary", "WP9 (quotes)"))
    }

    /// TODO(WP6): pandoc/typst export pipeline. Stub on purpose.
    #[zbus(name = "Export")]
    async fn export(
        &self,
        selection: Vec<String>,
        format: String,
        drive_id: String,
    ) -> fdo::Result<String> {
        let _ = (selection, format, drive_id);
        Err(not_implemented("Export", "WP6 (export)"))
    }

    /// TODO(WP6): copy files from a USB drive. Stub on purpose.
    #[zbus(name = "Import")]
    async fn import(&self, drive_id: String, paths: Vec<String>) -> fdo::Result<Vec<String>> {
        let _ = (drive_id, paths);
        Err(not_implemented("Import", "WP6 (export)"))
    }

    /// TODO(WP7): signed sysupdate payloads. Stub on purpose.
    #[zbus(name = "ApplyUpdate")]
    async fn apply_update(&self) -> fdo::Result<()> {
        Err(not_implemented("ApplyUpdate", "WP7 (image integration)"))
    }

    // ----- signals ---------------------------------------------------------

    /// The battery reading changed. The shell shows 10 % and 5 % from this.
    #[zbus(signal, name = "BatteryChanged")]
    async fn battery_changed(
        emitter: &SignalEmitter<'_>,
        status: BatteryStatus,
    ) -> zbus::Result<()>;

    /// A sync started, finished or failed.
    #[zbus(signal, name = "SyncStatus")]
    async fn sync_status(emitter: &SignalEmitter<'_>, info: SyncInfo) -> zbus::Result<()>;

    /// A pairing was approved, failed or expired.
    #[zbus(signal, name = "PairStatus")]
    async fn pair_status(
        emitter: &SignalEmitter<'_>,
        state: String,
        code: String,
        message: String,
    ) -> zbus::Result<()>;
}

fn failed(error: anyhow::Error) -> fdo::Error {
    fdo::Error::Failed(format!("{error:#}"))
}

fn not_implemented(method: &str, package: &str) -> fdo::Error {
    fdo::Error::NotSupported(format!(
        "{method} is not implemented yet (arrives in {package})"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stubs_say_not_implemented_and_name_the_work_package() {
        for (method, package) in [
            ("SearchLibrary", "WP9"),
            ("Export", "WP6"),
            ("Import", "WP6"),
            ("ApplyUpdate", "WP7"),
        ] {
            let text = not_implemented(method, package).to_string();
            assert!(text.contains("not implemented"), "{text}");
            assert!(text.contains(method), "{text}");
            assert!(text.contains(package), "{text}");
        }
    }

    #[test]
    fn api_constants_match_the_spec() {
        assert_eq!(BUS_NAME, "org.stylos.Daemon1");
        assert_eq!(OBJECT_PATH, "/org/stylos/Daemon1");
    }

    #[test]
    fn the_service_files_match_the_interface() {
        let policy = include_str!("../dbus/org.stylos.Daemon1.conf");
        // The blanket deny comes first (D-Bus applies the last matching rule)
        // and only root and writer are allowed after it.
        let deny = policy
            .find("<deny send_destination=\"org.stylos.Daemon1\"/>")
            .expect("the policy denies the service by default");
        let writer = policy
            .find("<policy user=\"writer\">")
            .expect("the writer user is allowed");
        assert!(deny < writer, "the blanket deny must come first");
        assert!(policy.contains("<policy user=\"root\">"));
        assert!(policy.contains("<allow send_destination=\"org.stylos.Daemon1\"/>"));

        let unit = include_str!("../systemd/stylosd.service");
        assert!(unit.contains("BusName=org.stylos.Daemon1"));
        assert!(unit.contains("ExecStart=/usr/bin/stylosd run"));
    }
}
