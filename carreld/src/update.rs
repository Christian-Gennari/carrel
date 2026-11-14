//! Signed online updates.
//!
//! `systemd-sysupdate` downloads a release (root, verity hash, verity
//! signature, boot entry, and the kernel and initrds for a kernel change)
//! from the URL baked into the image.  The manifest `SHA256SUMS` is verified
//! against the public keyring installed in the image
//! (`/usr/lib/systemd/import-pubring.gpg`).  The kernel files are versioned
//! by the kernel version, so they are a second sysupdate pass.
//!
//! Network access is never opened here: the daemon calls
//! [`Updater::check_and_stage`] from inside an existing sync session's radio
//! guard, and the manual `carreld update` command opens the same kind of
//! window.  `ApplyUpdate` only moves the already verified payload into place
//! and starts `carrel-update.service`, which applies it and reboots.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Where mkosi.build writes the release URL configured at build time.
pub const UPDATE_URL_FILE: &str = "/usr/lib/carrel/update-url";
/// systemd's updater (not in $PATH on Debian).
pub const SYSUPDATE: &str = "/usr/lib/systemd/systemd-sysupdate";
const STATUS_FILE: &str = "status.json";
const READY_FILE: &str = "ready";

/// The update state as `UpdateStatus()` returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateState {
    /// `none`, `available`, `downloading`, `ready` or `error`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self::none()
    }
}

impl UpdateState {
    pub fn none() -> Self {
        Self {
            state: "none".into(),
            version: None,
            percent: None,
            message: None,
        }
    }

    pub fn available(version: impl Into<String>) -> Self {
        Self {
            state: "available".into(),
            version: Some(version.into()),
            percent: None,
            message: None,
        }
    }

    pub fn downloading(percent: u8) -> Self {
        Self {
            state: "downloading".into(),
            version: None,
            percent: Some(percent.min(100)),
            message: None,
        }
    }

    pub fn ready(version: impl Into<String>) -> Self {
        Self {
            state: "ready".into(),
            version: Some(version.into()),
            percent: None,
            message: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            state: "error".into(),
            version: None,
            percent: None,
            message: Some(message.into()),
        }
    }

    /// The one-line string the shell's update row shows.
    pub fn wire(&self) -> String {
        match self.state.as_str() {
            "available" => format!("available {}", self.version.as_deref().unwrap_or("")),
            "downloading" => format!("downloading {}", self.percent.unwrap_or(0)),
            "ready" => format!("ready {}", self.version.as_deref().unwrap_or("")),
            "error" => format!("error {}", self.message.as_deref().unwrap_or("")),
            _ => "none".to_owned(),
        }
        .trim_end()
        .to_owned()
    }
}

/// Paths and settings for the updater, kept together so tests can point it at
/// a temporary directory and a local release server.
#[derive(Debug, Clone)]
pub struct Updater {
    dir: PathBuf,
    url: Option<String>,
    sysupdate: PathBuf,
    systemctl: PathBuf,
    slot: Option<char>,
}

impl Updater {
    /// Build the updater for a daemon state directory (usually
    /// `/data/.carrel`).  Environment variables make it testable.
    pub fn new(state_dir: &Path) -> Self {
        let dir = std::env::var_os("CARREL_UPDATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| state_dir.join("update"));
        let url = std::env::var("CARREL_UPDATE_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(read_update_url);
        let sysupdate = std::env::var_os("CARREL_SYSUPDATE_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(SYSUPDATE));
        let systemctl = std::env::var_os("CARREL_SYSTEMCTL_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/usr/bin/systemctl"));
        let slot = std::env::var("CARREL_SLOT")
            .ok()
            .and_then(|value| value.chars().next())
            .or_else(current_slot);
        Self {
            dir,
            url,
            sysupdate,
            systemctl,
            slot,
        }
    }

    /// Override the build-time release URL (manual checks and tests).
    pub fn with_url(mut self, url: Option<String>) -> Self {
        if let Some(url) = url.filter(|value| !value.trim().is_empty()) {
            self.url = Some(url);
        }
        self
    }

    pub fn status(&self) -> UpdateState {
        fs::read_to_string(self.dir.join(STATUS_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn write_status(&self, state: &UpdateState) -> Result<()> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("create update directory {}", self.dir.display()))?;
        let path = self.dir.join(STATUS_FILE);
        let temporary = path.with_extension("json.tmp");
        let text = serde_json::to_vec(state)?;
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&text)?;
        file.sync_all()?;
        fs::rename(&temporary, &path)?;
        Ok(())
    }

    /// Check the release URL and download the newest release into the staging
    /// directory.  This performs network I/O and must only be called while the
    /// radio is on for a sync (or the manual `carreld update` command).
    pub fn check_and_stage(&self) -> Result<UpdateState> {
        let Some(url) = self.url.clone() else {
            return Ok(self.status());
        };
        let slot = self.slot.context("cannot determine the running slot")?;
        let other = if slot == 'a' { 'b' } else { 'a' };
        let staged = self.dir.join("staged");
        fs::create_dir_all(&staged)
            .with_context(|| format!("create staging directory {}", staged.display()))?;
        let definitions = self.write_definitions(&url, &staged, other)?;

        let check = Command::new(&self.sysupdate)
            .arg(format!("--definitions={}", definitions.display()))
            .arg("check-new")
            .output()
            .with_context(|| format!("run {}", self.sysupdate.display()))?;
        if !check.status.success() {
            let message = stderr_tail(&check.stderr);
            if check.status.code() == Some(1) && !looks_like_update_error(&message) {
                // No newer version.  Keep a previously staged update ready.
                let current = self.status();
                if current.state == "ready" {
                    return Ok(current);
                }
                let none = UpdateState::none();
                self.write_status(&none)?;
                return Ok(none);
            }
            let state = UpdateState::error(message.clone());
            self.write_status(&state)?;
            bail!("checking for updates failed: {message}");
        }
        let version = String::from_utf8_lossy(&check.stdout)
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .to_owned();
        if version.is_empty() {
            let none = UpdateState::none();
            self.write_status(&none)?;
            return Ok(none);
        }

        let available = UpdateState::available(&version);
        self.write_status(&available)?;

        // Remove files from an older staged version so the progress count and
        // the eventual payload only contain this release.
        remove_staged_files(&staged, &version)?;
        self.run_update(&definitions, &staged, 4, "sysupdate.log")?;

        // The kernel and initrds are versioned by the kernel, not the
        // release, so they are a second sysupdate pass.  A release without
        // them (up to 0.2.2) makes this a no-op.
        let kernel_definitions = self.write_kernel_definitions(&url, &staged)?;
        self.run_update(&kernel_definitions, &staged, 5, "sysupdate-kernel.log")?;

        let ready = UpdateState::ready(&version);
        fs::write(self.dir.join(READY_FILE), format!("{version}\n"))?;
        self.write_status(&ready)?;
        Ok(ready)
    }

    /// Run one sysupdate pass and report the staged-file percentage until it
    /// exits.  `expected` is the number of files the pass can leave in the
    /// staging directory (four release files plus the kernel archive).
    fn run_update(
        &self,
        definitions: &Path,
        staged: &Path,
        expected: u8,
        log_name: &str,
    ) -> Result<()> {
        let log_path = self.dir.join(log_name);
        let log = fs::File::create(&log_path)?;
        let mut child = Command::new(&self.sysupdate)
            .arg(format!("--definitions={}", definitions.display()))
            .arg("update")
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()
            .with_context(|| format!("run {}", self.sysupdate.display()))?;

        let mut percent = 0;
        loop {
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    let message = read_tail(&log_path, 400);
                    let state = UpdateState::error(message.clone());
                    self.write_status(&state)?;
                    bail!("downloading update failed: {message}");
                }
                break;
            }
            let now = staged_percent(staged, expected);
            if now != percent {
                percent = now;
                self.write_status(&UpdateState::downloading(percent))?;
            }
            sleep(Duration::from_millis(500));
        }
        Ok(())
    }

    fn write_definitions(&self, url: &str, staged: &Path, slot: char) -> Result<PathBuf> {
        let definitions = self.dir.join("online.d");
        fs::create_dir_all(&definitions)?;
        let url = url.trim_end_matches('/');
        for (name, pattern) in [
            ("10-root", "carrel-root-@v.raw".to_owned()),
            ("20-verity", "carrel-verity-@v.raw".to_owned()),
            ("30-sig", "carrel-verity-sig-@v.sig".to_owned()),
            ("40-entry", format!("carrel-{slot}-@v.conf")),
        ] {
            let text = format!(
                "[Transfer]\n\
                 InstancesMax=1\n\n\
                 [Source]\n\
                 Type=url-file\n\
                 Path={url}\n\
                 MatchPattern={pattern}\n\n\
                 [Target]\n\
                 Type=regular-file\n\
                 Path={}\n\
                 MatchPattern={pattern}\n",
                staged.display(),
            );
            fs::write(definitions.join(format!("{name}.transfer")), text)?;
        }
        Ok(definitions)
    }

    /// The kernel and initrds are versioned by the kernel version, not the
    /// release version, so they need their own sysupdate pass (all transfers
    /// in one directory must offer the same @v).  They travel as one tar
    /// archive so the ESP gets a single /carrel/<kernel-version>/ directory.
    fn write_kernel_definitions(&self, url: &str, staged: &Path) -> Result<PathBuf> {
        let definitions = self.dir.join("online-kernel.d");
        fs::create_dir_all(&definitions)?;
        let url = url.trim_end_matches('/');
        let pattern = "carrel-kernel-@v.tar";
        let text = format!(
            "[Transfer]\n\
             InstancesMax=1\n\n\
             [Source]\n\
             Type=url-file\n\
             Path={url}\n\
             MatchPattern={pattern}\n\n\
             [Target]\n\
             Type=regular-file\n\
             Path={}\n\
             MatchPattern={pattern}\n",
            staged.display(),
        );
        fs::write(definitions.join("10-kernel.transfer"), text)?;
        Ok(definitions)
    }

    /// Move a fully verified staged release to the payload directory and let
    /// the existing `carrel-update.service` apply it and reboot.
    pub fn apply(&self) -> Result<()> {
        let state = self.status();
        if state.state != "ready" || !self.dir.join(READY_FILE).is_file() {
            bail!("no update is ready; start a sync to check for updates");
        }
        let staged = self.dir.join("staged");
        let payload = self.dir.join("payload");
        if !staged.is_dir() {
            bail!("the staged update is missing; start a sync to download it again");
        }
        if payload.exists() {
            fs::remove_dir_all(&payload)
                .with_context(|| format!("remove old payload {}", payload.display()))?;
        }
        fs::rename(&staged, &payload)
            .with_context(|| format!("stage the update at {}", payload.display()))?;
        let status = Command::new(&self.systemctl)
            .args(["start", "--no-block", "carrel-update.service"])
            .status()
            .with_context(|| {
                format!(
                    "start carrel-update.service with {}",
                    self.systemctl.display()
                )
            })?;
        if !status.success() {
            bail!("carrel-update.service could not be started");
        }
        Ok(())
    }
}

fn read_update_url() -> Option<String> {
    fs::read_to_string(UPDATE_URL_FILE)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn current_slot() -> Option<char> {
    let cmdline = fs::read_to_string("/proc/cmdline").ok()?;
    cmdline
        .split_whitespace()
        .find_map(|word| word.strip_prefix("carrel.slot="))
        .and_then(|value| value.chars().next())
}

fn remove_staged_files(staged: &Path, version: &str) -> Result<()> {
    for entry in fs::read_dir(staged)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(".#") || name == "sysupdate.log" {
            continue;
        }
        if !name.contains(version) {
            if entry.path().is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}

fn staged_percent(staged: &Path, expected: u8) -> u8 {
    let mut files = 0u8;
    if let Ok(entries) = fs::read_dir(staged) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with(".#") {
                files = files.saturating_add(1);
            }
        }
    }
    if expected == 0 {
        return 0;
    }
    files.min(expected) * 100 / expected
}

fn looks_like_update_error(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "failed",
        "error",
        "bad signature",
        "cannot verify",
        "not found",
        "404",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(3)..].join(" ")
}

fn read_tail(path: &Path, max: usize) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    let text = text.trim();
    if text.len() <= max {
        return text.to_owned();
    }
    text[text.len() - max..].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn wire_format_matches_the_shell_contract() {
        assert_eq!(UpdateState::none().wire(), "none");
        assert_eq!(UpdateState::available("0.2.0").wire(), "available 0.2.0");
        assert_eq!(UpdateState::downloading(42).wire(), "downloading 42");
        assert_eq!(UpdateState::ready("0.2.0").wire(), "ready 0.2.0");
        assert_eq!(
            UpdateState::error("bad signature").wire(),
            "error bad signature"
        );
    }

    #[test]
    fn status_survives_a_restart() {
        let temp = TempDir::new().unwrap();
        let updater = Updater {
            dir: temp.path().join("update"),
            url: None,
            sysupdate: PathBuf::from("/bin/false"),
            systemctl: PathBuf::from("/bin/false"),
            slot: Some('a'),
        };
        updater.write_status(&UpdateState::ready("0.2.0")).unwrap();
        assert_eq!(updater.status(), UpdateState::ready("0.2.0"));
    }

    #[test]
    fn apply_refuses_when_nothing_is_ready() {
        let temp = TempDir::new().unwrap();
        let updater = Updater {
            dir: temp.path().join("update"),
            url: None,
            sysupdate: PathBuf::from("/bin/false"),
            systemctl: PathBuf::from("/bin/false"),
            slot: Some('a'),
        };
        let error = updater.apply().unwrap_err().to_string();
        assert!(error.contains("no update is ready"), "{error}");
    }

    #[test]
    fn apply_moves_the_staged_payload_and_starts_the_service() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path().join("update");
        let staged = dir.join("staged");
        fs::create_dir_all(&staged).unwrap();
        fs::write(staged.join("carrel-root-b-0.2.0.raw"), b"root").unwrap();
        fs::write(dir.join(READY_FILE), "0.2.0\n").unwrap();
        let marker = temp.path().join("systemctl-called");
        let script = temp.path().join("systemctl");
        fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        let updater = Updater {
            dir: dir.clone(),
            url: None,
            sysupdate: PathBuf::from("/bin/false"),
            systemctl: script,
            slot: Some('a'),
        };
        updater.write_status(&UpdateState::ready("0.2.0")).unwrap();
        updater.apply().unwrap();
        assert!(dir.join("payload/carrel-root-b-0.2.0.raw").is_file());
        assert!(!staged.exists());
        assert!(marker.is_file());
    }
}
