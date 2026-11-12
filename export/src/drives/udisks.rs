//! udisks2 implementation of [`Drives`] over the system D-Bus.

use std::collections::HashMap;
use std::path::PathBuf;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, Value};

use super::{DriveInfo, Drives, MOUNT_OPTIONS};
use crate::{Error, Result};

const SERVICE: &str = "org.freedesktop.UDisks2";
const MANAGER_PATH: &str = "/org/freedesktop/UDisks2";
const MANAGER_INTERFACE: &str = "org.freedesktop.UDisks2.Manager";
const BLOCK_INTERFACE: &str = "org.freedesktop.UDisks2.Block";
const FILESYSTEM_INTERFACE: &str = "org.freedesktop.UDisks2.Filesystem";
const DRIVE_INTERFACE: &str = "org.freedesktop.UDisks2.Drive";

/// Lists, mounts and powers off removable drives through udisks2.
#[derive(Debug)]
pub struct UDisksDrives {
    connection: Connection,
}

impl UDisksDrives {
    /// Connect to the system bus. Fails when the daemon is not running
    /// (for example in a test container).
    pub fn new() -> Result<Self> {
        Ok(Self {
            connection: Connection::system()?,
        })
    }

    fn proxy(&self, path: &str, interface: &'static str) -> Result<Proxy<'_>> {
        Proxy::new(&self.connection, SERVICE, path.to_owned(), interface).map_err(Error::from)
    }

    fn describe(&self, id: &str) -> Result<Option<DriveInfo>> {
        let block = self.proxy(id, BLOCK_INTERFACE)?;
        let usage: String = block.get_property("IdUsage")?;
        if usage != "filesystem" {
            return Ok(None);
        }

        let drive_path: OwnedObjectPath = block.get_property("Drive")?;
        if drive_path.as_str() == "/" {
            return Ok(None);
        }
        let drive = self.proxy(drive_path.as_str(), DRIVE_INTERFACE)?;

        // udisks marks sticks `Removable`, but USB hard disks often are not.
        // Both should appear on the Export screen; internal disks must not.
        let removable: bool = drive.get_property("Removable").unwrap_or(false);
        let connection_bus: String = drive.get_property("ConnectionBus").unwrap_or_default();
        if !removable && !connection_bus.eq_ignore_ascii_case("usb") {
            return Ok(None);
        }
        let system: bool = block.get_property("HintSystem").unwrap_or(false);
        if system {
            return Ok(None);
        }

        let label: String = block.get_property("IdLabel").unwrap_or_default();
        let size: u64 = block.get_property("Size").unwrap_or(0);
        let device: Vec<u8> = block.get_property("Device").unwrap_or_default();
        let vendor: String = drive.get_property("Vendor").unwrap_or_default();
        let model: String = drive.get_property("Model").unwrap_or_default();

        let filesystem = self.proxy(id, FILESYSTEM_INTERFACE)?;
        let mount_points: Vec<Vec<u8>> = filesystem.get_property("MountPoints").unwrap_or_default();
        let mount_points = mount_points
            .iter()
            .map(|bytes| PathBuf::from(String::from_utf8_lossy(trim_nul(bytes)).into_owned()))
            .collect();

        let device = String::from_utf8_lossy(trim_nul(&device)).into_owned();
        let label = label.trim();
        let name = if !label.is_empty() {
            label.to_string()
        } else {
            let model = format!("{} {}", vendor.trim(), model.trim());
            let model = model.trim();
            if model.is_empty() {
                device
            } else {
                model.to_string()
            }
        };

        Ok(Some(DriveInfo {
            id: id.to_string(),
            name,
            size,
            mount_points,
        }))
    }

    fn filesystem(&self, id: &str) -> Result<Proxy<'_>> {
        check_id(id)?;
        self.proxy(id, FILESYSTEM_INTERFACE)
    }
}

impl Drives for UDisksDrives {
    fn list(&self) -> Result<Vec<DriveInfo>> {
        let manager = self.proxy(MANAGER_PATH, MANAGER_INTERFACE)?;
        let options: HashMap<String, Value<'_>> = HashMap::new();
        let paths: Vec<OwnedObjectPath> = manager.call("GetBlockDevices", &(options,))?;

        let mut drives = Vec::new();
        for path in paths {
            // A device can disappear while the list is being built (the user
            // pulled the stick); one bad device must not break the screen.
            if let Ok(Some(drive)) = self.describe(path.as_str()) {
                drives.push(drive);
            }
        }
        drives.sort_by_key(|drive| drive.name.to_lowercase());
        Ok(drives)
    }

    fn mount(&self, id: &str) -> Result<PathBuf> {
        let filesystem = self.filesystem(id)?;
        let options = mount_options();
        let mount_point: String = filesystem.call("Mount", &(options,))?;
        if mount_point.is_empty() {
            return Err(Error::Drive("udisks returned an empty mount point".into()));
        }
        Ok(PathBuf::from(mount_point))
    }

    fn unmount(&self, id: &str) -> Result<()> {
        let filesystem = self.filesystem(id)?;
        let options = user_options();
        let _: () = filesystem.call("Unmount", &(options,))?;
        Ok(())
    }

    fn power_off(&self, id: &str) -> Result<()> {
        check_id(id)?;
        let block = self.proxy(id, BLOCK_INTERFACE)?;
        let drive_path: OwnedObjectPath = block.get_property("Drive")?;
        if drive_path.as_str() == "/" {
            return Err(Error::Drive(format!("{id} has no drive to power off")));
        }
        let drive = self.proxy(drive_path.as_str(), DRIVE_INTERFACE)?;
        let options = user_options();
        let _: () = drive.call("PowerOff", &(options,))?;
        Ok(())
    }
}

/// udisks accepts a comma-separated mount-options string and a flag that keeps
/// the desktop from asking for a password. The daemon runs as `writer`.
fn mount_options() -> HashMap<&'static str, Value<'static>> {
    let mut options = HashMap::new();
    options.insert("options", Value::from(MOUNT_OPTIONS));
    options.insert("auth.no_user_interaction", Value::from(true));
    options
}

fn user_options() -> HashMap<&'static str, Value<'static>> {
    let mut options = HashMap::new();
    options.insert("auth.no_user_interaction", Value::from(true));
    options
}

fn check_id(id: &str) -> Result<()> {
    if id.starts_with('/') && id.len() > 1 {
        Ok(())
    } else {
        Err(Error::InvalidDriveId(id.to_string()))
    }
}

/// D-Bus byte arrays for mount points and device nodes are NUL-terminated.
fn trim_nul(bytes: &[u8]) -> &[u8] {
    match bytes.iter().position(|byte| *byte == 0) {
        Some(position) => &bytes[..position],
        None => bytes,
    }
}
