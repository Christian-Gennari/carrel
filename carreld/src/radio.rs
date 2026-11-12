//! Radio control: Wi-Fi is blocked (rfkill) unless something explicitly turns
//! it on.
//!
//! The daemon unblocks the radio for exactly three things: a sync, pairing and
//! the Wi-Fi screen. Syncs and pairing hold a [`RadioGuard`], which blocks the
//! radio again when it is dropped — including when the operation returns an
//! error or the task panics. The Wi-Fi screen opens a two-minute window that
//! every Wi-Fi call extends and a sweeper closes again.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// The Wi-Fi screen keeps the radio on for this long after the last call.
pub const WIFI_WINDOW: Duration = Duration::from_secs(120);

/// The one thing the radio manager needs from the kernel.
///
/// A trait keeps the policy (when is the radio on?) testable with fakes; the
/// device itself is `/sys/class/rfkill` in production.
pub trait Rfkill: Send + Sync {
    fn block_wifi(&self) -> Result<()>;
    fn unblock_wifi(&self) -> Result<()>;
    fn is_blocked(&self) -> Result<bool>;
}

/// `rfkill` through sysfs: every `/sys/class/rfkill/rfkill<N>` entry whose
/// `type` is `wlan` is a Wi-Fi radio. Writing `soft` blocks or unblocks it.
pub struct SysfsRfkill {
    root: PathBuf,
}

impl SysfsRfkill {
    /// Use `$CARREL_RFKILL_DIR` (tests, containers) or `/sys/class/rfkill`.
    pub fn new() -> Self {
        let root = std::env::var_os("CARREL_RFKILL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/sys/class/rfkill"));
        Self::at(root)
    }

    /// Read radios from an explicit directory.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn wifi_soft_paths(&self) -> Result<Vec<PathBuf>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            // No rfkill at all (a container, or a machine without a radio) is
            // not an error: there is nothing to block.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read rfkill directory {}", self.root.display()))
            }
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let kind = match fs::read_to_string(path.join("type")) {
                Ok(kind) => kind,
                Err(_) => continue,
            };
            if kind.trim() == "wlan" {
                paths.push(path.join("soft"));
            }
        }
        Ok(paths)
    }

    fn write_soft(&self, value: &str) -> Result<()> {
        let paths = self.wifi_soft_paths()?;
        let mut first_error = None;
        for path in paths {
            // Try every radio even if one fails, then report the first error.
            let result = (|| -> std::io::Result<()> {
                let mut file = fs::OpenOptions::new().write(true).open(&path)?;
                file.write_all(value.as_bytes())
            })();
            if let Err(error) = result {
                if first_error.is_none() {
                    first_error = Some(anyhow::Error::new(error).context(format!(
                        "write {} to {}",
                        value,
                        path.display()
                    )));
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Default for SysfsRfkill {
    fn default() -> Self {
        Self::new()
    }
}

impl Rfkill for SysfsRfkill {
    fn block_wifi(&self) -> Result<()> {
        self.write_soft("1")
    }

    fn unblock_wifi(&self) -> Result<()> {
        self.write_soft("0")
    }

    fn is_blocked(&self) -> Result<bool> {
        let paths = self.wifi_soft_paths()?;
        let mut all_blocked = true;
        for path in paths {
            match fs::read_to_string(&path) {
                Ok(value) if value.trim() == "1" => {}
                Ok(_) => all_blocked = false,
                Err(error) => {
                    return Err(error).with_context(|| format!("read {}", path.display()));
                }
            }
        }
        // No radio at all counts as blocked: there is nothing that could
        // connect.
        Ok(all_blocked)
    }
}

struct RadioState {
    holders: usize,
    window_until: Option<Instant>,
}

impl RadioState {
    fn window_open(&self, now: Instant) -> bool {
        self.window_until.is_some_and(|until| until > now)
    }
}

/// Decides when the Wi-Fi radio is on. Always starts blocked: the radio is
/// blocked at boot (WP1) and the daemon blocks it again when it starts.
pub struct RadioManager {
    device: Arc<dyn Rfkill>,
    state: Mutex<RadioState>,
}

impl RadioManager {
    pub fn new(device: Arc<dyn Rfkill>) -> Self {
        Self {
            device,
            state: Mutex::new(RadioState {
                holders: 0,
                window_until: None,
            }),
        }
    }

    /// Turn the radio on for the duration of the returned guard: a sync or a
    /// pairing. Dropping the guard (normal return, error or panic) blocks the
    /// radio again unless the Wi-Fi screen window is open.
    pub fn hold(self: &Arc<Self>, reason: &'static str) -> Result<RadioGuard> {
        let mut state = self.state.lock().expect("radio state poisoned");
        if state.holders == 0 && !state.window_open(Instant::now()) {
            self.device
                .unblock_wifi()
                .context("unblock the Wi-Fi radio")?;
        }
        state.holders += 1;
        Ok(RadioGuard {
            manager: Arc::clone(self),
            reason,
        })
    }

    /// Open (or extend) the Wi-Fi screen's two-minute window.
    pub fn open_window(&self, duration: Duration) -> Result<()> {
        let mut state = self.state.lock().expect("radio state poisoned");
        let now = Instant::now();
        if state.holders == 0 && !state.window_open(now) {
            self.device
                .unblock_wifi()
                .context("unblock the Wi-Fi radio")?;
        }
        state.window_until = Some(now + duration);
        Ok(())
    }

    /// Close the Wi-Fi window when it has expired and no one holds the radio.
    /// Called by a sweeper task and by tests, so no timer is needed here.
    pub fn expire_window(&self) {
        let mut state = self.state.lock().expect("radio state poisoned");
        let now = Instant::now();
        let expired = match state.window_until {
            Some(until) if until > now => return,
            Some(_) => {
                state.window_until = None;
                true
            }
            None => false,
        };
        if expired && state.holders == 0 {
            if let Err(error) = self.device.block_wifi() {
                tracing::error!(%error, "could not block the Wi-Fi radio");
            }
        }
    }

    /// Block the radio no matter what. Used at startup and on shutdown.
    pub fn force_block(&self) -> Result<()> {
        let mut state = self.state.lock().expect("radio state poisoned");
        state.window_until = None;
        state.holders = 0;
        self.device.block_wifi().context("block the Wi-Fi radio")
    }

    /// True while a sync/pairing holds the radio or the Wi-Fi window is open.
    pub fn is_on(&self) -> bool {
        let state = self.state.lock().expect("radio state poisoned");
        state.holders > 0 || state.window_open(Instant::now())
    }

    fn release(&self, reason: &'static str) {
        let mut state = self.state.lock().expect("radio state poisoned");
        state.holders = state.holders.saturating_sub(1);
        if state.holders == 0 && !state.window_open(Instant::now()) {
            state.window_until = None;
            if let Err(error) = self.device.block_wifi() {
                tracing::error!(%error, reason, "could not block the Wi-Fi radio");
            }
        }
    }
}

/// Keeps the radio on until dropped.
pub struct RadioGuard {
    manager: Arc<RadioManager>,
    reason: &'static str,
}

impl RadioGuard {
    /// For tests that want to release the radio early.
    pub fn release(self) {
        drop(self);
    }
}

impl Drop for RadioGuard {
    fn drop(&mut self) {
        tracing::debug!(reason = self.reason, "releasing the Wi-Fi radio");
        self.manager.release(self.reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRfkill {
        calls: Mutex<Vec<&'static str>>,
        blocked: AtomicBool,
        fail_unblock: AtomicBool,
        fail_block: AtomicBool,
    }

    impl FakeRfkill {
        fn new() -> Arc<Self> {
            let fake = Self {
                blocked: AtomicBool::new(true),
                ..Self::default()
            };
            Arc::new(fake)
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }

        fn is_blocked(&self) -> bool {
            self.blocked.load(Ordering::SeqCst)
        }
    }

    impl Rfkill for FakeRfkill {
        fn block_wifi(&self) -> Result<()> {
            self.calls.lock().unwrap().push("block");
            if self.fail_block.load(Ordering::SeqCst) {
                bail!("block failed");
            }
            self.blocked.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn unblock_wifi(&self) -> Result<()> {
            self.calls.lock().unwrap().push("unblock");
            if self.fail_unblock.load(Ordering::SeqCst) {
                bail!("unblock failed");
            }
            self.blocked.store(false, Ordering::SeqCst);
            Ok(())
        }

        fn is_blocked(&self) -> Result<bool> {
            Ok(self.blocked.load(Ordering::SeqCst))
        }
    }

    fn manager() -> (Arc<RadioManager>, Arc<FakeRfkill>) {
        let fake = FakeRfkill::new();
        (Arc::new(RadioManager::new(fake.clone())), fake)
    }

    #[test]
    fn radio_is_blocked_until_a_guard_is_taken() {
        let (manager, fake) = manager();
        assert!(!manager.is_on());
        {
            let _guard = manager.hold("sync").unwrap();
            assert!(manager.is_on());
            assert!(!fake.is_blocked());
        }
        assert!(!manager.is_on());
        assert!(fake.is_blocked());
        assert_eq!(fake.calls(), ["unblock", "block"]);
    }

    #[test]
    fn radio_is_blocked_after_an_error() {
        let (manager, fake) = manager();
        let result: Result<()> = (|| {
            let _guard = manager.hold("sync")?;
            bail!("network went away");
        })();
        assert!(result.is_err());
        assert!(fake.is_blocked());
        assert_eq!(fake.calls(), ["unblock", "block"]);
    }

    #[test]
    fn radio_is_blocked_when_the_task_panics() {
        let (manager, fake) = manager();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = manager.hold("sync").unwrap();
            panic!("boom");
        }));
        assert!(result.is_err());
        assert!(fake.is_blocked());
        assert_eq!(fake.calls(), ["unblock", "block"]);
    }

    #[test]
    fn a_failed_unblock_leaves_the_radio_blocked() {
        let (manager, fake) = manager();
        fake.fail_unblock.store(true, Ordering::SeqCst);
        assert!(manager.hold("sync").is_err());
        assert!(fake.is_blocked());
        assert_eq!(fake.calls(), ["unblock"]);
    }

    #[test]
    fn a_failed_block_after_a_guard_is_only_logged() {
        let (manager, fake) = manager();
        {
            let _guard = manager.hold("sync").unwrap();
            fake.fail_block.store(true, Ordering::SeqCst);
        }
        // The fake stays "unblocked" but the guard did not panic; the next
        // block attempt (for example at startup) can retry.
        assert_eq!(fake.calls(), ["unblock", "block"]);
    }

    #[test]
    fn the_wifi_window_blocks_the_radio_when_it_expires() {
        let (manager, fake) = manager();
        manager.open_window(Duration::from_millis(50)).unwrap();
        assert!(manager.is_on());
        assert!(!fake.is_blocked());
        std::thread::sleep(Duration::from_millis(70));
        manager.expire_window();
        assert!(!manager.is_on());
        assert!(fake.is_blocked());
        assert_eq!(fake.calls(), ["unblock", "block"]);
    }

    #[test]
    fn wifi_calls_extend_the_window() {
        let (manager, fake) = manager();
        manager.open_window(Duration::from_millis(80)).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        manager.open_window(Duration::from_millis(80)).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        manager.expire_window();
        assert!(manager.is_on());
        assert!(!fake.is_blocked());
        assert_eq!(fake.calls(), ["unblock"]);
    }

    #[test]
    fn a_guard_does_not_block_while_the_wifi_window_is_open() {
        let (manager, fake) = manager();
        manager.open_window(WIFI_WINDOW).unwrap();
        {
            let _guard = manager.hold("sync").unwrap();
            assert!(!fake.is_blocked());
        }
        assert!(!fake.is_blocked());
        manager.expire_window();
        // The window is still open, so nothing to do.
        assert!(!fake.is_blocked());
        assert_eq!(fake.calls(), ["unblock"]);
    }

    #[test]
    fn sweeping_without_a_window_does_not_touch_the_radio() {
        let (manager, fake) = manager();
        manager.expire_window();
        manager.expire_window();
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn force_block_overrides_the_window() {
        let (manager, fake) = manager();
        manager.open_window(WIFI_WINDOW).unwrap();
        manager.force_block().unwrap();
        assert!(fake.is_blocked());
        assert!(!manager.is_on());
        assert_eq!(fake.calls(), ["unblock", "block"]);
    }

    #[test]
    fn sysfs_blocks_only_wifi_radios() {
        let dir = tempfile::tempdir().unwrap();
        let wifi = dir.path().join("rfkill0");
        std::fs::create_dir(&wifi).unwrap();
        std::fs::write(wifi.join("type"), "wlan\n").unwrap();
        std::fs::write(wifi.join("soft"), "0").unwrap();
        let bluetooth = dir.path().join("rfkill1");
        std::fs::create_dir(&bluetooth).unwrap();
        std::fs::write(bluetooth.join("type"), "bluetooth\n").unwrap();
        std::fs::write(bluetooth.join("soft"), "0").unwrap();

        let rfkill = SysfsRfkill::at(dir.path());
        assert!(!rfkill.is_blocked().unwrap());
        rfkill.block_wifi().unwrap();
        assert_eq!(std::fs::read_to_string(wifi.join("soft")).unwrap(), "1");
        assert_eq!(
            std::fs::read_to_string(bluetooth.join("soft")).unwrap(),
            "0"
        );
        assert!(rfkill.is_blocked().unwrap());
        rfkill.unblock_wifi().unwrap();
        assert_eq!(std::fs::read_to_string(wifi.join("soft")).unwrap(), "0");
    }

    #[test]
    fn sysfs_without_radios_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let rfkill = SysfsRfkill::at(dir.path().join("missing"));
        rfkill.block_wifi().unwrap();
        rfkill.unblock_wifi().unwrap();
        assert!(rfkill.is_blocked().unwrap());
    }
}
