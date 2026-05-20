//! Knowledge base state extracted from Editor.
//! All fields were previously `kb_*` / `capture_state` on Editor;
//! now accessed via `editor.kb.*`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::kb_ops::KbWatcherStats;
use super::CaptureState;

/// Knowledge base context: backing store, federation, watchers, and config.
pub struct KbContext {
    /// Primary knowledge base instance (manual + user notes + AI-facing kb_* tools).
    pub primary: mae_kb::KnowledgeBase,
    /// KB federation: registry of external KB instances (org-roam dirs etc.).
    pub registry: mae_kb::federation::KbRegistry,
    /// KB federation: loaded KB instances keyed by registry UUID.
    pub instances: HashMap<String, mae_kb::KnowledgeBase>,
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
    pub fn new(primary: mae_kb::KnowledgeBase) -> Self {
        Self {
            primary,
            registry: mae_kb::federation::KbRegistry::default(),
            instances: HashMap::new(),
            watchers: HashMap::new(),
            last_drain: HashMap::new(),
            watcher_stats: KbWatcherStats::default(),
            capture_state: None,
            ai_visited_ids: HashSet::new(),
            write_guard: HashSet::new(),
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
