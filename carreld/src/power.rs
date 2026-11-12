//! Battery (UPower) and sleep (logind).
//!
//! The daemon watches the battery, reports changes to the shell and suspends at
//! 3 % when the charger is not connected — after committing history. A logind
//! delay inhibitor gives the commit time to finish before the system sleeps.

use std::os::fd::OwnedFd;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::unix::AsyncFd;
use tokio::sync::Mutex;
use zbus::proxy::SignalStream;
use zbus::Connection;

/// Suspend threshold from `spec/03-ui.md`.
pub const LOW_BATTERY_PERCENT: u8 = 3;

const UPOWER: &str = "org.freedesktop.UPower";
const UPOWER_PATH: &str = "/org/freedesktop/UPower";
const LOGIND: &str = "org.freedesktop.login1";
const LOGIND_PATH: &str = "/org/freedesktop/login1";

const UPOWER_DEVICE_BATTERY: u32 = 2;
const UPOWER_STATE_CHARGING: u32 = 1;
const UPOWER_STATE_FULLY_CHARGED: u32 = 4;
const UPOWER_STATE_PENDING_CHARGE: u32 = 5;

/// The battery as the bottom line and the Battery screen show it.
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

/// Battery readings, abstracted so the 3 % policy is testable with a fake.
#[async_trait]
pub trait Power: Send + Sync {
    async fn battery(&self) -> Result<BatteryStatus>;
}

/// UPower over the system bus.
#[derive(Debug, Default)]
pub struct UPowerPower;

impl UPowerPower {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Power for UPowerPower {
    async fn battery(&self) -> Result<BatteryStatus> {
        let connection = Connection::system()
            .await
            .context("connect to the system bus to reach UPower")?;
        let upower = zbus::Proxy::new(&connection, UPOWER, UPOWER_PATH, UPOWER).await?;

        // Prefer the display device: UPower aggregates several batteries there.
        if let Ok(path) = upower
            .get_property::<zbus::zvariant::OwnedObjectPath>("DisplayDevice")
            .await
        {
            if let Ok(status) = read_device(&connection, path.as_str()).await {
                return Ok(status);
            }
        }

        let reply = upower.call_method("EnumerateDevices", &()).await?;
        let devices: Vec<zbus::zvariant::OwnedObjectPath> = reply.body().deserialize()?;
        let mut best: Option<BatteryStatus> = None;
        for path in devices {
            let Ok(status) = read_battery(&connection, path.as_str()).await else {
                continue;
            };
            if !status.present {
                continue;
            }
            best = Some(match best {
                Some(current) if current.percent <= status.percent => BatteryStatus {
                    present: true,
                    // Charging wins: the laptop is on the charger.
                    charging: current.charging || status.charging,
                    ..current
                },
                _ => status,
            });
        }
        Ok(best.unwrap_or(BatteryStatus::UNKNOWN))
    }
}

async fn device_proxy<'a>(connection: &'a Connection, path: &'a str) -> Result<zbus::Proxy<'a>> {
    Ok(zbus::Proxy::new(connection, UPOWER, path, "org.freedesktop.UPower.Device").await?)
}

async fn read_device(connection: &Connection, path: &str) -> Result<BatteryStatus> {
    let proxy = device_proxy(connection, path).await?;
    let present: bool = proxy.get_property("IsPresent").await.unwrap_or(false);
    if !present {
        return Ok(BatteryStatus::UNKNOWN);
    }
    let percentage: f64 = proxy.get_property("Percentage").await.unwrap_or(0.0);
    let state: u32 = proxy.get_property("State").await.unwrap_or(0);
    Ok(BatteryStatus {
        present: true,
        percent: percentage.clamp(0.0, 100.0).round() as u8,
        charging: is_charging(state),
    })
}

async fn read_battery(connection: &Connection, path: &str) -> Result<BatteryStatus> {
    let proxy = device_proxy(connection, path).await?;
    let kind: u32 = proxy.get_property("Type").await.unwrap_or(0);
    if kind != UPOWER_DEVICE_BATTERY {
        bail!("not a battery");
    }
    read_device(connection, path).await
}

fn is_charging(state: u32) -> bool {
    matches!(
        state,
        UPOWER_STATE_CHARGING | UPOWER_STATE_FULLY_CHARGED | UPOWER_STATE_PENDING_CHARGE
    )
}

/// A held logind delay inhibitor. Dropping it lets the system continue to
/// sleep; that is the acknowledgement logind waits for.
#[async_trait]
pub trait SleepGuard: Send {
    /// Resolve when the system is about to suspend.
    async fn wait_pending(&mut self) -> Result<()>;
}

/// logind, abstracted so the sleep path is testable.
#[async_trait]
pub trait Session: Send + Sync {
    /// Ask logind to suspend now (low battery).
    async fn suspend(&self) -> Result<()>;
    /// Take a delay inhibitor for the next suspend.
    async fn inhibit_sleep(&self) -> Result<Box<dyn SleepGuard>>;
    /// Resolve after the system resumed from a suspend.
    async fn wait_resume(&self) -> Result<()>;
}

/// logind over the system bus.
#[derive(Default)]
pub struct LogindSession {
    connection: Mutex<Option<Connection>>,
    resume: Mutex<Option<SignalStream<'static>>>,
}

impl LogindSession {
    pub fn new() -> Self {
        Self::default()
    }

    async fn connection(&self) -> Result<Connection> {
        let mut slot = self.connection.lock().await;
        if let Some(connection) = slot.as_ref() {
            if !connection.is_closed() {
                return Ok(connection.clone());
            }
        }
        let connection = Connection::system()
            .await
            .context("connect to the system bus to reach logind")?;
        *slot = Some(connection.clone());
        Ok(connection)
    }

    async fn manager(&self) -> Result<zbus::Proxy<'static>> {
        let connection = self.connection().await?;
        Ok(zbus::Proxy::new_owned(
            connection,
            LOGIND,
            LOGIND_PATH,
            "org.freedesktop.login1.Manager",
        )
        .await?)
    }
}

#[async_trait]
impl Session for LogindSession {
    async fn suspend(&self) -> Result<()> {
        let manager = self.manager().await?;
        manager
            .call_method("Suspend", &(false,))
            .await
            .context("ask logind to suspend")?;
        Ok(())
    }

    async fn inhibit_sleep(&self) -> Result<Box<dyn SleepGuard>> {
        let manager = self.manager().await?;
        let reply = manager
            .call_method(
                "Inhibit",
                &("sleep", "carreld", "Save writing before suspend", "delay"),
            )
            .await
            .context("take a logind sleep delay inhibitor")?;
        let fd: zbus::zvariant::OwnedFd = reply.body().deserialize()?;
        let fd: OwnedFd = fd.into();
        // SAFETY: `fd` is an owned descriptor that stays open for the whole
        // lifetime of the `AsyncFd` and is never swapped.
        let fd = unsafe { AsyncFd::register(fd) }
            .map_err(|error| error.into_parts().1)
            .context("watch the logind inhibitor")?;
        Ok(Box::new(LogindInhibitor { fd }))
    }

    async fn wait_resume(&self) -> Result<()> {
        let mut slot = self.resume.lock().await;
        if slot.is_none() {
            let manager = self.manager().await?;
            *slot = Some(manager.receive_signal("PrepareForSleep").await?);
        }
        let resumed = {
            let stream = slot.as_mut().expect("resume stream just created");
            let wait = async {
                while let Some(message) = stream.next().await {
                    if let Ok(sleeping) = message.body().deserialize::<bool>() {
                        if !sleeping {
                            return true;
                        }
                    }
                }
                false
            };
            tokio::time::timeout(Duration::from_secs(3600), wait).await
        };
        match resumed {
            Ok(true) => Ok(()),
            // The stream ended (logind restarted). Drop it so the next call
            // subscribes again instead of returning immediately forever.
            Ok(false) => {
                *slot = None;
                Ok(())
            }
            // Never resume silently for a whole hour: re-arm the watcher so a
            // missed signal cannot stop history commits before the next sleep.
            Err(_) => Ok(()),
        }
    }
}

struct LogindInhibitor {
    fd: AsyncFd<OwnedFd>,
}

#[async_trait]
impl SleepGuard for LogindInhibitor {
    async fn wait_pending(&mut self) -> Result<()> {
        let mut ready = self.fd.readable().await?;
        // The descriptor becomes readable when a suspend is pending. Reading
        // is not needed; closing the descriptor releases the delay inhibitor.
        ready.clear_ready();
        Ok(())
    }
}
