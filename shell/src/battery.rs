//! The bottom line's battery value and the low-battery note.
//!
//! WP4 reads UPower and sends `BatteryChanged`; this module only turns the
//! readings into words and decides when the 10 % / 5 % note is due. The note
//! exists because the writer may have hidden the bottom line with F1.

use std::fmt;
use std::sync::Mutex;

use crate::daemon::BatteryStatus;

pub trait Battery: fmt::Debug {
    fn percent(&self) -> u8;
    fn charging(&self) -> bool;
    /// False on a desktop without a battery.
    fn present(&self) -> bool {
        true
    }

    /// What the bottom line shows. The word "Battery" keeps the percentage
    /// from reading as goal progress. No icon fonts, no tofu.
    fn label(&self) -> String {
        if !self.present() {
            return "No battery".to_owned();
        }
        if self.charging() {
            format!("Battery {}% charging", self.percent())
        } else {
            format!("Battery {}%", self.percent())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FakeBattery {
    percent: u8,
    charging: bool,
}

impl FakeBattery {
    pub fn new(percent: u8, charging: bool) -> Self {
        Self {
            percent: percent.min(100),
            charging,
        }
    }
}

impl Default for FakeBattery {
    fn default() -> Self {
        Self::new(76, false)
    }
}

impl Battery for FakeBattery {
    fn percent(&self) -> u8 {
        self.percent
    }

    fn charging(&self) -> bool {
        self.charging
    }
}

/// The live reading from the daemon, shared by the app and the editor.
#[derive(Debug)]
pub struct SharedBattery {
    state: Mutex<BatteryStatus>,
}

impl SharedBattery {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(BatteryStatus::UNKNOWN),
        }
    }

    pub fn set(&self, status: BatteryStatus) {
        if let Ok(mut state) = self.state.lock() {
            *state = status;
        }
    }

    pub fn status(&self) -> BatteryStatus {
        self.state
            .lock()
            .map(|state| *state)
            .unwrap_or(BatteryStatus::UNKNOWN)
    }
}

impl Default for SharedBattery {
    fn default() -> Self {
        Self::new()
    }
}

impl Battery for SharedBattery {
    fn percent(&self) -> u8 {
        self.status().percent
    }

    fn charging(&self) -> bool {
        self.status().charging
    }

    fn present(&self) -> bool {
        self.status().present
    }
}

/// The 10 % and 5 % notes. Each threshold is shown once per discharge; going
/// back on the charger re-arms them. The caller shows the note for five
/// seconds (spec/03-ui.md).
#[derive(Debug, Default)]
pub struct BatteryHint {
    announced_ten: bool,
    announced_five: bool,
}

impl BatteryHint {
    pub fn observe(&mut self, percent: u8, charging: bool) -> Option<u8> {
        if charging {
            self.announced_ten = false;
            self.announced_five = false;
            return None;
        }
        if percent <= 5 && !self.announced_five {
            self.announced_five = true;
            self.announced_ten = true;
            return Some(percent);
        }
        if percent <= 10 && !self.announced_ten {
            self.announced_ten = true;
            return Some(percent);
        }
        None
    }

    pub fn is_announced(&self, percent: u8) -> bool {
        percent <= 5 && self.announced_five || percent <= 10 && self.announced_ten
    }
}

pub const BATTERY_NOTE_SECONDS: u64 = 5;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_plain_and_charging() {
        assert_eq!(FakeBattery::new(76, false).label(), "Battery 76%");
        assert_eq!(FakeBattery::new(3, true).label(), "Battery 3% charging");
    }

    #[test]
    fn clamps_percent() {
        assert_eq!(FakeBattery::new(200, false).percent(), 100);
    }

    #[test]
    fn no_battery_says_so_instead_of_zero_percent() {
        let battery = SharedBattery::new();
        assert_eq!(battery.label(), "No battery");
        battery.set(BatteryStatus {
            present: true,
            percent: 42,
            charging: false,
        });
        assert_eq!(battery.label(), "Battery 42%");
    }

    #[test]
    fn ten_and_five_percent_are_each_announced_once() {
        let mut hint = BatteryHint::default();
        assert_eq!(hint.observe(76, false), None);
        assert_eq!(hint.observe(10, false), Some(10));
        assert_eq!(hint.observe(10, false), None);
        assert_eq!(hint.observe(9, false), None);
        assert_eq!(hint.observe(5, false), Some(5));
        assert_eq!(hint.observe(4, false), None);
        assert!(hint.is_announced(4));
    }

    #[test]
    fn a_battery_that_starts_low_announces_the_highest_threshold_first() {
        let mut hint = BatteryHint::default();
        assert_eq!(hint.observe(7, false), Some(7));
        assert_eq!(hint.observe(7, false), None);
        assert_eq!(hint.observe(5, false), Some(5));
    }

    #[test]
    fn charging_rearms_the_notes_and_shows_nothing() {
        let mut hint = BatteryHint::default();
        assert_eq!(hint.observe(10, false), Some(10));
        assert_eq!(hint.observe(10, true), None);
        assert!(!hint.is_announced(10));
        assert_eq!(hint.observe(10, false), Some(10));
        assert_eq!(BATTERY_NOTE_SECONDS, 5);
    }
}
