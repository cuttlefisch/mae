//! Daemon configuration — loaded from `~/.config/mae/daemon.toml`.

use serde::Deserialize;
use std::path::PathBuf;

/// Top-level daemon configuration.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    /// Unix socket path for client connections.
    pub socket: PathBuf,
    /// Bind address for TCP listener (empty = disabled).
    pub bind: String,
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
}

impl Default for DaemonConfig {
    fn default() -> Self {
        let runtime_dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
        Self {
            socket: runtime_dir.join("mae-daemon.sock"),
            bind: String::new(),
            watcher_interval_ms: 500,
            maintenance_interval_secs: 3600,
            sync_interval_secs: 30,
            decay_interval_secs: 3600,
            health_interval_secs: 300,
            data_dir: None,
            log_level: "info".to_string(),
        }
    }
}

impl DaemonConfig {
    /// Load config from `~/.config/mae/daemon.toml`, falling back to defaults.
    pub fn load() -> Self {
        let config_path = dirs::config_dir().map(|d| d.join("mae").join("daemon.toml"));

        if let Some(path) = config_path {
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(contents) => match toml::from_str(&contents) {
                        Ok(config) => return config,
                        Err(e) => {
                            eprintln!("Warning: failed to parse {}: {}", path.display(), e);
                        }
                    },
                    Err(e) => {
                        eprintln!("Warning: failed to read {}: {}", path.display(), e);
                    }
                }
            }
        }
        Self::default()
    }

    /// Effective KB data directory (explicit config or XDG default).
    pub fn effective_data_dir(&self) -> PathBuf {
        self.data_dir.clone().unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("mae")
        })
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
    }
}
