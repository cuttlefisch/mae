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
    /// Handle to the browse thread (if active).
    browse_handle: Option<std::thread::JoinHandle<()>>,
}

impl MdnsManager {
    /// Create a new mDNS manager.
    pub fn new() -> Result<Self, String> {
        let daemon = ServiceDaemon::new().map_err(|e| format!("mDNS daemon init failed: {}", e))?;
        Ok(Self {
            daemon,
            registered_name: None,
            discovered: Arc::new(Mutex::new(HashMap::new())),
            browse_handle: None,
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

    /// True if a background browse is already running.
    pub fn is_browsing(&self) -> bool {
        self.browse_handle.is_some()
    }

    /// Start browsing for `_mae-sync._tcp.local` services.
    /// Returns a handle that populates `discovered` peers in the background.
    /// Idempotent: a second call while a browse is already running is a no-op
    /// (so callers can `ensure` browsing without spawning duplicate threads).
    pub fn start_browse(&mut self) -> Result<(), String> {
        if self.browse_handle.is_some() {
            return Ok(());
        }
        let receiver = self
            .daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| format!("mDNS browse failed: {}", e))?;

        let discovered = Arc::clone(&self.discovered);
        let our_name = self.registered_name.clone();

        let handle = std::thread::spawn(move || {
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
                        let addrs: Vec<std::net::IpAddr> = info
                            .get_addresses()
                            .iter()
                            .map(|s| s.to_ip_addr())
                            .collect();
                        let peer = build_discovered_peer(
                            info.get_property_val_str("user"),
                            info.get_property_val_str("version"),
                            info.get_property_val_str("kb_count"),
                            &addrs,
                            info.get_port(),
                        );
                        debug!(
                            instance = %instance,
                            user = %peer.user_name,
                            address = %peer.address,
                            "discovered MAE peer via mDNS"
                        );
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
        self.browse_handle = Some(handle);

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
    #[cfg(test)]
    pub fn is_registered(&self) -> bool {
        self.registered_name.is_some()
    }
}

/// Build a [`DiscoveredPeer`] from the fields a resolved mDNS service exposes — the
/// parse path the browse loop drives. Takes primitives (not an mdns-sd type), so it
/// is pure + unit-testable without multicast: applies the TXT defaults
/// (`user`→"unknown", `version`→"?", non-numeric/absent `kb_count`→0) and selects an
/// address (IPv4-preferred, else the first, else loopback).
fn build_discovered_peer(
    user: Option<&str>,
    version: Option<&str>,
    kb_count: Option<&str>,
    addresses: &[std::net::IpAddr],
    port: u16,
) -> DiscoveredPeer {
    let host = addresses
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| addresses.first())
        .map(|a| a.to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    DiscoveredPeer {
        user_name: user.unwrap_or("unknown").to_string(),
        address: format!("{host}:{port}"),
        version: version.unwrap_or("?").to_string(),
        kb_count: kb_count.and_then(|s| s.parse().ok()).unwrap_or(0),
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
    use std::time::Duration;

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

    // --- the parse path the browse loop drives (deterministic, no multicast) ---

    fn ip(s: &str) -> std::net::IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn build_discovered_peer_reads_txt_and_address() {
        let peer = build_discovered_peer(
            Some("alice"),
            Some("1"),
            Some("3"),
            &[ip("192.168.1.10")],
            9473,
        );
        assert_eq!(peer.user_name, "alice");
        assert_eq!(peer.version, "1");
        assert_eq!(peer.kb_count, 3);
        assert_eq!(peer.address, "192.168.1.10:9473");
    }

    #[test]
    fn build_discovered_peer_defaults_for_missing_txt() {
        // No TXT + no address → safe defaults (never panics on a sparse peer).
        let peer = build_discovered_peer(None, None, None, &[], 1234);
        assert_eq!(peer.user_name, "unknown");
        assert_eq!(peer.version, "?");
        assert_eq!(peer.kb_count, 0);
        assert_eq!(peer.address, "127.0.0.1:1234");
    }

    #[test]
    fn build_discovered_peer_garbage_kb_count_is_zero() {
        let peer = build_discovered_peer(Some("bob"), None, Some("not-a-number"), &[], 5555);
        assert_eq!(peer.kb_count, 0);
    }

    #[test]
    fn build_discovered_peer_prefers_ipv4_over_ipv6() {
        // A peer advertising both v6 and v4 → we pick v4 for the connect address.
        let peer = build_discovered_peer(
            Some("carol"),
            Some("1"),
            Some("0"),
            &[ip("fe80::1"), ip("10.0.0.7")],
            9473,
        );
        assert_eq!(peer.address, "10.0.0.7:9473");
    }

    // --- real register → browse → discover round-trip (needs LAN multicast) ---
    // Gated behind MAE_MDNS_E2E=1 so it asserts real discovery where multicast is
    // available (a CI step + local runs) and is cleanly SKIPPED elsewhere — never a
    // permissive pass that succeeds whether or not discovery actually works.

    #[test]
    fn mdns_round_trip_discovers_a_registered_peer() {
        if std::env::var("MAE_MDNS_E2E").is_err() {
            eprintln!("skipping mDNS round-trip — set MAE_MDNS_E2E=1 to run (needs multicast)");
            return;
        }

        // A server registers a service; a separate manager browses and must resolve
        // it with the advertised TXT props — and must NOT surface its OWN service.
        let mut server = MdnsManager::new().expect("server mDNS daemon");
        server.register("alice-rt", 39473, 5).expect("register");
        assert!(server.is_registered());

        let mut client = MdnsManager::new().expect("client mDNS daemon");
        client
            .register("bob-rt", 39474, 0)
            .expect("client register");
        client.start_browse().expect("browse");
        assert!(client.is_browsing());

        let mut found = None;
        for _ in 0..150 {
            std::thread::sleep(Duration::from_millis(100));
            let peers = client.discovered_peers();
            // Self-filter: the client never discovers its own bob-rt service.
            assert!(
                !peers.iter().any(|p| p.user_name == "bob-rt"),
                "client must filter out its own service"
            );
            if let Some(p) = peers.into_iter().find(|p| p.user_name == "alice-rt") {
                found = Some(p);
                break;
            }
        }
        let peer = found.expect("client should discover the registered peer within 15s");
        assert_eq!(peer.kb_count, 5, "TXT kb_count propagated");
        assert_eq!(peer.version, "1");
        assert!(
            peer.address.ends_with(":39473"),
            "discovered the server's port: {}",
            peer.address
        );

        server.unregister();
        assert!(!server.is_registered());
    }
}
