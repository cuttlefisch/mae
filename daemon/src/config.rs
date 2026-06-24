//! Daemon configuration — loaded from `~/.config/mae/daemon.toml`.
//!
//! Also loads legacy `state-server.toml` for migration from the old
//! mae-state-server binary.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

/// XDG-first config base dir: `$XDG_CONFIG_HOME/mae` when set, else the platform
/// default (`dirs::config_dir()/mae`). Per CLAUDE.md principle #13 the daemon must
/// honor XDG on macOS too — the bare `dirs` crate uses Apple paths there and
/// silently ignores env-var isolation, diverging from the `mae-mcp` identity /
/// keystore resolution and breaking the collab e2e harness on macOS.
fn xdg_config_base() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("XDG_CONFIG_HOME") {
        if !v.is_empty() {
            return Some(PathBuf::from(v).join("mae"));
        }
    }
    dirs::config_dir().map(|d| d.join("mae"))
}

/// XDG-first data base dir: `$XDG_DATA_HOME/mae` when set, else `dirs::data_dir()/mae`.
fn xdg_data_base() -> PathBuf {
    if let Some(v) = std::env::var_os("XDG_DATA_HOME") {
        if !v.is_empty() {
            return PathBuf::from(v).join("mae");
        }
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mae")
}

/// Top-level daemon configuration.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    /// Unix socket path for KB client connections.
    pub socket: PathBuf,
    /// Watcher drain interval in milliseconds.
    pub watcher_interval_ms: u64,
    /// DB maintenance interval in seconds.
    pub maintenance_interval_secs: u64,
    /// CRDT sync push interval in seconds.
    pub sync_interval_secs: u64,
    /// Activity decay interval in seconds.
    pub decay_interval_secs: u64,
    /// Health check interval in seconds.
    pub health_interval_secs: u64,
    /// KB data directory (XDG-compliant default).
    pub data_dir: Option<PathBuf>,
    /// Log level filter (e.g. "info", "mae_daemon=debug,warn").
    pub log_level: String,
    /// Collaboration server settings (absorbed from mae-state-server).
    pub collab: CollabConfig,
}

/// Collaboration server configuration (TCP sync, persistence, auth).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CollabConfig {
    /// Whether the collab TCP listener is enabled.
    pub enabled: bool,
    /// TCP bind address for collab connections.
    pub bind: SocketAddr,
    /// Storage backend configuration.
    pub storage: StorageConfig,
    /// Sync engine configuration.
    pub sync: SyncConfig,
    /// Authentication configuration.
    pub auth: AuthConfig,
}

impl Default for CollabConfig {
    fn default() -> Self {
        CollabConfig {
            enabled: true,
            bind: "127.0.0.1:9473".parse().unwrap(),
            storage: StorageConfig::default(),
            sync: SyncConfig::default(),
            auth: AuthConfig::default(),
        }
    }
}

/// Authentication configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Auth mode: "none" or "psk".
    pub mode: String,
    /// PSK command (legacy — e.g., `pass show mae/key`). Loaded as one
    /// (unnamed) trusted key, in addition to the keystore.
    pub psk_command: Option<String>,
    /// PSK fallback (legacy plaintext — prefer the keystore). Loaded as one
    /// (unnamed) trusted key.
    pub psk: Option<String>,
    /// Path to the trusted-keys keystore. Defaults to
    /// `$XDG_DATA_HOME/mae/collab/trusted_keys`. The daemon trusts every key
    /// in this file (named or unnamed) as a peer credential.
    pub keystore: Option<String>,
    /// (mode = "key") Path to the asymmetric authorized_keys file. Defaults to
    /// `$XDG_DATA_HOME/mae/collab/authorized_keys`.
    pub authorized_keys: Option<String>,
    /// (mode = "key") Directory holding the daemon's Ed25519 identity. Defaults
    /// to `$XDG_DATA_HOME/mae/collab`.
    pub identity_dir: Option<String>,
    /// (mode = "key") Use native mTLS for confidentiality (recommended). When
    /// false, falls back to the plaintext JSON KeyAuth handshake.
    pub tls: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        AuthConfig {
            mode: "none".to_string(),
            psk_command: None,
            psk: None,
            keystore: None,
            authorized_keys: None,
            identity_dir: None,
            tls: true,
        }
    }
}

impl AuthConfig {
    /// Resolve the keystore path: the configured override, else the shared
    /// default (`$XDG_DATA_HOME/mae/collab/trusted_keys`).
    pub fn keystore_path(&self) -> Option<std::path::PathBuf> {
        self.keystore
            .as_ref()
            .map(std::path::PathBuf::from)
            .or_else(mae_mcp::keystore::default_keystore_path)
    }

    /// Number of trusted keys available from the keystore file (0 if missing).
    pub fn keystore_key_count(&self) -> usize {
        self.keystore_path()
            .and_then(|p| mae_mcp::keystore::load_optional(&p).ok().flatten())
            .map(|ks| ks.len())
            .unwrap_or(0)
    }

    /// (mode = "key") Directory holding the daemon's Ed25519 identity.
    pub fn identity_dir(&self) -> Option<std::path::PathBuf> {
        self.identity_dir
            .as_ref()
            .map(std::path::PathBuf::from)
            .or_else(mae_mcp::identity::default_collab_dir)
    }

    /// (mode = "key") Path to the authorized_keys file.
    pub fn authorized_keys_path(&self) -> Option<std::path::PathBuf> {
        self.authorized_keys
            .as_ref()
            .map(std::path::PathBuf::from)
            .or_else(|| mae_mcp::identity::default_collab_dir().map(|d| d.join("authorized_keys")))
    }

    /// (mode = "key") Number of authorized client keys (0 if the file is absent).
    pub fn authorized_key_count(&self) -> usize {
        self.authorized_keys_path()
            .map(|p| mae_mcp::identity::AuthorizedKeys::load(&p).len())
            .unwrap_or(0)
    }
}

/// Storage backend configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Backend type (currently only "sqlite").
    pub backend: String,
    /// Data directory path for collab state. Defaults to XDG data dir.
    pub data_dir: Option<PathBuf>,
    /// WAL compaction threshold (number of updates per document).
    pub compact_threshold: u64,
    /// Maximum WAL entries between forced compactions (0 = no forced compaction).
    pub max_wal_entries: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        StorageConfig {
            backend: "sqlite".to_string(),
            data_dir: None,
            compact_threshold: 500,
            max_wal_entries: 5000,
        }
    }
}

/// Sync engine configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SyncConfig {
    /// Heartbeat interval in seconds.
    pub heartbeat_interval_secs: u64,
    /// Maximum concurrent documents in memory.
    pub max_documents: usize,
    /// Idle eviction timeout in seconds (0 = disabled).
    pub idle_eviction_secs: u64,
    /// Background compaction interval in seconds.
    pub compaction_interval_secs: u64,
    /// Maximum update payload size in bytes (0 = unlimited).
    pub max_update_size_bytes: usize,
    /// Maximum document size in bytes before warning (0 = unlimited).
    pub max_document_size_bytes: usize,
}

impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig {
            heartbeat_interval_secs: 30,
            max_documents: 1000,
            idle_eviction_secs: 300,
            compaction_interval_secs: 60,
            max_update_size_bytes: 1_048_576,    // 1 MB
            max_document_size_bytes: 10_485_760, // 10 MB
        }
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        let runtime_dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
        Self {
            socket: runtime_dir.join("mae-daemon.sock"),
            watcher_interval_ms: 500,
            maintenance_interval_secs: 3600,
            sync_interval_secs: 30,
            decay_interval_secs: 3600,
            health_interval_secs: 300,
            data_dir: None,
            log_level: "info".to_string(),
            collab: CollabConfig::default(),
        }
    }
}

impl DaemonConfig {
    /// Load config from the given path, falling back to defaults.
    pub fn load_from(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(config) => config,
                Err(e) => {
                    eprintln!("Warning: failed to parse {}: {}", path.display(), e);
                    // Try legacy format
                    if let Ok(legacy) = toml::from_str::<LegacyServerConfig>(&contents) {
                        let mut config = Self::default();
                        config.collab.bind = legacy.bind;
                        config.collab.storage = legacy.storage;
                        config.collab.sync = legacy.sync;
                        config.collab.auth = legacy.auth;
                        return config;
                    }
                    Self::default()
                }
            },
            Err(e) => {
                eprintln!("Warning: failed to read {}: {}", path.display(), e);
                Self::default()
            }
        }
    }

    /// Load config from `~/.config/mae/daemon.toml`, falling back to defaults.
    /// Also checks for legacy `state-server.toml` and auto-migrates collab settings.
    pub fn load() -> Self {
        let config_dir = xdg_config_base();

        if let Some(ref dir) = config_dir {
            let daemon_path = dir.join("daemon.toml");
            if daemon_path.exists() {
                match std::fs::read_to_string(&daemon_path) {
                    Ok(contents) => match toml::from_str(&contents) {
                        Ok(config) => return config,
                        Err(e) => {
                            eprintln!("Warning: failed to parse {}: {}", daemon_path.display(), e);
                        }
                    },
                    Err(e) => {
                        eprintln!("Warning: failed to read {}: {}", daemon_path.display(), e);
                    }
                }
            }

            // Auto-migrate from legacy state-server.toml
            let legacy_path = dir.join("state-server.toml");
            if legacy_path.exists() {
                eprintln!(
                    "Note: migrating collab settings from {} (mae-state-server is now part of mae-daemon)",
                    legacy_path.display()
                );
                if let Ok(contents) = std::fs::read_to_string(&legacy_path) {
                    if let Ok(legacy) = toml::from_str::<LegacyServerConfig>(&contents) {
                        let mut config = Self::default();
                        config.collab.bind = legacy.bind;
                        config.collab.storage = legacy.storage;
                        config.collab.sync = legacy.sync;
                        config.collab.auth = legacy.auth;
                        return config;
                    }
                }
            }
        }

        Self::default()
    }

    /// Effective KB data directory (explicit config or XDG-first default).
    pub fn effective_data_dir(&self) -> PathBuf {
        self.data_dir.clone().unwrap_or_else(xdg_data_base)
    }

    /// Resolve the collab data directory, creating it if needed.
    pub fn resolve_collab_data_dir(&self) -> PathBuf {
        let dir = self
            .collab
            .storage
            .data_dir
            .clone()
            .unwrap_or_else(|| self.effective_data_dir().join("collab"));
        if !dir.exists() {
            let _ = std::fs::create_dir_all(&dir);
        }
        dir
    }

    /// Validate collab configuration and return issues.
    pub fn check_collab(&self) -> Vec<String> {
        let mut issues = Vec::new();
        let c = &self.collab;

        if c.storage.compact_threshold == 0 {
            issues.push("collab.storage.compact_threshold must be > 0".to_string());
        }

        if c.sync.heartbeat_interval_secs == 0 {
            issues.push("collab.sync.heartbeat_interval_secs must be > 0".to_string());
        }

        if c.sync.max_documents == 0 {
            issues.push("collab.sync.max_documents must be > 0".to_string());
        }

        if c.storage.backend != "sqlite" {
            issues.push(format!(
                "unknown collab storage backend '{}' (only 'sqlite' is supported)",
                c.storage.backend
            ));
        }

        match c.auth.mode.as_str() {
            "none" | "psk" | "key" => {}
            other => {
                issues.push(format!(
                    "unknown collab auth mode '{other}' (supported: 'none', 'psk', 'key')"
                ));
            }
        }

        if c.auth.mode == "psk"
            && c.auth.psk_command.is_none()
            && c.auth.psk.is_none()
            && c.auth.keystore_key_count() == 0
        {
            issues.push(
                "collab.auth.mode = 'psk' but no keys available — add a key to the keystore \
                 (mae-daemon keygen) or set collab.auth.psk_command / collab.auth.psk"
                    .to_string(),
            );
        }

        if c.auth.mode == "key" && c.auth.authorized_key_count() == 0 {
            issues.push(
                "collab.auth.mode = 'key' but authorized_keys is empty — no client can connect \
                 (authorize a client key with: mae-daemon authorize <pubkey-line>)"
                    .to_string(),
            );
        }

        issues
    }
}

/// Legacy state-server.toml format for migration.
#[derive(Debug, Deserialize)]
#[serde(default)]
struct LegacyServerConfig {
    bind: SocketAddr,
    storage: StorageConfig,
    sync: SyncConfig,
    auth: AuthConfig,
}

impl Default for LegacyServerConfig {
    fn default() -> Self {
        LegacyServerConfig {
            bind: "127.0.0.1:9473".parse().unwrap(),
            storage: StorageConfig::default(),
            sync: SyncConfig::default(),
            auth: AuthConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_reasonable_values() {
        let config = DaemonConfig::default();
        assert!(config.socket.to_str().unwrap().contains("mae-daemon"));
        assert_eq!(config.watcher_interval_ms, 500);
        assert_eq!(config.maintenance_interval_secs, 3600);
        assert_eq!(config.log_level, "info");
        assert_eq!(config.collab.bind.port(), 9473);
        assert_eq!(config.collab.storage.backend, "sqlite");
    }

    #[test]
    fn check_collab_catches_invalid() {
        let mut config = DaemonConfig::default();
        config.collab.storage.compact_threshold = 0;
        config.collab.storage.backend = "postgres".to_string();
        let issues = config.check_collab();
        assert_eq!(issues.len(), 2);
    }

    #[test]
    fn check_collab_valid_default() {
        let config = DaemonConfig::default();
        assert!(config.check_collab().is_empty());
    }
}
