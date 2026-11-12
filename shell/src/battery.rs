//! The bottom line's battery value.
//!
//! WP4 replaces `FakeBattery` with UPower over D-Bus. Keeping this behind a
//! trait means the editor never has to wait for the daemon and tests can fake
//! any percentage.

use std::fmt;

pub trait Battery: fmt::Debug {
    fn percent(&self) -> u8;
    fn charging(&self) -> bool;

    /// What the bottom line shows. No icon fonts, no tofu.
    fn label(&self) -> String {
        if self.charging() {
            format!("{}% charging", self.percent())
        } else {
            format!("{}%", self.percent())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_plain_and_charging() {
        assert_eq!(FakeBattery::new(76, false).label(), "76%");
        assert_eq!(FakeBattery::new(3, true).label(), "3% charging");
    }

    #[test]
    fn clamps_percent() {
        assert_eq!(FakeBattery::new(200, false).percent(), 100);
    }
}
