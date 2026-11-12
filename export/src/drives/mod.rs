//! Removable-drive helpers for the Export screen.
//!
//! The caller decides when to mount: [`Drives::list`] only lists, [`Drives::mount`]
//! mounts one drive with [`MOUNT_OPTIONS`], and the daemon unmounts and powers
//! the drive off when the export has finished ("Safe to remove").

mod fake;
mod udisks;

pub use fake::{DriveCall, FakeDrives};
pub use udisks::UDisksDrives;

use std::path::{Path, PathBuf};

use crate::Result;

/// Mount options for every drive the daemon mounts (`spec/01-architecture.md`):
/// nothing on the stick may execute, setuid or create device nodes.
pub const MOUNT_OPTIONS: &str = "noexec,nosuid,nodev";

/// A mounted or mountable removable drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveInfo {
    /// Opaque id passed back to [`Drives::mount`]. The udisks implementation
    /// uses the D-Bus block-device object path.
    pub id: String,
    /// Label, else vendor/model, else the device node.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Current mount points, if the drive is already mounted.
    pub mount_points: Vec<PathBuf>,
}

impl DriveInfo {
    pub fn is_mounted(&self) -> bool {
        !self.mount_points.is_empty()
    }

    pub fn mount_point(&self) -> Option<&Path> {
        self.mount_points.first().map(PathBuf::as_path)
    }
}

/// The two implementations are the real udisks2 D-Bus API and [`FakeDrives`]
/// for tests.
pub trait Drives: Send + Sync {
    /// Removable drives currently attached. Listing never mounts anything.
    fn list(&self) -> Result<Vec<DriveInfo>>;

    /// Mount one drive with `noexec,nosuid,nodev` and return its mount point.
    fn mount(&self, id: &str) -> Result<PathBuf>;

    /// Unmount a mounted drive.
    fn unmount(&self, id: &str) -> Result<()>;

    /// Power the drive off after unmounting, so the stick is safe to remove.
    fn power_off(&self, id: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_options_forbid_execution_and_devices() {
        assert_eq!(MOUNT_OPTIONS, "noexec,nosuid,nodev");
    }

    #[test]
    fn drive_info_reports_its_mount_point() {
        let drive = DriveInfo {
            id: "id".into(),
            name: "USB stick".into(),
            size: 16,
            mount_points: vec![PathBuf::from("/media/writer/STICK")],
        };
        assert!(drive.is_mounted());
        assert_eq!(drive.mount_point(), Some(Path::new("/media/writer/STICK")));

        let unmounted = DriveInfo {
            mount_points: Vec::new(),
            ..drive
        };
        assert!(!unmounted.is_mounted());
        assert_eq!(unmounted.mount_point(), None);
    }
}
