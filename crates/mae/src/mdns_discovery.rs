//! mDNS service discovery for peer-to-peer KB sharing.
//!
//! When the embedded state server is started via `collab-start`, we register
//! a `_mae-sync._tcp.local` mDNS service so peers on the same LAN can
//! discover it via `:collab-discover`.
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

/// Service type for MAE collaborative editing.
pub const SERVICE_TYPE: &str = "_mae-sync._tcp.local.";

/// A discovered MAE peer on the local network.
#[derive(Debug, Clone)]
pub struct DiscoveredPeer {
    /// Display name of the peer (from TXT record).
    pub user_name: String,
    /// Address to connect to (host:port).
    pub address: String,
    /// Protocol version (from TXT record).
    pub version: String,
    /// Number of shared KBs (from TXT record).
    pub kb_count: u32,
}

/// Manages mDNS registration and discovery for MAE collab.
pub struct MdnsManager {
    daemon: ServiceDaemon,
    /// Our registered service instance name (if any).
    registered_name: Option<String>,
    /// Discovered peers, keyed by mDNS instance name.
    discovered: Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
}

impl MdnsManager {
    /// Create a new mDNS manager.
    pub fn new() -> Result<Self, String> {
        let daemon = ServiceDaemon::new().map_err(|e| format!("mDNS daemon init failed: {}", e))?;
        Ok(Self {
            daemon,
            registered_name: None,
            discovered: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Register this MAE instance as an mDNS service.
    ///
    /// Called when `collab-start` spawns the embedded server.
    pub fn register(&mut self, user_name: &str, port: u16, kb_count: u32) -> Result<(), String> {
        let hostname = hostname::get()
            .unwrap_or_else(|_| "mae-host".into())
            .to_string_lossy()
            .to_string();
        let instance_name = format!("mae-{}-{}", user_name, port);
        let kb_count_str = kb_count.to_string();
        let properties = [
            ("user", user_name),
            ("version", "1"),
            ("kb_count", kb_count_str.as_str()),
        ];

        let service = ServiceInfo::new(
            SERVICE_TYPE,
            &instance_name,
            &format!("{}.local.", hostname),
            "",
            port,
            &properties[..],
        )
        .map_err(|e| format!("mDNS service info creation failed: {}", e))?
        .enable_addr_auto();

        self.daemon
            .register(service)
            .map_err(|e| format!("mDNS register failed: {}", e))?;

        self.registered_name = Some(instance_name.clone());
        info!(
            instance = %instance_name,
            port,
            user = user_name,
            kb_count,
            "registered mDNS service"
        );
        Ok(())
    }

    /// Unregister our mDNS service (called on server stop / editor quit).
    pub fn unregister(&mut self) {
        if let Some(ref name) = self.registered_name.take() {
            let fullname = format!("{}.{}", name, SERVICE_TYPE);
            if let Err(e) = self.daemon.unregister(&fullname) {
                warn!(error = %e, "failed to unregister mDNS service");
            } else {
                info!(instance = %name, "unregistered mDNS service");
            }
        }
    }

    /// Start browsing for `_mae-sync._tcp.local` services.
    /// Returns a handle that populates `discovered` peers in the background.
    pub fn start_browse(&self) -> Result<(), String> {
        let receiver = self
            .daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| format!("mDNS browse failed: {}", e))?;

        let discovered = Arc::clone(&self.discovered);
        let our_name = self.registered_name.clone();

        std::thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let instance = info.get_fullname().to_string();
                        // Skip our own service.
                        if let Some(ref ours) = our_name {
                            if instance.starts_with(ours) {
                                continue;
                            }
                        }
                        let user_name = info
                            .get_property_val_str("user")
                            .unwrap_or("unknown")
                            .to_string();
                        let version = info
                            .get_property_val_str("version")
                            .unwrap_or("?")
                            .to_string();
                        let kb_count: u32 = info
                            .get_property_val_str("kb_count")
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(0);
                        let port = info.get_port();
                        let addresses = info.get_addresses();
                        let host = addresses
                            .iter()
                            .find(|a| a.is_ipv4())
                            .or_else(|| addresses.iter().next())
                            .map(|a| a.to_string())
                            .unwrap_or_else(|| "127.0.0.1".to_string());
                        let address = format!("{}:{}", host, port);

                        debug!(
                            instance = %instance,
                            user = %user_name,
                            address = %address,
                            "discovered MAE peer via mDNS"
                        );

                        let peer = DiscoveredPeer {
                            user_name,
                            address,
                            version,
                            kb_count,
                        };
                        if let Ok(mut map) = discovered.lock() {
                            map.insert(instance, peer);
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        debug!(instance = %fullname, "MAE peer removed from mDNS");
                        if let Ok(mut map) = discovered.lock() {
                            map.remove(&fullname);
                        }
                    }
                    _ => {}
                }
            }
        });

        Ok(())
    }

    /// Get a snapshot of currently discovered peers.
    pub fn discovered_peers(&self) -> Vec<DiscoveredPeer> {
        self.discovered
            .lock()
            .map(|map| map.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Check if we have a registered service.
    pub fn is_registered(&self) -> bool {
        self.registered_name.is_some()
    }
}

impl Drop for MdnsManager {
    fn drop(&mut self) {
        self.unregister();
        if let Err(e) = self.daemon.shutdown() {
            warn!(error = %e, "mDNS daemon shutdown error");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mdns_manager_creation() {
        // May fail on CI without network — just verify the API compiles.
        let result = MdnsManager::new();
        // On systems without multicast, this might fail — that's OK.
        if let Ok(mgr) = result {
            assert!(!mgr.is_registered());
            assert!(mgr.discovered_peers().is_empty());
        }
    }

    #[test]
    fn discovered_peer_clone_and_debug() {
        let peer = DiscoveredPeer {
            user_name: "alice".to_string(),
            address: "192.168.1.10:9473".to_string(),
            version: "1".to_string(),
            kb_count: 3,
        };
        let clone = peer.clone();
        assert_eq!(clone.user_name, "alice");
        assert_eq!(clone.address, "192.168.1.10:9473");
        assert!(format!("{:?}", peer).contains("alice"));
    }

    #[test]
    fn register_and_unregister_lifecycle() {
        let result = MdnsManager::new();
        if let Ok(mut mgr) = result {
            // Register may fail on CI without multicast — that's expected.
            let reg_result = mgr.register("test-user", 19473, 2);
            if reg_result.is_ok() {
                assert!(mgr.is_registered());
                mgr.unregister();
                assert!(!mgr.is_registered());
            }
        }
    }

    #[test]
    fn browse_discovers_self_filtered() {
        // This test verifies the browse API compiles and self-filtering logic.
        // Actual mDNS discovery needs multicast networking.
        let result = MdnsManager::new();
        if let Ok(mut mgr) = result {
            if mgr.register("browse-test", 29473, 0).is_ok() {
                // Start browsing — should filter out our own service.
                if mgr.start_browse().is_ok() {
                    // Give mDNS a moment to discover.
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    // Our own service should NOT appear in discovered peers.
                    let peers = mgr.discovered_peers();
                    assert!(
                        !peers.iter().any(|p| p.user_name == "browse-test"),
                        "should filter out our own mDNS service"
                    );
                }
            }
        }
    }
}
