//! Configuration loading for mae-state-server.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

/// Top-level server configuration (from state-server.toml).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// TCP bind address.
    pub bind: SocketAddr,
    /// Optional Unix socket path.
    pub unix_socket: Option<PathBuf>,
    /// Storage configuration.
    pub storage: StorageConfig,
    /// Sync engine configuration.
    pub sync: SyncConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            bind: "127.0.0.1:9473".parse().unwrap(),
            unix_socket: None,
            storage: StorageConfig::default(),
            sync: SyncConfig::default(),
        }
    }
}

/// Storage backend configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Backend type (currently only "sqlite").
    pub backend: String,
    /// Data directory path. Defaults to XDG data dir.
    pub data_dir: Option<PathBuf>,
    /// WAL compaction threshold (number of updates per document).
    pub compact_threshold: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        StorageConfig {
            backend: "sqlite".to_string(),
            data_dir: None,
            compact_threshold: 500,
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
}

impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig {
            heartbeat_interval_secs: 30,
            max_documents: 1000,
            idle_eviction_secs: 300,
            compaction_interval_secs: 60,
        }
    }
}

impl ServerConfig {
    /// Load config from a TOML file. Returns default config if file doesn't exist.
    pub fn load(path: Option<&PathBuf>) -> Result<Self, String> {
        let path = match path {
            Some(p) => p.clone(),
            None => default_config_path(),
        };

        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
        toml::from_str(&content).map_err(|e| format!("failed to parse {}: {}", path.display(), e))
    }

    /// Resolve the data directory, creating it if needed.
    pub fn resolve_data_dir(&self) -> PathBuf {
        let dir = self
            .storage
            .data_dir
            .clone()
            .unwrap_or_else(default_data_dir);
        if !dir.exists() {
            let _ = std::fs::create_dir_all(&dir);
        }
        dir
    }

    /// Validate configuration and return a report.
    pub fn check(&self) -> Vec<String> {
        let mut issues = Vec::new();

        if self.storage.compact_threshold == 0 {
            issues.push("storage.compact_threshold must be > 0".to_string());
        }

        if self.sync.heartbeat_interval_secs == 0 {
            issues.push("sync.heartbeat_interval_secs must be > 0".to_string());
        }

        if self.sync.max_documents == 0 {
            issues.push("sync.max_documents must be > 0".to_string());
        }

        if self.storage.backend != "sqlite" {
            issues.push(format!(
                "unknown storage backend '{}' (only 'sqlite' is supported)",
                self.storage.backend
            ));
        }

        issues
    }
}

/// Default config file path: ~/.config/mae/state-server.toml
fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mae")
        .join("state-server.toml")
}

/// Default data directory: ~/.local/share/mae/state-server/
fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mae")
        .join("state-server")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let config = ServerConfig::default();
        assert!(config.check().is_empty());
        assert_eq!(config.bind.port(), 9473);
        assert_eq!(config.storage.backend, "sqlite");
    }

    #[test]
    fn parse_toml_config() {
        let toml_str = r#"
bind = "0.0.0.0:9999"

[storage]
backend = "sqlite"
compact_threshold = 1000

[sync]
heartbeat_interval_secs = 15
max_documents = 500
"#;
        let config: ServerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.bind.port(), 9999);
        assert_eq!(config.storage.compact_threshold, 1000);
        assert_eq!(config.sync.heartbeat_interval_secs, 15);
        assert_eq!(config.sync.max_documents, 500);
    }

    #[test]
    fn check_catches_invalid() {
        let mut config = ServerConfig::default();
        config.storage.compact_threshold = 0;
        config.storage.backend = "postgres".to_string();
        let issues = config.check();
        assert_eq!(issues.len(), 2);
    }

    #[test]
    fn missing_config_returns_default() {
        let config = ServerConfig::load(Some(&PathBuf::from("/nonexistent/path.toml"))).unwrap();
        assert_eq!(config.bind.port(), 9473);
    }
}
