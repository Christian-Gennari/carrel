//! A [`Drives`] implementation for tests and for the UI's fake-drive flow.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use super::{DriveInfo, Drives};
use crate::{Error, Result};

/// One call the daemon made on a [`FakeDrives`], in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriveCall {
    List,
    Mount(String),
    Unmount(String),
    PowerOff(String),
}

#[derive(Debug)]
struct State {
    drives: Vec<DriveInfo>,
    mounted: BTreeMap<String, PathBuf>,
    calls: Vec<DriveCall>,
}

/// An in-memory drive list that creates real mount-point directories below
/// `mount_root`, so an export can run against it in a test.
#[derive(Debug)]
pub struct FakeDrives {
    mount_root: PathBuf,
    state: Mutex<State>,
}

impl FakeDrives {
    pub fn new(mount_root: impl Into<PathBuf>, drives: Vec<DriveInfo>) -> Self {
        Self {
            mount_root: mount_root.into(),
            state: Mutex::new(State {
                drives,
                mounted: BTreeMap::new(),
                calls: Vec::new(),
            }),
        }
    }

    /// Everything the caller did so far, in order. Used to assert that a drive
    /// is mounted only on request and powered off only after unmounting.
    pub fn calls(&self) -> Vec<DriveCall> {
        self.state().calls.clone()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("fake drives lock poisoned")
    }

    fn index(state: &State, id: &str) -> Result<usize> {
        state
            .drives
            .iter()
            .position(|drive| drive.id == id)
            .ok_or_else(|| Error::Drive(format!("no drive with id {id:?}")))
    }
}

impl Drives for FakeDrives {
    fn list(&self) -> Result<Vec<DriveInfo>> {
        let mut state = self.state();
        state.calls.push(DriveCall::List);
        Ok(state.drives.clone())
    }

    fn mount(&self, id: &str) -> Result<PathBuf> {
        let mut state = self.state();
        state.calls.push(DriveCall::Mount(id.to_string()));
        let index = Self::index(&state, id)?;
        if let Some(path) = state.mounted.get(id) {
            return Ok(path.clone());
        }

        let path = self.mount_root.join(format!("drive-{index}"));
        fs::create_dir_all(&path).map_err(|error| Error::io(&path, error))?;
        state.drives[index].mount_points = vec![path.clone()];
        state.mounted.insert(id.to_string(), path.clone());
        Ok(path)
    }

    fn unmount(&self, id: &str) -> Result<()> {
        let mut state = self.state();
        state.calls.push(DriveCall::Unmount(id.to_string()));
        let index = Self::index(&state, id)?;
        state.drives[index].mount_points.clear();
        state.mounted.remove(id);
        Ok(())
    }

    fn power_off(&self, id: &str) -> Result<()> {
        let mut state = self.state();
        state.calls.push(DriveCall::PowerOff(id.to_string()));
        Self::index(&state, id)?;
        Ok(())
    }
}

impl FakeDrives {
    /// Whether a drive is currently mounted.
    pub fn is_mounted(&self, id: &str) -> bool {
        self.state().mounted.contains_key(id)
    }
}
