//! Knowledge base state extracted from Editor.
//! All fields were previously `kb_*` / `capture_state` on Editor;
//! now accessed via `editor.kb.*`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use mae_kb::query::KbQueryLayer;

use super::kb_ops::KbWatcherStats;
use super::CaptureState;

/// Synchronous daemon control-socket operations the editor needs but cannot
/// perform itself — `mae-core` deliberately does not depend on `mae-mcp` /
/// `DaemonClient`. The binary injects a concrete `DaemonClient`-backed
/// implementation, exactly like [`KbContext::set_daemon_query_layer`].
///
/// This is the **single backend** behind every P2P lifecycle surface (editor
/// command, Scheme primitive, MCP tool) — the contract that lets a human (editor
/// or CLI) and an AI peer drive the mesh identically (ADR-025 §"Driving
/// surfaces"). New P2P actions add one method here + thin shims, never
/// surface-specific logic.
pub trait DaemonControl: Send + Sync {
    /// Mint a shareable P2P join ticket ("magnet link") for `kb_id` via the
    /// daemon's `p2p/mint_ticket` control method. Returns the `mae://join/…`
    /// string, or a human-readable error (daemon down, P2P disabled, …).
    fn mint_p2p_ticket(&self, kb_id: &str) -> Result<String, String>;
}

/// Knowledge base context: backing store, federation, watchers, and config.
pub struct KbContext {
    /// Primary knowledge base instance (manual + user notes + AI-facing kb_* tools).
    pub primary: mae_kb::KnowledgeBase,
    /// Persistent backing store (CozoDB). When present, all KB mutations
    /// are written through to this store. Loaded at startup, persists across sessions.
    pub store: Option<Arc<dyn mae_kb::KbStore>>,
    /// Typed CozoDB store handle (same as `store`, but typed for query layer construction).
    pub primary_cozo: Option<Arc<mae_kb::CozoKbStore>>,
    /// Pre-built manual KB store (read-only, shipped with MAE binary).
    pub manual_cozo: Option<Arc<mae_kb::CozoKbStore>>,
    /// Standardized KB data directory layout (XDG-compliant).
    pub data_dir: Option<mae_kb::data_dir::KbDataDir>,
    /// KB federation: registry of external KB instances (org-roam dirs etc.).
    pub registry: mae_kb::federation::KbRegistry,
    /// KB federation: loaded KB instances keyed by registry UUID.
    pub instances: HashMap<String, mae_kb::KnowledgeBase>,
    /// CozoDB store handles for federated KB instances (retained for runtime queries).
    pub instance_stores: HashMap<String, Arc<mae_kb::CozoKbStore>>,
    /// KB federation: live file watchers for registered org directories.
    pub watchers: HashMap<String, mae_kb::watch::OrgDirWatcher>,
    /// KB watcher: last drain timestamp per instance UUID (for debounce).
    pub last_drain: HashMap<String, std::time::Instant>,
    /// KB watcher: cumulative statistics.
    pub watcher_stats: KbWatcherStats,
    /// Active capture state (org-roam C-c C-c / C-c C-k flow).
    pub capture_state: Option<CaptureState>,
    /// KB node IDs visited via AI tools (kb_get/links_from/links_to) this session.
    pub ai_visited_ids: HashSet<String>,
    /// Per-node last-visit ordinal (monotonic; higher = more recently visited).
    /// Drives `KbSort::Recency`. Ordering-only, so a sequence counter rather
    /// than wall-clock — deterministic and free of `SystemTime` skew.
    pub visit_log: HashMap<String, u64>,
    /// Monotonic counter backing `visit_log`; bumped on every recorded visit.
    pub visit_seq: u64,
    /// Paths currently being written by MAE itself (activity tracking, chain-fill).
    pub write_guard: HashSet<PathBuf>,
    /// CozoDB-first query layer (federated across primary + instances).
    /// Falls back to in-memory KnowledgeBase when no CozoDB store is available.
    query: Option<Arc<dyn KbQueryLayer>>,
    /// LRU-cached query layer backed by daemon RPC.
    /// When set, `effective_query_layer()` returns this instead of the local query layer.
    daemon_query: Option<Arc<dyn KbQueryLayer>>,
    /// Whether daemon connection is enabled.
    pub daemon_enabled: bool,
    /// Daemon control channel for synchronous control-socket ops (P2P ticket
    /// mint/join, …). Injected by the binary; `None` when no daemon is wired.
    daemon_control: Option<Arc<dyn DaemonControl>>,
    /// Daemon Unix socket path.
    pub daemon_socket: std::path::PathBuf,
    /// LRU cache capacity (0 = unbounded).
    pub daemon_cache_size: usize,

    // --- Options ---
    /// KB option: enable/disable file watchers.
    pub watcher_enabled: bool,
    /// KB option: debounce interval in ms between watcher drains.
    pub watcher_debounce_ms: u64,
    /// KB option: max events processed per idle tick.
    pub max_drain_events: usize,
    /// KB option: max bytes for RAG excerpt truncation.
    pub search_excerpt_length: usize,
    /// KB option: hard cap for kb_search_context results.
    pub search_max_results: usize,
    /// KB option: auto-register org directories in project root.
    pub auto_register: bool,
    /// KB option: default directory for user-created notes (org-roam-directory equivalent).
    pub notes_dir: Option<PathBuf>,
    /// KB option: enable activity tracking (last-accessed/modified/linked timestamps).
    pub activity_tracking: bool,
    /// KB option: decay rate for activity scoring.
    pub activity_decay: f64,
    /// KB option: search result ordering ("relevance", "activity", "alphabetical", "recency").
    pub search_sort: String,
    /// KB option: default search scope ("all", "local", "remote", or instance name).
    pub search_scope: String,
    /// KB option: dailies directory (explicit setting or derived from notes_dir/daily).
    pub dailies_dir: Option<PathBuf>,
    /// KB option: max days to walk backwards when chain-filling dailies (default 90).
    pub daily_chain_gap_max: usize,
}

impl KbContext {
    /// Name of the currently active KB instance for collab operations.
    ///
    /// Returns the first registered instance name, or None (caller should
    /// default to "default" which maps to `self.primary`).
    pub fn active_instance_name(&self) -> Option<String> {
        self.registry.instances.first().map(|e| e.name.clone())
    }

    /// Record that node `id` was just visited (by the user via `:help` or the
    /// AI via kb tools). Bumps the monotonic counter so later visits sort ahead
    /// of earlier ones under `KbSort::Recency`.
    pub fn record_visit(&mut self, id: &str) {
        self.visit_seq += 1;
        self.visit_log.insert(id.to_string(), self.visit_seq);
    }

    /// Last-visit ordinal for `id` (0 if never visited this session).
    pub fn visit_rank(&self, id: &str) -> u64 {
        self.visit_log.get(id).copied().unwrap_or(0)
    }

    /// Return the effective query layer: daemon LRU if connected, else local.
    pub fn query_layer(&self) -> Option<&dyn KbQueryLayer> {
        self.daemon_query.as_deref().or(self.query.as_deref())
    }

    /// Return the local-only query layer (bypasses daemon).
    pub fn local_query_layer(&self) -> Option<&dyn KbQueryLayer> {
        self.query.as_deref()
    }

    /// Set the daemon-backed LRU query layer.
    pub fn set_daemon_query_layer(&mut self, layer: Option<Arc<dyn KbQueryLayer>>) {
        self.daemon_query = layer;
    }

    /// Inject the daemon control channel (binary-provided, `DaemonClient`-backed).
    pub fn set_daemon_control(&mut self, control: Option<Arc<dyn DaemonControl>>) {
        self.daemon_control = control;
    }

    /// Whether a daemon control channel is wired (P2P control ops are available).
    pub fn has_daemon_control(&self) -> bool {
        self.daemon_control.is_some()
    }

    /// Mint a P2P join ticket ("magnet link") for `kb_id` over the daemon control
    /// channel. The **single backend** behind the `kb-share-p2p` command, the
    /// `(kb-share-p2p)` Scheme primitive, and the `kb_share_p2p` MCP tool — so
    /// the human and the AI peer drive the identical action (ADR-025 parity).
    pub fn share_p2p(&self, kb_id: &str) -> Result<String, String> {
        let control = self.daemon_control.as_deref().ok_or_else(|| {
            "not connected to a daemon — start one with `mae setup-daemon` and enable \
             P2P with `mae setup-collab --p2p`"
                .to_string()
        })?;
        control.mint_p2p_ticket(kb_id)
    }

    /// Whether a daemon query layer is active.
    pub fn has_daemon(&self) -> bool {
        self.daemon_query.is_some()
    }

    /// Build or rebuild the federated query layer from current stores.
    /// Call after store/instance_store changes (register, unregister, reimport).
    pub fn rebuild_query_layer(&mut self) {
        // Determine the primary query layer: prefer user's primary CozoDB store,
        // fall back to the manual KB store if no user store is available.
        let primary_arc = self
            .primary_cozo
            .as_ref()
            .or(self.manual_cozo.as_ref())
            .cloned();

        if let Some(ref cozo) = primary_arc {
            let primary_layer = Arc::new(mae_kb::CozoQueryLayer::new(cozo.clone()));
            let mut federated = mae_kb::FederatedQuery::new(primary_layer);

            // If we used the user store as primary AND a manual store exists separately,
            // add the manual store as an instance so its nodes are queryable.
            if let Some(ref primary) = self.primary_cozo {
                if let Some(ref manual) = self.manual_cozo {
                    if !Arc::ptr_eq(primary, manual) {
                        let manual_layer = Arc::new(mae_kb::CozoQueryLayer::new(manual.clone()));
                        federated.add_instance("manual".to_string(), manual_layer);
                    }
                }
            }

            for (name, inst_store) in &self.instance_stores {
                let layer = Arc::new(mae_kb::CozoQueryLayer::new(inst_store.clone()));
                federated.add_instance(name.clone(), layer);
            }
            self.query = Some(Arc::new(federated));
        }
    }

    /// Return all available CozoDB store handles (primary + instances).
    pub fn all_stores(&self) -> Vec<(&str, &dyn mae_kb::KbStore)> {
        let mut stores: Vec<(&str, &dyn mae_kb::KbStore)> = Vec::new();
        if let Some(ref s) = self.store {
            stores.push(("primary", s.as_ref()));
        }
        for (name, store) in &self.instance_stores {
            stores.push((name.as_str(), store.as_ref()));
        }
        stores
    }

    pub fn new(primary: mae_kb::KnowledgeBase) -> Self {
        Self {
            primary,
            store: None,
            primary_cozo: None,
            manual_cozo: None,
            data_dir: None,
            registry: mae_kb::federation::KbRegistry::default(),
            instances: HashMap::new(),
            instance_stores: HashMap::new(),
            watchers: HashMap::new(),
            last_drain: HashMap::new(),
            watcher_stats: KbWatcherStats::default(),
            capture_state: None,
            ai_visited_ids: HashSet::new(),
            visit_log: HashMap::new(),
            visit_seq: 0,
            write_guard: HashSet::new(),
            query: None,
            daemon_query: None,
            daemon_enabled: false,
            daemon_control: None,
            daemon_socket: std::path::PathBuf::from("/tmp/mae-daemon.sock"),
            daemon_cache_size: 200,
            watcher_enabled: true,
            watcher_debounce_ms: 500,
            max_drain_events: 100,
            search_excerpt_length: 500,
            search_max_results: 20,
            auto_register: false,
            notes_dir: None,
            activity_tracking: true,
            activity_decay: 0.01,
            search_sort: "relevance".to_string(),
            search_scope: "all".to_string(),
            dailies_dir: None,
            daily_chain_gap_max: 90,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stub control channel returning a fixed result — stands in for the
    /// binary's `DaemonClient`-backed impl so the single-backend dispatch is
    /// testable without a running daemon.
    struct StubControl(Result<String, String>);
    impl DaemonControl for StubControl {
        fn mint_p2p_ticket(&self, _kb_id: &str) -> Result<String, String> {
            self.0.clone()
        }
    }

    fn ctx() -> KbContext {
        KbContext::new(mae_kb::KnowledgeBase::new())
    }

    #[test]
    fn share_p2p_without_daemon_control_is_an_actionable_error() {
        let kb = ctx();
        assert!(!kb.has_daemon_control());
        let err = kb.share_p2p("concept:x").unwrap_err();
        assert!(
            err.contains("daemon"),
            "error should point the user at the daemon: {err}"
        );
    }

    #[test]
    fn share_p2p_delegates_to_the_injected_control() {
        let mut kb = ctx();
        kb.set_daemon_control(Some(Arc::new(StubControl(Ok(
            "mae://join/STUB".to_string()
        )))));
        assert!(kb.has_daemon_control());
        assert_eq!(kb.share_p2p("concept:x").unwrap(), "mae://join/STUB");

        // Backend errors propagate verbatim — every surface shows the same message.
        kb.set_daemon_control(Some(Arc::new(StubControl(Err(
            "P2P mesh not enabled".into()
        )))));
        assert_eq!(kb.share_p2p("k").unwrap_err(), "P2P mesh not enabled");
    }
}
