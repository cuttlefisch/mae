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
    /// RESERVED — not consumed by any task today (CRDT sync is event-driven, not
    /// polled). Kept for forward-compat + config stability; see issue #263.
    pub sync_interval_secs: u64,
    /// RESERVED — not consumed by any task today. See issue #263.
    pub decay_interval_secs: u64,
    /// Health check interval in seconds.
    pub health_interval_secs: u64,
    /// KB data directory (XDG-compliant default).
    pub data_dir: Option<PathBuf>,
    /// Log level filter (e.g. "info", "mae_daemon=debug,warn").
    pub log_level: String,
    /// Collaboration server settings (absorbed from mae-state-server).
    pub collab: CollabConfig,
    /// OAuth 2.1 resource-server settings (ADR-052). A dedicated HTTPS
    /// listener, deliberately separate from `collab` (which stays
    /// mTLS/PSK-authenticated JSON-RPC) — the MCP spec scopes OAuth to
    /// HTTP-based transports specifically.
    pub oauth: OAuthConfig,
    /// KB Unix-socket connection hardening (ADR-054). This socket is local,
    /// unauthenticated, filesystem-permissions-only trust (SECURITY.md) — no
    /// per-principal/per-IP sub-limits apply here (there is no principal or
    /// IP on a Unix domain socket), only a total connection cap + idle
    /// timeout, mirroring the collab TCP listener's own `#342` hardening.
    pub kb_socket: KbSocketConfig,
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
    /// P2P daemon-mesh configuration (ADR-025).
    pub p2p: P2pConfig,
    /// Hard cap on concurrent TCP connections (accepted sockets, authenticated or
    /// not) on the collab listener. 0 = unlimited. #342: before this, a client that
    /// opened the connection and never completed its handshake — deliberately, or
    /// just a stalled network — parked a task+socket forever, with nothing bounding
    /// how many could accumulate; combined with the handshake timeout below, this
    /// closes the one genuinely open-ended resource on the whole hub-model surface.
    pub max_connections: usize,
}

impl Default for CollabConfig {
    fn default() -> Self {
        CollabConfig {
            enabled: true,
            bind: "127.0.0.1:9473".parse().unwrap(),
            storage: StorageConfig::default(),
            sync: SyncConfig::default(),
            auth: AuthConfig::default(),
            p2p: P2pConfig::default(),
            // Generous default for a small/self-hosted team daemon; raise for a
            // larger deployment, or set 0 to disable the cap entirely.
            max_connections: 256,
        }
    }
}

/// P2P daemon-mesh configuration (ADR-025). Opt-in. The mesh reuses the
/// `[collab.auth]` key-mode Ed25519 identity as its node identity, so a peer's
/// iroh `EndpointId` is exactly its `authorized_keys` principal — there is no
/// separate P2P identity to manage.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct P2pConfig {
    /// Join the iroh P2P mesh (alongside the TCP listener). Requires
    /// `collab.auth.mode = "key"` — the mesh has no PSK/anonymous path.
    pub enabled: bool,
    /// Relay selection: `"default"` (public n0 relays — global discovery + NAT
    /// hole-punch), `"disabled"` (LAN/direct only, the mDNS fast-path), or a
    /// self-hosted relay URL.
    pub relay: String,
    /// Connection-trust gate (ADR-025):
    /// - `"authorized_keys"` (**default**): hard-reject any peer not already in
    ///   `authorized_keys` at connect — a closed mesh whose peer set the admin
    ///   manages. Conservative / security-forward.
    /// - `"open"`: admit any iroh-authenticated peer to *connect* (we always know
    ///   who via the verified `remote_id`); per-KB access stays fully mediated by
    ///   membership + JoinPolicy. Enables the frictionless magnet-link join.
    pub connection_gate: String,
    /// Hard cap on concurrent mesh connections (0 = unlimited), RAII-counted
    /// via `conn_limit::ConnLimiter` (ADR-054) — same shape as
    /// `collab.max_connections`, bounding an authenticated-but-otherwise-silent
    /// peer parking a task forever alongside the existing `accept_bi` timeout.
    pub max_connections: usize,
}

impl Default for P2pConfig {
    fn default() -> Self {
        P2pConfig {
            enabled: false,
            relay: "default".to_string(),
            connection_gate: "authorized_keys".to_string(),
            max_connections: 256,
        }
    }
}

impl P2pConfig {
    /// Whether the connection gate is `open` (admit any authenticated peer to
    /// connect; access stays membership-gated). Unknown values fall back to the
    /// conservative closed gate.
    pub fn gate_open(&self) -> bool {
        self.connection_gate == "open"
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
    /// Number of SQLite connections opened in WAL mode to the same file
    /// (`SqliteBackend::open_with_pool_size`, ADR-054) — was hardcoded to 4;
    /// raising it gives concurrent writers more shards to spread across
    /// under load, at the cost of one more open file descriptor/connection
    /// per shard.
    pub shard_count: usize,
}

impl Default for StorageConfig {
    fn default() -> Self {
        StorageConfig {
            backend: "sqlite".to_string(),
            data_dir: None,
            compact_threshold: 500,
            max_wal_entries: 5000,
            shard_count: 4,
        }
    }
}

/// Sync engine configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SyncConfig {
    /// RESERVED — not consumed by any server-side task today. The live client
    /// keepalive is the EDITOR-side `collab_heartbeat_interval` option, not this.
    /// Kept for forward-compat + config stability; see issue #263.
    pub heartbeat_interval_secs: u64,
    /// Working-set cap: max concurrent yrs documents held in memory (LRU-evicted;
    /// evicted docs lazily reload from SQLite on next access — a cap, not a limit
    /// on KB size). NOTE: each KB **node** is its own doc (`kb:{node}`) plus one
    /// `kbc:{kb}` collection doc, so a 2,800-node KB is ~2,801 docs — set this
    /// above your largest KB's node count to avoid reload churn during active sync.
    pub max_documents: usize,
    /// Idle eviction timeout in seconds (0 = disabled).
    pub idle_eviction_secs: u64,
    /// Background compaction interval in seconds.
    pub compaction_interval_secs: u64,
    /// Hard cap on a single sync-update payload (bytes; 0 = built-in default). A
    /// DoS/allocation safety bound — an over-cap update is REJECTED, not truncated,
    /// so a large node's full-state push (e.g. on reseal/share) must fit under it.
    /// Raise for KBs with large individual nodes.
    pub max_update_size_bytes: usize,
    /// Maximum document size in bytes before warning (0 = unlimited).
    pub max_document_size_bytes: usize,
}

impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig {
            heartbeat_interval_secs: 30,
            // Covers a few-thousand-node KB out of the box (one doc per node); a
            // pure LRU cap, so raising it only costs memory when the working set
            // actually exceeds it. Tune up in daemon.toml for very large KBs.
            max_documents: 4096,
            idle_eviction_secs: 300,
            compaction_interval_secs: 60,
            // 4 MiB: headroom for a large node's full-state push while still bounding
            // per-message allocation. Over-cap updates are rejected — see the field doc.
            max_update_size_bytes: 4_194_304,    // 4 MiB
            max_document_size_bytes: 10_485_760, // 10 MB
        }
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            // Shared resolver — clients (CLI + editor) default to the SAME path.
            socket: mae_mcp::daemon_client::default_daemon_socket(),
            watcher_interval_ms: 500,
            maintenance_interval_secs: 3600,
            sync_interval_secs: 30,
            decay_interval_secs: 3600,
            health_interval_secs: 300,
            data_dir: None,
            log_level: "info".to_string(),
            collab: CollabConfig::default(),
            oauth: OAuthConfig::default(),
            kb_socket: KbSocketConfig::default(),
        }
    }
}

/// OAuth 2.1 resource-server configuration (ADR-052). Never on by default
/// (principle #12 — daemon value is earned by an explicit need, not
/// assumed) — an operator opts in by setting `enabled = true` and pointing
/// `jwks_url`/`issuer` at their chosen external authorization server.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OAuthConfig {
    /// Whether the OAuth HTTPS listener starts at all.
    pub enabled: bool,
    /// TCP bind address for the OAuth-protected HTTPS listener — separate
    /// from `collab.bind` (the mTLS/PSK JSON-RPC listener).
    pub bind: SocketAddr,
    /// This server's own canonical resource URI (RFC 8707 `resource` /
    /// RFC 9728 protected-resource identifier). MUST be set by the
    /// operator to a real, stable, externally-reachable URL before
    /// `enabled = true` is meaningful — there is no safe default to infer
    /// this from.
    pub canonical_resource_uri: String,
    /// URL to fetch the authorization server's JWKS from.
    pub jwks_url: String,
    /// The authorization server's issuer, checked against each token's
    /// `iss` claim. Strongly recommended to set; `None` skips issuer
    /// validation.
    pub issuer: Option<String>,
    /// Which JWT claim becomes the mapped `kb_access` principal.
    pub principal_claim: String,
    /// PEM-encoded TLS certificate chain path for the HTTPS listener.
    pub cert_path: PathBuf,
    /// PEM-encoded TLS private key path for the HTTPS listener.
    pub key_path: PathBuf,
    /// ADR-053/Phase G (#382): whether the `kb/query.get`/`search`/`graph`/
    /// `capabilities` RPC family is reachable on this listener at all.
    /// Independently toggleable from `enabled` — an operator may want the
    /// OAuth listener up (e.g. for the plain bearer-verification diagnostic)
    /// without exposing the KB-query surface yet. Default false (principle
    /// #12 — never on by default). Also requires `collab.enabled` (a
    /// `DocStore` must exist to serve from — see `main.rs`'s
    /// `doc_store_for_query` wiring); this flag alone does not create one.
    pub kb_query_enabled: bool,
    /// Cap on a single `kb/query.get` response body's node-body size, bytes
    /// (unencrypted KBs only — an E2E KB's response is raw ciphertext,
    /// capped by nothing since the daemon can't inspect it to truncate it
    /// meaningfully; the op-set itself is already bounded elsewhere).
    /// Prevents a single "get" from being a disguised bulk-content vector.
    pub kb_query_max_body_bytes: usize,
    /// Cap on how many nodes a single `kb/query.search` call will
    /// materialize and scan (unencrypted KBs only) — bounds server-side
    /// cost and is the literal "prevent search from being a disguised
    /// full-dump vector" cap (ADR-053 decision 3), independent of
    /// `kb_query_max_search_results` below (a cap on the *scan*, not just
    /// the returned count).
    pub kb_query_max_scan_nodes: usize,
    /// Cap on the number of results a single `kb/query.search` call returns.
    pub kb_query_max_search_results: usize,
}

impl Default for OAuthConfig {
    fn default() -> Self {
        OAuthConfig {
            enabled: false,
            bind: "127.0.0.1:9474".parse().unwrap(),
            canonical_resource_uri: String::new(),
            jwks_url: String::new(),
            issuer: None,
            principal_claim: "sub".to_string(),
            cert_path: PathBuf::new(),
            key_path: PathBuf::new(),
            kb_query_enabled: false,
            kb_query_max_body_bytes: 65_536,
            kb_query_max_scan_nodes: 500,
            kb_query_max_search_results: 20,
        }
    }
}

/// KB Unix-socket connection hardening (ADR-054). This is the daemon's local,
/// filesystem-permissions-only-trust listener (SECURITY.md) that every
/// locally-connected frontend's routine `kb_search`/`kb_get`/etc. calls
/// actually use — unlike `collab`/`oauth`, there is no per-principal or
/// per-IP identity to sub-limit against here (a Unix domain socket carries
/// neither), so this config deliberately offers only a total connection cap
/// and an idle-read timeout, not the finer-grained knobs the network-facing
/// listeners have.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct KbSocketConfig {
    /// Hard cap on concurrent connections (0 = unlimited), RAII-counted via
    /// `conn_limit::ConnLimiter` — same shape as `collab.max_connections`.
    pub max_connections: usize,
    /// Seconds a connection may sit with no request in flight before the
    /// server closes it (0 = disabled). `DaemonClient` keeps one persistent
    /// connection open for the whole editor session and transparently
    /// reconnects on I/O error, so a server-side idle-close is self-healing
    /// from the client's perspective, not a hard failure. Default is
    /// generous (mirrors `collab.sync.idle_eviction_secs`'s own default)
    /// since a genuinely idle-but-still-open editor session is normal.
    pub idle_timeout_secs: u64,
}

impl Default for KbSocketConfig {
    fn default() -> Self {
        KbSocketConfig {
            max_connections: 256,
            idle_timeout_secs: 300,
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

        if c.p2p.enabled {
            // The mesh authenticates peers by their Ed25519 key (reusing the
            // key-mode trusted-peer identity), so it has no PSK/anonymous path.
            if c.auth.mode != "key" {
                issues.push(format!(
                    "collab.p2p.enabled = true requires collab.auth.mode = 'key' (the mesh \
                     authenticates peers by their Ed25519 key; mode is '{}')",
                    c.auth.mode
                ));
            }
            // Catch a malformed relay early (same parse used at activation).
            if let Err(e) = crate::p2p::relay_mode_from_config(&c.p2p.relay) {
                issues.push(e);
            }
            // Validate the connection-trust gate.
            if !matches!(c.p2p.connection_gate.as_str(), "open" | "authorized_keys") {
                issues.push(format!(
                    "unknown collab.p2p.connection_gate '{}' (supported: 'authorized_keys', 'open')",
                    c.p2p.connection_gate
                ));
            }
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

    #[test]
    fn p2p_disabled_by_default() {
        let config = DaemonConfig::default();
        assert!(!config.collab.p2p.enabled);
        assert_eq!(config.collab.p2p.relay, "default");
    }

    #[test]
    fn p2p_enabled_requires_key_mode() {
        let mut config = DaemonConfig::default();
        config.collab.p2p.enabled = true;
        // Default auth mode is "none" → the mesh has no way to authenticate peers.
        let issues = config.check_collab();
        assert!(
            issues
                .iter()
                .any(|i| i.contains("collab.auth.mode = 'key'")),
            "enabling the mesh without key-mode auth must be flagged; got: {issues:?}"
        );
    }

    #[test]
    fn p2p_rejects_malformed_relay() {
        let mut config = DaemonConfig::default();
        config.collab.p2p.enabled = true;
        config.collab.auth.mode = "key".to_string();
        config.collab.p2p.relay = "not a relay".to_string();
        let issues = config.check_collab();
        assert!(
            issues.iter().any(|i| i.contains("collab.p2p.relay")),
            "a malformed relay value must be flagged; got: {issues:?}"
        );
    }

    #[test]
    fn p2p_connection_gate_defaults_to_closed() {
        let config = DaemonConfig::default();
        // Security-forward default: hard-reject unknown peers (Phase-1 behavior).
        assert_eq!(config.collab.p2p.connection_gate, "authorized_keys");
        assert!(!config.collab.p2p.gate_open());
    }

    #[test]
    fn p2p_rejects_unknown_connection_gate() {
        let mut config = DaemonConfig::default();
        config.collab.p2p.enabled = true;
        config.collab.auth.mode = "key".to_string();
        config.collab.p2p.connection_gate = "wide-open".to_string();
        let issues = config.check_collab();
        assert!(
            issues.iter().any(|i| i.contains("connection_gate")),
            "an unknown connection_gate must be flagged; got: {issues:?}"
        );
        // The valid values pass.
        for gate in ["open", "authorized_keys"] {
            config.collab.p2p.connection_gate = gate.to_string();
            assert!(
                !config
                    .check_collab()
                    .iter()
                    .any(|i| i.contains("connection_gate")),
                "'{gate}' should be accepted"
            );
        }
    }
}
