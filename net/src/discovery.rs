//! Finding the other computer on the local network with mDNS (Bonjour). Both the
//! Windows and the Mac app announce themselves, with their role in the TXT record.

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::mpsc::{channel, Receiver};
use std::thread;

pub const SERVICE: &str = "_kvbridge._tcp.local.";

/// A computer seen on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub id: String,
    pub name: String,
    /// "windows" or "mac".
    pub role: String,
    pub addrs: Vec<IpAddr>,
    pub port: u16,
}

/// Keeps the advertisement alive while held.
pub struct Advertiser {
    daemon: ServiceDaemon,
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

/// Announce this computer (id, display name, role, port) on the LAN.
pub fn advertise(id: &str, name: &str, role: &str, port: u16) -> Result<Advertiser, String> {
    let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
    let mut props = HashMap::new();
    props.insert("id".to_string(), id.to_string());
    props.insert("name".to_string(), name.to_string());
    props.insert("role".to_string(), role.to_string());
    let host = format!("kvb-{id}.local.");
    let info = ServiceInfo::new(SERVICE, &format!("kvb-{id}"), &host, "", port, props)
        .map_err(|e| e.to_string())?
        .enable_addr_auto();
    daemon.register(info).map_err(|e| e.to_string())?;
    Ok(Advertiser { daemon })
}

/// Keeps browsing alive while held.
pub struct Browser {
    daemon: ServiceDaemon,
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

/// Look for KV Bridge computers. Each time one is (re)resolved it is sent on the channel.
pub fn browse() -> Result<(Browser, Receiver<Found>), String> {
    let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
    let events = daemon.browse(SERVICE).map_err(|e| e.to_string())?;
    let (tx, rx) = channel();
    thread::Builder::new()
        .name("mdns-browse".into())
        .spawn(move || {
            while let Ok(ev) = events.recv() {
                if let ServiceEvent::ServiceResolved(info) = ev {
                    let Some(id) = info.get_property_val_str("id").map(str::to_string) else {
                        continue;
                    };
                    let name = info.get_property_val_str("name").unwrap_or("Computer").to_string();
                    // Versions before 0.4 only ran this on the Mac.
                    let role = info.get_property_val_str("role").unwrap_or("mac").to_string();
                    let mut addrs: Vec<IpAddr> = info.get_addresses().iter().copied().collect();
                    // Prefer IPv4: simplest on home networks.
                    addrs.sort_by_key(|a| !a.is_ipv4());
                    if tx.send(Found { id, name, role, addrs, port: info.get_port() }).is_err() {
                        break;
                    }
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok((Browser { daemon }, rx))
}
