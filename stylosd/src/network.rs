//! Wi-Fi through NetworkManager.
//!
//! `stylosd` never talks to the network stack directly: NetworkManager owns the
//! device and the saved profiles. The daemon only decides when the radio may be
//! on (see [`crate::radio`]) and asks NetworkManager to scan, connect or forget.
//!
//! Captive portals need a browser, which Stylos does not have. The Wi-Fi screen
//! shows the message from [`PORTAL_MESSAGE`] instead of trying to open one.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use zbus::Connection;

/// The one line the Wi-Fi screen shows when a network needs a login page.
pub const PORTAL_MESSAGE: &str = "This network needs a login page. Use a phone hotspot.";

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";

const DEVICE_TYPE_WIFI: u32 = 2;
const AP_FLAG_PRIVACY: u32 = 0x1;
const ACTIVE_STATE_ACTIVATED: u32 = 2;
const ACTIVE_STATE_DEACTIVATED: u32 = 4;

/// One access point as the Wi-Fi screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct WifiNetwork {
    pub ssid: String,
    /// 0-100.
    pub strength: u8,
    /// True when the network needs a password (WPA/WEP/802.1X).
    pub secured: bool,
    pub connected: bool,
}

/// NetworkManager's view of whether the internet actually works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connectivity {
    Unknown,
    /// No connectivity at all.
    None,
    /// Behind a captive portal (hotel, café).
    Portal,
    Limited,
    Full,
}

/// Everything the daemon needs from the network manager.
///
/// The trait keeps the daemon logic (radio window, portal message) testable
/// with a fake.
#[async_trait]
pub trait Network: Send + Sync {
    async fn scan(&self) -> Result<Vec<WifiNetwork>>;
    async fn connect(&self, ssid: &str, password: &str) -> Result<()>;
    async fn forget(&self, ssid: &str) -> Result<()>;
    async fn connectivity(&self) -> Result<Connectivity>;
}

/// NetworkManager over the system bus.
#[derive(Debug, Default)]
pub struct NetworkManager;

impl NetworkManager {
    pub fn new() -> Self {
        Self
    }

    async fn connection(&self) -> Result<Connection> {
        Connection::system()
            .await
            .context("connect to the system bus to reach NetworkManager")
    }
}

#[async_trait]
impl Network for NetworkManager {
    async fn scan(&self) -> Result<Vec<WifiNetwork>> {
        let connection = self.connection().await?;
        scan(&connection).await
    }

    async fn connect(&self, ssid: &str, password: &str) -> Result<()> {
        let connection = self.connection().await?;
        connect(&connection, ssid, password).await
    }

    async fn forget(&self, ssid: &str) -> Result<()> {
        let connection = self.connection().await?;
        forget(&connection, ssid).await
    }

    async fn connectivity(&self) -> Result<Connectivity> {
        let connection = self.connection().await?;
        let nm = manager_proxy(&connection).await?;
        let connectivity: u32 = nm.get_property("Connectivity").await.unwrap_or(0);
        Ok(match connectivity {
            1 => Connectivity::None,
            2 => Connectivity::Portal,
            3 => Connectivity::Limited,
            4 => Connectivity::Full,
            _ => Connectivity::Unknown,
        })
    }
}

/// A NetworkManager proxy with property caching off.
///
/// Caching is zbus's default ("lazily"), but the daemon polls `LastScan`,
/// `Percentage` and `State`: a cached value would never change.
async fn nm_proxy<'a>(
    connection: &'a Connection,
    path: &'a str,
    interface: &'a str,
) -> Result<zbus::Proxy<'a>> {
    Ok(zbus::proxy::Builder::new(connection)
        .destination(NM)?
        .path(path)?
        .interface(interface)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await?)
}

async fn manager_proxy(connection: &Connection) -> Result<zbus::Proxy<'_>> {
    nm_proxy(connection, NM_PATH, NM).await
}

async fn wifi_devices(connection: &Connection) -> Result<Vec<OwnedObjectPath>> {
    let nm = manager_proxy(connection).await?;
    let devices: Vec<OwnedObjectPath> = nm.get_property("Devices").await?;
    let mut wifi = Vec::new();
    for device in devices {
        let proxy = nm_proxy(
            connection,
            device.as_str(),
            "org.freedesktop.NetworkManager.Device",
        )
        .await?;
        let device_type: u32 = proxy.get_property("DeviceType").await.unwrap_or(0);
        if device_type == DEVICE_TYPE_WIFI {
            wifi.push(device);
        }
    }
    Ok(wifi)
}

async fn scan(connection: &Connection) -> Result<Vec<WifiNetwork>> {
    let devices = wifi_devices(connection).await?;
    let Some(device) = devices.first() else {
        bail!("this computer has no Wi-Fi device");
    };
    let wireless = nm_proxy(
        connection,
        device.as_str(),
        "org.freedesktop.NetworkManager.Device.Wireless",
    )
    .await?;

    let before: i64 = wireless.get_property("LastScan").await.unwrap_or(0);
    let options: HashMap<String, Value<'_>> = HashMap::new();
    // "Already scanning" and "scanning not allowed now" are not fatal: the
    // access point list below still shows what NetworkManager knows.
    let _ = wireless.call_method("RequestScan", &(options,)).await;
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let now: i64 = wireless.get_property("LastScan").await.unwrap_or(0);
        if now != before {
            break;
        }
    }

    let access_points: Vec<OwnedObjectPath> = wireless
        .get_property("AccessPoints")
        .await
        .unwrap_or_default();
    let active: OwnedObjectPath = wireless
        .get_property("ActiveAccessPoint")
        .await
        .unwrap_or_else(|_| OwnedObjectPath::try_from("/").expect("root path"));

    let mut networks: Vec<WifiNetwork> = Vec::new();
    for path in access_points {
        let ap = nm_proxy(
            connection,
            path.as_str(),
            "org.freedesktop.NetworkManager.AccessPoint",
        )
        .await?;
        let ssid: Vec<u8> = ap.get_property("Ssid").await.unwrap_or_default();
        let ssid = String::from_utf8_lossy(&ssid).into_owned();
        if ssid.is_empty() {
            // Hidden network; the Wi-Fi screen offers "Other network…".
            continue;
        }
        let strength: u8 = ap.get_property("Strength").await.unwrap_or(0);
        let flags: u32 = ap.get_property("Flags").await.unwrap_or(0);
        let connected = path == active;
        match networks.iter_mut().find(|network| network.ssid == ssid) {
            Some(existing) => {
                existing.strength = existing.strength.max(strength);
                existing.secured |= flags & AP_FLAG_PRIVACY != 0;
                existing.connected |= connected;
            }
            None => networks.push(WifiNetwork {
                ssid,
                strength,
                secured: flags & AP_FLAG_PRIVACY != 0,
                connected,
            }),
        }
    }
    networks.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(b.strength.cmp(&a.strength))
            .then(a.ssid.cmp(&b.ssid))
    });
    Ok(networks)
}

async fn connect(connection: &Connection, ssid: &str, password: &str) -> Result<()> {
    if ssid.trim().is_empty() {
        bail!("the network name is empty");
    }
    if ssid.len() > 32 {
        bail!("the network name is longer than 32 bytes");
    }

    let nm = manager_proxy(connection).await?;
    let root = OwnedObjectPath::try_from("/")?;

    // A saved network is updated and activated, not duplicated. This is also
    // how "reconnect" works without asking for the password again.
    if let Some(saved) = find_profile(connection, ssid).await? {
        if !password.is_empty() {
            update_profile_password(connection, &saved, password)
                .await
                .with_context(|| format!("update the saved network {ssid}"))?;
        }
        let reply = nm
            .call_method("ActivateConnection", &(saved, root.clone(), root))
            .await
            .with_context(|| format!("activate the saved network {ssid}"))?;
        let active: OwnedObjectPath = reply.body().deserialize()?;
        return wait_activated(connection, &active, ssid).await;
    }

    let mut wireless: HashMap<String, Value<'_>> = HashMap::new();
    wireless.insert("ssid".into(), Value::from(ssid.as_bytes().to_vec()));
    wireless.insert("mode".into(), Value::from("infrastructure"));

    let mut profile: HashMap<String, Value<'_>> = HashMap::new();
    profile.insert("id".into(), Value::from(ssid.to_string()));
    profile.insert("type".into(), Value::from("802-11-wireless"));
    profile.insert("uuid".into(), Value::from(uuid::Uuid::now_v7().to_string()));
    profile.insert("autoconnect".into(), Value::from(true));

    let mut settings: HashMap<String, HashMap<String, Value<'_>>> = HashMap::new();
    settings.insert("connection".into(), profile);
    settings.insert("802-11-wireless".into(), wireless);
    if !password.is_empty() {
        let mut security: HashMap<String, Value<'_>> = HashMap::new();
        security.insert("key-mgmt".into(), Value::from("wpa-psk"));
        security.insert("psk".into(), Value::from(password.to_string()));
        settings.insert("802-11-wireless-security".into(), security);
    }

    let reply = nm
        .call_method("AddAndActivateConnection", &(settings, root.clone(), root))
        .await
        .with_context(|| format!("ask NetworkManager to connect to {ssid}"))?;
    let (_, active): (OwnedObjectPath, OwnedObjectPath) = reply.body().deserialize()?;
    wait_activated(connection, &active, ssid).await
}

async fn wait_activated(
    connection: &Connection,
    active: &OwnedObjectPath,
    ssid: &str,
) -> Result<()> {
    // Wait for the access point to finish associating. 30 seconds is enough
    // for a normal network and keeps a wrong password from hanging the screen.
    for _ in 0..120 {
        let proxy = nm_proxy(
            connection,
            active.as_str(),
            "org.freedesktop.NetworkManager.Connection.Active",
        )
        .await?;
        let state: u32 = proxy.get_property("State").await.unwrap_or(0);
        if state == ACTIVE_STATE_ACTIVATED {
            return Ok(());
        }
        if state == ACTIVE_STATE_DEACTIVATED {
            bail!("{ssid} refused the connection (wrong password?)");
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    bail!("timed out connecting to {ssid}")
}

/// All saved connection profiles, as `(path, settings)`.
async fn saved_profiles(
    connection: &Connection,
) -> Result<
    Vec<(
        OwnedObjectPath,
        HashMap<String, HashMap<String, OwnedValue>>,
    )>,
> {
    let settings = nm_proxy(
        connection,
        NM_SETTINGS_PATH,
        "org.freedesktop.NetworkManager.Settings",
    )
    .await?;
    let reply = settings.call_method("ListConnections", &()).await?;
    let connections: Vec<OwnedObjectPath> = reply.body().deserialize()?;
    let mut profiles = Vec::new();
    for path in connections {
        let proxy = nm_proxy(
            connection,
            path.as_str(),
            "org.freedesktop.NetworkManager.Settings.Connection",
        )
        .await?;
        let Ok(reply) = proxy.call_method("GetSettings", &()).await else {
            continue;
        };
        let stored: HashMap<String, HashMap<String, OwnedValue>> = reply.body().deserialize()?;
        profiles.push((path, stored));
    }
    Ok(profiles)
}

async fn find_profile(connection: &Connection, ssid: &str) -> Result<Option<OwnedObjectPath>> {
    Ok(saved_profiles(connection)
        .await?
        .into_iter()
        .find(|(_, settings)| profile_ssid(settings).as_deref() == Some(ssid))
        .map(|(path, _)| path))
}

async fn update_profile_password(
    connection: &Connection,
    profile: &OwnedObjectPath,
    password: &str,
) -> Result<()> {
    let proxy = nm_proxy(
        connection,
        profile.as_str(),
        "org.freedesktop.NetworkManager.Settings.Connection",
    )
    .await?;
    let reply = proxy.call_method("GetSettings", &()).await?;
    let mut stored: HashMap<String, HashMap<String, OwnedValue>> = reply.body().deserialize()?;
    let security = stored
        .entry("802-11-wireless-security".to_string())
        .or_default();
    security.insert(
        "key-mgmt".to_string(),
        OwnedValue::try_from(Value::from("wpa-psk"))?,
    );
    security.insert(
        "psk".to_string(),
        OwnedValue::try_from(Value::from(password.to_string()))?,
    );
    proxy
        .call_method("Update", &(stored,))
        .await
        .context("write the updated connection settings")?;
    Ok(())
}

async fn forget(connection: &Connection, ssid: &str) -> Result<()> {
    let Some(path) = find_profile(connection, ssid).await? else {
        bail!("no saved Wi-Fi network named {ssid}");
    };
    let proxy = nm_proxy(
        connection,
        path.as_str(),
        "org.freedesktop.NetworkManager.Settings.Connection",
    )
    .await?;
    proxy
        .call_method("Delete", &())
        .await
        .with_context(|| format!("delete the saved network {ssid}"))?;
    Ok(())
}

fn profile_ssid(settings: &HashMap<String, HashMap<String, OwnedValue>>) -> Option<String> {
    let value = settings.get("802-11-wireless")?.get("ssid")?;
    let bytes: Vec<u8> = value.clone().try_into().ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_ssid_reads_the_802_11_wireless_section() {
        let mut wireless = HashMap::new();
        wireless.insert(
            "ssid".to_string(),
            OwnedValue::try_from(Value::from(b"Study".to_vec())).unwrap(),
        );
        let mut settings = HashMap::new();
        settings.insert("802-11-wireless".to_string(), wireless);
        assert_eq!(profile_ssid(&settings).as_deref(), Some("Study"));
        assert_eq!(profile_ssid(&HashMap::new()), None);
    }
}
