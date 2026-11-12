//! The daemon boundary: every screen talks to `stylosd` through this module.
//!
//! Rule 2: the shell never touches the network. Commits, sync, pairing, Wi-Fi,
//! battery, updates and export all go through `org.stylos.Daemon1` on the
//! system bus. [`DaemonClient`] mirrors that interface method for method; the
//! types are copied field for field from `stylosd/src/{history,network,power,
//! daemon}.rs` so the D-Bus struct signatures match (the shell does not depend
//! on the daemon crate in production).
//!
//! Two implementations exist:
//!
//! * [`ZbusDaemon`] calls the real service. Its methods block, so [`Client`]
//!   runs them on a worker thread; the GTK main loop only ever sees a callback.
//! * [`FakeDaemon`] is an in-memory daemon for tests and for the screenshots
//!   (`STYLOS_FAKE_DAEMON=1`). It has the same data shapes, so a state machine
//!   tested against it behaves the same against the real service.
//!
//! Signals arrive through [`DaemonClient::subscribe`]. A listener runs on the
//! daemon's thread and must hand data to the UI; [`Client::subscribe`] already
//! moves them to the GTK main loop.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use gtk::glib;
use serde::{Deserialize, Serialize};

pub const BUS_NAME: &str = "org.stylos.Daemon1";
pub const OBJECT_PATH: &str = "/org/stylos/Daemon1";
pub const INTERFACE: &str = "org.stylos.Daemon1";

/// A signal listener shared between the daemon task and `subscribe`.
type SharedListener = Arc<dyn Fn(DaemonEvent) + Send + Sync>;
type OwnedListener = Box<dyn Fn(DaemonEvent) + Send + Sync + 'static>;
type ListenerList = Mutex<Vec<SharedListener>>;

/// Test hook shared with `stylosd`: connect to a private bus address.
pub const BUS_ADDRESS_ENV: &str = "STYLOS_DBUS_ADDRESS";
/// Test hook shared with `stylosd`: authenticate anonymously on a private bus.
pub const BUS_ANONYMOUS_ENV: &str = "STYLOS_DBUS_ANONYMOUS";

// ---------------------------------------------------------------------------
// Types mirrored from stylosd. Field order is the D-Bus struct signature.
// ---------------------------------------------------------------------------

/// One restorable version of a file (`stylosd::history::HistoryEntry`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct HistoryEntry {
    /// Git commit id (hex).
    pub rev: String,
    /// Commit time, Unix seconds.
    pub time: i64,
    /// Words in the file at that version.
    pub words: u32,
    /// Word-count change against the previous (older) version.
    pub delta: i64,
}

/// One access point as the Wi-Fi screen shows it (`stylosd::network::WifiNetwork`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct WifiNetwork {
    pub ssid: String,
    /// 0-100.
    pub strength: u8,
    /// True when the network needs a password (WPA/WEP/802.1X).
    pub secured: bool,
    pub connected: bool,
}

/// The battery as the bottom line shows it (`stylosd::power::BatteryStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct BatteryStatus {
    /// False on a desktop without a battery.
    pub present: bool,
    /// 0-100.
    pub percent: u8,
    /// True while the charger is connected and the battery is filling.
    pub charging: bool,
}

impl BatteryStatus {
    pub const UNKNOWN: Self = Self {
        present: false,
        percent: 0,
        charging: false,
    };
}

/// What the Sync screen shows (`stylosd::daemon::SyncInfo`).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct SyncInfo {
    /// `"idle"`, `"syncing"` or `"error"`.
    pub state: String,
    pub message: String,
    /// Unix seconds of the last successful sync, 0 before the first one.
    pub last_sync: i64,
}

/// The code and approval URL the Sync screen shows while pairing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct PairingInfo {
    pub backend: String,
    pub code: String,
    /// The URL the user opens on a phone to approve the code.
    pub approval_url: String,
    /// Unix seconds.
    pub expires_at: i64,
}

/// One ranked hit from the Quotes library (`stylosd::library::LibraryHit`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct LibraryHit {
    pub id: String,
    /// `"highlight"`, `"note"` or `"passage"`.
    pub kind: String,
    pub text: String,
    pub book_title: String,
    pub author: String,
    pub location: String,
    pub citation_markdown: String,
}

impl LibraryHit {
    /// What the palette calls the kind, with an empty field for an unknown one.
    pub fn kind_label(&self) -> &str {
        match self.kind.as_str() {
            "highlight" => "Highlight",
            "note" => "Note",
            "passage" => "Passage",
            _ => "",
        }
    }
}

/// One book in the filter list (`stylosd::library::LibraryBookOption`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct LibraryBookOption {
    pub id: String,
    pub title: String,
    pub author: String,
}

/// Books, authors and topics for the palette's filter chips.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct LibraryFilterOptions {
    pub books: Vec<LibraryBookOption>,
    pub authors: Vec<String>,
    pub topics: Vec<String>,
}

/// A removable drive for the Export screen. Owned by WP6; `ListDrives` is not
/// on the D-Bus interface yet, so [`ZbusDaemon`] reports it as unsupported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drive {
    pub id: String,
    pub label: String,
    /// Free space in bytes.
    pub free_bytes: u64,
}

/// A sync backend, with the exact strings `styłosd` accepts on `Pair`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Nostos,
    SelfHosted,
    GoogleDrive,
}

impl Backend {
    pub const ALL: [Self; 3] = [Self::Nostos, Self::SelfHosted, Self::GoogleDrive];

    /// The wire value used by `Pair(backend, …)`.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Nostos => "nostos",
            Self::SelfHosted => "self-hosted",
            Self::GoogleDrive => "google-drive",
        }
    }

    /// What the screen calls it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Nostos => "Nostos",
            Self::SelfHosted => "Self-hosted server",
            Self::GoogleDrive => "Google Drive",
        }
    }

    /// The pairing instruction line.
    pub fn pair_hint(self) -> &'static str {
        match self {
            Self::Nostos => "Approve the code at app.nostos.page/pair.",
            Self::SelfHosted => "Approve the code on your server's page.",
            Self::GoogleDrive => "Enter the code at google.com/device.",
        }
    }
}

/// A daemon failure, split by what the screen should do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonErrorKind {
    /// A normal failure; show the message.
    Failed,
    /// The daemon does not implement this yet (stubs name their work package).
    Unsupported,
    /// The network needs a login page; Stylos cannot show one.
    CaptivePortal,
    /// `stylosd` is not running or the bus is unreachable.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonError {
    pub kind: DaemonErrorKind,
    pub message: String,
}

impl DaemonError {
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            kind: DaemonErrorKind::Failed,
            message: message.into(),
        }
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self {
            kind: DaemonErrorKind::Unsupported,
            message: message.into(),
        }
    }

    pub fn captive_portal(message: impl Into<String>) -> Self {
        Self {
            kind: DaemonErrorKind::CaptivePortal,
            message: message.into(),
        }
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            kind: DaemonErrorKind::Unavailable,
            message: message.into(),
        }
    }

    pub fn is_unavailable(&self) -> bool {
        self.kind == DaemonErrorKind::Unavailable
    }
}

impl fmt::Display for DaemonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DaemonError {}

/// A signal from the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonEvent {
    /// `BatteryChanged`.
    Battery(BatteryStatus),
    /// `SyncStatus`.
    Sync(SyncInfo),
    /// `PairStatus`: state is `"approved"` or `"error"`.
    Pair {
        state: String,
        code: String,
        message: String,
    },
}

/// Everything the shell needs from `org.stylos.Daemon1`.
///
/// The methods are synchronous on purpose: they mirror the D-Bus calls, and
/// implementations may block. The GTK screens never call them directly; they
/// go through [`Client`], which runs the call on a worker thread and delivers
/// the result on the main loop. This keeps the trait small and testable with
/// [`FakeDaemon`].
pub trait DaemonClient: Send + Sync + 'static {
    /// `Commit()`: snapshot the writing directory into local history.
    fn commit(&self) -> Result<(), DaemonError>;

    /// `History(path)` → versions of one file, newest first.
    fn history(&self, path: &str) -> Result<Vec<HistoryEntry>, DaemonError>;

    /// `Restore(path, rev)` → the old text. Read-only: the caller applies it
    /// as a new edit, so history is never rewritten.
    fn restore(&self, path: &str, rev: &str) -> Result<String, DaemonError>;

    /// `Sync()`: start a sync; progress arrives as [`DaemonEvent::Sync`].
    fn sync(&self) -> Result<(), DaemonError>;

    /// The `SyncInfo` property: state, one line and the last sync time.
    fn sync_info(&self) -> Result<SyncInfo, DaemonError>;

    /// `Pair(backend, url)` → the code to show; approval arrives as a signal.
    /// `backend` is [`Backend::wire`]; `url` is empty for Nostos.
    fn pair(&self, backend: &str, url: &str) -> Result<PairingInfo, DaemonError>;

    /// `Unpair()`: forget the paired server.
    fn unpair(&self) -> Result<(), DaemonError>;

    /// `WifiScan()`: opens the Wi-Fi screen's two-minute radio window.
    fn wifi_scan(&self) -> Result<Vec<WifiNetwork>, DaemonError>;

    /// `WifiConnect(ssid, password)`; the password is empty for open networks.
    fn wifi_connect(&self, ssid: &str, password: &str) -> Result<(), DaemonError>;

    /// `WifiForget(ssid)`.
    fn wifi_forget(&self, ssid: &str) -> Result<(), DaemonError>;

    /// `Battery()`.
    fn battery(&self) -> Result<BatteryStatus, DaemonError>;

    /// `UpdateStatus()`: `"none"` until WP7 defines other values.
    fn update_status(&self) -> Result<String, DaemonError>;

    /// `ApplyUpdate()`.
    fn apply_update(&self) -> Result<(), DaemonError>;

    /// `ListDrives()`. Not on the daemon interface yet (WP6 adds it).
    fn list_drives(&self) -> Result<Vec<Drive>, DaemonError>;

    /// `Export(selection, format, drive_id)` → the path written.
    fn export(
        &self,
        selection: Vec<String>,
        format: String,
        drive_id: String,
    ) -> Result<String, DaemonError>;

    /// `Import(drive_id, paths)` → the imported paths.
    fn import(&self, drive_id: String, paths: Vec<String>) -> Result<Vec<String>, DaemonError>;

    /// `SearchLibrary(query, filters)` → ranked quotes. Filters are `book`
    /// (id or title), `author` and `topic`.
    fn search_library(
        &self,
        query: &str,
        filters: HashMap<String, String>,
    ) -> Result<Vec<LibraryHit>, DaemonError>;

    /// `LibraryFilters()` → the books, authors and topics worth offering.
    fn library_filters(&self) -> Result<LibraryFilterOptions, DaemonError>;

    /// Subscribe to `BatteryChanged`, `SyncStatus` and `PairStatus`.
    fn subscribe(&self, listener: Box<dyn Fn(DaemonEvent) + Send + Sync + 'static>);
}

// ---------------------------------------------------------------------------
// The GTK-facing client: daemon calls stay off the main loop.
// ---------------------------------------------------------------------------

/// A cloneable handle that runs [`DaemonClient`] calls on worker threads and
/// delivers results and signals on the GTK main loop.
#[derive(Clone)]
pub struct Client {
    inner: Arc<dyn DaemonClient>,
}

impl Client {
    pub fn new(inner: Arc<dyn DaemonClient>) -> Self {
        Self { inner }
    }

    pub fn daemon(&self) -> Arc<dyn DaemonClient> {
        Arc::clone(&self.inner)
    }

    /// Run `work` on a worker thread, then `done` on the main loop.
    ///
    /// The page that calls this stays alive until the answer arrives; that is
    /// what a user expects and it means results never update a dropped screen.
    pub fn spawn<T, W, D>(&self, work: W, done: D)
    where
        T: Send + 'static,
        W: FnOnce(Arc<dyn DaemonClient>) -> Result<T, DaemonError> + Send + 'static,
        D: FnOnce(Result<T, DaemonError>) + 'static,
    {
        let (sender, receiver) = async_channel::bounded(1);
        let inner = Arc::clone(&self.inner);
        let spawned = std::thread::Builder::new()
            .name("stylos-daemon-call".to_owned())
            .spawn(move || {
                let result = work(inner);
                let _ = sender.send_blocking(result);
            });
        if spawned.is_err() {
            return;
        }
        glib::spawn_future_local(async move {
            if let Ok(result) = receiver.recv().await {
                done(result);
            }
        });
    }

    /// Deliver daemon signals on the main loop.
    pub fn subscribe<F>(&self, listener: F)
    where
        F: Fn(DaemonEvent) + 'static,
    {
        let (sender, receiver) = async_channel::unbounded();
        self.inner.subscribe(Box::new(move |event| {
            let _ = sender.send_blocking(event);
        }));
        glib::spawn_future_local(async move {
            while let Ok(event) = receiver.recv().await {
                listener(event);
            }
        });
    }
}

// ---------------------------------------------------------------------------
// ZbusDaemon: the real service.
// ---------------------------------------------------------------------------

/// A client for `org.stylos.Daemon1` at `/org/stylos/Daemon1` on the system
/// bus. All zbus work runs on a small tokio runtime; the trait methods block
/// the calling worker thread only.
pub struct ZbusDaemon {
    _runtime: tokio::runtime::Runtime,
    connector: Connector,
    listeners: Arc<ListenerList>,
    subscribed: AtomicBool,
}

#[derive(Clone)]
struct Connector {
    handle: tokio::runtime::Handle,
    connection: Arc<tokio::sync::OnceCell<zbus::Connection>>,
    address: Option<String>,
    anonymous: bool,
}

impl Connector {
    async fn connection(&self) -> zbus::Result<zbus::Connection> {
        self.connection
            .get_or_try_init(|| async {
                match &self.address {
                    Some(address) => {
                        let mut builder = zbus::connection::Builder::address(address.as_str())?;
                        if self.anonymous {
                            builder =
                                builder.auth_mechanism(zbus::connection::AuthMechanism::Anonymous);
                        }
                        builder.build().await
                    }
                    None => zbus::Connection::system().await,
                }
            })
            .await
            .cloned()
    }

    async fn proxy<'a>(&self, connection: &'a zbus::Connection) -> zbus::Result<zbus::Proxy<'a>> {
        zbus::Proxy::new(connection, BUS_NAME, OBJECT_PATH, INTERFACE).await
    }
}

impl ZbusDaemon {
    /// Connect to the real system bus. The connection itself is lazy so a
    /// missing daemon cannot delay startup.
    pub fn new() -> Self {
        Self::build(None, false)
    }

    /// A client for a private bus, used by the integration test.
    pub fn with_address(address: &str, anonymous: bool) -> Self {
        Self::build(Some(address.to_owned()), anonymous)
    }

    fn build(address: Option<String>, anonymous: bool) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("stylos-zbus")
            .build()
            .expect("build the zbus runtime");
        let connector = Connector {
            handle: runtime.handle().clone(),
            connection: Arc::new(tokio::sync::OnceCell::new()),
            address,
            anonymous,
        };
        Self {
            _runtime: runtime,
            connector,
            listeners: Arc::new(Mutex::new(Vec::new())),
            subscribed: AtomicBool::new(false),
        }
    }

    fn block_on<T>(&self, future: impl std::future::Future<Output = T>) -> T {
        self.connector.handle.block_on(future)
    }

    /// One D-Bus method with no return value.
    fn call_unit(
        &self,
        method: &'static str,
        body: &(impl Serialize + zbus::zvariant::Type),
    ) -> Result<(), DaemonError> {
        self.block_on(async {
            let connection = self.connector.connection().await.map_err(map_zbus)?;
            let proxy = self.connector.proxy(&connection).await.map_err(map_zbus)?;
            proxy
                .call_method(method, body)
                .await
                .map(|_| ())
                .map_err(map_zbus)
        })
    }
}

impl Default for ZbusDaemon {
    fn default() -> Self {
        Self::new()
    }
}

impl DaemonClient for ZbusDaemon {
    fn commit(&self) -> Result<(), DaemonError> {
        self.call_unit("Commit", &())
    }

    fn history(&self, path: &str) -> Result<Vec<HistoryEntry>, DaemonError> {
        self.block_on(async {
            let connection = self.connector.connection().await.map_err(map_zbus)?;
            let proxy = self.connector.proxy(&connection).await.map_err(map_zbus)?;
            let reply = proxy
                .call_method("History", &(path,))
                .await
                .map_err(map_zbus)?;
            reply.body().deserialize().map_err(map_zbus)
        })
    }

    fn restore(&self, path: &str, rev: &str) -> Result<String, DaemonError> {
        self.block_on(async {
            let connection = self.connector.connection().await.map_err(map_zbus)?;
            let proxy = self.connector.proxy(&connection).await.map_err(map_zbus)?;
            let reply = proxy
                .call_method("Restore", &(path, rev))
                .await
                .map_err(map_zbus)?;
            reply.body().deserialize().map_err(map_zbus)
        })
    }

    fn sync(&self) -> Result<(), DaemonError> {
        self.call_unit("Sync", &())
    }

    fn sync_info(&self) -> Result<SyncInfo, DaemonError> {
        self.block_on(async {
            let connection = self.connector.connection().await.map_err(map_zbus)?;
            let proxy = self.connector.proxy(&connection).await.map_err(map_zbus)?;
            let (state, message, last_sync): (String, String, i64) =
                proxy.get_property("SyncInfo").await.map_err(map_zbus)?;
            Ok(SyncInfo {
                state,
                message,
                last_sync,
            })
        })
    }

    fn pair(&self, backend: &str, url: &str) -> Result<PairingInfo, DaemonError> {
        self.block_on(async {
            let connection = self.connector.connection().await.map_err(map_zbus)?;
            let proxy = self.connector.proxy(&connection).await.map_err(map_zbus)?;
            let reply = proxy
                .call_method("Pair", &(backend, url))
                .await
                .map_err(map_zbus)?;
            reply.body().deserialize().map_err(map_zbus)
        })
    }

    fn unpair(&self) -> Result<(), DaemonError> {
        self.call_unit("Unpair", &())
    }

    fn wifi_scan(&self) -> Result<Vec<WifiNetwork>, DaemonError> {
        self.block_on(async {
            let connection = self.connector.connection().await.map_err(map_zbus)?;
            let proxy = self.connector.proxy(&connection).await.map_err(map_zbus)?;
            let reply = proxy.call_method("WifiScan", &()).await.map_err(map_zbus)?;
            reply.body().deserialize().map_err(map_zbus)
        })
    }

    fn wifi_connect(&self, ssid: &str, password: &str) -> Result<(), DaemonError> {
        self.call_unit("WifiConnect", &(ssid, password))
    }

    fn wifi_forget(&self, ssid: &str) -> Result<(), DaemonError> {
        self.call_unit("WifiForget", &(ssid,))
    }

    fn battery(&self) -> Result<BatteryStatus, DaemonError> {
        self.block_on(async {
            let connection = self.connector.connection().await.map_err(map_zbus)?;
            let proxy = self.connector.proxy(&connection).await.map_err(map_zbus)?;
            let reply = proxy.call_method("Battery", &()).await.map_err(map_zbus)?;
            reply.body().deserialize().map_err(map_zbus)
        })
    }

    fn update_status(&self) -> Result<String, DaemonError> {
        self.block_on(async {
            let connection = self.connector.connection().await.map_err(map_zbus)?;
            let proxy = self.connector.proxy(&connection).await.map_err(map_zbus)?;
            let reply = proxy
                .call_method("UpdateStatus", &())
                .await
                .map_err(map_zbus)?;
            reply.body().deserialize().map_err(map_zbus)
        })
    }

    fn apply_update(&self) -> Result<(), DaemonError> {
        self.call_unit("ApplyUpdate", &())
    }

    fn list_drives(&self) -> Result<Vec<Drive>, DaemonError> {
        Err(DaemonError::unsupported(
            "USB drives arrive with the export service (WP6).",
        ))
    }

    fn export(
        &self,
        selection: Vec<String>,
        format: String,
        drive_id: String,
    ) -> Result<String, DaemonError> {
        self.block_on(async {
            let connection = self.connector.connection().await.map_err(map_zbus)?;
            let proxy = self.connector.proxy(&connection).await.map_err(map_zbus)?;
            let reply = proxy
                .call_method("Export", &(selection, format, drive_id))
                .await
                .map_err(map_zbus)?;
            reply.body().deserialize().map_err(map_zbus)
        })
    }

    fn import(&self, drive_id: String, paths: Vec<String>) -> Result<Vec<String>, DaemonError> {
        self.block_on(async {
            let connection = self.connector.connection().await.map_err(map_zbus)?;
            let proxy = self.connector.proxy(&connection).await.map_err(map_zbus)?;
            let reply = proxy
                .call_method("Import", &(drive_id, paths))
                .await
                .map_err(map_zbus)?;
            reply.body().deserialize().map_err(map_zbus)
        })
    }

    fn search_library(
        &self,
        query: &str,
        filters: HashMap<String, String>,
    ) -> Result<Vec<LibraryHit>, DaemonError> {
        self.block_on(async {
            let connection = self.connector.connection().await.map_err(map_zbus)?;
            let proxy = self.connector.proxy(&connection).await.map_err(map_zbus)?;
            let reply = proxy
                .call_method("SearchLibrary", &(query, filters))
                .await
                .map_err(map_zbus)?;
            reply.body().deserialize().map_err(map_zbus)
        })
    }

    fn library_filters(&self) -> Result<LibraryFilterOptions, DaemonError> {
        self.block_on(async {
            let connection = self.connector.connection().await.map_err(map_zbus)?;
            let proxy = self.connector.proxy(&connection).await.map_err(map_zbus)?;
            let reply = proxy
                .call_method("LibraryFilters", &())
                .await
                .map_err(map_zbus)?;
            reply.body().deserialize().map_err(map_zbus)
        })
    }

    fn subscribe(&self, listener: Box<dyn Fn(DaemonEvent) + Send + Sync + 'static>) {
        let listener: Arc<dyn Fn(DaemonEvent) + Send + Sync> = Arc::from(listener);
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.push(Arc::clone(&listener));
        }
        if self.subscribed.swap(true, Ordering::SeqCst) {
            return;
        }

        let connector = self.connector.clone();
        let listeners = Arc::clone(&self.listeners);
        connector.handle.clone().spawn(async move {
            let mut reported = false;
            loop {
                let problem = match run_signal_loop(&connector, &listeners).await {
                    Ok(()) => "the daemon signal stream ended".to_owned(),
                    Err(error) => format!("daemon signals unavailable: {error}"),
                };
                if !reported {
                    tracing_warn(&problem);
                    reported = true;
                }
                // The daemon may start after the shell (or be restarted by
                // systemd). Retrying quietly is cheap; the screens keep
                // working with the last state they have.
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });
    }
}

fn tracing_warn(message: &str) {
    eprintln!("stylos: {message}");
}

/// Forward signals until the connection or the stream ends.
async fn run_signal_loop(
    connector: &Connector,
    listeners: &ListenerList,
) -> Result<(), zbus::Error> {
    let connection = connector.connection().await?;
    let proxy = connector.proxy(&connection).await?;
    let mut signals = proxy.receive_all_signals().await?;
    while let Some(message) = signals.next().await {
        let member = match message.header().member() {
            Some(member) => member.as_str().to_owned(),
            None => continue,
        };
        let event = match member.as_str() {
            "BatteryChanged" => message
                .body()
                .deserialize::<BatteryStatus>()
                .ok()
                .map(DaemonEvent::Battery),
            "SyncStatus" => message
                .body()
                .deserialize::<SyncInfo>()
                .ok()
                .map(DaemonEvent::Sync),
            "PairStatus" => message
                .body()
                .deserialize::<(String, String, String)>()
                .ok()
                .map(|(state, code, message)| DaemonEvent::Pair {
                    state,
                    code,
                    message,
                }),
            _ => None,
        };
        if let Some(event) = event {
            if let Ok(listeners) = listeners.lock() {
                for listener in listeners.iter() {
                    listener(event.clone());
                }
            }
        }
    }
    Ok(())
}

fn map_zbus(error: impl fmt::Display) -> DaemonError {
    let message = format!("{error}");
    if message.contains("not implemented") || message.contains("NotSupported") {
        DaemonError::unsupported(message)
    } else if message.contains("login page") {
        DaemonError::captive_portal(message)
    } else if message.contains("connect to the system bus")
        || message.contains("I/O error")
        || message.contains("Connection refused")
        || message.contains("does not exist")
    {
        DaemonError::unavailable(format!("stylosd is not running. {message}"))
    } else {
        DaemonError::failed(message)
    }
}

// ---------------------------------------------------------------------------
// FakeDaemon: tests and screenshots.
// ---------------------------------------------------------------------------

struct FakeQuote {
    hit: LibraryHit,
    book_id: String,
    topics: Vec<String>,
}

struct FakeState {
    battery: BatteryStatus,
    sync: SyncInfo,
    versions: Vec<HistoryEntry>,
    preview: String,
    networks: Vec<WifiNetwork>,
    drives: Vec<Drive>,
    update: String,
    pending_pair: Option<PairingInfo>,
    committed: usize,
    unpaired: usize,
    exported: Vec<(Vec<String>, String, String)>,
    imported: Vec<(String, Vec<String>)>,
    quotes: Vec<FakeQuote>,
    library_unavailable: bool,
    search_filters: Vec<(String, HashMap<String, String>)>,
}

/// An in-memory daemon with plausible data.
///
/// Tests construct it directly (and can [`FakeDaemon::emit`] signals); the
/// screenshot script uses [`FakeDaemon::demo`], which reads a few environment
/// variables so every state can be photographed.
pub struct FakeDaemon {
    state: Mutex<FakeState>,
    listeners: Mutex<Vec<OwnedListener>>,
}

impl FakeDaemon {
    pub fn new() -> Self {
        let now = now_unix();
        let versions = vec![
            HistoryEntry {
                rev: "9f1c0a2".into(),
                time: now - 600,
                words: 1_204,
                delta: 86,
            },
            HistoryEntry {
                rev: "77b3e41".into(),
                time: now - 3_600,
                words: 1_118,
                delta: 132,
            },
            HistoryEntry {
                rev: "2ad8c05".into(),
                time: now - 29 * 3_600,
                words: 986,
                delta: -18,
            },
            HistoryEntry {
                rev: "5c40f7b".into(),
                time: now - 2 * 86_400,
                words: 1_004,
                delta: 1_004,
            },
        ];
        let preview = concat!(
            "The kettle clicks off at six. I write at the kitchen table while ",
            "the house is still asleep, and the first sentence arrives as if it ",
            "had been waiting under the floorboards.\n\n",
            "> A sentence is a small room. You walk in, close the door, and look around.\n\n",
            "Some mornings the words come slowly; other mornings they arrive all ",
            "at once, like rain on a tin roof."
        )
        .to_owned();
        Self {
            state: Mutex::new(FakeState {
                battery: BatteryStatus {
                    present: true,
                    percent: 76,
                    charging: false,
                },
                sync: SyncInfo {
                    state: "idle".into(),
                    message: "Already up to date.".into(),
                    last_sync: now - 3_600,
                },
                versions,
                preview,
                networks: vec![
                    WifiNetwork {
                        ssid: "Kitchen".into(),
                        strength: 92,
                        secured: true,
                        connected: true,
                    },
                    WifiNetwork {
                        ssid: "Studio 5G".into(),
                        strength: 74,
                        secured: true,
                        connected: false,
                    },
                    WifiNetwork {
                        ssid: "Hotel Guest".into(),
                        strength: 46,
                        secured: false,
                        connected: false,
                    },
                    WifiNetwork {
                        ssid: "Bibliothek".into(),
                        strength: 18,
                        secured: true,
                        connected: false,
                    },
                ],
                drives: vec![Drive {
                    id: "usb-1".into(),
                    label: "STYLOS USB".into(),
                    free_bytes: 12_400_000_000,
                }],
                update: "ready:0.2.0".into(),
                pending_pair: None,
                committed: 0,
                unpaired: 0,
                exported: Vec::new(),
                imported: Vec::new(),
                quotes: demo_quotes(),
                library_unavailable: false,
                search_filters: Vec::new(),
            }),
            listeners: Mutex::new(Vec::new()),
        }
    }

    /// The screenshot daemon: same data, adjusted by environment variables.
    /// `STYLOS_BATTERY=10` or `STYLOS_BATTERY=18,charging` changes the battery;
    /// `STYLOS_FAKE_NO_DRIVE=1` removes the USB stick.
    pub fn demo() -> Self {
        let fake = Self::new();
        if let Ok(value) = std::env::var("STYLOS_BATTERY") {
            let (percent, charging) = value.split_once(',').unwrap_or((&value, ""));
            if let Ok(percent) = percent.trim().parse::<u8>() {
                fake.set_battery(BatteryStatus {
                    present: true,
                    percent: percent.min(100),
                    charging: charging.trim() == "charging",
                });
            }
        }
        if std::env::var_os("STYLOS_FAKE_NO_DRIVE").is_some() {
            if let Ok(mut state) = fake.state.lock() {
                state.drives.clear();
            }
        }
        if std::env::var_os("STYLOS_FAKE_UNPAIRED").is_some() {
            if let Ok(mut state) = fake.state.lock() {
                state.sync = SyncInfo {
                    state: "idle".into(),
                    message: "Not synced yet.".into(),
                    last_sync: 0,
                };
            }
        }
        match std::env::var("STYLOS_FAKE_LIBRARY").as_deref() {
            Ok("unavailable") => fake.set_library_unavailable(true),
            Ok("empty") => fake.set_quotes(Vec::new()),
            _ => {}
        }
        fake
    }

    pub fn set_battery(&self, battery: BatteryStatus) {
        if let Ok(mut state) = self.state.lock() {
            state.battery = battery;
        }
    }

    pub fn set_sync(&self, sync: SyncInfo) {
        if let Ok(mut state) = self.state.lock() {
            state.sync = sync;
        }
    }

    pub fn set_versions(&self, versions: Vec<HistoryEntry>) {
        if let Ok(mut state) = self.state.lock() {
            state.versions = versions;
        }
    }

    pub fn set_networks(&self, networks: Vec<WifiNetwork>) {
        if let Ok(mut state) = self.state.lock() {
            state.networks = networks;
        }
    }

    pub fn set_drives(&self, drives: Vec<Drive>) {
        if let Ok(mut state) = self.state.lock() {
            state.drives = drives;
        }
    }

    pub fn set_update(&self, update: impl Into<String>) {
        if let Ok(mut state) = self.state.lock() {
            state.update = update.into();
        }
    }

    /// Replace the demo library. The screenshot script and the palette tests
    /// use the default set; an empty set exercises the first-sync message.
    pub fn set_quotes(&self, quotes: Vec<LibraryHit>) {
        if let Ok(mut state) = self.state.lock() {
            state.quotes = quotes
                .into_iter()
                .map(|hit| FakeQuote {
                    book_id: hit.book_title.clone(),
                    topics: Vec::new(),
                    hit,
                })
                .collect();
        }
    }

    fn set_library_unavailable(&self, unavailable: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.library_unavailable = unavailable;
        }
    }

    pub fn committed(&self) -> usize {
        self.state.lock().map(|state| state.committed).unwrap_or(0)
    }

    pub fn unpaired(&self) -> usize {
        self.state.lock().map(|state| state.unpaired).unwrap_or(0)
    }

    pub fn exported(&self) -> Vec<(Vec<String>, String, String)> {
        self.state
            .lock()
            .map(|state| state.exported.clone())
            .unwrap_or_default()
    }

    pub fn imported(&self) -> Vec<(String, Vec<String>)> {
        self.state
            .lock()
            .map(|state| state.imported.clone())
            .unwrap_or_default()
    }

    pub fn search_filters(&self) -> Vec<(String, HashMap<String, String>)> {
        self.state
            .lock()
            .map(|state| state.search_filters.clone())
            .unwrap_or_default()
    }

    /// Deliver a signal to every listener, as `stylosd` would.
    pub fn emit(&self, event: DaemonEvent) {
        if let Ok(listeners) = self.listeners.lock() {
            for listener in listeners.iter() {
                listener(event.clone());
            }
        }
    }

    pub fn approve_pairing(&self) {
        let code = self
            .state
            .lock()
            .ok()
            .and_then(|state| state.pending_pair.as_ref().map(|pair| pair.code.clone()))
            .unwrap_or_default();
        self.emit(DaemonEvent::Pair {
            state: "approved".into(),
            code,
            message: "Paired with Studio.".into(),
        });
    }
}

impl Default for FakeDaemon {
    fn default() -> Self {
        Self::new()
    }
}

impl DaemonClient for FakeDaemon {
    fn commit(&self) -> Result<(), DaemonError> {
        if let Ok(mut state) = self.state.lock() {
            state.committed += 1;
        }
        Ok(())
    }

    fn history(&self, _path: &str) -> Result<Vec<HistoryEntry>, DaemonError> {
        Ok(self
            .state
            .lock()
            .map(|state| state.versions.clone())
            .unwrap_or_default())
    }

    fn restore(&self, _path: &str, rev: &str) -> Result<String, DaemonError> {
        let state = self
            .state
            .lock()
            .map_err(|_| DaemonError::failed("fake lock"))?;
        if state.versions.iter().any(|version| version.rev == rev) {
            Ok(state.preview.clone())
        } else {
            Err(DaemonError::failed(format!("unknown revision {rev}")))
        }
    }

    fn sync(&self) -> Result<(), DaemonError> {
        let info = self
            .state
            .lock()
            .map(|state| state.sync.clone())
            .map_err(|_| DaemonError::failed("fake lock"))?;
        let running = SyncInfo {
            state: "syncing".into(),
            message: "Syncing…".into(),
            last_sync: info.last_sync,
        };
        self.set_sync(running.clone());
        self.emit(DaemonEvent::Sync(running));
        let done = SyncInfo {
            state: "idle".into(),
            message: "Synced: 2 pulled, 3 pushed, 0 conflicts.".into(),
            last_sync: now_unix(),
        };
        self.set_sync(done.clone());
        self.emit(DaemonEvent::Sync(done));
        Ok(())
    }

    fn sync_info(&self) -> Result<SyncInfo, DaemonError> {
        self.state
            .lock()
            .map(|state| state.sync.clone())
            .map_err(|_| DaemonError::failed("fake lock"))
    }

    fn pair(&self, backend: &str, url: &str) -> Result<PairingInfo, DaemonError> {
        let info = match backend {
            "nostos" => PairingInfo {
                backend: backend.into(),
                code: "K7Q-4MZ".into(),
                approval_url: "https://app.nostos.page/pair".into(),
                expires_at: now_unix() + 600,
            },
            "self-hosted" => PairingInfo {
                backend: backend.into(),
                code: "M2V-9RT".into(),
                approval_url: format!("{}/approve", url.trim_end_matches('/')),
                expires_at: now_unix() + 600,
            },
            "google-drive" => PairingInfo {
                backend: backend.into(),
                code: "HXQJ-7KLM".into(),
                approval_url: "https://www.google.com/device".into(),
                expires_at: now_unix() + 1_800,
            },
            other => return Err(DaemonError::failed(format!("unknown backend {other}"))),
        };
        if let Ok(mut state) = self.state.lock() {
            state.pending_pair = Some(info.clone());
        }
        Ok(info)
    }

    fn unpair(&self) -> Result<(), DaemonError> {
        if let Ok(mut state) = self.state.lock() {
            state.unpaired += 1;
            state.pending_pair = None;
        }
        Ok(())
    }

    fn wifi_scan(&self) -> Result<Vec<WifiNetwork>, DaemonError> {
        self.state
            .lock()
            .map(|state| state.networks.clone())
            .map_err(|_| DaemonError::failed("fake lock"))
    }

    fn wifi_connect(&self, ssid: &str, _password: &str) -> Result<(), DaemonError> {
        if ssid == "Hotel Guest" {
            return Err(DaemonError::captive_portal(
                "This network needs a login page. Use a phone hotspot.",
            ));
        }
        if let Ok(mut state) = self.state.lock() {
            for network in &mut state.networks {
                network.connected = network.ssid == ssid;
            }
        }
        Ok(())
    }

    fn wifi_forget(&self, ssid: &str) -> Result<(), DaemonError> {
        if let Ok(mut state) = self.state.lock() {
            state.networks.retain(|network| network.ssid != ssid);
        }
        Ok(())
    }

    fn battery(&self) -> Result<BatteryStatus, DaemonError> {
        self.state
            .lock()
            .map(|state| state.battery)
            .map_err(|_| DaemonError::failed("fake lock"))
    }

    fn update_status(&self) -> Result<String, DaemonError> {
        self.state
            .lock()
            .map(|state| state.update.clone())
            .map_err(|_| DaemonError::failed("fake lock"))
    }

    fn apply_update(&self) -> Result<(), DaemonError> {
        if let Ok(mut state) = self.state.lock() {
            state.update = "applying".into();
        }
        Ok(())
    }

    fn list_drives(&self) -> Result<Vec<Drive>, DaemonError> {
        self.state
            .lock()
            .map(|state| state.drives.clone())
            .map_err(|_| DaemonError::failed("fake lock"))
    }

    fn export(
        &self,
        selection: Vec<String>,
        format: String,
        drive_id: String,
    ) -> Result<String, DaemonError> {
        let folder = dated_folder(now_unix());
        if let Ok(mut state) = self.state.lock() {
            state
                .exported
                .push((selection.clone(), format.clone(), drive_id.clone()));
        }
        Ok(format!(
            "/media/stylos/{drive_id}/Stylos export {folder}/ ({format})"
        ))
    }

    fn import(&self, drive_id: String, paths: Vec<String>) -> Result<Vec<String>, DaemonError> {
        if let Ok(mut state) = self.state.lock() {
            state.imported.push((drive_id, paths.clone()));
        }
        Ok(paths)
    }

    fn search_library(
        &self,
        query: &str,
        filters: HashMap<String, String>,
    ) -> Result<Vec<LibraryHit>, DaemonError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| DaemonError::failed("fake lock"))?;
        state
            .search_filters
            .push((query.to_owned(), filters.clone()));
        if state.library_unavailable {
            return Err(DaemonError::failed("Quotes need a Nostos account."));
        }
        Ok(fake_search(&state.quotes, query, &filters))
    }

    fn library_filters(&self) -> Result<LibraryFilterOptions, DaemonError> {
        let state = self
            .state
            .lock()
            .map_err(|_| DaemonError::failed("fake lock"))?;
        if state.library_unavailable {
            return Err(DaemonError::failed("Quotes need a Nostos account."));
        }
        let mut books: Vec<LibraryBookOption> = Vec::new();
        let mut authors = std::collections::BTreeSet::new();
        let mut topics = std::collections::BTreeSet::new();
        for quote in &state.quotes {
            if !quote.hit.book_title.is_empty()
                && !books.iter().any(|book| book.id == quote.book_id)
            {
                books.push(LibraryBookOption {
                    id: quote.book_id.clone(),
                    title: quote.hit.book_title.clone(),
                    author: quote.hit.author.clone(),
                });
            }
            if !quote.hit.author.trim().is_empty() {
                authors.insert(quote.hit.author.clone());
            }
            for topic in &quote.topics {
                topics.insert(topic.clone());
            }
        }
        books.sort_by_key(|book| book.title.to_lowercase());
        Ok(LibraryFilterOptions {
            books,
            authors: authors.into_iter().collect(),
            topics: topics.into_iter().collect(),
        })
    }

    fn subscribe(&self, listener: Box<dyn Fn(DaemonEvent) + Send + Sync + 'static>) {
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.push(listener);
        }
    }
}

fn slug(title: &str) -> String {
    let mut slug = String::new();
    for character in title.chars() {
        if character.is_alphanumeric() {
            slug.extend(character.to_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

fn hit(
    id: &str,
    kind: &str,
    text: &str,
    book_title: &str,
    author: &str,
    location: &str,
    topics: &[&str],
) -> FakeQuote {
    FakeQuote {
        hit: LibraryHit {
            id: id.into(),
            kind: kind.into(),
            text: text.into(),
            book_title: book_title.into(),
            author: author.into(),
            location: location.into(),
            citation_markdown: format!("[{book_title}, {location}](nostos:cite/{id})"),
        },
        book_id: slug(book_title),
        topics: topics.iter().map(|topic| topic.to_string()).collect(),
    }
}

/// Plausible quotes for the palette, the screenshots and the shell tests.
fn demo_quotes() -> Vec<FakeQuote> {
    vec![
        hit(
            "quiet-hours-41",
            "highlight",
            "A sentence is a small room. You walk in, close the door, and look around.",
            "The Quiet Hours",
            "Mara Ellison",
            "p. 41",
            &["writing", "attention"],
        ),
        hit(
            "quiet-hours-88",
            "highlight",
            "Silence is not the absence of work; it is the room the work needs.",
            "The Quiet Hours",
            "Mara Ellison",
            "p. 88",
            &["attention", "silence"],
        ),
        hit(
            "quiet-hours-63",
            "note",
            "Write before the house wakes; the morning is the only quiet editor.",
            "The Quiet Hours",
            "Mara Ellison",
            "p. 63",
            &["writing", "habits"],
        ),
        hit(
            "on-attention-8",
            "highlight",
            "Attention is a door you can close.",
            "On Attention",
            "Iris Vale",
            "p. 8",
            &["attention"],
        ),
        hit(
            "on-attention-12",
            "passage",
            "The first draft is you telling yourself the story.",
            "On Attention",
            "Iris Vale",
            "p. 12",
            &["writing"],
        ),
        hit(
            "the-workshop-102",
            "passage",
            "We shape our tools and afterwards our tools shape us.",
            "The Workshop",
            "Nadia Brandt",
            "p. 102",
            &["tools"],
        ),
        hit(
            "rooms-for-writing-7",
            "highlight",
            "A room of one's own is not a luxury; it is a sentence in the world.",
            "Rooms for Writing",
            "Clara Wren",
            "p. 7",
            &["writing", "place"],
        ),
    ]
}

fn fake_search(
    quotes: &[FakeQuote],
    query: &str,
    filters: &HashMap<String, String>,
) -> Vec<LibraryHit> {
    let filter = |key: &str| {
        filters
            .get(key)
            .map(|value| value.trim().to_lowercase())
            .filter(|value| !value.is_empty())
    };
    let book = filter("book");
    let author = filter("author");
    let topic = filter("topic");
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in query.chars() {
        if character.is_alphanumeric() {
            current.extend(character.to_lowercase());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }

    quotes
        .iter()
        .filter(|quote| {
            if let Some(book) = &book {
                if quote.book_id.to_lowercase() != *book
                    && quote.hit.book_title.to_lowercase() != *book
                {
                    return false;
                }
            }
            if let Some(author) = &author {
                if quote.hit.author.to_lowercase() != *author {
                    return false;
                }
            }
            if let Some(topic) = &topic {
                if !quote
                    .topics
                    .iter()
                    .any(|value| value.to_lowercase() == *topic)
                {
                    return false;
                }
            }
            if tokens.is_empty() {
                return true;
            }
            let haystack = format!(
                "{} {} {} {}",
                quote.hit.text,
                quote.hit.book_title,
                quote.hit.author,
                quote.topics.join(" ")
            )
            .to_lowercase();
            tokens.iter().all(|token| haystack.contains(token))
        })
        .take(50)
        .map(|quote| quote.hit.clone())
        .collect()
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// `2026-11-12`, for the "Everything" export folder name.
pub fn dated_folder(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Howard Hinnant's civil calendar conversion, valid for every date Stylos
/// will ever see.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_daemon_has_a_version_history_with_deltas() {
        let fake = FakeDaemon::new();
        let versions = fake.history("draft.md").unwrap();
        assert!(versions.len() >= 3);
        assert!(versions.windows(2).all(|pair| pair[0].time > pair[1].time));
        assert!(fake
            .restore("draft.md", &versions[1].rev)
            .unwrap()
            .contains("sentence"));
        assert!(fake.restore("draft.md", "nope").is_err());
    }

    #[test]
    fn the_fake_daemon_emits_sync_progress() {
        let fake = Arc::new(FakeDaemon::new());
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        fake.subscribe(Box::new(move |event| {
            sink.lock().unwrap().push(event);
        }));
        fake.sync().unwrap();
        let events = events.lock().unwrap();
        assert!(matches!(&events[0], DaemonEvent::Sync(info) if info.state == "syncing"));
        assert!(matches!(&events[1], DaemonEvent::Sync(info) if info.state == "idle"));
        assert_eq!(fake.sync_info().unwrap().state, "idle");
    }

    #[test]
    fn pairing_codes_are_shown_for_every_backend() {
        let fake = FakeDaemon::new();
        for backend in Backend::ALL {
            let info = fake.pair(backend.wire(), "https://studio.local").unwrap();
            assert!(!info.code.is_empty());
            assert!(!info.approval_url.is_empty());
            assert_eq!(info.backend, backend.wire());
        }
        assert!(fake.pair("dropbox", "").is_err());
    }

    #[test]
    fn the_captive_portal_error_is_the_spec_sentence() {
        let fake = FakeDaemon::new();
        let error = fake.wifi_connect("Hotel Guest", "").unwrap_err();
        assert_eq!(error.kind, DaemonErrorKind::CaptivePortal);
        assert_eq!(
            error.message,
            "This network needs a login page. Use a phone hotspot."
        );
    }

    #[test]
    fn dated_folders_and_civil_dates() {
        // 2026-11-12 00:00:00 UTC.
        let unix = 1_763_000_000;
        let folder = dated_folder(unix);
        assert_eq!(folder.len(), 10);
        assert_eq!(&folder[4..5], "-");
        assert_eq!(dated_folder(0), "1970-01-01");
    }

    #[test]
    fn mirrored_types_have_the_daemons_dbus_signatures() {
        use zbus::zvariant::Type;
        assert_eq!(
            <HistoryEntry as Type>::SIGNATURE,
            <stylosd::history::HistoryEntry as Type>::SIGNATURE
        );
        assert_eq!(
            <WifiNetwork as Type>::SIGNATURE,
            <stylosd::network::WifiNetwork as Type>::SIGNATURE
        );
        assert_eq!(
            <BatteryStatus as Type>::SIGNATURE,
            <stylosd::power::BatteryStatus as Type>::SIGNATURE
        );
        assert_eq!(
            <SyncInfo as Type>::SIGNATURE,
            <stylosd::daemon::SyncInfo as Type>::SIGNATURE
        );
        assert_eq!(
            <PairingInfo as Type>::SIGNATURE,
            <stylosd::daemon::PairingInfo as Type>::SIGNATURE
        );
        assert_eq!(
            <LibraryHit as Type>::SIGNATURE,
            <stylosd::library::LibraryHit as Type>::SIGNATURE
        );
        assert_eq!(
            <LibraryFilterOptions as Type>::SIGNATURE,
            <stylosd::library::LibraryFilterOptions as Type>::SIGNATURE
        );
    }

    #[test]
    fn the_fake_library_searches_filters_and_explains_absence() {
        let fake = Arc::new(FakeDaemon::new());
        let all = fake.search_library("", HashMap::new()).unwrap();
        assert!(all.len() >= 5);
        assert_eq!(all[0].kind, "highlight");

        let hits = fake.search_library("attention", HashMap::new()).unwrap();
        assert!(hits.iter().any(|hit| hit.id == "on-attention-8"));
        assert!(hits.iter().any(|hit| hit.id == "quiet-hours-88"));

        let mut filters = HashMap::new();
        filters.insert("book".to_string(), "the-quiet-hours".to_string());
        let hits = fake.search_library("", filters).unwrap();
        assert_eq!(hits.len(), 3);
        assert!(hits.iter().all(|hit| hit.book_title == "The Quiet Hours"));

        let options = fake.library_filters().unwrap();
        assert!(options.books.len() >= 4);
        assert!(options.authors.contains(&"Iris Vale".to_string()));
        assert!(options.topics.contains(&"attention".to_string()));

        fake.set_library_unavailable(true);
        let error = fake.search_library("sentence", HashMap::new()).unwrap_err();
        assert_eq!(error.message, "Quotes need a Nostos account.");
        assert!(fake.library_filters().is_err());
    }

    #[test]
    fn the_demo_battery_can_be_forced_for_screenshots() {
        std::env::set_var("STYLOS_BATTERY", "10");
        let fake = FakeDaemon::demo();
        assert_eq!(fake.battery().unwrap().percent, 10);
        assert!(!fake.battery().unwrap().charging);
        std::env::remove_var("STYLOS_BATTERY");
    }
}
