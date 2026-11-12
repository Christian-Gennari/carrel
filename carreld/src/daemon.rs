//! The daemon's policy: what happens on a commit, a sync, a pairing, a Wi-Fi
//! call, a battery reading and before suspend.
//!
//! Everything here takes its outside world as a trait, so the whole policy is
//! unit-tested with fakes. [`crate::dbus`] only translates D-Bus calls into
//! these methods and [`DaemonEvent`]s back into signals.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use carrel_export::{Drives, ExportReport};
use carrel_protocol::now_unix;
use url::Url;

use crate::config::{state_database, SyncConfig};
use crate::export::{
    import_sources, list_drives, parse_format, selection_from_request, MountedDrive, UsbDrive,
};
use crate::gdrive::{
    GdriveConfig, GdriveReport, GdriveRunner, OAuthApp, OAuthClient, OAuthEndpoints,
};
use crate::history::{History, HistoryEntry};
use crate::library::{
    clear as clear_library, LibraryCache, LibraryFilterOptions, LibraryHit, UNAVAILABLE_MESSAGE,
};
use crate::network::{Connectivity, Network, WifiNetwork, PORTAL_MESSAGE};
use crate::pairing::{generate_device_pubkey, PairingSession};
use crate::power::{BatteryStatus, Power, Session, LOW_BATTERY_PERCENT};
use crate::radio::{RadioGuard, RadioManager};
use crate::sync::{sync_once, SyncOptions, SyncReport};
use crate::update::Updater;

/// History is committed after this long without a single changed file.
pub const IDLE_COMMIT: Duration = Duration::from_secs(5 * 60);

/// How often the daemon looks at the writing directory and the battery.
pub const TICK: Duration = Duration::from_secs(30);

/// How long a pairing may wait for the user to approve the code.
const PAIRING_TIMEOUT: Duration = Duration::from_secs(600);
const PAIRING_POLL: Duration = Duration::from_secs(2);

/// Default Nostos endpoint from `spec/02-sync-protocol.md`.
pub const NOSTOS_URL: &str = "https://app.nostos.page/carrel/v1";

/// What the Sync screen shows: one state, one line, one time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct SyncInfo {
    /// `"idle"`, `"syncing"` or `"error"`.
    pub state: String,
    pub message: String,
    /// Unix seconds of the last successful sync, 0 before the first one.
    pub last_sync: i64,
}

impl Default for SyncInfo {
    fn default() -> Self {
        Self {
            state: "idle".into(),
            message: "Not synced yet.".into(),
            last_sync: 0,
        }
    }
}

/// The code and approval URL the Sync screen shows while pairing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct PairingInfo {
    pub backend: String,
    pub code: String,
    /// URL the user opens on a phone to approve the code.
    pub approval_url: String,
    /// Unix seconds.
    pub expires_at: i64,
}

/// The Export screen's progress line.
///
/// `state` is `"exporting"`, `"done"` or `"error"`. While a file is being
/// written, `destination` is that file; when the export finished it is the
/// folder the writer may unplug the drive after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct ExportProgress {
    pub state: String,
    pub message: String,
    /// 1-based index of the file just written, 0 before the first one.
    pub index: u32,
    /// Number of files in this export, 0 until the plan is known.
    pub total: u32,
    /// File just written, or the export folder when the export is done.
    pub destination: String,
}

/// Result of one sync across every configured backend, in run order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncSummary {
    /// The Nostos/self-hosted server sync, when one is paired.
    pub server: Option<SyncReport>,
    /// The Google Drive mirror, when one is paired.
    pub gdrive: Option<GdriveReport>,
}

/// Something the daemon wants to tell the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonEvent {
    /// The battery reading changed (including reaching the suspend threshold).
    BatteryChanged(BatteryStatus),
    /// A sync started, finished or failed.
    SyncStatus(SyncInfo),
    /// A pairing was approved, failed or expired.
    PairStatus {
        state: String,
        code: String,
        message: String,
    },
    /// An export started, wrote a file, finished or failed.
    ExportProgress(ExportProgress),
}

/// Where events go. The daemon emits zbus signals; tests record them.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: DaemonEvent);
}

/// Drops every event. Used when a caller does not need signals.
#[derive(Debug, Default)]
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: DaemonEvent) {}
}

/// The one network operation a sync performs.
///
/// Split into a trait so `sync-triggers-commit` can be tested without a server:
/// the fake records that history was committed before it was called.
#[async_trait]
pub trait SyncRunner: Send + Sync {
    async fn sync(&self, writing_dir: &Path, state_dir: &Path) -> Result<SyncReport>;
}

/// The real sync client from WP3.
#[derive(Debug, Default)]
pub struct HttpSyncRunner;

#[async_trait]
impl SyncRunner for HttpSyncRunner {
    async fn sync(&self, writing_dir: &Path, state_dir: &Path) -> Result<SyncReport> {
        sync_once(writing_dir, state_dir, &SyncOptions::default()).await
    }
}

/// Everything [`DaemonCore`] needs from the outside world.
pub struct DaemonDeps {
    pub writing_dir: PathBuf,
    pub state_dir: PathBuf,
    pub radio: Arc<RadioManager>,
    pub network: Arc<dyn Network>,
    pub power: Arc<dyn Power>,
    pub session: Arc<dyn Session>,
    pub runner: Arc<dyn SyncRunner>,
    /// Removable drives for the Export screen (udisks2, or the test fake).
    pub drives: Arc<dyn Drives>,
    /// The Google Drive backend, when this build has one. `None` in tests
    /// that only exercise the server backend.
    pub gdrive: Option<Arc<dyn GdriveRunner>>,
    pub events: Arc<dyn EventSink>,
}

#[derive(Default)]
struct CoreStatus {
    battery: Option<BatteryStatus>,
    /// True after the 3 % action, until the charger returns or the level rises.
    low_battery_handled: bool,
    sync: SyncInfo,
    sync_running: bool,
    pairing_running: bool,
    /// A USB export or import is in progress; mounting is one at a time.
    drive_operation_running: bool,
}

/// The daemon's policy, independent of D-Bus and the kernel.
pub struct DaemonCore {
    pub writing_dir: PathBuf,
    pub state_dir: PathBuf,
    radio: Arc<RadioManager>,
    network: Arc<dyn Network>,
    power: Arc<dyn Power>,
    session: Arc<dyn Session>,
    runner: Arc<dyn SyncRunner>,
    drives: Arc<dyn Drives>,
    gdrive: Option<Arc<dyn GdriveRunner>>,
    events: Arc<dyn EventSink>,
    history: Mutex<History>,
    update: Updater,
    status: Mutex<CoreStatus>,
}

impl DaemonCore {
    pub fn new(deps: DaemonDeps) -> Result<Self> {
        fs::create_dir_all(&deps.state_dir)
            .with_context(|| format!("create state directory {}", deps.state_dir.display()))?;
        let history = History::open(&deps.writing_dir)?;
        let update = Updater::new(&deps.state_dir);
        Ok(Self {
            writing_dir: deps.writing_dir,
            state_dir: deps.state_dir,
            radio: deps.radio,
            network: deps.network,
            power: deps.power,
            session: deps.session,
            runner: deps.runner,
            drives: deps.drives,
            gdrive: deps.gdrive,
            events: deps.events,
            history: Mutex::new(history),
            update,
            status: Mutex::new(CoreStatus::default()),
        })
    }

    /// The radio manager, for the daemon's startup/shutdown blocking.
    pub fn radio(&self) -> &Arc<RadioManager> {
        &self.radio
    }

    /// The session used for sleep handling.
    pub fn session(&self) -> &Arc<dyn Session> {
        &self.session
    }

    fn emit(&self, event: DaemonEvent) {
        self.events.emit(event);
    }

    // ----- history ---------------------------------------------------------

    /// Commit the writing directory to local history.
    pub fn commit(&self, reason: &str) -> Result<Option<String>> {
        let history = self.history.lock().expect("history lock poisoned");
        history
            .commit(reason)
            .with_context(|| format!("commit history ({reason})"))
    }

    /// Commit only when the writing directory has been quiet for `idle`.
    pub fn commit_if_idle(&self, now: SystemTime, idle: Duration) -> Result<Option<String>> {
        let history = self.history.lock().expect("history lock poisoned");
        history
            .commit_if_idle(now, idle, "idle")
            .context("idle history commit")
    }

    /// Versions of one file, newest first.
    pub fn history(&self, path: &str) -> Result<Vec<HistoryEntry>> {
        let history = self.history.lock().expect("history lock poisoned");
        history.history(path)
    }

    /// Old text of `path` at `rev`; reads only, never changes history or the file.
    pub fn restore(&self, path: &str, rev: &str) -> Result<String> {
        let history = self.history.lock().expect("history lock poisoned");
        history.restore(path, rev)
    }

    /// Current history commit, for the Sync screen and tests.
    pub fn history_head(&self) -> Result<Option<String>> {
        let history = self.history.lock().expect("history lock poisoned");
        history.head()
    }

    /// Commit with the shutdown reason and block the radio.
    pub fn shutdown(&self) {
        match self.commit("shutdown") {
            Ok(Some(rev)) => tracing::info!(rev, "committed history at shutdown"),
            Ok(None) => tracing::debug!("nothing to commit at shutdown"),
            Err(error) => tracing::error!(%error, "history commit at shutdown failed"),
        }
        if let Err(error) = self.radio.force_block() {
            tracing::error!(%error, "could not block the Wi-Fi radio at shutdown");
        }
    }

    // ----- sync ------------------------------------------------------------

    pub fn sync_info(&self) -> SyncInfo {
        self.status
            .lock()
            .expect("status lock poisoned")
            .sync
            .clone()
    }

    /// Start a sync in the background. The result arrives as a `SyncStatus`
    /// event, so the Sync screen shows progress without holding a D-Bus call
    /// open for minutes.
    pub fn trigger_sync(self: &Arc<Self>) -> Result<()> {
        let info = {
            let mut status = self.status.lock().expect("status lock poisoned");
            if status.sync_running {
                bail!("a sync is already running");
            }
            status.sync_running = true;
            status.sync = SyncInfo {
                state: "syncing".into(),
                message: "Syncing…".into(),
                last_sync: status.sync.last_sync,
            };
            status.sync.clone()
        };
        self.emit(DaemonEvent::SyncStatus(info));

        let core = Arc::clone(self);
        tokio::spawn(async move {
            let result = core.run_sync_now().await;
            core.finish_sync(result);
        });
        Ok(())
    }

    fn finish_sync(&self, result: Result<SyncSummary>) {
        let info = {
            let mut status = self.status.lock().expect("status lock poisoned");
            status.sync_running = false;
            status.sync = match result {
                Ok(summary) => SyncInfo {
                    state: "idle".into(),
                    message: describe_summary(&summary),
                    last_sync: now_unix(),
                },
                Err(error) => {
                    tracing::warn!(%error, "sync failed");
                    SyncInfo {
                        state: "error".into(),
                        message: format!("{error:#}"),
                        last_sync: status.sync.last_sync,
                    }
                }
            };
            status.sync.clone()
        };
        self.emit(DaemonEvent::SyncStatus(info));
    }

    /// One sync, end to end: commit, radio on, every configured backend in
    /// order, radio off (also on error).
    pub async fn run_sync_now(&self) -> Result<SyncSummary> {
        self.commit("before sync")?;
        // The guard blocks the radio again when it drops, after both success and
        // failure. A panic unwinding through this function drops it as well.
        let _guard: RadioGuard = self.radio.hold("sync")?;

        let server_configured = SyncConfig::is_configured(&self.state_dir)?;
        let gdrive_configured = match &self.gdrive {
            Some(_) => GdriveConfig::is_configured(&self.state_dir)?,
            None => false,
        };
        if self.gdrive.is_some() && !server_configured && !gdrive_configured {
            bail!("this device is not paired with a sync server or Google Drive");
        }

        let mut summary = SyncSummary::default();
        // Order is part of the spec: server first, then Google Drive. An error
        // in either ends the sync before the next backend starts.
        if server_configured || self.gdrive.is_none() {
            summary.server = Some(self.runner.sync(&self.writing_dir, &self.state_dir).await?);
        }
        if gdrive_configured {
            let runner = self
                .gdrive
                .as_ref()
                .expect("gdrive is configured only when the backend exists");
            summary.gdrive = Some(
                runner
                    .sync(&self.writing_dir, &self.state_dir)
                    .await
                    .context("Google Drive mirror")?,
            );
        }
        match (&summary.server, &summary.gdrive) {
            (Some(report), None) => tracing::info!(
                pulled = report.pulled,
                pushed = report.pushed,
                conflicts = report.conflicts,
                "sync finished"
            ),
            (None, Some(report)) => tracing::info!(
                uploaded = report.uploaded,
                deleted = report.deleted,
                "Google Drive mirror finished"
            ),
            (Some(server), Some(gdrive)) => tracing::info!(
                pulled = server.pulled,
                pushed = server.pushed,
                uploaded = gdrive.uploaded,
                deleted = gdrive.deleted,
                "sync and Google Drive mirror finished"
            ),
            (None, None) => tracing::info!("sync finished with no backends"),
        }
        // The radio guard is still alive here: updates are checked and
        // downloaded only inside a sync session's window (rule 2).
        self.check_updates().await;
        Ok(summary)
    }

    // ----- library (Quotes palette) ---------------------------------------

    /// Ranked library hits for the Quotes palette.
    ///
    /// A device with no server pairing, or a self-hosted server that answered
    /// `404` for `/library`, gets one calm sentence instead of an empty list:
    /// the palette shows it as the reason there are no quotes.
    pub fn search_library(
        &self,
        query: &str,
        filters: &HashMap<String, String>,
    ) -> Result<Vec<LibraryHit>> {
        let cache = self.open_library()?;
        cache.search(query, filters)
    }

    /// Books, authors and topics for the palette's filter chips.
    pub fn library_filters(&self) -> Result<LibraryFilterOptions> {
        let cache = self.open_library()?;
        cache.filter_options()
    }

    fn open_library(&self) -> Result<LibraryCache> {
        if !SyncConfig::is_configured(&self.state_dir)? {
            bail!("{UNAVAILABLE_MESSAGE}");
        }
        let cache = LibraryCache::from_state_dir(&self.state_dir)?;
        if cache.is_unavailable()? {
            bail!("{UNAVAILABLE_MESSAGE}");
        }
        Ok(cache)
    }

    // ----- export and import ----------------------------------------------

    /// Removable drives attached to this device, for the Export screen.
    pub fn list_drives(&self) -> Result<Vec<UsbDrive>> {
        list_drives(self.drives.as_ref())
    }

    /// Export one selection to `drive_id`, then unmount and power the drive off.
    ///
    /// The filesystem and pandoc work runs on a blocking thread; progress
    /// arrives through [`DaemonEvent::ExportProgress`]. The returned path is the
    /// folder the writer may unplug the drive after ("Safe to remove").
    pub async fn export(
        self: &Arc<Self>,
        selection: Vec<String>,
        format: String,
        drive_id: String,
    ) -> Result<String> {
        self.begin_drive_operation()?;
        self.emit(DaemonEvent::ExportProgress(ExportProgress {
            state: "exporting".into(),
            message: "Exporting…".into(),
            index: 0,
            total: 0,
            destination: String::new(),
        }));
        let core = Arc::clone(self);
        let outcome = tokio::task::spawn_blocking(move || {
            core.export_blocking(&selection, &format, &drive_id)
        })
        .await;
        let result = match outcome {
            Ok(result) => result,
            Err(error) => Err(anyhow::anyhow!("the export task stopped: {error}")),
        };
        self.finish_export(&result);
        self.end_drive_operation();
        result.map(|report| report.destination.display().to_string())
    }

    fn export_blocking(
        &self,
        selection: &[String],
        format: &str,
        drive_id: &str,
    ) -> Result<ExportReport> {
        let selection = selection_from_request(selection, &self.writing_dir)?;
        let format = parse_format(format)?;
        let drive = MountedDrive::mount(Arc::clone(&self.drives), drive_id)?;
        let report = carrel_export::export(
            selection,
            format,
            &self.writing_dir,
            drive.path(),
            &carrel_export::MapCitations::new(),
            |progress| {
                self.emit(DaemonEvent::ExportProgress(ExportProgress {
                    state: "exporting".into(),
                    message: format!("Writing file {} of {}.", progress.index, progress.total),
                    index: progress.index as u32,
                    total: progress.total as u32,
                    destination: progress.destination.display().to_string(),
                }));
            },
        )
        .context("export to the USB drive")?;
        Ok(report)
    }

    fn finish_export(&self, result: &Result<ExportReport>) {
        let progress = match result {
            Ok(report) => ExportProgress {
                state: "done".into(),
                message: format!("Exported {} file(s). Safe to remove.", report.count()),
                index: report.count() as u32,
                total: report.count() as u32,
                destination: report.destination.display().to_string(),
            },
            Err(error) => ExportProgress {
                state: "error".into(),
                message: format!("{error:#}"),
                index: 0,
                total: 0,
                destination: String::new(),
            },
        };
        self.emit(DaemonEvent::ExportProgress(progress));
    }

    /// Copy files from `drive_id` into `<writing>/Imported/`.
    ///
    /// No paths means everything on the drive. Like an export, the drive is
    /// mounted only for this call and powered off afterwards.
    pub async fn import(
        self: &Arc<Self>,
        drive_id: String,
        paths: Vec<String>,
    ) -> Result<Vec<String>> {
        self.begin_drive_operation()?;
        let core = Arc::clone(self);
        let outcome =
            tokio::task::spawn_blocking(move || core.import_blocking(&drive_id, &paths)).await;
        self.end_drive_operation();
        match outcome {
            Ok(result) => result,
            Err(error) => Err(anyhow::anyhow!("the import task stopped: {error}")),
        }
    }

    fn import_blocking(&self, drive_id: &str, paths: &[String]) -> Result<Vec<String>> {
        let drive = MountedDrive::mount(Arc::clone(&self.drives), drive_id)?;
        let sources = import_sources(drive.path(), paths)?;
        let report = carrel_export::import(&sources, &self.writing_dir)
            .context("import from the USB drive")?;
        Ok(report
            .files
            .into_iter()
            .map(|file| file.destination.display().to_string())
            .collect())
    }

    fn begin_drive_operation(&self) -> Result<()> {
        let mut status = self.status.lock().expect("status lock poisoned");
        if status.drive_operation_running {
            bail!("an export or import is already running");
        }
        status.drive_operation_running = true;
        Ok(())
    }

    fn end_drive_operation(&self) {
        self.status
            .lock()
            .expect("status lock poisoned")
            .drive_operation_running = false;
    }

    // ----- pairing ---------------------------------------------------------

    /// Start pairing and return the code at once. Approval is reported later
    /// through [`DaemonEvent::PairStatus`]; the radio stays on until then.
    pub async fn pair(self: &Arc<Self>, backend: &str, url: &str) -> Result<PairingInfo> {
        match backend {
            "gdrive" | "google-drive" => return self.pair_gdrive().await,
            "nostos" | "self-hosted" => {}
            other => bail!("unknown sync backend {other:?}"),
        }
        let url = resolve_server_url(backend, url)?;
        {
            let mut status = self.status.lock().expect("status lock poisoned");
            if status.pairing_running {
                bail!("a pairing is already in progress");
            }
            status.pairing_running = true;
        }

        let guard = match self.radio.hold("pairing") {
            Ok(guard) => guard,
            Err(error) => {
                self.finish_pairing_attempt();
                return Err(error);
            }
        };
        let session = match start_session(&url).await {
            Ok(session) => session,
            Err(error) => {
                drop(guard);
                self.finish_pairing_attempt();
                return Err(error);
            }
        };

        let info = PairingInfo {
            backend: backend.to_string(),
            code: session.code().to_string(),
            approval_url: session.approval_url(),
            expires_at: session.expires_at(),
        };

        let core = Arc::clone(self);
        let code = info.code.clone();
        tokio::spawn(async move {
            // Keep the radio on for the whole pairing; the guard blocks it
            // again on every exit path.
            let _guard = guard;
            let outcome: Result<String> = match session.wait(PAIRING_POLL, PAIRING_TIMEOUT).await {
                Ok(credential) => {
                    let config = credential.to_sync_config();
                    let label = config
                        .server_name
                        .clone()
                        .unwrap_or_else(|| config.server_url.clone());
                    config.save(&core.state_dir).map(|()| {
                        // The cache belongs to the previous account; never show
                        // one writer's quotes under another server.
                        clear_library(&core.state_dir);
                        label
                    })
                }
                Err(error) => Err(error),
            };
            core.finish_pairing_attempt();
            match outcome {
                Ok(label) => core.emit(DaemonEvent::PairStatus {
                    state: "approved".into(),
                    code,
                    message: format!("Paired with {label}."),
                }),
                Err(error) => core.emit(DaemonEvent::PairStatus {
                    state: "error".into(),
                    code,
                    message: format!("{error:#}"),
                }),
            }
        });
        Ok(info)
    }

    /// Start the Google device authorization flow: return the URL and user
    /// code, then poll in the background inside the same radio window.
    async fn pair_gdrive(self: &Arc<Self>) -> Result<PairingInfo> {
        {
            let mut status = self.status.lock().expect("status lock poisoned");
            if status.pairing_running {
                bail!("a pairing is already in progress");
            }
            status.pairing_running = true;
        }
        // Fail before touching the radio when the image has no OAuth client.
        let client = match OAuthApp::load()
            .and_then(|app| OAuthClient::new(app, OAuthEndpoints::from_env_or_default()))
        {
            Ok(client) => client,
            Err(error) => {
                self.finish_pairing_attempt();
                return Err(error);
            }
        };
        let guard = match self.radio.hold("pairing") {
            Ok(guard) => guard,
            Err(error) => {
                self.finish_pairing_attempt();
                return Err(error);
            }
        };
        let device = match client.start_device_flow().await {
            Ok(device) => device,
            Err(error) => {
                drop(guard);
                self.finish_pairing_attempt();
                return Err(error);
            }
        };

        let info = PairingInfo {
            backend: "gdrive".to_string(),
            code: device.user_code.clone(),
            approval_url: device.verification_url.clone(),
            expires_at: now_unix() + device.expires_in.max(0),
        };
        let core = Arc::clone(self);
        let code = info.code.clone();
        tokio::spawn(async move {
            // Keep the radio on for the whole pairing; the guard blocks it
            // again on every exit path.
            let _guard = guard;
            let outcome: Result<()> = async {
                let tokens = client.wait_for_approval(&device, PAIRING_TIMEOUT).await?;
                let refresh_token = tokens
                    .refresh_token
                    .filter(|token| !token.trim().is_empty())
                    .context("Google did not return a refresh token")?;
                let docx_copy = GdriveConfig::load(&core.state_dir)?
                    .map(|config| config.docx_copy)
                    .unwrap_or(false);
                GdriveConfig {
                    refresh_token,
                    docx_copy,
                }
                .save(&core.state_dir)
            }
            .await;
            core.finish_pairing_attempt();
            match outcome {
                Ok(()) => core.emit(DaemonEvent::PairStatus {
                    state: "approved".into(),
                    code,
                    message: "Paired with Google Drive.".to_string(),
                }),
                Err(error) => core.emit(DaemonEvent::PairStatus {
                    state: "error".into(),
                    code,
                    message: format!("{error:#}"),
                }),
            }
        });
        Ok(info)
    }

    fn finish_pairing_attempt(&self) {
        let mut status = self.status.lock().expect("status lock poisoned");
        status.pairing_running = false;
    }

    /// Forget every paired backend. Idempotent.
    ///
    /// A Google refresh token is revoked while the radio is on; being offline
    /// must never keep the user paired on the device, so a failed revoke is
    /// only logged.
    pub async fn unpair(&self) -> Result<()> {
        if let Some(runner) = &self.gdrive {
            match GdriveConfig::load(&self.state_dir) {
                Ok(Some(_)) => match self.radio.hold("unpair") {
                    Ok(guard) => {
                        if let Err(error) = runner.revoke(&self.state_dir).await {
                            tracing::warn!(%error, "could not revoke the Google Drive token");
                        }
                        drop(guard);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "could not open the radio to revoke Google Drive");
                    }
                },
                Ok(None) => {}
                Err(error) => tracing::warn!(%error, "could not read the Google Drive pairing"),
            }
        }
        remove_if_exists(&SyncConfig::path(&self.state_dir))?;
        let database = state_database(&self.state_dir);
        for path in [
            database.clone(),
            database.with_extension("sqlite-wal"),
            database.with_extension("sqlite-shm"),
        ] {
            remove_if_exists(&path)?;
        }
        clear_library(&self.state_dir);
        Ok(())
    }

    // ----- Wi-Fi -----------------------------------------------------------

    /// Scan. Starting a scan opens (or extends) the Wi-Fi screen's radio window.
    pub async fn wifi_scan(&self) -> Result<Vec<WifiNetwork>> {
        self.radio
            .open_window(crate::radio::WIFI_WINDOW)
            .context("open the Wi-Fi radio window")?;
        self.network.scan().await
    }

    /// Connect and check for a captive portal, which Carrel cannot show.
    pub async fn wifi_connect(&self, ssid: &str, password: &str) -> Result<()> {
        self.radio
            .open_window(crate::radio::WIFI_WINDOW)
            .context("open the Wi-Fi radio window")?;
        self.network.connect(ssid, password).await?;
        match self.network.connectivity().await? {
            Connectivity::Portal => bail!(PORTAL_MESSAGE),
            _ => Ok(()),
        }
    }

    /// Forget a saved network.
    pub async fn wifi_forget(&self, ssid: &str) -> Result<()> {
        self.radio
            .open_window(crate::radio::WIFI_WINDOW)
            .context("open the Wi-Fi radio window")?;
        self.network.forget(ssid).await
    }

    // ----- battery and sleep ----------------------------------------------

    /// Read the battery without applying the low-battery policy.
    pub async fn battery(&self) -> Result<BatteryStatus> {
        self.power.battery().await
    }

    /// Read the battery, emit a change, and at 3 % (not charging) commit,
    /// emit the signal and suspend.
    pub async fn check_battery(&self) -> Result<BatteryStatus> {
        let battery = self.power.battery().await?;
        let (changed, low) = {
            let mut status = self.status.lock().expect("status lock poisoned");
            let changed = status.battery != Some(battery);
            if changed {
                status.battery = Some(battery);
            }
            let low =
                if battery.present && !battery.charging && battery.percent <= LOW_BATTERY_PERCENT {
                    if status.low_battery_handled {
                        false
                    } else {
                        status.low_battery_handled = true;
                        true
                    }
                } else {
                    // Back on the charger or above the threshold: re-arm.
                    status.low_battery_handled = false;
                    false
                };
            (changed, low)
        };
        if changed {
            self.emit(DaemonEvent::BatteryChanged(battery));
        }
        if low {
            tracing::warn!(
                percent = battery.percent,
                "battery low: committing history and suspending"
            );
            self.commit("low battery")?;
            // The signal was already emitted by the change above; a device that
            // starts at 3 % has no previous value, so it always emits here.
            self.session.suspend().await?;
        }
        Ok(battery)
    }

    // ----- updates ---------------------------------------------------------

    /// The update row's one-line state.
    pub fn update_status(&self) -> String {
        self.update.status().wire()
    }

    /// Apply an update that a sync session already downloaded and verified.
    /// This never touches the network: it stages the payload and starts the
    /// existing `carrel-update.service`, which applies it and reboots.
    pub fn apply_update(&self) -> Result<()> {
        self.update.apply()
    }

    /// Called at the end of a sync session, while the radio guard is still
    /// held.  An update problem is logged and shown on the update row; it does
    /// not turn a successful file sync into an error.
    async fn check_updates(&self) {
        let update = self.update.clone();
        match tokio::task::spawn_blocking(move || update.check_and_stage()).await {
            Ok(Ok(state)) => tracing::info!(status = %state.wire(), "update check finished"),
            Ok(Err(error)) => tracing::warn!(%error, "update check failed"),
            Err(error) => tracing::warn!(%error, "update check task stopped"),
        }
    }

    /// Called when logind says the system is about to sleep.
    pub async fn handle_sleep_pending(&self) {
        match self.commit("before suspend") {
            Ok(Some(rev)) => tracing::info!(rev, "committed history before suspend"),
            Ok(None) => tracing::debug!("nothing to commit before suspend"),
            Err(error) => tracing::error!(%error, "history commit before suspend failed"),
        }
    }
}

/// The daemon's long-running sleeps: commit when idle, watch the battery,
/// sweep the Wi-Fi window and re-arm the logind delay inhibitor.
pub struct DaemonTasks {
    core: Arc<DaemonCore>,
}

impl DaemonTasks {
    pub fn new(core: Arc<DaemonCore>) -> Self {
        Self { core }
    }

    /// Spawn all periodic tasks. They stop when the runtime shuts down.
    pub fn spawn(&self) {
        let core = Arc::clone(&self.core);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(TICK);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                core.radio().expire_window();
                if let Err(error) = core.commit_if_idle(SystemTime::now(), IDLE_COMMIT) {
                    tracing::warn!(%error, "idle history commit failed");
                }
            }
        });

        let core = Arc::clone(&self.core);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(TICK);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if let Err(error) = core.check_battery().await {
                    // No UPower in a container is normal; log once per tick at
                    // debug level so the journal stays readable.
                    tracing::debug!(%error, "battery check failed");
                }
            }
        });

        let core = Arc::clone(&self.core);
        tokio::spawn(async move { sleep_watcher(core).await });
    }
}

async fn sleep_watcher(core: Arc<DaemonCore>) {
    let session = Arc::clone(core.session());
    loop {
        let mut guard = match session.inhibit_sleep().await {
            Ok(guard) => guard,
            Err(error) => {
                tracing::debug!(%error, "logind is not available for sleep handling");
                tokio::time::sleep(Duration::from_secs(60)).await;
                continue;
            }
        };
        if let Err(error) = guard.wait_pending().await {
            tracing::warn!(%error, "waiting for suspend failed");
            tokio::time::sleep(Duration::from_secs(60)).await;
            continue;
        }
        core.handle_sleep_pending().await;
        drop(guard);
        if let Err(error) = session.wait_resume().await {
            tracing::debug!(%error, "waiting for resume failed");
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    }
}

async fn start_session(url: &str) -> Result<PairingSession> {
    let device_name = device_name();
    let device_pubkey = generate_device_pubkey()?;
    PairingSession::start(url, &device_name, &device_pubkey)
        .await
        .context("start pairing with the server")
}

fn resolve_server_url(backend: &str, url: &str) -> Result<String> {
    let url = url.trim();
    if url.is_empty() && backend == "nostos" {
        return Ok(NOSTOS_URL.to_string());
    }
    validate_server_url(url)
}

fn validate_server_url(url: &str) -> Result<String> {
    let url = url.trim();
    if url.is_empty() {
        bail!("the server URL is empty");
    }
    let parsed = Url::parse(url).with_context(|| format!("invalid server URL {url:?}"))?;
    if !matches!(parsed.scheme(), "https" | "http") || parsed.host_str().is_none() {
        bail!("the server URL must be an http:// or https:// URL");
    }
    Ok(url.to_string())
}

fn device_name() -> String {
    if let Ok(name) = std::env::var("CARREL_DEVICE_NAME") {
        let name = name.trim();
        if !name.is_empty() && name.chars().count() <= 100 {
            return name.to_string();
        }
    }
    if let Ok(name) = std::env::var("HOSTNAME") {
        let name = name.trim();
        if !name.is_empty() && name.chars().count() <= 100 {
            return name.to_string();
        }
    }
    if let Ok(name) = fs::read_to_string("/etc/hostname") {
        let name = name.trim();
        if !name.is_empty() && name.chars().count() <= 100 {
            return name.to_string();
        }
    }
    "carrel".to_string()
}

fn describe_summary(summary: &SyncSummary) -> String {
    match (&summary.server, &summary.gdrive) {
        (Some(server), None) => describe_report(server),
        (None, Some(drive)) => describe_gdrive(drive),
        (Some(server), Some(drive)) if server.is_noop() && drive.is_noop() => {
            "Already up to date.".to_string()
        }
        (Some(server), Some(drive)) => {
            let mut line = format!(
                "Synced: {} pulled, {} pushed; Google Drive: {}.",
                server.pulled,
                server.pushed,
                describe_gdrive_counts(drive)
            );
            if let Some(library) = server.library_summary() {
                line.push(' ');
                line.push_str(&library);
            }
            line
        }
        (None, None) => "Already up to date.".to_string(),
    }
}

fn describe_gdrive(report: &GdriveReport) -> String {
    if report.is_noop() {
        "Already up to date on Google Drive.".to_string()
    } else {
        format!("Google Drive: {}.", describe_gdrive_counts(report))
    }
}

fn describe_gdrive_counts(report: &GdriveReport) -> String {
    format!(
        "{} uploaded, {} moved to .deleted",
        report.uploaded, report.deleted
    )
}

fn describe_report(report: &SyncReport) -> String {
    if report.is_noop() {
        return "Already up to date.".to_string();
    }
    if report.library_only() {
        return report
            .library_summary()
            .unwrap_or_else(|| "Already up to date.".to_string());
    }
    let mut line = format!(
        "Synced: {} pulled, {} pushed, {} conflict(s).",
        report.pulled, report.pushed, report.conflicts
    );
    if let Some(library) = report.library_summary() {
        line.push(' ');
        line.push_str(&library);
    }
    line
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;

    use crate::power::SleepGuard;
    use crate::radio::{Rfkill, WIFI_WINDOW};
    use carrel_export::{DriveCall, DriveInfo, FakeDrives};

    struct FakeRfkill {
        blocked: AtomicBool,
        calls: StdMutex<Vec<&'static str>>,
    }

    impl FakeRfkill {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                blocked: AtomicBool::new(true),
                calls: StdMutex::new(Vec::new()),
            })
        }
    }

    impl Rfkill for FakeRfkill {
        fn block_wifi(&self) -> Result<()> {
            self.calls.lock().unwrap().push("block");
            self.blocked.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn unblock_wifi(&self) -> Result<()> {
            self.calls.lock().unwrap().push("unblock");
            self.blocked.store(false, Ordering::SeqCst);
            Ok(())
        }

        fn is_blocked(&self) -> Result<bool> {
            Ok(self.blocked.load(Ordering::SeqCst))
        }
    }

    struct FakeNetwork {
        networks: StdMutex<Vec<WifiNetwork>>,
        connectivity: StdMutex<Connectivity>,
        calls: StdMutex<Vec<String>>,
        fail_scan: AtomicBool,
    }

    impl FakeNetwork {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                networks: StdMutex::new(vec![WifiNetwork {
                    ssid: "Study".into(),
                    strength: 80,
                    secured: true,
                    connected: false,
                }]),
                connectivity: StdMutex::new(Connectivity::Full),
                calls: StdMutex::new(Vec::new()),
                fail_scan: AtomicBool::new(false),
            })
        }
    }

    #[async_trait]
    impl Network for FakeNetwork {
        async fn scan(&self) -> Result<Vec<WifiNetwork>> {
            self.calls.lock().unwrap().push("scan".into());
            if self.fail_scan.load(Ordering::SeqCst) {
                bail!("scan failed");
            }
            Ok(self.networks.lock().unwrap().clone())
        }

        async fn connect(&self, ssid: &str, _password: &str) -> Result<()> {
            self.calls.lock().unwrap().push(format!("connect {ssid}"));
            Ok(())
        }

        async fn forget(&self, ssid: &str) -> Result<()> {
            self.calls.lock().unwrap().push(format!("forget {ssid}"));
            self.networks
                .lock()
                .unwrap()
                .retain(|network| network.ssid != ssid);
            Ok(())
        }

        async fn connectivity(&self) -> Result<Connectivity> {
            Ok(*self.connectivity.lock().unwrap())
        }
    }

    struct FakePower {
        battery: StdMutex<BatteryStatus>,
    }

    impl FakePower {
        fn new(battery: BatteryStatus) -> Arc<Self> {
            Arc::new(Self {
                battery: StdMutex::new(battery),
            })
        }

        fn set(&self, battery: BatteryStatus) {
            *self.battery.lock().unwrap() = battery;
        }
    }

    #[async_trait]
    impl Power for FakePower {
        async fn battery(&self) -> Result<BatteryStatus> {
            Ok(*self.battery.lock().unwrap())
        }
    }

    struct FakeGuard;

    #[async_trait]
    impl SleepGuard for FakeGuard {
        async fn wait_pending(&mut self) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeSession {
        suspends: AtomicUsize,
    }

    #[async_trait]
    impl Session for FakeSession {
        async fn suspend(&self) -> Result<()> {
            self.suspends.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn inhibit_sleep(&self) -> Result<Box<dyn SleepGuard>> {
            Ok(Box::new(FakeGuard))
        }

        async fn wait_resume(&self) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingSink {
        events: StdMutex<Vec<DaemonEvent>>,
    }

    impl RecordingSink {
        fn events(&self) -> Vec<DaemonEvent> {
            self.events.lock().unwrap().clone()
        }

        fn battery_events(&self) -> usize {
            self.events()
                .iter()
                .filter(|event| matches!(event, DaemonEvent::BatteryChanged(_)))
                .count()
        }
    }

    impl EventSink for RecordingSink {
        fn emit(&self, event: DaemonEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[derive(Default)]
    struct FakeRunner {
        fail: bool,
        /// Whether local history had a commit at the moment the sync ran.
        saw_history: AtomicBool,
        calls: AtomicUsize,
    }

    impl FakeRunner {
        fn new(fail: bool) -> Arc<Self> {
            Arc::new(Self {
                fail,
                ..Self::default()
            })
        }
    }

    #[async_trait]
    impl SyncRunner for FakeRunner {
        async fn sync(&self, writing_dir: &Path, state_dir: &Path) -> Result<SyncReport> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let git_dir = History::git_dir(writing_dir);
            let has_commit = git2::Repository::open_bare(&git_dir)
                .map(|repo| repo.head().is_ok())
                .unwrap_or(false);
            self.saw_history.store(has_commit, Ordering::SeqCst);
            // The daemon's radio is not visible here; the test checks its state
            // after the call instead.
            let _ = state_dir;
            if self.fail {
                bail!("network is off");
            }
            Ok(SyncReport::default())
        }
    }

    #[derive(Default)]
    struct FakeGdrive {
        fail: bool,
        calls: AtomicUsize,
    }

    impl FakeGdrive {
        fn new(fail: bool) -> Arc<Self> {
            Arc::new(Self {
                fail,
                ..Self::default()
            })
        }
    }

    #[async_trait]
    impl GdriveRunner for FakeGdrive {
        async fn sync(&self, _writing_dir: &Path, _state_dir: &Path) -> Result<GdriveReport> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                bail!("Google Drive is unreachable");
            }
            Ok(GdriveReport {
                uploaded: 1,
                ..GdriveReport::default()
            })
        }
    }

    struct Harness {
        core: Arc<DaemonCore>,
        dir: tempfile::TempDir,
        rfkill: Arc<FakeRfkill>,
        network: Arc<FakeNetwork>,
        power: Arc<FakePower>,
        session: Arc<FakeSession>,
        events: Arc<RecordingSink>,
        drives: Arc<FakeDrives>,
    }

    impl Harness {
        fn new(runner: Arc<FakeRunner>, battery: BatteryStatus) -> Self {
            Self::build(runner, battery, None)
        }

        fn with_gdrive(
            runner: Arc<FakeRunner>,
            gdrive: Arc<FakeGdrive>,
            battery: BatteryStatus,
        ) -> Self {
            Self::build(runner, battery, Some(gdrive))
        }

        fn build(
            runner: Arc<FakeRunner>,
            battery: BatteryStatus,
            gdrive: Option<Arc<dyn GdriveRunner>>,
        ) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let writing = dir.path().join("writing");
            let state = dir.path().join("state");
            fs::create_dir_all(&writing).unwrap();
            let rfkill = FakeRfkill::new();
            let network = FakeNetwork::new();
            let power = FakePower::new(battery);
            let session = Arc::new(FakeSession::default());
            let events = Arc::new(RecordingSink::default());
            let drives = Arc::new(FakeDrives::new(
                dir.path().join("drives"),
                vec![DriveInfo {
                    id: "usb-1".into(),
                    name: "USB stick".into(),
                    size: 16 * 1024 * 1024,
                    mount_points: Vec::new(),
                }],
            ));
            let core = Arc::new(
                DaemonCore::new(DaemonDeps {
                    writing_dir: writing.clone(),
                    state_dir: state,
                    radio: Arc::new(RadioManager::new(rfkill.clone())),
                    network: network.clone(),
                    power: power.clone(),
                    session: session.clone(),
                    runner: runner.clone(),
                    drives: drives.clone(),
                    gdrive,
                    events: events.clone(),
                })
                .unwrap(),
            );
            Self {
                core,
                dir,
                rfkill,
                network,
                power,
                session,
                events,
                drives,
            }
        }

        fn write(&self, path: &str, text: &str) {
            let target = self.dir.path().join("writing").join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, text).unwrap();
        }

        /// Where [`FakeDrives`] mounts its first drive.
        fn drive_mount(&self) -> PathBuf {
            self.dir.path().join("drives/drive-0")
        }
    }

    fn plugged_in(percent: u8) -> BatteryStatus {
        BatteryStatus {
            present: true,
            percent,
            charging: true,
        }
    }

    fn discharging(percent: u8) -> BatteryStatus {
        BatteryStatus {
            present: true,
            percent,
            charging: false,
        }
    }

    #[test]
    fn list_drives_never_mounts() {
        let harness = Harness::new(FakeRunner::new(false), plugged_in(80));
        let drives = harness.core.list_drives().unwrap();
        assert_eq!(drives.len(), 1);
        assert_eq!(drives[0].name, "USB stick");
        assert_eq!(drives[0].size, 16 * 1024 * 1024);
        assert!(!drives[0].mounted);
        assert_eq!(harness.drives.calls(), [DriveCall::List]);
    }

    #[tokio::test]
    async fn export_writes_to_the_drive_then_unmounts_and_powers_off() {
        let harness = Harness::new(FakeRunner::new(false), plugged_in(80));
        harness.write("draft.md", "# One\n");
        harness.write("Notes/second.md", "two");

        let destination = harness
            .core
            .export(vec![], "markdown".into(), "usb-1".into())
            .await
            .unwrap();
        let destination = PathBuf::from(destination);
        assert!(
            destination.starts_with(harness.drive_mount()),
            "{destination:?}"
        );
        assert_eq!(
            fs::read_to_string(destination.join("draft.md")).unwrap(),
            "# One\n"
        );
        assert_eq!(
            fs::read_to_string(destination.join("Notes/second.md")).unwrap(),
            "two"
        );
        assert_eq!(
            harness.drives.calls(),
            [
                DriveCall::Mount("usb-1".into()),
                DriveCall::Unmount("usb-1".into()),
                DriveCall::PowerOff("usb-1".into()),
            ],
            "the drive is mounted only for the export and powered off after"
        );
        assert!(!harness.drives.is_mounted("usb-1"));

        let progress: Vec<(String, u32, u32)> = harness
            .events
            .events()
            .into_iter()
            .filter_map(|event| match event {
                DaemonEvent::ExportProgress(progress) => {
                    Some((progress.state, progress.index, progress.total))
                }
                _ => None,
            })
            .collect();
        assert_eq!(progress.first(), Some(&("exporting".into(), 0, 0)));
        assert_eq!(
            progress.last(),
            Some(&("done".into(), 2, 2)),
            "{progress:?}"
        );
        assert!(harness.events.events().iter().any(|event| matches!(
            event,
            DaemonEvent::ExportProgress(progress)
                if progress.message.contains("Safe to remove")
                    && progress.destination == destination.display().to_string()
        )));
    }

    #[tokio::test]
    async fn export_errors_release_the_lock_and_an_error_after_mount_powers_off() {
        let harness = Harness::new(FakeRunner::new(false), plugged_in(80));

        // A bad format and a missing selection are rejected before mounting.
        let error = harness
            .core
            .export(vec![], "rtf".into(), "usb-1".into())
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("unknown export format"));
        let error = harness
            .core
            .export(vec!["missing.md".into()], "markdown".into(), "usb-1".into())
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("missing.md"));
        assert!(harness.drives.calls().is_empty());

        // An empty writing root fails after the drive was mounted: the guard
        // still unmounts and powers it off, and the lock is released.
        let error = harness
            .core
            .export(vec![], "markdown".into(), "usb-1".into())
            .await
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("nothing to export"),
            "{error:#}"
        );
        assert_eq!(
            harness.drives.calls(),
            [
                DriveCall::Mount("usb-1".into()),
                DriveCall::Unmount("usb-1".into()),
                DriveCall::PowerOff("usb-1".into()),
            ]
        );
        assert!(harness.events.events().iter().any(|event| matches!(
            event,
            DaemonEvent::ExportProgress(progress) if progress.state == "error"
        )));

        harness.write("after.md", "still works");
        harness
            .core
            .export(vec![], "markdown".into(), "usb-1".into())
            .await
            .expect("the failed export released the lock");
    }

    #[tokio::test]
    async fn import_copies_text_files_into_imported_and_powers_off() {
        let harness = Harness::new(FakeRunner::new(false), plugged_in(80));
        let inbox = harness.drive_mount().join("Inbox");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("letter.txt"), "from the stick").unwrap();
        fs::write(inbox.join("cover.png"), b"png").unwrap();

        let imported = harness
            .core
            .import("usb-1".into(), vec!["Inbox".into()])
            .await
            .unwrap();
        assert_eq!(imported.len(), 1, "{imported:?}");
        let path = PathBuf::from(&imported[0]);
        assert_eq!(path.file_name().unwrap(), "letter.md");
        assert_eq!(fs::read_to_string(&path).unwrap(), "from the stick");
        assert!(harness
            .dir
            .path()
            .join("writing/Imported/letter.md")
            .is_file());
        assert_eq!(
            harness.drives.calls(),
            [
                DriveCall::Mount("usb-1".into()),
                DriveCall::Unmount("usb-1".into()),
                DriveCall::PowerOff("usb-1".into()),
            ]
        );
    }

    #[tokio::test]
    async fn import_rejects_a_path_outside_the_drive() {
        let harness = Harness::new(FakeRunner::new(false), plugged_in(80));
        fs::write(harness.dir.path().join("outside.md"), "no").unwrap();
        let outside = harness.dir.path().join("outside.md").display().to_string();
        let error = harness
            .core
            .import("usb-1".into(), vec![outside])
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("outside the drive"));
        assert!(!harness.dir.path().join("writing/Imported").exists());
        // The drive was mounted for the attempt but still powered off.
        assert_eq!(
            harness.drives.calls(),
            [
                DriveCall::Mount("usb-1".into()),
                DriveCall::Unmount("usb-1".into()),
                DriveCall::PowerOff("usb-1".into()),
            ]
        );
    }

    #[test]
    fn commit_history_and_restore_round_trip() {
        let harness = Harness::new(FakeRunner::new(false), plugged_in(80));
        harness.write("draft.md", "one two");
        let rev = harness.core.commit("test").unwrap().unwrap();
        harness.write("draft.md", "one two three");
        harness.core.commit("test").unwrap();

        let versions = harness.core.history("draft.md").unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].words, 3);
        assert_eq!(versions[0].delta, 1);
        assert_eq!(harness.core.restore("draft.md", &rev).unwrap(), "one two");
        assert_eq!(
            fs::read_to_string(harness.dir.path().join("writing/draft.md")).unwrap(),
            "one two three"
        );
    }

    #[tokio::test]
    async fn sync_commits_history_first_and_blocks_the_radio_after() {
        let runner = FakeRunner::new(false);
        let harness = Harness::new(runner.clone(), plugged_in(80));
        harness.write("draft.md", "text");

        harness.core.run_sync_now().await.unwrap();

        assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
        assert!(
            runner.saw_history.load(Ordering::SeqCst),
            "history must be committed before the sync starts"
        );
        assert!(!harness.core.radio().is_on());
        assert!(harness.rfkill.blocked.load(Ordering::SeqCst));
        let calls = harness.rfkill.calls.lock().unwrap().clone();
        assert_eq!(calls, ["unblock", "block"]);
    }

    #[tokio::test]
    async fn a_failed_sync_still_blocks_the_radio() {
        let runner = FakeRunner::new(true);
        let harness = Harness::new(runner, plugged_in(80));
        harness.write("draft.md", "text");

        let error = harness.core.run_sync_now().await.unwrap_err();
        assert!(error.to_string().contains("network is off"));
        assert!(!harness.core.radio().is_on());
        assert!(harness.rfkill.blocked.load(Ordering::SeqCst));
        let calls = harness.rfkill.calls.lock().unwrap().clone();
        assert_eq!(calls, ["unblock", "block"]);
    }

    #[tokio::test]
    async fn trigger_sync_reports_progress_and_refuses_a_second_run() {
        let runner = FakeRunner::new(false);
        let harness = Harness::new(runner, plugged_in(80));
        harness.write("draft.md", "text");

        harness.core.trigger_sync().unwrap();
        assert!(harness.core.trigger_sync().is_err());
        assert_eq!(harness.core.sync_info().state, "syncing");

        // Let the spawned task finish.
        for _ in 0..100 {
            if harness.core.sync_info().state != "syncing" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let info = harness.core.sync_info();
        assert_eq!(info.state, "idle", "{info:?}");
        assert!(info.last_sync > 0);
        let states: Vec<String> = harness
            .events
            .events()
            .into_iter()
            .filter_map(|event| match event {
                DaemonEvent::SyncStatus(info) => Some(info.state),
                _ => None,
            })
            .collect();
        assert_eq!(states, ["syncing", "idle"]);
    }

    #[tokio::test]
    async fn battery_at_three_percent_commits_and_suspends_once() {
        let harness = Harness::new(FakeRunner::new(false), plugged_in(50));
        harness.write("draft.md", "text");

        harness.core.check_battery().await.unwrap();
        assert_eq!(harness.session.suspends.load(Ordering::SeqCst), 0);
        assert!(harness.core.history_head().unwrap().is_none());

        harness.power.set(discharging(3));
        harness.core.check_battery().await.unwrap();
        assert_eq!(harness.session.suspends.load(Ordering::SeqCst), 1);
        assert!(
            harness.core.history_head().unwrap().is_some(),
            "history must be committed at 3 %"
        );
        assert!(harness.events.events().iter().any(
            |event| matches!(event, DaemonEvent::BatteryChanged(status) if status.percent == 3)
        ));

        // Still at 3 % and not charging: do not suspend again.
        harness.core.check_battery().await.unwrap();
        assert_eq!(harness.session.suspends.load(Ordering::SeqCst), 1);

        // Charging re-arms the threshold, but does not suspend.
        harness.power.set(plugged_in(3));
        harness.core.check_battery().await.unwrap();
        assert_eq!(harness.session.suspends.load(Ordering::SeqCst), 1);

        harness.power.set(discharging(2));
        harness.core.check_battery().await.unwrap();
        assert_eq!(harness.session.suspends.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn battery_changes_are_only_reported_when_they_change() {
        let harness = Harness::new(FakeRunner::new(false), plugged_in(80));
        harness.core.check_battery().await.unwrap();
        harness.core.check_battery().await.unwrap();
        assert_eq!(harness.events.battery_events(), 1);
        harness.power.set(plugged_in(79));
        harness.core.check_battery().await.unwrap();
        assert_eq!(harness.events.battery_events(), 2);
    }

    #[tokio::test]
    async fn sleep_pending_commits_history() {
        let harness = Harness::new(FakeRunner::new(false), plugged_in(80));
        harness.write("draft.md", "before sleep");
        harness.core.handle_sleep_pending().await;
        assert!(harness.core.history_head().unwrap().is_some());
        assert_eq!(harness.core.history("draft.md").unwrap().len(), 1);
    }

    #[tokio::test]
    async fn wifi_scan_opens_the_radio_window() {
        let harness = Harness::new(FakeRunner::new(false), plugged_in(80));
        let networks = harness.core.wifi_scan().await.unwrap();
        assert_eq!(networks[0].ssid, "Study");
        assert!(harness.core.radio().is_on());
        assert!(!harness.rfkill.blocked.load(Ordering::SeqCst));
        assert!(harness.core.radio().is_on());
        // The 2-minute window will close later; force it closed for the test.
        harness.core.radio().force_block().unwrap();
        assert!(harness.rfkill.blocked.load(Ordering::SeqCst));
        assert!(WIFI_WINDOW >= Duration::from_secs(60));
    }

    #[tokio::test]
    async fn a_portal_gets_the_wifi_screen_message() {
        let harness = Harness::new(FakeRunner::new(false), plugged_in(80));
        *harness.network.connectivity.lock().unwrap() = Connectivity::Portal;
        let error = harness
            .core
            .wifi_connect("Hotel", "guest")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("login page"));
        assert_eq!(
            harness.network.calls.lock().unwrap().as_slice(),
            ["connect Hotel"]
        );
    }

    #[tokio::test]
    async fn wifi_forget_reaches_the_network_manager() {
        let harness = Harness::new(FakeRunner::new(false), plugged_in(80));
        harness.core.wifi_forget("Study").await.unwrap();
        assert_eq!(
            harness.network.calls.lock().unwrap().as_slice(),
            ["forget Study"]
        );
        assert!(harness.network.networks.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn pairing_rejects_unknown_backends_before_using_the_network() {
        let harness = Harness::new(FakeRunner::new(false), plugged_in(80));
        assert!(harness
            .core
            .pair("dropbox", "https://example.com")
            .await
            .is_err());
        assert!(harness
            .core
            .pair("caldav", "https://example.com")
            .await
            .is_err());
        assert!(harness.core.pair("self-hosted", "not a url").await.is_err());
        // Nothing was left half-started.
        assert_eq!(harness.core.sync_info().state, "idle");
    }

    #[test]
    fn nostos_uses_its_default_url() {
        assert_eq!(resolve_server_url("nostos", "").unwrap(), NOSTOS_URL);
        assert_eq!(
            resolve_server_url("nostos", " https://nostos.example/carrel/v1 ").unwrap(),
            "https://nostos.example/carrel/v1"
        );
        assert!(resolve_server_url("self-hosted", "").is_err());
        assert!(resolve_server_url("self-hosted", "ftp://example.com").is_err());
        assert!(resolve_server_url("self-hosted", "https://example.com").is_ok());
    }

    #[tokio::test]
    async fn unpair_removes_the_configuration_and_state() {
        let harness = Harness::new(FakeRunner::new(false), plugged_in(80));
        let config = SyncConfig {
            server_url: "https://127.0.0.1:8443".into(),
            token: "secret".into(),
            cert_sha256: None,
            server_name: Some("Test".into()),
            device_pubkey: "ab".repeat(32),
        };
        config.save(&harness.core.state_dir).unwrap();
        fs::write(state_database(&harness.core.state_dir), "db").unwrap();
        harness.core.unpair().await.unwrap();
        assert!(!SyncConfig::path(&harness.core.state_dir).exists());
        assert!(!state_database(&harness.core.state_dir).exists());
        // Idempotent.
        harness.core.unpair().await.unwrap();
    }

    fn pair_google_drive(harness: &Harness) {
        GdriveConfig {
            refresh_token: "refresh-1".into(),
            docx_copy: false,
        }
        .save(&harness.core.state_dir)
        .unwrap();
    }

    fn pair_server(harness: &Harness) {
        SyncConfig {
            server_url: "https://127.0.0.1:8443".into(),
            token: "server-token".into(),
            cert_sha256: None,
            server_name: Some("Test".into()),
            device_pubkey: "ab".repeat(32),
        }
        .save(&harness.core.state_dir)
        .unwrap();
    }

    #[tokio::test]
    async fn sync_runs_all_configured_backends_in_one_radio_window() {
        let runner = FakeRunner::new(false);
        let gdrive = FakeGdrive::new(false);
        let harness = Harness::with_gdrive(runner.clone(), gdrive.clone(), plugged_in(80));
        pair_server(&harness);
        pair_google_drive(&harness);
        harness.write("draft.md", "text");

        let summary = harness.core.run_sync_now().await.unwrap();
        assert!(summary.server.is_some());
        assert!(summary.gdrive.is_some());
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
        assert_eq!(gdrive.calls.load(Ordering::SeqCst), 1);
        let calls = harness.rfkill.calls.lock().unwrap().clone();
        assert_eq!(calls, ["unblock", "block"], "one radio window for both");
    }

    #[tokio::test]
    async fn a_google_only_device_skips_the_server_backend() {
        let runner = FakeRunner::new(false);
        let gdrive = FakeGdrive::new(false);
        let harness = Harness::with_gdrive(runner.clone(), gdrive.clone(), plugged_in(80));
        pair_google_drive(&harness);
        harness.write("draft.md", "text");

        let summary = harness.core.run_sync_now().await.unwrap();
        assert!(summary.server.is_none());
        assert!(summary.gdrive.is_some());
        assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
        assert_eq!(gdrive.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_google_drive_error_ends_the_sync_and_blocks_the_radio() {
        let gdrive = FakeGdrive::new(true);
        let harness = Harness::with_gdrive(FakeRunner::new(false), gdrive.clone(), plugged_in(80));
        pair_google_drive(&harness);
        harness.write("draft.md", "text");

        let error = harness.core.run_sync_now().await.unwrap_err();
        assert!(format!("{error:#}").contains("unreachable"), "{error:#}");
        assert_eq!(gdrive.calls.load(Ordering::SeqCst), 1);
        assert!(!harness.core.radio().is_on());
        assert!(harness.rfkill.blocked.load(Ordering::SeqCst));
        let calls = harness.rfkill.calls.lock().unwrap().clone();
        assert_eq!(calls, ["unblock", "block"]);
    }

    #[tokio::test]
    async fn a_sync_without_any_backend_says_not_paired() {
        let harness = Harness::with_gdrive(
            FakeRunner::new(false),
            FakeGdrive::new(false),
            plugged_in(80),
        );
        harness.write("draft.md", "text");
        let error = harness.core.run_sync_now().await.unwrap_err();
        assert!(error.to_string().contains("not paired"), "{error:#}");
        assert!(harness.rfkill.blocked.load(Ordering::SeqCst));
    }
}
