//! Daemon configuration — loaded from `~/.config/mae/daemon.toml`.
//!
//! Also loads legacy `state-server.toml` for migration from the old
//! mae-state-server binary.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

/// Re-exported so `config::OAuthConfig` keeps resolving for every existing
/// caller after the split (pure code motion).
pub use super::oauth_config::OAuthConfig;

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
#[derive(Debug, Clone, Deserialize)]
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
    /// ADR-060 Phase C: named tenants sharing this daemon, each with its own
    /// cost-weighted request-points budget + concurrent-request cap. Zero
    /// entries (the default) means zero behavior change — no tenant
    /// resolves, so every request is treated exactly as it is today.
    pub tenant: Vec<TenantConfig>,
    /// ADR-061 Phase C: KB enrichment sweep settings, dispatched off the same
    /// `maintenance_tick` as the deterministic integrity scan.
    pub enrichment: EnrichmentConfig,
}

/// ADR-061 Phase C: background embedding-enrichment sweep settings. Disabled
/// by default (`enabled: false`) — an operator must opt in explicitly, since
/// this is the first daemon workload that spends real external API cost/time
/// on a background tick (the ADR's own Costs section names this directly).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EnrichmentConfig {
    /// Whether the enrichment sweep runs at all.
    pub enabled: bool,
    /// Provider name, consulted at the SAME `residency_permits_provider`
    /// gate ADR-048's chat/completion calls already use — "ollama" is the
    /// only local, day-one option (ADR-061 Phase A), matching a
    /// `LocalModelsOnly`-residency KB's requirement that no byte of its
    /// content ever reach a hosted provider.
    pub provider: String,
    /// Ollama's native API base URL.
    pub base_url: String,
    /// Optional bearer token, forwarded exactly as `crates/ai`'s
    /// `OllamaProvider` already does.
    pub api_key: Option<String>,
    /// Embedding model name (must be pulled in the target Ollama instance).
    pub model: String,
    /// ADR-031's cache-key third component — bump to force re-embedding of
    /// every node under a changed chunking strategy without disturbing
    /// entries under the old key.
    pub chunk_version: i64,
    /// Nodes embedded per `/api/embed` batch call, bounding both the request
    /// body size and how much work is lost if a single batch call fails.
    pub batch_size: usize,
    /// ADR-061 Phase D2: how long this daemon's ADR-033 advisory lease claim
    /// on the enrichment lock is valid before another daemon may claim it.
    /// Only consulted for a KB that is actually collab-shared (`shared`/
    /// `collab_id` set in the registry) — an unshared KB has no coordination
    /// to do at all. No mid-sweep renewal is implemented in this phase
    /// (named scope limit, not an oversight): the default is generous enough
    /// to cover a realistic sweep's embed-batch loop without one.
    pub lease_ttl_secs: u64,
}

impl Default for EnrichmentConfig {
    fn default() -> Self {
        EnrichmentConfig {
            enabled: false,
            provider: "ollama".to_string(),
            base_url: "http://localhost:11434".to_string(),
            api_key: None,
            model: "nomic-embed-text".to_string(),
            chunk_version: 1,
            batch_size: 16,
            lease_ttl_secs: 300,
        }
    }
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
    /// Drive the CRDT→Cozo projection from the doc-store change feed (ADR-029 B2/B3).
    ///
    /// Default **on**: without it, Cozo never learns about CRDT writes, so `kb/search`,
    /// `kb/health`, the webview and every `kb/query.*` caller serve stale or empty
    /// results while sync itself works perfectly — a silent split-brain between what
    /// peers see and what queries return. The switch exists for a pure relay, which
    /// carries no Cozo projection worth maintaining.
    pub projector_enabled: bool,
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
            projector_enabled: true,
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
    /// Require every KB content op to carry an ADR-036 signature, on the hub
    /// transport as well as the mesh.
    ///
    /// The mesh has always required one (`Transport::P2p`). The hub accepts
    /// unsigned ops as a **migration accommodation**, whose own code comment
    /// reads "hub migration: accept legacy unsigned" -- and which had no config
    /// flag, no deadline and no metric, so nobody could ever tell when it was
    /// safe to close. A migration path with no exit criterion is permanent by
    /// default.
    ///
    /// Defaults to `false` to preserve today's behaviour for existing
    /// deployments. Flip it once the accepted-unsigned counter has stayed at
    /// zero across a representative period: that counter is the evidence this
    /// flag exists to be decided by.
    ///
    /// The count is reported **by the running daemon at shutdown** — not by
    /// `daemon doctor`, which is a separate process and cannot read another
    /// process's counter.
    pub require_signed_content_ops: bool,
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
            // Preserve existing behaviour; see the field doc for the exit
            // criterion that decides when a deployment can flip it.
            require_signed_content_ops: false,
        }
    }
}

/// Configured paths that will never resolve, named rather than left to surface
/// later as a missing file.
fn unresolvable_path_issues(c: &CollabConfig) -> Vec<String> {
    let mut issues = Vec::new();
    for (label, raw) in [
        (
            "collab.auth.authorized_keys",
            c.auth.authorized_keys.as_deref(),
        ),
        ("collab.auth.keystore", c.auth.keystore.as_deref()),
        ("collab.auth.identity_dir", c.auth.identity_dir.as_deref()),
    ] {
        let Some(raw) = raw else { continue };
        if unexpanded_variable(&expand_config_path(raw).to_string_lossy()) {
            issues.push(format!(
                "{label} = '{raw}' still contains an unexpanded variable after expanding ~ \
                 and $HOME — it will never resolve. Use an absolute path."
            ));
        }
    }
    issues
}

/// Expand `~`, `$HOME` and `${HOME}` in a configured path.
///
/// @ai-caution: [config] TOML does no interpolation, and these fields were
/// consumed as bare `PathBuf::from`. A `daemon.toml` written with
/// `"${HOME}/.local/share/..."` — which reads perfectly naturally, and which a
/// real deployment used — produced a path with a LITERAL `${HOME}` component
/// that can never exist. The daemon then reported "authorized_keys is empty"
/// and disabled collab, describing the symptom and not the cause. Expand here,
/// and let `unexpanded_variable` below turn anything still unresolved into a
/// config error rather than a mystery.
pub(crate) fn expand_config_path(raw: &str) -> std::path::PathBuf {
    let home = std::env::var("HOME").ok();
    let mut out = raw.to_string();
    if let Some(h) = home.as_deref() {
        out = out.replace("${HOME}", h).replace("$HOME", h);
        if out == "~" {
            out = h.to_string();
        } else if let Some(rest) = out.strip_prefix("~/") {
            out = format!("{h}/{rest}");
        }
    }
    std::path::PathBuf::from(out)
}

/// The first unexpanded `$VAR`/`~` left in `raw`, if any.
///
/// A path that still contains one will not resolve, so it is a configuration
/// error worth naming rather than a file that mysteriously does not exist.
pub(crate) fn unexpanded_variable(raw: &str) -> bool {
    raw.contains('$') || raw == "~" || raw.starts_with("~/")
}

impl AuthConfig {
    /// Resolve the keystore path: the configured override, else the shared
    /// default (`$XDG_DATA_HOME/mae/collab/trusted_keys`).
    pub fn keystore_path(&self) -> Option<std::path::PathBuf> {
        self.keystore
            .as_ref()
            .map(|p| expand_config_path(p))
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
            .map(|p| expand_config_path(p))
            .or_else(mae_mcp::identity::default_collab_dir)
    }

    /// (mode = "key") Path to the authorized_keys file.
    pub fn authorized_keys_path(&self) -> Option<std::path::PathBuf> {
        self.authorized_keys
            .as_ref()
            .map(|p| expand_config_path(p))
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
            tenant: Vec::new(),
            enrichment: EnrichmentConfig::default(),
        }
    }
}

/// ADR-060 Phase C: one named tenant sharing this daemon. `instances` keys
/// the KB-socket path (Phase A instance addresses this tenant owns);
/// `principals` keys the collab/OAuth path (authenticated identities this
/// tenant owns) — the two-key design the ADR's Decision section requires,
/// since the KB Unix socket has no principal concept to key on uniformly.
#[derive(Debug, Clone, Deserialize)]
pub struct TenantConfig {
    /// Unique tenant name (referenced by `daemon/evict_tenant` and in logs).
    pub name: String,
    /// KB-socket instance addresses (names or UUIDs, `handler::instance_addr`'s
    /// address space) this tenant owns. May be empty for a collab/OAuth-only
    /// tenant.
    #[serde(default)]
    pub instances: Vec<String>,
    /// Authenticated principals (Ed25519 fingerprint or `psk:<keyid>`) this
    /// tenant owns. May be empty for a KB-socket-only tenant.
    #[serde(default)]
    pub principals: Vec<String>,
    #[serde(default)]
    pub quota: TenantQuotaConfig,
}

/// Per-tenant resource limits (ADR-060 Phase C).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TenantQuotaConfig {
    /// Concurrent in-flight request cap for this tenant (0 = unlimited),
    /// same `ConnLimiter` shape as `kb_socket.max_connections`/
    /// `collab.max_connections`, instantiated once per tenant.
    pub max_connections: usize,
    /// Cost-weighted request-points budget per fixed 60-second window (0 =
    /// unlimited). Reads cost 1 point, scans cost 3, mutations cost 5 — see
    /// `tenant::RequestCost`.
    pub budget_per_minute: u32,
    /// Result-size overage threshold in bytes. Part of the `[[tenant]]`
    /// schema (the ADR's cost model reserves +2 points for an over-size
    /// response) but not yet enforced by any call site in this
    /// implementation pass — no `handler.rs` arm currently measures its own
    /// response size before returning it. Configurable now so a future pass
    /// wiring enforcement doesn't also need a config-schema/migration change.
    pub max_result_bytes: usize,
    /// Seconds of no activity before this tenant's live quota/connection
    /// state is idle-evicted by `DaemonScheduler::run_maintenance_tick` (0 =
    /// never idle-evict). Eviction is a pure cache-drop: the next request
    /// rebuilds fresh state (see `TenantRegistry::evict_idle`).
    pub idle_evict_secs: u64,
}

impl Default for TenantQuotaConfig {
    fn default() -> Self {
        TenantQuotaConfig {
            max_connections: 32,
            budget_per_minute: 1000,
            max_result_bytes: 4_194_304,
            idle_evict_secs: 1800,
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

/// The resources a single daemon instance owns exclusively — see
/// [`DaemonConfig::instance_paths`].
#[derive(Debug, Clone)]
pub struct InstancePaths {
    pub socket: PathBuf,
    pub data_dir: PathBuf,
    pub collab_data_dir: PathBuf,
    /// `None` when collab is disabled (no port is claimed at all).
    pub collab_bind: Option<SocketAddr>,
    /// `None` when the OAuth listener is disabled.
    pub oauth_bind: Option<SocketAddr>,
    pub identity_dir: Option<PathBuf>,
    pub authorized_keys: Option<PathBuf>,
    pub keystore: Option<PathBuf>,
}

impl InstancePaths {
    /// `(label, value)` for every resource, in reporting order. Resources that
    /// are not claimed at all are omitted.
    pub fn labelled(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![
            ("socket", self.socket.display().to_string()),
            ("data_dir", self.data_dir.display().to_string()),
            (
                "collab data_dir",
                self.collab_data_dir.display().to_string(),
            ),
        ];
        if let Some(a) = self.collab_bind {
            out.push(("collab.bind", a.to_string()));
        }
        if let Some(a) = self.oauth_bind {
            out.push(("oauth.bind", a.to_string()));
        }
        for (label, p) in [
            ("identity_dir", &self.identity_dir),
            ("authorized_keys", &self.authorized_keys),
            ("keystore", &self.keystore),
        ] {
            if let Some(p) = p {
                out.push((label, p.display().to_string()));
            }
        }
        out
    }

    /// Resources this instance shares with `other` — empty iff the two can run
    /// side by side without interfering.
    pub fn conflicts_with(&self, other: &InstancePaths) -> Vec<String> {
        let (a, b) = (self.labelled(), other.labelled());
        a.iter()
            .filter_map(|(label, value)| {
                b.iter()
                    .find(|(l, v)| l == label && v == value)
                    .map(|_| format!("{label} = {value}"))
            })
            .collect()
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

    /// The collab data directory, WITHOUT creating it. Use this for reporting
    /// and validation; `resolve_collab_data_dir` is the one that creates.
    pub fn collab_data_dir(&self) -> PathBuf {
        self.collab
            .storage
            .data_dir
            .clone()
            .unwrap_or_else(|| self.effective_data_dir().join("collab"))
    }

    /// Resolve the collab data directory, creating it if needed.
    pub fn resolve_collab_data_dir(&self) -> PathBuf {
        let dir = self.collab_data_dir();
        if !dir.exists() {
            let _ = std::fs::create_dir_all(&dir);
        }
        dir
    }

    /// Every resource this daemon instance holds EXCLUSIVELY. Two instances on
    /// one host must not share any of them.
    ///
    /// @ai-caution: [multi-instance] The identity, authorized-keys and keystore
    /// paths do NOT derive from `data_dir` — they default to the shared
    /// `$XDG_DATA_HOME/mae/collab/` regardless of it. So a staging and a
    /// production instance distinguished only by `data_dir` + ports still share
    /// one `authorized_keys` file, and authorising a peer for staging silently
    /// authorises it for production too. That default is deliberate for the
    /// single-instance case (one host, one identity) and is NOT changed here —
    /// moving it would relocate existing operators' identity keys, and losing an
    /// identity key loses access to every shared KB with no recovery
    /// (`IDENTITY_BACKUP_ADVISORY`). Instead it is made VISIBLE: reported by
    /// `--check-config` and `doctor`, and asserted distinct by
    /// `two_instances_do_not_share_any_path`.
    pub fn instance_paths(&self) -> InstancePaths {
        InstancePaths {
            socket: self.socket.clone(),
            data_dir: self.effective_data_dir(),
            collab_data_dir: self.collab_data_dir(),
            collab_bind: self.collab.enabled.then_some(self.collab.bind),
            oauth_bind: self.oauth.enabled.then_some(self.oauth.bind),
            identity_dir: self.collab.auth.identity_dir(),
            authorized_keys: self.collab.auth.authorized_keys_path(),
            keystore: self.collab.auth.keystore_path(),
        }
    }

    /// Validate collab configuration and return issues.
    pub fn check_collab(&self) -> Vec<String> {
        let mut issues = Vec::new();
        let c = &self.collab;

        issues.extend(crate::config_guards::unauthenticated_bind_issues(c));

        issues.extend(unresolvable_path_issues(c));

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

    /// Validate `[[tenant]]` configuration: duplicate tenant names, and an
    /// instance address or principal claimed by more than one tenant (both
    /// are silent-data-leak shaped bugs if left unchecked — the second
    /// tenant registered would win the routing table race, silently pooling
    /// two supposedly-isolated tenants' quota into one).
    pub fn check_tenants(&self) -> Vec<String> {
        let mut issues = Vec::new();
        let mut seen_names = std::collections::HashSet::new();
        let mut seen_instances = std::collections::HashMap::new();
        let mut seen_principals = std::collections::HashMap::new();

        for t in &self.tenant {
            if t.name.is_empty() {
                issues.push("tenant.name must not be empty".to_string());
            }
            if !seen_names.insert(t.name.clone()) {
                issues.push(format!("duplicate tenant name '{}'", t.name));
            }
            for inst in &t.instances {
                if let Some(prior) = seen_instances.insert(inst.clone(), t.name.clone()) {
                    issues.push(format!(
                        "instance '{inst}' claimed by both tenant '{prior}' and tenant '{}'",
                        t.name
                    ));
                }
            }
            for p in &t.principals {
                if let Some(prior) = seen_principals.insert(p.clone(), t.name.clone()) {
                    issues.push(format!(
                        "principal '{p}' claimed by both tenant '{prior}' and tenant '{}'",
                        t.name
                    ));
                }
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

    fn tenant(name: &str, instances: &[&str], principals: &[&str]) -> TenantConfig {
        TenantConfig {
            name: name.to_string(),
            instances: instances.iter().map(|s| s.to_string()).collect(),
            principals: principals.iter().map(|s| s.to_string()).collect(),
            quota: TenantQuotaConfig::default(),
        }
    }

    #[test]
    fn zero_tenant_tables_is_valid_and_matches_the_default() {
        let config = DaemonConfig::default();
        assert!(config.tenant.is_empty());
        assert!(config.check_tenants().is_empty());
    }

    fn config_with_tenants(tenants: Vec<TenantConfig>) -> DaemonConfig {
        DaemonConfig {
            tenant: tenants,
            ..Default::default()
        }
    }

    #[test]
    fn duplicate_tenant_name_is_rejected() {
        let config = config_with_tenants(vec![
            tenant("team-a", &["kb-1"], &[]),
            tenant("team-a", &["kb-2"], &[]),
        ]);
        let issues = config.check_tenants();
        assert!(
            issues.iter().any(|i| i.contains("duplicate tenant name")),
            "got: {issues:?}"
        );
    }

    #[test]
    fn instance_claimed_by_two_tenants_is_rejected() {
        let config = config_with_tenants(vec![
            tenant("team-a", &["shared-kb"], &[]),
            tenant("team-b", &["shared-kb"], &[]),
        ]);
        let issues = config.check_tenants();
        assert!(
            issues
                .iter()
                .any(|i| i.contains("shared-kb") && i.contains("team-a") && i.contains("team-b")),
            "got: {issues:?}"
        );
    }

    #[test]
    fn principal_claimed_by_two_tenants_is_rejected() {
        let config = config_with_tenants(vec![
            tenant("team-a", &[], &["ed25519:same"]),
            tenant("team-b", &[], &["ed25519:same"]),
        ]);
        let issues = config.check_tenants();
        assert!(
            issues.iter().any(|i| i.contains("ed25519:same")),
            "got: {issues:?}"
        );
    }

    #[test]
    fn disjoint_tenants_pass_validation() {
        let config = config_with_tenants(vec![
            tenant("team-a", &["kb-1"], &["ed25519:a"]),
            tenant("team-b", &["kb-2"], &["ed25519:b"]),
        ]);
        assert!(config.check_tenants().is_empty());
    }

    /// Round-trip the exact `[[tenant]]` TOML shape from the ADR's own
    /// example — a config schema that parses in isolation but silently
    /// diverges from what the ADR documents (a field renamed, a nesting
    /// level wrong) would still pass every other test here.
    #[test]
    fn parses_the_documented_toml_tenant_table_shape() {
        let toml_str = r#"
[[tenant]]
name = "team-a"
instances = ["team-a-kb", "shared-ref"]
principals = ["ed25519:AbCd...", "psk:teamA-key1"]

[tenant.quota]
max_connections = 32
budget_per_minute = 1000
max_result_bytes = 4194304
idle_evict_secs = 1800
"#;
        let config: DaemonConfig = toml::from_str(toml_str).expect("valid [[tenant]] TOML");
        assert_eq!(config.tenant.len(), 1);
        let t = &config.tenant[0];
        assert_eq!(t.name, "team-a");
        assert_eq!(t.instances, vec!["team-a-kb", "shared-ref"]);
        assert_eq!(t.principals, vec!["ed25519:AbCd...", "psk:teamA-key1"]);
        assert_eq!(t.quota.max_connections, 32);
        assert_eq!(t.quota.budget_per_minute, 1000);
        assert_eq!(t.quota.max_result_bytes, 4_194_304);
        assert_eq!(t.quota.idle_evict_secs, 1800);
        assert!(config.check_tenants().is_empty());
    }
}

#[cfg(test)]
mod path_expansion_tests {
    use super::*;

    /// **The bug this closes.** TOML does no interpolation, and these fields
    /// were consumed as bare `PathBuf::from`, so a `daemon.toml` written with
    /// `"${HOME}/..."` — which reads perfectly naturally — produced a path with
    /// a literal `${HOME}` component that can never exist. The daemon then
    /// reported "authorized_keys is empty" and disabled collab, naming the
    /// symptom and not the cause. Observed on a real deployment.
    #[test]
    fn home_forms_expand_in_configured_paths() {
        let home = std::env::var("HOME").expect("HOME set in tests");
        for raw in [
            "${HOME}/.local/share/mae/x/authorized_keys",
            "$HOME/.local/share/mae/x/authorized_keys",
            "~/.local/share/mae/x/authorized_keys",
        ] {
            let got = expand_config_path(raw);
            assert!(
                got.starts_with(&home),
                "{raw} must expand to an absolute path under $HOME, got {}",
                got.display()
            );
            assert!(
                !got.to_string_lossy().contains('$') && !got.to_string_lossy().contains('~'),
                "{raw} left an unexpanded marker: {}",
                got.display()
            );
        }
    }

    /// **The wiring, not just the helper.** An earlier version of these tests
    /// called `expand_config_path` directly, so removing its use from the
    /// getters left every test green — the helper was correct and unreached.
    /// These go through the accessors the daemon actually calls.
    #[test]
    fn the_path_accessors_expand_what_the_daemon_reads() {
        let home = std::env::var("HOME").expect("HOME set in tests");
        let auth = AuthConfig {
            authorized_keys: Some("${HOME}/mae/authorized_keys".to_string()),
            keystore: Some("$HOME/mae/trusted_keys".to_string()),
            identity_dir: Some("~/mae/collab".to_string()),
            ..AuthConfig::default()
        };

        for (label, got) in [
            ("authorized_keys", auth.authorized_keys_path()),
            ("keystore", auth.keystore_path()),
            ("identity_dir", auth.identity_dir()),
        ] {
            let p = got.unwrap_or_else(|| panic!("{label} must resolve"));
            assert!(
                p.starts_with(&home),
                "{label} was not expanded by the accessor: {}",
                p.display()
            );
        }
    }

    /// An absolute path is passed through untouched — expansion must not
    /// rewrite a path that was already correct.
    #[test]
    fn an_absolute_path_is_unchanged() {
        let raw = "/etc/mae/authorized_keys";
        assert_eq!(expand_config_path(raw), std::path::PathBuf::from(raw));
    }

    /// A `~` that is not a home prefix is not a home reference — `~backup/x` is
    /// a real relative directory name and must not be mangled.
    #[test]
    fn a_tilde_that_is_not_a_home_prefix_is_left_alone() {
        assert_eq!(
            expand_config_path("~backup/keys"),
            std::path::PathBuf::from("~backup/keys")
        );
    }

    /// A variable we cannot expand becomes a NAMED config error instead of a
    /// file that mysteriously does not exist.
    #[test]
    fn an_unexpandable_variable_is_reported_as_a_config_issue() {
        let mut config = DaemonConfig::default();
        config.collab.auth.mode = "key".to_string();
        config.collab.auth.authorized_keys = Some("$XDG_DATA_HOME/mae/authorized_keys".to_string());

        let issues = config.check_collab();
        assert!(
            issues
                .iter()
                .any(|i| i.contains("unexpanded variable")
                    && i.contains("collab.auth.authorized_keys")),
            "an unresolvable path must be named, got: {issues:?}"
        );
    }
}
