//! Knowledge base state extracted from Editor.
//! All fields were previously `kb_*` / `capture_state` on Editor;
//! now accessed via `editor.kb.*`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use mae_kb::query::KbQueryLayer;

use super::kb_ops::KbWatcherStats;
use super::CaptureState;

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
    /// Paths currently being written by MAE itself (activity tracking, chain-fill).
    pub write_guard: HashSet<PathBuf>,
    /// CozoDB-first query layer (federated across primary + instances).
    /// Falls back to in-memory KnowledgeBase when no CozoDB store is available.
    query: Option<Arc<dyn KbQueryLayer>>,

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
    /// KB option: search result ordering ("relevance", "activity", "alphabetical").
    pub search_sort: String,
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

    /// Return the CozoDB-first query layer, if available.
    pub fn query_layer(&self) -> Option<&dyn KbQueryLayer> {
        self.query.as_deref()
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
            write_guard: HashSet::new(),
            query: None,
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
            dailies_dir: None,
            daily_chain_gap_max: 90,
        }
    }
}
