//! KB federation operations: register, unregister, reimport.

use std::path::{Path, PathBuf};

use mae_kb::federation::{ImportHealth, ImportReport};
use mae_kb::KbStore;

use super::Editor;

/// The honest, point-of-action advisory shown when a user enables E2E content
/// encryption on a KB (CF1, `docs/SECURITY_REVIEW.md §6.3`). "E2E" connotes
/// Signal-like privacy; MAE's model protects node *content* from non-members but
/// does NOT provide forward secrecy / PCS, hide metadata, or retroactively protect
/// already-shared plaintext. Surfacing this at enable-time — not only in
/// `docs/E2E_ENCRYPTION.md §7` — keeps the label from overselling. Kept as one
/// shared const so the enable surface, the `*KB Sharing*` buffer, and the Scheme
/// primitive doc all say the same thing (CLAUDE.md #3).
pub const E2E_ENABLE_ADVISORY: &str = "\
E2E content encryption is now ENABLED on this KB (one-way — it cannot be disabled).

What it protects: node CONTENT (titles + bodies) is sealed so the daemon/relay and \
non-members see only ciphertext.

What it does NOT protect (be aware before relying on it):
  • No forward secrecy / post-compromise security — a leaked key opens past AND future content.
  • Metadata is visible to the host: who is in the KB, who admitted whom, which node each \
edit touches, when, by whom, and the size of each edit — just not the content.
  • Node IDs remain cleartext in the collection manifest (titles are blanked).
  • It is NOT retroactive: anything already shared as plaintext stays on the relay until \
re-sealed — enable BEFORE sharing for full protection.
  • If you lose your identity key you lose access permanently — back it up.

See :help concept:kb-encryption and docs/E2E_ENCRYPTION.md §7 for the full model.";

/// Cumulative statistics for KB watcher drain operations.
#[derive(Debug, Default)]
pub struct KbWatcherStats {
    /// Total nodes upserted via watcher drain.
    pub events_upserted: u64,
    /// Total nodes removed via watcher drain.
    pub events_removed: u64,
    /// Events skipped due to debounce (too recent).
    pub suppressed_debounce: u64,
    /// Events skipped due to 50ms timebox deadline.
    pub suppressed_timebox: u64,
    /// Events suppressed by write-guard (MAE-initiated writes).
    pub events_suppressed: u64,
    /// Total reimport calls from all sources (save, watcher, explicit).
    pub reimports_total: u64,
    /// Watcher errors encountered.
    pub errors: u64,
    /// Durable-store write-through failures during watcher/reimport drain.
    pub store_write_errors: u64,
    /// Duration of the last drain operation in microseconds.
    pub last_drain_us: u64,
    /// Number of events processed in the last drain.
    pub last_drain_event_count: usize,
    /// Cumulative drain microseconds (for computing avg).
    pub drain_us_sum: u64,
    /// Number of drain cycles that processed at least one event.
    pub drain_count: u64,
}

/// Result of a KB registration or reimport operation.
#[derive(Debug, Clone)]
pub struct KbImportResult {
    pub name: String,
    pub uuid: String,
    pub report: ImportReport,
    pub health: ImportHealth,
}

/// Result of promoting a federated/imported node into the primary KB
/// (#303's interim bridge toward issue #111 / ADR-029's "org dirs are
/// import-only" direction — see `Editor::kb_promote_node`).
#[derive(Debug, Clone)]
pub struct KbPromoteResult {
    pub node_id: String,
    pub promoted_from_uuid: String,
    pub promoted_from_org_dir: PathBuf,
}

impl KbImportResult {
    /// Format as a user-facing status message.
    pub fn status_summary(&self) -> String {
        let mut s = format!(
            "Registered '{}': {} nodes, {} links",
            self.name, self.report.nodes_imported, self.report.links_created,
        );
        if self.report.nodes_updated > 0 {
            s.push_str(&format!(", {} updated", self.report.nodes_updated));
        }
        if self.report.nodes_unchanged > 0 {
            s.push_str(&format!(", {} unchanged", self.report.nodes_unchanged));
        }
        if self.report.nodes_removed > 0 {
            s.push_str(&format!(", {} removed", self.report.nodes_removed));
        }
        s.push_str(&format!(
            " | Health: {} orphans, {} broken links",
            self.health.orphan_count, self.health.broken_link_count,
        ));
        if !self.report.duplicate_ids.is_empty() {
            s.push_str(&format!(
                ", {} duplicate IDs",
                self.report.duplicate_ids.len()
            ));
        }
        if self.report.nodes_skipped > 0 {
            s.push_str(&format!(
                ", {} files without :ID:",
                self.report.nodes_skipped
            ));
        }
        if !self.report.errors.is_empty() {
            s.push_str(&format!(", {} read errors", self.report.errors.len()));
        }
        if self.report.duration_ms > 0 {
            s.push_str(&format!(" ({}ms)", self.report.duration_ms));
        }
        s
    }

    /// Format as structured JSON for the AI agent.
    pub fn to_json(&self) -> String {
        let ns_counts: Vec<String> = self
            .health
            .namespace_counts
            .iter()
            .map(|(k, v)| format!("    \"{}\": {}", k, v))
            .collect();

        format!(
            concat!(
                "{{\n",
                "  \"name\": \"{}\",\n",
                "  \"uuid\": \"{}\",\n",
                "  \"nodes_imported\": {},\n",
                "  \"links_created\": {},\n",
                "  \"files_without_id\": {},\n",
                "  \"duplicate_ids\": {},\n",
                "  \"read_errors\": {},\n",
                "  \"health\": {{\n",
                "    \"total_nodes\": {},\n",
                "    \"total_links\": {},\n",
                "    \"orphan_count\": {},\n",
                "    \"broken_link_count\": {},\n",
                "    \"namespace_counts\": {{\n{}\n    }}\n",
                "  }}\n",
                "}}"
            ),
            self.name,
            self.uuid,
            self.report.nodes_imported,
            self.report.links_created,
            self.report.nodes_skipped,
            self.report.duplicate_ids.len(),
            self.report.errors.len(),
            self.health.total_nodes,
            self.health.total_links,
            self.health.orphan_count,
            self.health.broken_link_count,
            ns_counts.join(",\n"),
        )
    }
}

impl Editor {
    /// Above this loaded-node count, kb-find switches from eager all-load +
    /// client-filter to a bounded, query-driven ranked window (lazy at scale).
    /// Sits above the bundled manual (~870) so the default UX is unchanged.
    pub const KB_FIND_LAZY_THRESHOLD: usize = 2000;
    /// Size of the lazy ranked window fetched per query for large KBs.
    pub const KB_FIND_LAZY_LIMIT: usize = 200;

    /// Resolve the MAE config directory (~/.config/mae).
    /// Checks `config_dir_override` first (for test isolation).
    #[allow(dead_code)]
    fn mae_config_dir(&self) -> Option<PathBuf> {
        if let Some(ref dir) = self.config_dir_override {
            return Some(dir.clone());
        }
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            Some(PathBuf::from(xdg).join("mae"))
        } else if let Ok(home) = std::env::var("HOME") {
            Some(PathBuf::from(home).join(".config").join("mae"))
        } else {
            None
        }
    }

    /// Resolve the MAE data directory (~/.local/share/mae).
    /// Checks `data_dir_override` first (for test isolation).
    pub fn mae_data_dir(&self) -> Option<PathBuf> {
        if let Some(ref dir) = self.data_dir_override {
            return Some(dir.clone());
        }
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            Some(PathBuf::from(xdg).join("mae"))
        } else if let Ok(home) = std::env::var("HOME") {
            Some(PathBuf::from(home).join(".local").join("share").join("mae"))
        } else {
            None
        }
    }

    /// Open a federated KB instance's durable store, honoring the configured
    /// `kb_storage_engine` (default sqlite) and auto-migrating an existing sled
    /// store once — the same multi-process-safe path the primary store takes
    /// (main.rs). Without this, callers using `CozoKbStore::open()` directly
    /// get sled unconditionally (its hardcoded default), permanently stuck on
    /// sled's single-writer exclusive lock regardless of `kb_storage_engine`.
    pub fn kb_open_instance_store(
        &self,
        path: &Path,
    ) -> Result<mae_kb::CozoKbStore, mae_kb::KbStoreError> {
        let mut engine = self
            .get_option("kb_storage_engine")
            .map(|(v, _)| v)
            .unwrap_or_else(|| "sqlite".to_string());

        if engine == "sqlite" {
            if let Err(e) = mae_kb::migrate::migrate_sled_to_sqlite(path) {
                tracing::warn!(
                    error = %e,
                    path = %path.display(),
                    "sled→sqlite migration failed; opening existing store"
                );
                if path.is_dir() {
                    engine = "sled".to_string();
                }
            }
        }

        mae_kb::CozoKbStore::open_with_engine(path, &engine)
    }

    /// Open the durable store for a registered org-dir KB instance, import
    /// its org files, insert it into `self.kb.instances`, and start a file
    /// watcher for live updates — the common "adopt this instance" tail
    /// shared by `kb_register()` (an instance this process just registered)
    /// and `drain_kb_registry_watch()` (an instance that appeared via
    /// another `mae` process's registration). Returns the import report and
    /// health so callers building a `KbImportResult` don't need to duplicate
    /// the try-CozoDB-then-fall-back-to-in-memory logic.
    fn kb_adopt_instance(
        &mut self,
        uuid: &str,
        org_dir: &Path,
        db_path: Option<&Path>,
    ) -> (ImportReport, ImportHealth) {
        let (kb, report, health) = if let Some(db_path) = db_path {
            match self.kb_open_instance_store(db_path) {
                Ok(store) => {
                    match mae_kb::federation::import_org_dir_to_store(
                        org_dir,
                        &store,
                        &mae_kb::IngestMode::Full,
                    ) {
                        Ok((kb, report)) => {
                            let health = mae_kb::ImportHealth::from_kb(&kb);
                            // Retain the CozoDB store handle for runtime queries.
                            self.kb
                                .instance_stores
                                .insert(uuid.to_string(), std::sync::Arc::new(store));
                            (kb, report, health)
                        }
                        Err(e) => {
                            // #265: a persistent-store ingestion failure must NOT swap silently
                            // to an unpersisted in-memory KB — the user would lose everything on
                            // restart with no warning. (Per-node parse errors no longer land here;
                            // `import_org_dir_to_store` now tolerates those and reports them in
                            // `report.errors`. Reaching this arm means a catastrophic store
                            // failure.) Surface it prominently, then fall back so the editor is
                            // still usable — but the user KNOWS this KB is in-memory only.
                            tracing::warn!(
                                error = %e,
                                "CozoDB ingestion failed, falling back to in-memory import"
                            );
                            self.message_log.push(
                                crate::messages::MessageLevel::Error,
                                "kb-import",
                                format!(
                                    "KB '{uuid}' could NOT be persisted ({e}) — loaded IN-MEMORY only; \
                                     changes will be LOST on restart. Fix the store and re-import."
                                ),
                            );
                            mae_kb::federation::import_org_dir(org_dir)
                        }
                    }
                }
                Err(_) => mae_kb::federation::import_org_dir(org_dir),
            }
        } else {
            mae_kb::federation::import_org_dir(org_dir)
        };

        // Store the instance
        self.kb.instances.insert(uuid.to_string(), kb);

        // Start file watcher for live updates (if enabled)
        if self.kb.watcher_enabled {
            match mae_kb::watch::OrgDirWatcher::new(org_dir) {
                Ok(watcher) => {
                    watcher.seed(
                        report
                            .path_to_ids
                            .iter()
                            .map(|(p, ids)| (p.clone(), ids.clone())),
                    );
                    self.kb.watchers.insert(uuid.to_string(), watcher);
                    self.kb.watcher_attach_errors.remove(uuid);
                }
                Err(e) => {
                    let msg = e.to_string();
                    // Watcher is optional — registration still succeeds — but every
                    // attach failure is now surfaced, not just the inotify-limit
                    // case: `watcher_count: 0` alone is otherwise ambiguous between
                    // "no instance was ever registered" and "a watcher should exist
                    // but silently didn't attach." Tracked in watcher_attach_errors
                    // for kb_sync_status; also always logged/status'd.
                    tracing::warn!(uuid = %uuid, org_dir = %org_dir.display(), error = %msg, "KB watcher failed to attach");
                    self.kb
                        .watcher_attach_errors
                        .insert(uuid.to_string(), msg.clone());
                    if msg.contains("inotify") || msg.contains("No space left") {
                        self.set_status(
                            "KB watcher failed: inotify limit reached. \
                             Run `sysctl fs.inotify.max_user_watches=65536` \
                             or set `kb_watcher_enabled=false`.",
                        );
                    } else {
                        self.set_status(format!(
                            "KB watcher failed to attach for this instance: {msg}"
                        ));
                    }
                }
            }
        }

        (report, health)
    }

    /// Register an external org directory as a federated KB instance.
    ///
    /// Recursively imports all `.org` files, computes health metrics,
    /// and reports results via the status bar.
    pub fn kb_register(&mut self, name: &str, org_dir: &Path) -> Option<KbImportResult> {
        if !org_dir.exists() {
            self.set_status(format!(
                "KB register error: path does not exist: {}",
                org_dir.display()
            ));
            return None;
        }
        if !org_dir.is_dir() {
            self.set_status(format!(
                "KB register error: not a directory: {}",
                org_dir.display()
            ));
            return None;
        }

        let Some(data_dir) = self.mae_data_dir() else {
            self.set_status("KB register error: cannot determine data directory");
            return None;
        };
        let _ = std::fs::create_dir_all(&data_dir);

        let (registry, uuid, saved) = mae_kb::federation::KbRegistry::update(&data_dir, |reg| {
            reg.register(
                name.to_string(),
                org_dir.to_path_buf(),
                &data_dir,
                self.kb.data_dir.as_ref(),
            )
        });
        if let Err(e) = saved {
            tracing::warn!(error = %e, "failed to persist KB registry");
        }
        self.kb.registry = registry;
        self.kb.last_local_registry_write = Some(std::time::Instant::now());

        // Import org files, open the durable store, start a watcher — shared
        // with `drain_kb_registry_watch` (an instance appearing via another
        // process's registration goes through the exact same adoption path).
        let db_path = self.kb.registry.find(&uuid).map(|i| i.db_path.clone());
        let (report, health) = self.kb_adopt_instance(&uuid, org_dir, db_path.as_deref());

        // Update last_import timestamp and persist.
        let (registry, (), saved) = mae_kb::federation::KbRegistry::update(&data_dir, |reg| {
            if let Some(inst) = reg.instances.iter_mut().find(|i| i.uuid == uuid) {
                inst.last_import = Some(chrono_now());
            }
        });
        if let Err(e) = saved {
            tracing::warn!(error = %e, "failed to persist KB registry");
        }
        self.kb.registry = registry;
        self.kb.last_local_registry_write = Some(std::time::Instant::now());

        let result = KbImportResult {
            name: name.to_string(),
            uuid,
            report,
            health,
        };

        // Rebuild the query layer to include the new instance.
        self.kb.rebuild_query_layer();

        self.set_status(result.status_summary());
        Some(result)
    }

    /// Unregister a KB instance by name or UUID.
    pub fn kb_unregister(&mut self, name_or_uuid: &str) {
        let found = self.kb.registry.find(name_or_uuid).map(|i| i.uuid.clone());
        match found {
            Some(uuid) => {
                self.kb.instances.remove(&uuid);
                self.kb.instance_stores.remove(&uuid);
                self.kb.watchers.remove(&uuid);
                if let Some(data_dir) = self.mae_data_dir() {
                    let (registry, (), saved) =
                        mae_kb::federation::KbRegistry::update(&data_dir, |reg| {
                            reg.unregister(name_or_uuid)
                        });
                    if let Err(e) = saved {
                        tracing::warn!(error = %e, "failed to persist KB registry");
                    }
                    self.kb.registry = registry;
                    self.kb.last_local_registry_write = Some(std::time::Instant::now());
                } else {
                    self.kb.registry.unregister(name_or_uuid);
                }
                // Rebuild query layer without the removed instance.
                self.kb.rebuild_query_layer();
                self.set_status(format!("KB instance '{}' unregistered", name_or_uuid));
            }
            None => {
                self.set_status(format!(
                    "KB unregister: no instance found matching '{}'",
                    name_or_uuid
                ));
            }
        }
    }

    /// Set a KB's AI-residency policy (ADR-048): `"primary"` for the primary/local KB, or
    /// an instance name/UUID. A `LocalModelsOnly` KB may only be read/written by a
    /// locally-classified AI provider (see `ai_event_handler.rs`'s residency gate) — this
    /// is a plain, freely-toggleable local registry field, not the anti-downgrade signed
    /// op-log `kb_set_encryption`/`kb_set_policy` use for *shared*-KB peer trust (that
    /// mechanism doesn't apply here: this is one local user's own KB, not a multi-peer
    /// trust problem).
    pub fn kb_set_ai_residency(
        &mut self,
        name_or_uuid: &str,
        policy: mae_kb::federation::AiResidency,
    ) -> Result<String, String> {
        let is_primary = name_or_uuid.eq_ignore_ascii_case("primary");
        let label = if is_primary {
            "primary".to_string()
        } else {
            name_or_uuid.to_string()
        };
        let changed = if let Some(data_dir) = self.mae_data_dir() {
            let (registry, changed, saved) =
                mae_kb::federation::KbRegistry::update(&data_dir, |reg| {
                    reg.set_ai_residency(name_or_uuid, policy)
                });
            if let Err(e) = saved {
                tracing::warn!(error = %e, "failed to persist KB registry");
            }
            self.kb.registry = registry;
            self.kb.last_local_registry_write = Some(std::time::Instant::now());
            changed
        } else {
            self.kb.registry.set_ai_residency(name_or_uuid, policy)
        };
        if !changed {
            return Err(format!(
                "KB set-ai-residency: no instance found matching '{}'",
                label
            ));
        }
        let policy_str = match policy {
            mae_kb::federation::AiResidency::Open => "open",
            mae_kb::federation::AiResidency::LocalModelsOnly => "local_models_only",
        };
        Ok(format!("KB '{}' AI residency set to {}", label, policy_str))
    }

    /// Re-import an existing KB instance (refresh after org file edits).
    ///
    /// When `mode` is `None`, defaults to `IngestMode::Full`.
    pub fn kb_reimport(
        &mut self,
        name_or_uuid: &str,
        mode: Option<mae_kb::IngestMode>,
    ) -> Option<KbImportResult> {
        let inst = self.kb.registry.find(name_or_uuid).cloned();
        match inst {
            Some(instance) => {
                let mode = mode.unwrap_or_default();

                // Reuse the already-open store handle if this instance's store
                // was opened at startup (or a prior register/reimport) — sled is
                // single-writer with an exclusive dir lock, so opening a second
                // handle to the same store from within this same process fails
                // and silently falls back to a non-persistent in-memory import.
                let existing_store = self.kb.instance_stores.get(&instance.uuid).cloned();
                let (kb, report, health, store_for_layer) = match existing_store.or_else(|| {
                    self.kb_open_instance_store(&instance.db_path)
                        .ok()
                        .map(std::sync::Arc::new)
                }) {
                    Some(store) => {
                        match mae_kb::federation::import_org_dir_to_store(
                            &instance.org_dir,
                            &store,
                            &mode,
                        ) {
                            Ok((kb, report)) => {
                                let health = mae_kb::ImportHealth::from_kb(&kb);
                                (kb, report, health, Some(store))
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "CozoDB ingestion failed, falling back to in-memory import"
                                );
                                let (kb, report, health) =
                                    mae_kb::federation::import_org_dir(&instance.org_dir);
                                (kb, report, health, None)
                            }
                        }
                    }
                    None => {
                        // No CozoDB store for this instance — use in-memory import.
                        let (kb, report, health) =
                            mae_kb::federation::import_org_dir(&instance.org_dir);
                        (kb, report, health, None)
                    }
                };

                self.kb.instances.insert(instance.uuid.clone(), kb);
                if let Some(store) = store_for_layer {
                    self.kb.instance_stores.insert(instance.uuid.clone(), store);
                }

                // Update timestamp and persist.
                if let Some(data_dir) = self.mae_data_dir() {
                    let (registry, (), saved) =
                        mae_kb::federation::KbRegistry::update(&data_dir, |reg| {
                            if let Some(reg_inst) =
                                reg.instances.iter_mut().find(|i| i.uuid == instance.uuid)
                            {
                                reg_inst.last_import = Some(chrono_now());
                            }
                        });
                    if let Err(e) = saved {
                        tracing::warn!(error = %e, "failed to persist KB registry");
                    }
                    self.kb.registry = registry;
                    self.kb.last_local_registry_write = Some(std::time::Instant::now());
                }

                // Rebuild the query layer so kb-find and other query-layer
                // consumers see the reimported nodes immediately (matches
                // kb_register/kb_unregister — previously missing here, so
                // reimports were invisible to kb-find whenever a query layer
                // was active).
                self.kb.rebuild_query_layer();

                let result = KbImportResult {
                    name: instance.name.clone(),
                    uuid: instance.uuid.clone(),
                    report,
                    health,
                };

                let msg = format!(
                    "Reimported '{}': {}",
                    instance.name,
                    result.status_summary()
                );
                self.set_status(&msg);
                Some(result)
            }
            None => {
                self.set_status(format!(
                    "KB reimport: no instance found matching '{}'",
                    name_or_uuid
                ));
                None
            }
        }
    }

    /// Persist a node to the backing store (if present). Best-effort — logs errors.
    fn kb_persist_node(&self, node: &mae_kb::Node) {
        // Phase D3b: when the daemon hosts the primary, the daemon's CRDT is the
        // source of truth — retire the per-edit local write-through. Edits already
        // reach the daemon (pending queue); the local cozo is refreshed in batch via
        // snapshot-back on disconnect/shutdown and remains the daemon-less fallback.
        if self.kb.daemon_hosts_primary() {
            return;
        }
        if let Some(ref store) = self.kb.store {
            if let Err(e) = store.update_node(node) {
                tracing::warn!(node_id = %node.id, error = %e, "KB store write-through failed");
            }
        }
    }

    /// Write freshly-ingested nodes through to the durable primary store.
    ///
    /// `KnowledgeBase::ingest_org_dir` only populates the in-memory mirror. On a
    /// daemon-less primary nothing else flushes that mirror to disk (the shutdown
    /// snapshot is gated on `daemon_hosts_primary`), so without this a
    /// `:kb-ingest <dir>` import silently vanishes on the next launch — `load_all`
    /// reads the durable store, which never saw the nodes. Persist the exact set
    /// the ingest reported (looked up from the mirror, which now holds them).
    ///
    /// No-op when the daemon hosts the primary: there the daemon's CRDT is the
    /// source of truth and the local store is refreshed via snapshot-back instead
    /// (mirrors the `kb_persist_node` write-through guard).
    pub fn kb_persist_ingested(&self, ids: &[String]) -> usize {
        if self.kb.daemon_hosts_primary() {
            return 0;
        }
        let Some(ref store) = self.kb.store else {
            return 0;
        };
        let mut n = 0usize;
        for id in ids {
            if let Some(node) = self.kb.primary.get(id) {
                if store.update_node(node).is_ok() {
                    n += 1;
                }
            }
        }
        n
    }

    /// Write freshly-ingested federated-instance nodes through to their durable
    /// instance store. The counterpart of [`Editor::kb_persist_ingested`] for a
    /// registered instance: `ingest_org_file` (file watcher / reimport) only fills
    /// the in-memory instance mirror, so without this the watcher/reimport edits are
    /// lost on restart — the same class of bug as the `:kb-ingest` durability gap.
    /// Returns the count persisted; counts failures into `watcher_stats`.
    fn kb_persist_instance_ids(&mut self, uuid: &str, ids: &[String]) -> usize {
        let Some(store) = self.kb.instance_stores.get(uuid).cloned() else {
            return 0;
        };
        let mut ok = 0usize;
        let mut errs = 0u64;
        if let Some(kb) = self.kb.instances.get(uuid) {
            for id in ids {
                if let Some(node) = kb.get(id) {
                    match store.update_node(node) {
                        Ok(()) => ok += 1,
                        Err(e) => {
                            errs += 1;
                            tracing::warn!(node_id = %id, error = %e, "KB instance store write-through (watcher/reimport) failed");
                        }
                    }
                }
            }
        }
        self.kb.watcher_stats.store_write_errors += errs;
        ok
    }

    /// Phase 0c: guard for KB mutations when the durable primary store failed to
    /// open (e.g. a second daemon-less process hit the sled single-writer lock, or
    /// corruption). Returns an actionable error to surface to the user instead of
    /// silently writing to a mirror that will never persist. No-op when the daemon
    /// hosts the primary (the daemon is the store of record then).
    pub fn kb_write_blocked(&self) -> Result<(), String> {
        if self.kb.store_unavailable && !self.kb.daemon_hosts_primary() {
            return Err("KB store unavailable — the durable store failed to open (another mae instance may hold it, or it is corrupt). Changes cannot be saved; see *Messages*.".into());
        }
        Ok(())
    }

    /// Mirror a watcher-driven removal into the durable instance store so a node
    /// deleted from an org file does not resurrect on restart. Best-effort.
    fn kb_persist_instance_delete(&self, uuid: &str, id: &str) {
        if let Some(store) = self.kb.instance_stores.get(uuid) {
            if let Err(e) = store.delete_node(id) {
                tracing::warn!(node_id = %id, error = %e, "KB instance store delete (watcher) failed");
            }
        }
    }

    /// Phase D3b: snapshot the in-memory primary mirror back to the local store so
    /// the daemon-less fallback stays coherent after the per-edit write-through is
    /// retired. Bypasses the retire guard (writes the store directly). Bounded by the
    /// (lazy) mirror size — only nodes touched this session. Called on collab
    /// disconnect + editor shutdown while the daemon hosts the primary.
    pub fn kb_snapshot_primary_to_store(&self) {
        let Some(ref store) = self.kb.store else {
            return;
        };
        let mut n = 0usize;
        for id in self.kb.primary.list_ids(None) {
            if let Some(node) = self.kb.primary.get(&id) {
                if store.update_node(node).is_ok() {
                    n += 1;
                }
            }
        }
        if n > 0 {
            tracing::debug!(target: "kb_sync", count = n, "D3b: snapshot primary mirror → local store");
        }
    }

    /// Locate the in-memory KB that owns `id`: `None` = primary, `Some(uuid)` =
    /// a federated instance. Used so writes (update/delete) resolve nodes the
    /// same way reads do — i.e. across `primary` ∪ `instances` — instead of
    /// primary-only (I-9).
    pub(crate) fn kb_owner_of(&self, id: &str) -> Option<Option<String>> {
        if self.kb.primary.contains(id) {
            return Some(None);
        }
        self.kb
            .instances
            .iter()
            .find(|(_, kb)| kb.contains(id))
            .map(|(uuid, _)| Some(uuid.clone()))
    }

    /// Register a joined collaborative KB as a first-class federated instance
    /// (ADR-019). Joined nodes become addressable in their own instance instead
    /// of being dumped into `primary` (fixes B-3: they appear in `kb_instances`
    /// and route correctly), and the instance carries the durable
    /// `shared`/`collab_id` markers that gate broadcasts + survive restart.
    ///
    /// ADR-020: nodes are MERGED via `apply_remote_update` (CRDT) rather than
    /// inserted/overwritten, so a member's offline/local edits survive a re-join
    /// (the join is no longer a lossy full-snapshot replace). `node_states` are
    /// the raw per-node CRDT state bytes. Idempotent: a re-join reuses the
    /// existing instance. Returns the uuid.
    pub fn kb_register_joined_instance(
        &mut self,
        kb_id: &str,
        nodes: Vec<crate::editor::JoinedNode>,
    ) -> String {
        // Reuse the existing instance for this collab id (idempotent re-join).
        let uuid = self
            .kb
            .registry
            .find_by_collab_id(kb_id)
            .map(|i| i.uuid.clone())
            .unwrap_or_else(mae_kb::federation::generate_uuid);

        // Best-effort durable store under the shared-KB data dir, so the joined
        // KB survives restart (the reconstruction phase reads it back).
        let mut db_path = std::path::PathBuf::new();
        if !self.kb.instance_stores.contains_key(&uuid) {
            if let Some(ref data_dir) = self.kb.data_dir {
                let slug = mae_kb::data_dir::slugify(kb_id);
                let meta = mae_kb::data_dir::SharedKbMeta {
                    name: kb_id.to_string(),
                    collab_id: kb_id.to_string(),
                    creator: String::new(),
                    created_at: mae_kb::data_dir::chrono_now_iso(),
                    peers: vec![],
                    last_sync: Some(mae_kb::data_dir::chrono_now_iso()),
                    sync_mode: crate::editor::KB_SYNC_MODE_DEFAULT.to_string(),
                };
                if let Ok(path) = data_dir.init_shared_kb(&slug, &meta) {
                    if let Ok(store) = self.kb_open_instance_store(&path) {
                        db_path = path;
                        self.kb
                            .instance_stores
                            .insert(uuid.clone(), std::sync::Arc::new(store));
                    }
                }
            }
        }

        // In-memory instance: get-or-create, then RECONCILE each node (ADR-022).
        // The daemon sends an incremental diff (against the SV we supplied) plus
        // its own SV: we MERGE the diff (never replace), so a durable-but-unsynced
        // local edit survives the (re)join, and we collect our local-ahead diff to
        // re-sync back up — the crash-safety path that does NOT depend on the
        // pending-queue row surviving. Two cases fall back to a full-state adopt:
        // a brand-new node (first join — `reconcile` Created via apply), and a
        // divergent independent same-id lineage (B-14): there the daemon's "diff"
        // against our disjoint SV is its full lineage, so adopting it establishes
        // the shared lineage without clobbering (the node was never in sync). A
        // pre-ADR-022 daemon sends no SV → legacy full-state adopt.
        let mut local_ahead: Vec<(String, Vec<u8>)> = Vec::new();
        // ADR-024 R5: nodes where adopting the remote lineage would overwrite
        // DIFFERENT local content (unsynced work) — surfaced for resolution instead
        // of silently clobbered.
        let mut divergent_conflicts: Vec<String> = Vec::new();
        let merged: Vec<mae_kb::Node> = {
            let kb = self.kb.instances.entry(uuid.clone()).or_default();
            let mut out = Vec::with_capacity(nodes.len());
            for jn in &nodes {
                let applied = match &jn.daemon_sv {
                    Some(daemon_sv) => match kb.reconcile_remote_node(&jn.id, &jn.bytes, daemon_sv)
                    {
                        Ok(outcome) => {
                            if outcome.action == mae_kb::ReconcileAction::DivergentLineage {
                                // The diff against our disjoint SV IS the daemon's
                                // full lineage — adopting establishes a shared lineage.
                                // ADR-024 R5 (hybrid no-silent-overwrite): if the local
                                // content DIFFERS from the authoritative version, adopting
                                // would lose the user's unsynced edits — defer + surface a
                                // resolution. If identical, it's a harmless lineage repair.
                                let local_differs = kb.get(&jn.id).is_some_and(|local| {
                                    mae_sync::kb::KbNodeDoc::from_bytes(&jn.bytes)
                                        .map(|remote| {
                                            local.title != remote.title()
                                                || local.body != remote.body()
                                                || local.tags != remote.tags()
                                        })
                                        .unwrap_or(false)
                                });
                                if local_differs {
                                    // Preserve local until the user resolves (no clobber).
                                    divergent_conflicts.push(jn.id.clone());
                                } else if let Err(e) = kb.adopt_remote_node(&jn.id, &jn.bytes) {
                                    tracing::warn!(target: "kb_sync", node_id = %jn.id, error = %e, "join: divergent-lineage adopt failed — skipping");
                                }
                            } else if let Some(la) = outcome.local_ahead {
                                local_ahead.push((jn.id.clone(), la));
                            }
                            true
                        }
                        Err(e) => {
                            tracing::warn!(target: "kb_sync", node_id = %jn.id, error = %e, "join: reconcile failed — skipping");
                            false
                        }
                    },
                    None => match kb.adopt_remote_node(&jn.id, &jn.bytes) {
                        Ok(_changed) => true,
                        Err(e) => {
                            tracing::warn!(target: "kb_sync", node_id = %jn.id, error = %e, "join: legacy full-state adopt failed — skipping");
                            false
                        }
                    },
                };
                if applied {
                    if let Some(n) = kb.get(&jn.id) {
                        out.push(n.clone());
                    }
                }
            }
            out
        };
        // Write-through the merged nodes to the durable instance store.
        if let Some(store) = self.kb.instance_stores.get(&uuid) {
            for node in &merged {
                if let Err(e) = store.update_node(node) {
                    tracing::warn!(node_id = %node.id, error = %e, "joined-KB instance write-through failed");
                }
            }
        }

        // ADR-022 crash-safety: re-sync any local-ahead edits the daemon lacked.
        // These were re-derived from the durable crdt_doc during reconcile, so they
        // are recovered even if the original pending-queue row was lost in a crash.
        // Route them through the same durable pending queue the live edit path uses
        // (single emit source); the post-(re)connect drain ships them to the daemon.
        if !local_ahead.is_empty() {
            tracing::info!(
                target: "kb_sync", kb_id = %kb_id, count = local_ahead.len(),
                "ADR-022 join: re-syncing recovered local-ahead edit(s) (crash-safe, independent of pending queue)"
            );
            for (node_id, bytes) in &local_ahead {
                if let Some(ref store) = self.kb.store {
                    if let Err(e) = store.push_pending_update(kb_id, node_id, bytes) {
                        tracing::warn!(target: "kb_sync", kb_id = %kb_id, node_id = %node_id, error = %e, "join: failed to re-queue local-ahead update");
                    }
                } else {
                    self.collab.pending_kb_updates.push((
                        kb_id.to_string(),
                        node_id.clone(),
                        bytes.clone(),
                    ));
                }
            }
        }

        // ADR-024 R5: for each node where the (re)join would have overwritten
        // DIFFERENT local content, raise an actionable notification (badge +
        // *Notifications* row) instead of silently clobbering. The local copy was
        // preserved above; the actions run the same adopt-and-re-author flow (R1).
        for node_id in &divergent_conflicts {
            tracing::warn!(target: "kb_sync", kb_id = %kb_id, node_id = %node_id, "join: divergent local content preserved — surfacing resolution (ADR-024 R5)");
            self.notify(
                crate::notifications::Notification::action_required(
                    "collab",
                    format!(
                        "KB '{kb_id}': {node_id} diverged — your local version differs from remote"
                    ),
                )
                .key(format!("collab:diverge:{kb_id}:{node_id}"))
                .body(
                    "Reconnecting found a different remote version. Adopt remote \
                     (discard local), keep yours (re-author), or stash it.",
                )
                .action(
                    "Accept-remote (clobber local)",
                    crate::notifications::NotifCommand::AdoptRemote {
                        kb_id: kb_id.to_string(),
                        node_id: node_id.clone(),
                    },
                )
                .action(
                    "Keep-mine (re-author)",
                    crate::notifications::NotifCommand::KeepMine {
                        kb_id: kb_id.to_string(),
                        node_id: node_id.clone(),
                    },
                )
                .action(
                    "Stash externally",
                    crate::notifications::NotifCommand::StashExternally {
                        kb_id: kb_id.to_string(),
                        node_id: node_id.clone(),
                    },
                ),
            );
        }

        // Durable registry marker (idempotent).
        if let Some(dir) = self.mae_data_dir() {
            let (registry, (), saved) = mae_kb::federation::KbRegistry::update(&dir, |reg| {
                let now = mae_kb::data_dir::chrono_now_iso();
                match reg.find_mut(&uuid) {
                    Some(inst) => {
                        inst.shared = true;
                        inst.collab_id = Some(kb_id.to_string());
                        inst.last_sync = Some(now);
                    }
                    None => {
                        reg.instances.push(mae_kb::federation::KbInstance {
                            uuid: uuid.clone(),
                            name: kb_id.to_string(),
                            org_dir: std::path::PathBuf::new(),
                            db_path,
                            primary: false,
                            enabled: true,
                            last_import: None,
                            collab_id: Some(kb_id.to_string()),
                            shared: true,
                            remote_peers: Vec::new(),
                            last_sync: Some(now),
                            ai_residency: mae_kb::federation::AiResidency::default(),
                        });
                    }
                }
            });
            if let Err(e) = saved {
                tracing::warn!(kb = %kb_id, error = %e, "failed to persist joined-KB registry marker");
            }
            self.kb.registry = registry;
            self.kb.last_local_registry_write = Some(std::time::Instant::now());
        }
        self.kb.rebuild_query_layer();
        tracing::debug!(target: "kb_sync", kb_id = %kb_id, uuid = %uuid, node_count = nodes.len(), merged = merged.len(), "join: registered first-class instance (reconciled)");
        uuid
    }

    /// The collab ids of every KB this editor durably syncs (ADR-019): the
    /// primary-share marker + each shared registered instance. Used on
    /// (re)connect to re-subscribe so remote edits resume flowing after a
    /// restart, and at startup to warm the cache.
    pub fn durable_shared_kb_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        if self.kb.registry.primary_shared {
            ids.push(
                self.kb
                    .registry
                    .primary_collab_id
                    .clone()
                    .unwrap_or_else(|| crate::editor::KB_DEFAULT_NAME.to_string()),
            );
        }
        for inst in &self.kb.registry.instances {
            if inst.shared {
                if let Some(c) = &inst.collab_id {
                    ids.push(c.clone());
                }
            }
        }
        ids
    }

    /// Re-subscribe intents for every durably-shared *instance* on reconnect
    /// (ADR-019). A **guest** (joined KB — empty `org_dir`) re-JOINS to
    /// re-subscribe (as a member the daemon returns it immediately, no pending
    /// pop); an **owner** (shared a registered instance — real `org_dir`)
    /// re-SHARES to re-establish + re-subscribe (silent). The **primary KB is
    /// skipped**: re-joining one's own primary produces a spurious pending
    /// request (and re-uploading thousands of nodes is wrong) — that was the
    /// "Collab Status pops up on launch" regression.
    /// Gather this editor's per-node state vectors for a shared KB (ADR-022),
    /// sent on (re)join so the daemon replies with incremental diffs and we
    /// reconcile (merge, no clobber) rather than adopt a full snapshot. Empty if
    /// we hold no local instance for `kb_id` (first-ever join → full state). This
    /// is the durable-content side of crash-safety: the SVs are derived from the
    /// persisted `crdt_doc`s, independent of any pending-queue row.
    pub fn kb_join_node_svs(&self, kb_id: &str) -> Vec<(String, Vec<u8>)> {
        let Some(inst) = self.kb.registry.find_by_collab_id(kb_id) else {
            return Vec::new();
        };
        let Some(kb) = self.kb.instances.get(&inst.uuid) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (id, node) in kb.iter() {
            match node.to_crdt_doc() {
                Ok(doc) => out.push((id.clone(), doc.state_vector())),
                Err(e) => {
                    tracing::warn!(node_id = %id, error = %e, "kb_join_node_svs: skipping node with no CRDT doc")
                }
            }
        }
        out
    }

    pub fn kb_resubscribe_intents(&self) -> Vec<crate::editor::CollabIntent> {
        use crate::editor::CollabIntent;
        let mut out = Vec::new();
        for inst in &self.kb.registry.instances {
            if !inst.shared {
                continue;
            }
            let Some(kb_id) = inst.collab_id.clone() else {
                continue;
            };
            if inst.org_dir.as_os_str().is_empty() {
                let node_svs = self.kb_join_node_svs(&kb_id);
                out.push(CollabIntent::JoinKb { kb_id, node_svs });
            } else {
                out.push(CollabIntent::ShareKb {
                    kb_name: inst.name.clone(),
                    node_ids: vec![],
                });
            }
        }
        out
    }

    /// Rebuild the transient `shared_kbs` node-id index from DURABLE markers
    /// (ADR-019). Local-only — no daemon round-trip. The emit gate already
    /// works from the markers; this warms the cache (status/mDNS counts, fast
    /// reverse lookups) so a restart leaves the editor in a consistent state.
    pub fn reconstruct_kb_sync_gate(&mut self) {
        if self.kb.registry.primary_shared {
            let kb_id = self
                .kb
                .registry
                .primary_collab_id
                .clone()
                .unwrap_or_else(|| crate::editor::KB_DEFAULT_NAME.to_string());
            let ids: std::collections::HashSet<String> =
                self.kb.primary.list_ids(None).into_iter().collect();
            self.collab.shared_kbs.insert(kb_id, ids);
        }
        let shared: Vec<(String, String)> = self
            .kb
            .registry
            .instances
            .iter()
            .filter(|i| i.shared)
            .filter_map(|i| i.collab_id.clone().map(|c| (i.uuid.clone(), c)))
            .collect();
        for (uuid, collab_id) in shared {
            let ids: std::collections::HashSet<String> = self
                .kb
                .instances
                .get(&uuid)
                .map(|kb| kb.list_ids(None).into_iter().collect())
                .unwrap_or_default();
            self.collab.shared_kbs.insert(collab_id, ids);
        }
    }

    /// The collaborative id a node's owning KB is shared under, derived from
    /// **durable** registry markers (ADR-019) — not the transient `shared_kbs`
    /// cache. This is the broadcast-gate authority, so a shared KB keeps
    /// propagating edits across editor restart/reconnect (the cache may be
    /// empty until reconstruction runs). `owner == None` ⇒ primary KB;
    /// `Some(uuid)` ⇒ a federated instance.
    fn kb_collab_id_of(&self, owner: &Option<String>) -> Option<String> {
        match owner {
            None => self.kb.registry.primary_shared.then(|| {
                self.kb
                    .registry
                    .primary_collab_id
                    .clone()
                    .unwrap_or_else(|| crate::editor::KB_DEFAULT_NAME.to_string())
            }),
            Some(uuid) => self
                .kb
                .registry
                .find_by_uuid(uuid)
                .filter(|i| i.shared)
                .and_then(|i| i.collab_id.clone()),
        }
    }

    /// Recompute whether the daemon is hosting the primary KB right now (Phase D,
    /// ADR-029). The **single writer** of `daemon_hosts_primary`: hosting is on iff
    /// the user opted in (`daemon_default`), a daemon read layer is wired
    /// (`has_daemon`), and the collab write channel is connected (so primary edits
    /// can reach the daemon's CRDT). Call after daemon connect, on collab
    /// connect/disconnect, and on `set_option("daemon_default", …)`.
    ///
    /// Deliberately distinct from the durable `registry.primary_shared` (peer-share
    /// intent): hosting is runtime-only, so it never implies peer broadcast and never
    /// leaks into a later daemon-less launch. The collab connection in the typical
    /// setup is the local daemon; distinguishing a remote peer from the local daemon
    /// is a later refinement (the gate is opt-in via `daemon_default`).
    pub fn refresh_daemon_host_state(&mut self) {
        let hosting = self.kb.daemon_default
            && self.kb.has_daemon()
            && matches!(
                self.collab.status,
                crate::editor::CollabStatus::Connected { .. }
            );
        self.kb.set_daemon_hosts_primary(hosting);
    }

    /// The collab id a node's edits should sync under, or `None` if this node's
    /// KB doesn't sync. The single broadcast gate (ADR-019 + Phase D): an owning
    /// KB with a durable share marker (`kb_collab_id_of`), or — for the primary —
    /// the daemon-hosted "default" when `daemon_hosts_primary`. Gated on
    /// `kb_sync_mode == "on_save"`. Shared by update/create/delete.
    fn kb_sync_target(&self, owner: &Option<String>) -> Option<String> {
        if self.collab.kb_sync_mode != "on_save" {
            return None;
        }
        self.kb_collab_id_of(owner).or_else(|| {
            (owner.is_none() && self.kb.daemon_hosts_primary())
                .then(|| crate::editor::KB_DEFAULT_NAME.to_string())
        })
    }

    /// CRDT-upsert `node` on its owning in-memory KB and enqueue the resulting
    /// `kb/node_update` to EXACTLY ONE queue (ADR-020 single-source emit): the
    /// crash-durable SQLite pending queue when a store exists (persisted at edit
    /// time, even offline), else the transient in-memory fallback. The peer's
    /// stable, epoch-rotated `client_id` authors the edit (ADR-020 B-16 / ADR-023).
    /// Shared by `kb_update_node` + `kb_create_node`.
    fn kb_enqueue_node_crdt(
        &mut self,
        owner: &Option<String>,
        kb_id: &str,
        node_id: &str,
        node: mae_kb::Node,
    ) {
        let cid = self.kb_client_id_for(kb_id);
        let update_bytes = match owner {
            None => self.kb.primary.upsert_with_crdt(node, cid),
            Some(uuid) => self
                .kb
                .instances
                .get_mut(uuid)
                .and_then(|kb| kb.upsert_with_crdt(node, cid)),
        };
        let Some(update_bytes) = update_bytes else {
            return;
        };
        if let Some(ref store) = self.kb.store {
            match store.push_pending_update(kb_id, node_id, &update_bytes) {
                Ok(()) => {
                    tracing::debug!(target: "kb_sync", kb_id = %kb_id, node_id = %node_id, bytes = update_bytes.len(), "edit: persisted to durable pending queue (survives offline + restart)")
                }
                Err(e) => {
                    tracing::warn!(target: "kb_sync", kb_id = %kb_id, node_id = %node_id, error = %e, "edit: failed to persist durable pending update")
                }
            }
        } else {
            self.collab.pending_kb_updates.push((
                kb_id.to_string(),
                node_id.to_string(),
                update_bytes,
            ));
        }
    }

    /// Phase D1.1: enqueue a collection-manifest op (`kb/collection_node_*`) so a
    /// created node joins the daemon's `kbc:` manifest (projector materializes it)
    /// or a deleted one leaves it. Best-effort (drained when connected; creates also
    /// self-heal on the reconnect re-share).
    fn kb_enqueue_manifest_op(&mut self, kb_id: &str, node_id: &str, title: &str, add: bool) {
        self.collab.pending_kb_manifest.push((
            kb_id.to_string(),
            node_id.to_string(),
            title.to_string(),
            add,
        ));
    }

    /// Phase D3 (ADR-029): ensure node `id` is present in the in-memory primary
    /// mirror, lazily hydrating it on a miss. When the daemon hosts the primary the
    /// mirror is NOT preloaded (thin startup), but the edit path needs the node WITH
    /// its CRDT lineage in `kb.primary`.
    ///
    /// D3b — true thin client: hydrate from the **daemon's authoritative CRDT state**
    /// (`node_crdt_state` → `apply_remote_update`, which adopts the daemon's lineage),
    /// so the edit chains onto current shared state. Falls back to the open local
    /// store only when the daemon can't serve it (offline robustness). No-op when
    /// already resident, not daemon-hosted, or absent everywhere.
    fn kb_ensure_node_loaded(&mut self, id: &str) {
        // Gate on the thin-mirror marker, NOT `daemon_hosts_primary` (which needs the
        // collab write channel): hydration must work as soon as the daemon read layer
        // is up — including the startup→collab-connect window.
        if !self.kb.primary_thin() || self.kb.primary.get(id).is_some() {
            return;
        }
        // Prefer the daemon (authoritative, fresh content + correct lineage).
        let daemon_state = self.kb.query_layer().and_then(|q| q.node_crdt_state(id));
        if let Some(state) = daemon_state {
            match self.kb.primary.apply_remote_update(id, &state) {
                Ok(_) if self.kb.primary.get(id).is_some() => {
                    tracing::debug!(target: "kb_sync", node_id = %id, "D3b: hydrated node from daemon for edit");
                    return;
                }
                Ok(_) => {} // applied but node still absent — fall through to the store
                Err(e) => {
                    tracing::warn!(target: "kb_sync", node_id = %id, error = %e, "D3b: daemon hydrate failed; trying local store")
                }
            }
        }
        // Fallback: the open local store (daemon miss / offline). Its row carries the
        // persisted `crdt_doc`, so the lineage is still preserved.
        if let Some(ref store) = self.kb.store {
            match store.get_node(id) {
                Ok(Some(node)) => {
                    tracing::debug!(target: "kb_sync", node_id = %id, "D3b: hydrated node from local store (daemon unavailable)");
                    self.kb.primary.insert(node);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(target: "kb_sync", node_id = %id, error = %e, "D3b: lazy node load failed");
                }
            }
        }
    }

    /// Apply a remote CRDT update to a KB node, routing it to its **owning**
    /// store — primary or the owning federated instance — not always primary
    /// (ADR-019 receive-side federation; mirror of the write-side fix). For a
    /// brand-new node not yet present locally, `collab_id_hint` (e.g. the
    /// node-id namespace prefix) routes it to the matching shared instance.
    /// Returns whether content changed. Write-through persists to the owner.
    pub fn kb_apply_remote_update(
        &mut self,
        node_id: &str,
        update: &[u8],
        collab_id_hint: Option<&str>,
    ) -> Result<bool, String> {
        let target: Option<String> = match self.kb_owner_of(node_id) {
            Some(owner) => owner, // Some(uuid) = instance, None = primary
            None => collab_id_hint
                .and_then(|c| self.kb.registry.find_by_collab_id(c))
                .map(|i| i.uuid.clone()),
        };
        let changed = match &target {
            Some(uuid) => match self.kb.instances.get_mut(uuid) {
                Some(kb) => kb
                    .apply_remote_update(node_id, update)
                    .map_err(|e| e.to_string())?,
                None => self
                    .kb
                    .primary
                    .apply_remote_update(node_id, update)
                    .map_err(|e| e.to_string())?,
            },
            None => self
                .kb
                .primary
                .apply_remote_update(node_id, update)
                .map_err(|e| e.to_string())?,
        };
        if changed {
            let node = match &target {
                Some(uuid) => self
                    .kb
                    .instances
                    .get(uuid)
                    .and_then(|kb| kb.get(node_id))
                    .cloned(),
                None => self.kb.primary.get(node_id).cloned(),
            };
            if let Some(node) = node {
                self.kb_persist_node_in(&target, &node);
            }
            // Phase D3b: the node changed remotely — evict the daemon LRU entry so the
            // next daemon-routed read returns the fresh content (no-op for the local
            // query layer, which has no cache). Keeps reads consistent without a full
            // mirror when several editors share a daemon-hosted KB.
            if let Some(q) = self.kb.query_layer() {
                q.invalidate(node_id);
            }
        }
        tracing::debug!(target: "kb_sync", node_id = %node_id, owner = ?target, changed, "recv: applied remote kb update");
        Ok(changed)
    }

    /// Persist a node to its owning store: the primary store, or the matching
    /// federated instance store (keyed by uuid). Best-effort write-through.
    fn kb_persist_node_in(&self, owner: &Option<String>, node: &mae_kb::Node) {
        match owner {
            None => self.kb_persist_node(node),
            Some(uuid) => {
                if let Some(store) = self.kb.instance_stores.get(uuid) {
                    if let Err(e) = store.update_node(node) {
                        tracing::warn!(node_id = %node.id, error = %e, "KB instance store write-through failed");
                    }
                }
            }
        }
    }

    /// Persist a deletion to the backing store (if present). Best-effort.
    fn kb_persist_delete(&self, id: &str) {
        if let Some(ref store) = self.kb.store {
            if let Err(e) = store.delete_node(id) {
                tracing::warn!(node_id = %id, error = %e, "KB store delete failed");
            }
        }
    }

    /// Promote a node from a federated/org-dir-imported instance into the
    /// primary (native, CozoDB-backed) KB, so it no longer depends on
    /// `source_file`/the instance's `org_dir` to resolve (#303).
    ///
    /// This is an interim, editor-side bridge toward issue #111 ("org
    /// ingestion as import + headless host") / ADR-029's "org dirs are
    /// import-only" direction — NOT that epic's full daemon-side pipeline.
    /// Deliberately narrow scope:
    ///  - Rejects a node already in primary, or one that doesn't exist
    ///    anywhere.
    ///  - Copies title/body/tags/kind/aliases; does NOT copy `source_file`
    ///    (ephemeral anyway, `#[serde(skip)]`) — the promoted copy is no
    ///    longer file-tethered.
    ///  - Stamps `promoted_from_{uuid,org_dir,path}`/`promoted_at` into
    ///    `node.properties` (already durably persisted as `properties_json`
    ///    — no schema migration) so provenance isn't silently lost.
    ///  - The node's id is UNCHANGED — nothing elsewhere in the KB graph
    ///    needs link-rewriting, since resolution is by id string.
    ///  - Leaves the original org file on disk untouched, and leaves the
    ///    federated instance's own copy of the node in place (no
    ///    dedup-on-promote in this first cut) — conservative, Alpha-
    ///    appropriate defaults.
    ///
    /// Persistence mirrors the existing `kb_create_node`/`kb_persist_node`
    /// idiom exactly (including the daemon-hosted-primary CRDT-enqueue
    /// path) rather than inventing a new write pattern: best-effort — a
    /// durable-store write failure is logged and does not roll back the
    /// in-memory insert, matching how every other primary-node write in
    /// this codebase already behaves.
    pub fn kb_promote_node(&mut self, node_id: &str) -> Result<KbPromoteResult, String> {
        self.kb_write_blocked()?;

        if self.kb.primary.contains(node_id) {
            return Err(format!("'{}' is already in the primary KB", node_id));
        }
        let owner_uuid = self
            .kb
            .instances
            .iter()
            .find(|(_, kb)| kb.contains(node_id))
            .map(|(uuid, _)| uuid.clone())
            .ok_or_else(|| format!("No KB node: {}", node_id))?;
        let instance = self
            .kb
            .registry
            .find(&owner_uuid)
            .cloned()
            .ok_or_else(|| format!("KB instance '{}' not found in registry", owner_uuid))?;
        let mut node = self
            .kb
            .instances
            .get(&owner_uuid)
            .and_then(|kb| kb.get(node_id))
            .cloned()
            .ok_or_else(|| format!("No KB node: {}", node_id))?;

        let promoted_from_path = node
            .source_file
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        node.source_file = None;
        node.properties
            .insert("promoted_from_uuid".to_string(), owner_uuid.clone());
        node.properties.insert(
            "promoted_from_org_dir".to_string(),
            instance.org_dir.display().to_string(),
        );
        node.properties
            .insert("promoted_from_path".to_string(), promoted_from_path);
        node.properties
            .insert("promoted_at".to_string(), chrono_now());

        let owner: Option<String> = None; // primary
        self.kb_persist_node_in(&owner, &node);
        if let Some(kb_id) = self.kb_sync_target(&owner) {
            self.kb_enqueue_node_crdt(&owner, &kb_id, node_id, node.clone());
            self.kb_enqueue_manifest_op(&kb_id, node_id, &node.title, true);
        } else {
            self.kb.primary.insert(node);
        }
        self.kb.last_local_store_write = Some(std::time::Instant::now());
        // So `kb_for_node`/`kb_contains_any`/`open_help_at` observe the
        // promoted (primary) copy on the very next lookup — a stale query
        // layer here would silently reintroduce a variant of #303.
        self.kb.rebuild_query_layer();

        self.set_status(format!("Promoted '{}' to the primary KB", node_id));
        Ok(KbPromoteResult {
            node_id: node_id.to_string(),
            promoted_from_uuid: owner_uuid,
            promoted_from_org_dir: instance.org_dir,
        })
    }

    /// Create a new KB node in the local knowledge base.
    /// Rejects overwriting seed nodes (built-in help).
    pub fn kb_create_node(
        &mut self,
        id: &str,
        title: &str,
        body: &str,
        kind: mae_kb::NodeKind,
    ) -> Result<(), String> {
        self.kb_write_blocked()?;
        // Guard: refuse to overwrite seed nodes
        if let Some(existing) = self.kb.primary.get(id) {
            if existing.source == Some(mae_kb::NodeSource::Seed) {
                return Err(format!(
                    "Cannot overwrite seed node '{}' — built-in help is protected",
                    id
                ));
            }
        }
        let node =
            mae_kb::Node::new(id, title, kind, body).with_source(mae_kb::NodeSource::Manual, 0);
        // #165: route by the id's instance prefix (`collabtest:foo` → the registered
        // `collabtest` federated instance), else the primary KB. A NEW node can't be
        // resolved with `kb_owner_of` (nothing exists yet), so route by the instance-name
        // prefix that federated-instance node ids follow — the prefix only diverts to an
        // instance that is actually REGISTERED (a primary-namespace prefix like `concept:`
        // with no matching instance stays in primary). Without this, every create fell to
        // owner=None → primary, so a node added to a shared instance never resolved its
        // collab_id, never fired the broadcast gate, and never synced.
        let owner: Option<String> = id
            .split_once(':')
            .and_then(|(prefix, _)| self.kb.registry.find(prefix).map(|i| i.uuid.clone()));
        // Persist to the OWNING store (primary or the matching instance store).
        self.kb_persist_node_in(&owner, &node);
        // Phase D1.1 (ADR-029): a created node on a daemon-hosted (or shared) KB must reach
        // the daemon's CRDT — author it via `upsert_with_crdt` (enqueues the node doc) AND
        // add it to the `kbc:` manifest, so the projector materializes it. Otherwise a
        // create would only sync on its first edit. Non-syncing → plain insert into the
        // owning in-memory KB (today's embedded behavior).
        if let Some(kb_id) = self.kb_sync_target(&owner) {
            self.kb_enqueue_node_crdt(&owner, &kb_id, id, node);
            self.kb_enqueue_manifest_op(&kb_id, id, title, true);
        } else {
            match &owner {
                Some(uuid) => match self.kb.instances.get_mut(uuid) {
                    Some(kb) => {
                        kb.insert(node);
                    }
                    None => {
                        self.kb.primary.insert(node);
                    }
                },
                None => {
                    self.kb.primary.insert(node);
                }
            }
        }
        // Phase 4: record the local write so the store watcher's cooldown skips a
        // redundant cross-instance reload of our own change.
        self.kb.last_local_store_write = Some(std::time::Instant::now());
        self.set_status(format!("KB node created: {}", id));
        Ok(())
    }

    /// Delete a KB node from the local knowledge base.
    /// Rejects deleting seed nodes (built-in help).
    pub fn kb_delete_node(&mut self, id: &str) -> Result<(), String> {
        self.kb_write_blocked()?;
        // Phase D3: lazily load the node into the thin-startup mirror so it resolves.
        self.kb_ensure_node_loaded(id);
        // Resolve across primary ∪ federated instances (I-9), like update/read.
        let owner = self
            .kb_owner_of(id)
            .ok_or_else(|| format!("No KB node: {}", id))?;
        let node = match &owner {
            None => self.kb.primary.get(id),
            Some(uuid) => self.kb.instances.get(uuid).and_then(|kb| kb.get(id)),
        }
        .ok_or_else(|| format!("No KB node: {}", id))?;
        if node.source == Some(mae_kb::NodeSource::Seed) {
            return Err(format!(
                "Cannot delete seed node '{}' — built-in help is protected",
                id
            ));
        }
        match &owner {
            None => {
                self.kb_persist_delete(id);
                self.kb.primary.remove(id);
            }
            Some(uuid) => {
                if let Some(store) = self.kb.instance_stores.get(uuid) {
                    if let Err(e) = store.delete_node(id) {
                        tracing::warn!(node_id = %id, error = %e, "KB instance store delete failed");
                    }
                }
                if let Some(kb) = self.kb.instances.get_mut(uuid) {
                    kb.remove(id);
                }
            }
        }
        // Phase D1.1 (ADR-029): if this node's KB syncs to the daemon, remove it from
        // the `kbc:` manifest so the projector deletes it from the daemon's projection.
        // (The node doc itself is left orphaned + idle-evicted.) Best-effort: an
        // offline delete is NOT healed by the reconnect re-share (a CRDT merge only
        // adds), so it propagates only when connected — acceptable while the local cozo
        // remains authoritative (durable manifest ops land in D3).
        if let Some(kb_id) = self.kb_sync_target(&owner) {
            self.kb_enqueue_manifest_op(&kb_id, id, "", false);
        }
        self.kb.last_local_store_write = Some(std::time::Instant::now());
        self.set_status(format!("KB node deleted: {}", id));
        Ok(())
    }

    /// This peer's stable, unique yrs `client_id` for local KB CRDT edits
    /// (ADR-020 B-16), set once at startup from the durable collab identity
    /// fingerprint. Falls back to `1` only when no collab identity is configured
    /// (single, unshared peer — no collision possible).
    pub fn kb_local_client_id(&self) -> u64 {
        if self.collab.local_kb_client_id != 0 {
            self.collab.local_kb_client_id
        } else {
            1
        }
    }

    /// ADR-023: the yrs `client_id` this peer must author edits to a *specific
    /// shared KB* under — its base identity client_id rotated by the KB's current
    /// **authorization epoch** (learned from that KB's `kbc:` collection doc). A
    /// role change bumps the epoch ⇒ a fresh, unrelated client_id, so the daemon
    /// fences the peer's pre-change lineage (`"rebase required"`) and only fresh,
    /// current-epoch ops are accepted. At epoch 0 (fresh grant / owner / directly-
    /// added editor) this equals `kb_local_client_id()` — no behavioural change.
    pub fn kb_client_id_for(&self, kb_id: &str) -> u64 {
        let epoch = self.collab.kb_epochs.get(kb_id).copied().unwrap_or(0);
        if epoch == 0 || self.collab.local_fingerprint.is_empty() {
            return self.kb_local_client_id();
        }
        crate::editor::derive_kb_client_id(&self.collab.local_fingerprint, epoch)
    }

    /// ADR-024 R1: replace a node's local CRDT doc with the daemon's authoritative
    /// `state`, DROPPING any divergent (fenced stale-epoch) local ops, then persist.
    /// This is the member-side "adopt authoritative state" the daemon's `rebase
    /// required` error asks for — the editor can't self-adopt because its local doc
    /// still carries the rejected op. Routes to the node's owning KB (instance or
    /// primary); falls back to primary if the node isn't found locally.
    pub fn kb_adopt_node(&mut self, node_id: &str, state: &[u8]) -> Result<bool, String> {
        let owner = self.kb_owner_of(node_id).unwrap_or(None);
        let changed = match &owner {
            None => self.kb.primary.adopt_remote_node(node_id, state),
            Some(uuid) => self
                .kb
                .instances
                .get_mut(uuid)
                .ok_or_else(|| format!("no KB instance {uuid}"))?
                .adopt_remote_node(node_id, state),
        }
        .map_err(|e| e.to_string())?;
        let node = match &owner {
            None => self.kb.primary.get(node_id).cloned(),
            Some(uuid) => self
                .kb
                .instances
                .get(uuid)
                .and_then(|k| k.get(node_id))
                .cloned(),
        };
        if let Some(n) = node {
            self.kb_persist_node_in(&owner, &n);
        }
        Ok(changed)
    }

    /// ADR-020 B-16: establish + persist a canonical CRDT lineage for every node
    /// about to be shared. A node that was never CRDT-edited has `crdt_doc == None`;
    /// `to_collection` would then mint an EPHEMERAL, non-persisted lineage (fresh
    /// random doc each call) — so the owner's local doc never matches the lineage
    /// peers adopt on join, and a peer's later edit no-ops against the owner's
    /// divergent doc. Here we `upsert_with_crdt` each such node with THIS peer's
    /// stable client_id (persisting the lineage onto the node) and write it through
    /// to the durable store, so the owner's local doc IS the shared lineage.
    /// Plaintext CRDT state per shared node `(node_id, encode_state)` — the canonical
    /// lineage the daemon already holds (established by [`Self::kb_prepare_share_lineage`]
    /// at share). Read-only. Used to RE-SEAL nodes when E2e is enabled on an
    /// already-shared KB (#171): the network task seeds `seal_op` with each node's
    /// current state so the sealed op-set CONTINUES the node's client-id lineage (no
    /// clock collision with the plaintext base) and joiners can open the sealed content.
    pub fn kb_share_node_states(&self, kb_name: &str) -> Vec<(String, Vec<u8>)> {
        let is_primary = kb_name == crate::editor::KB_DEFAULT_NAME || kb_name == "primary";
        let kb = if is_primary {
            Some(&self.kb.primary)
        } else {
            let uuid = self.kb.registry.find(kb_name).map(|i| i.uuid.clone());
            uuid.and_then(|u| self.kb.instances.get(&u))
                .or_else(|| self.kb.instances.get(kb_name))
        };
        kb.and_then(|kb| kb.to_collection(kb_name, "", &[]).ok())
            .map(|(_coll, node_states)| node_states)
            .unwrap_or_default()
    }

    pub fn kb_prepare_share_lineage(&mut self, kb_name: &str, node_ids: &[String]) {
        let cid = self.kb_local_client_id();
        let is_primary = kb_name == crate::editor::KB_DEFAULT_NAME || kb_name == "primary";
        let owner: Option<String> = if is_primary {
            None
        } else {
            match self.kb.registry.find(kb_name).map(|i| i.uuid.clone()) {
                Some(u) => Some(u),
                None => return,
            }
        };
        // Establish + persist lineage in-memory; collect the nodes to write through.
        let updated: Vec<mae_kb::Node> = {
            let kb = match &owner {
                None => &mut self.kb.primary,
                Some(u) => match self.kb.instances.get_mut(u) {
                    Some(k) => k,
                    None => return,
                },
            };
            let ids: Vec<String> = if node_ids.is_empty() {
                kb.list_ids(None)
            } else {
                node_ids.to_vec()
            };
            let mut out = Vec::new();
            for id in ids {
                let needs = kb.get(&id).map(|n| n.crdt_doc.is_none()).unwrap_or(false);
                if needs {
                    if let Some(node) = kb.get(&id).cloned() {
                        // upsert_with_crdt stores the new crdt_doc onto the node.
                        let _ = kb.upsert_with_crdt(node, cid);
                        if let Some(n) = kb.get(&id) {
                            out.push(n.clone());
                        }
                    }
                }
            }
            out
        };
        if !updated.is_empty() {
            tracing::debug!(target: "kb_sync", kb = %kb_name, count = updated.len(), client_id = cid, "share: established + persisted canonical lineage for unedited nodes");
            for node in &updated {
                self.kb_persist_node_in(&owner, node);
            }
        }
    }

    /// Update fields on an existing KB node.
    /// Rejects modifying seed nodes (built-in help).
    pub fn kb_update_node(
        &mut self,
        id: &str,
        title: Option<&str>,
        body: Option<&str>,
        tags: Option<Vec<String>>,
    ) -> Result<(), String> {
        self.kb_update_node_with(id, |updated| {
            if let Some(t) = title {
                updated.title = t.to_string();
            }
            if let Some(b) = body {
                updated.body = b.to_string();
            }
            if let Some(t) = tags {
                updated.tags = t;
            }
        })?;
        self.set_status(format!("KB node updated: {}", id));
        Ok(())
    }

    /// Set a node's molecular-note role (source | atom | molecule | hub), stamped into
    /// the generic `:role:` PROPERTIES-drawer field — orthogonal to `NodeKind`'s own
    /// `:kind:` (MAE's doc taxonomy: Concept/Task/etc). A node can be both `:kind:
    /// concept` and `:role: atom` simultaneously; the two axes are independent. Reuses
    /// the same generic PROPERTIES-drawer parsing `shared/kb/src/org.rs` already applies
    /// to any non-`:ID:` heading property — no new parsing code, just a new recognized
    /// value written through the existing update path.
    pub fn kb_set_role(&mut self, id: &str, role: &str) -> Result<String, String> {
        let role = role.to_ascii_lowercase();
        if !["source", "atom", "molecule", "hub"].contains(&role.as_str()) {
            return Err(format!(
                "Invalid role '{}': expected source|atom|molecule|hub",
                role
            ));
        }
        self.kb_update_node_with(id, |updated| {
            updated.properties.insert("role".to_string(), role.clone());
        })?;
        let msg = format!("KB node '{}' role set to {}", id, role);
        self.set_status(msg.clone());
        Ok(msg)
    }

    /// Shared resolve → mutate → persist skeleton behind `kb_update_node` and
    /// `kb_set_role` — the CRDT-enqueue-vs-direct-persist branching (ADR-019/ADR-020)
    /// is real, non-trivial logic; this avoids duplicating it for every field-specific
    /// update method, letting each just supply its own `mutate` closure.
    fn kb_update_node_with(
        &mut self,
        id: &str,
        mutate: impl FnOnce(&mut mae_kb::Node),
    ) -> Result<(), String> {
        self.kb_write_blocked()?;
        // Phase D3: thin-startup mirror may not hold this node yet — lazily load it
        // (with its CRDT lineage) from the open store before resolving the owner.
        self.kb_ensure_node_loaded(id);
        // Resolve the node across primary ∪ federated instances (I-9): a shared
        // KB lives in `instances` on the host that registered it, and in
        // `primary` on a peer that joined it. The write path must find it in
        // either, mirroring the read path — not primary-only.
        let owner = self
            .kb_owner_of(id)
            .ok_or_else(|| format!("No KB node: {}", id))?;
        let existing = match &owner {
            None => self.kb.primary.get(id),
            Some(uuid) => self.kb.instances.get(uuid).and_then(|kb| kb.get(id)),
        }
        .ok_or_else(|| format!("No KB node: {}", id))?
        .clone();
        if existing.source == Some(mae_kb::NodeSource::Seed) {
            return Err(format!(
                "Cannot modify seed node '{}' — built-in help is protected",
                id
            ));
        }
        let mut updated = existing;
        mutate(&mut updated);

        // Does this node's OWNING KB sync, per durable registry markers
        // (ADR-019)? Derived from the owning instance's `shared`/`collab_id`,
        // not the transient `shared_kbs` cache — so edits broadcast even right
        // after a restart, before the cache is reconstructed.
        let shared_kb_id = self.kb_sync_target(&owner);
        tracing::debug!(
            target: "kb_sync",
            node_id = %id,
            owner = ?owner,
            sync_mode = %self.collab.kb_sync_mode,
            gate_hit = shared_kb_id.is_some(),
            "kb edit: broadcast-gate decision"
        );

        if let Some(kb_id) = shared_kb_id {
            // CRDT-aware upsert on the OWNING in-memory KB → enqueue the kb/node_update
            // (durable or transient; ADR-020 B-16 / ADR-023 epoch-rotated client_id).
            self.kb_enqueue_node_crdt(&owner, &kb_id, id, updated);
            // Persist the updated node to its owning store.
            let persisted = match &owner {
                None => self.kb.primary.get(id).cloned(),
                Some(uuid) => self
                    .kb
                    .instances
                    .get(uuid)
                    .and_then(|kb| kb.get(id))
                    .cloned(),
            };
            if let Some(node) = persisted {
                self.kb_persist_node_in(&owner, &node);
            }
        } else {
            self.kb_persist_node_in(&owner, &updated);
            match &owner {
                None => {
                    self.kb.primary.insert(updated);
                }
                Some(uuid) => {
                    if let Some(kb) = self.kb.instances.get_mut(uuid) {
                        kb.insert(updated);
                    }
                }
            }
        }

        self.kb.last_local_store_write = Some(std::time::Instant::now());
        Ok(())
    }

    /// Queue a KB collaboration lifecycle action as a `CollabIntent` for the
    /// binary's collab bridge to drain — the single typed path the Scheme
    /// primitives (`(kb-share)` …) route through, so they reuse the SAME intent
    /// the commands + MCP tools use (no `(execute-ex …)` string building; #3, #7).
    /// `Join` computes its node state-vectors editor-side (ADR-022).
    pub fn queue_kb_collab_action(&mut self, action: crate::editor::KbCollabAction) {
        use crate::editor::{CollabIntent, KbCollabAction};
        let intent = match action {
            KbCollabAction::Share { kb_name } => CollabIntent::ShareKb {
                kb_name,
                node_ids: vec![],
            },
            KbCollabAction::Join { kb_id } => {
                let node_svs = self.kb_join_node_svs(&kb_id);
                CollabIntent::JoinKb { kb_id, node_svs }
            }
            KbCollabAction::Leave { kb_id } => CollabIntent::LeaveKb { kb_id },
            KbCollabAction::AddMember {
                kb_id,
                member,
                role,
            } => CollabIntent::KbAddMember {
                kb_id,
                member,
                role,
            },
            KbCollabAction::RemoveMember { kb_id, member } => {
                CollabIntent::KbRemoveMember { kb_id, member }
            }
            KbCollabAction::Approve {
                kb_id,
                principal,
                role,
            } => CollabIntent::KbApprove {
                kb_id,
                principal,
                role,
            },
            KbCollabAction::SetPolicy { kb_id, policy } => {
                CollabIntent::KbSetPolicy { kb_id, policy }
            }
            KbCollabAction::SetEncryption { kb_id, mode } => {
                // CF1 (SECURITY_REVIEW §6.3): the honest E2E caveats must reach the user at the
                // POINT OF ACTION, not only in docs/E2E_ENCRYPTION.md §7. Surface them the moment
                // E2E is enabled (one-way, irreversible) so "E2E" is not silently oversold.
                if mode.eq_ignore_ascii_case("e2e") {
                    self.message_log.push(
                        crate::messages::MessageLevel::Warn,
                        "kb-encryption",
                        E2E_ENABLE_ADVISORY,
                    );
                    self.set_status(
                        "E2E enabled (one-way): protects node CONTENT only — no forward secrecy, \
                         metadata still visible. See :help concept:kb-encryption (full note in *Messages*).",
                    );
                }
                CollabIntent::KbSetEncryption { kb_id, mode }
            }
            KbCollabAction::SetBlock {
                kb_id,
                member,
                blocked,
            } => CollabIntent::KbSetBlock {
                kb_id,
                member,
                blocked,
            },
        };
        // The command + MCP surfaces queue one action per apply cycle, but the
        // Scheme/AI surface can lower SEVERAL lifecycle calls in a single eval
        // (e.g. bulk member onboarding: `(kb-add-member …)(kb-add-member …)`).
        // The single `pending_intent` slot only holds the LAST, silently dropping
        // the rest. Fan the overflow through the same one-per-tick `reconnect_intents`
        // queue the reconnect path uses (see collab_bridge drain), preserving order.
        if self.collab.pending_intent.is_none() {
            self.collab.pending_intent = Some(intent);
        } else {
            self.collab.reconnect_intents.push_back(intent);
        }
    }

    /// Build this peer's KB-sharing introspection snapshot — the single source of
    /// truth shared by the `*KB Sharing*` buffer, the `kb_sharing_status` MCP tool,
    /// and the `(kb-sharing-status)` Scheme primitive (CLAUDE.md #3, #8). Pure read
    /// from local collection replicas; the daemon stays authoritative.
    pub fn kb_sharing_snapshot(&self) -> crate::kb_sharing::KbSharingSnapshot {
        crate::kb_sharing::build_snapshot(&self.collab)
    }

    /// The KB-sharing snapshot serialized to JSON — for the `(kb-sharing-status)`
    /// Scheme primitive and the `kb_sharing_status` MCP tool (serde lives here in
    /// mae-core, not in mae-scheme). `{}` if serialization fails.
    pub fn kb_sharing_snapshot_json(&self) -> String {
        serde_json::to_string(&self.kb_sharing_snapshot()).unwrap_or_else(|_| "{}".to_string())
    }

    /// Show KB instances in a dedicated read-only buffer.
    pub fn show_kb_instances(&mut self) {
        let mut lines = vec![
            "KB Instances".to_string(),
            "============".to_string(),
            String::new(),
        ];
        let count = self.kb.registry.instances.len();
        if self.kb.registry.instances.is_empty() {
            lines.push("  (none registered)".to_string());
        } else {
            for inst in &self.kb.registry.instances {
                let node_count = self
                    .kb
                    .instances
                    .get(&inst.uuid)
                    .map(|kb| kb.len())
                    .unwrap_or(0);
                let status = if inst.enabled { "enabled" } else { "disabled" };
                lines.push(format!(
                    "  {} [{}] — {} nodes, {} ({})",
                    inst.name,
                    inst.uuid,
                    node_count,
                    status,
                    inst.org_dir.display(),
                ));
            }
        }
        let content = lines.join("\n");
        let mut buf = crate::buffer::Buffer::new();
        buf.name = "*KB Instances*".to_string();
        buf.replace_contents(&content);
        buf.modified = false;
        buf.read_only = true;
        let buf_idx = self.buffers.len();
        self.buffers.push(buf);
        self.display_buffer(buf_idx);
        self.set_status(format!("{} KB instance(s) registered", count));
    }

    /// Create a KB note from just a title (org-roam-style).
    ///
    /// Auto-generates a `user:TIMESTAMP-slug` ID. If `kb_notes_dir` is set,
    /// persists the note as an `.org` file and imports it into the matching
    /// KB instance. Otherwise creates an ephemeral in-memory node.
    ///
    /// Returns `(id, Option<path>)` — the node id and the file path if written.
    pub fn kb_create_note_from_title(
        &mut self,
        title: &str,
    ) -> Result<(String, Option<std::path::PathBuf>), String> {
        let slug = mae_kb::slugify(title);
        if slug.is_empty() {
            return Err("Title cannot be empty".to_string());
        }
        let timestamp = mae_kb::timestamp_id();
        let id = format!("user:{}-{}", timestamp, slug);

        if let Some(dir) = self.kb.notes_dir.clone() {
            // Ensure directory exists
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("Cannot create kb-notes-dir: {}", e))?;

            // Write .org file
            let filename = format!("{}.org", slug);
            let path = dir.join(&filename);
            let content = format!(
                ":PROPERTIES:\n:ID: {}\n:END:\n#+title: {}\n#+filetags:\n\n",
                id, title
            );
            std::fs::write(&path, &content)
                .map_err(|e| format!("Cannot write note file: {}", e))?;

            // Insert into matching KB instance (if registered) — durably.
            let matched_instance = self.kb_insert_to_notes_instance(&id, title, &path);

            // Record return buffer before opening new file
            let return_idx = self.active_buffer_idx();

            // Open the file for editing
            self.open_file(&path);

            // Seed KB buffer (hidden) so SPC n v can toggle to rendered view later.
            // Do NOT call open_help_at() — that would display it and create a split.
            let help_idx = self.ensure_kb_buffer_idx(&id);
            self.kb_populate_buffer(help_idx);

            // Enter capture mode (C-c C-c to finalize, C-c C-k to abort)
            self.kb.capture_state = Some(super::CaptureState {
                node_id: id.clone(),
                file_path: Some(path.clone()),
                return_buffer_idx: return_idx,
            });

            let status = if matched_instance {
                format!("Capture: {} — SPC n s to finish | SPC n k to abort", title)
            } else {
                format!(
                    "Capture: {} — no registered KB instance covers kb_notes_dir; saved to primary only (won't sync to other mae processes). SPC n s to finish | SPC n k to abort",
                    title
                )
            };
            self.set_status(status);
            Ok((id, Some(path)))
        } else {
            // Ephemeral in-memory node (fallback)
            self.kb_create_node(&id, title, "", mae_kb::NodeKind::Note)?;
            Ok((id, None))
        }
    }

    /// Insert a node into the KB instance that covers `kb_notes_dir`, durably
    /// (not just the in-memory mirror — otherwise it's invisible to this same
    /// process's own instance-scoped/federated search until some LATER event
    /// happens to reimport it, and to any other process sharing this KB
    /// directory forever, since there's no file-write for a watcher to catch:
    /// the node exists nowhere but this one process's memory).
    /// Falls back to the local/primary KB (also durably) if no registered
    /// instance covers `kb_notes_dir` — which means this note won't be picked
    /// up by that instance's watcher in ANY process, so callers should warn.
    /// Returns `true` if a registered instance was matched, `false` if it fell
    /// back to primary.
    fn kb_insert_to_notes_instance(
        &mut self,
        id: &str,
        title: &str,
        path: &std::path::Path,
    ) -> bool {
        let node = mae_kb::Node::new(id, title, mae_kb::NodeKind::Note, "")
            .with_source(mae_kb::NodeSource::UserOrg, 0)
            .with_source_file(path);

        // Match by canonicalized path, not raw PathBuf equality -- a trailing
        // slash, a symlink, or a relative-vs-absolute kb_notes_dir would
        // otherwise silently fail to match a genuinely-covering instance.
        let notes_dir = self.kb.notes_dir.clone();
        if let Some(ref dir) = notes_dir {
            let dir_canon = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            let matched_uuid = self.kb.registry.instances.iter().find_map(|inst| {
                let inst_canon = inst
                    .org_dir
                    .canonicalize()
                    .unwrap_or_else(|_| inst.org_dir.clone());
                (inst_canon == dir_canon).then(|| inst.uuid.clone())
            });
            if let Some(uuid) = matched_uuid {
                if let Some(kb) = self.kb.instances.get_mut(&uuid) {
                    kb.insert(node.clone());
                }
                if let Some(store) = self.kb.instance_stores.get(&uuid) {
                    if let Err(e) = store.update_node(&node) {
                        tracing::warn!(node_id = %id, error = %e, "KB instance store write-through (note capture) failed");
                    }
                }
                return true;
            }
        }

        // Fallback: no registered instance covers kb_notes_dir -- insert into
        // the primary KB, durably, rather than a silent in-memory-only trap.
        self.kb.primary.insert(node.clone());
        self.kb_persist_node(&node);
        false
    }

    /// Collect all KB node (id, title) pairs from local + federated instances.
    pub fn kb_all_node_pairs(&self) -> Vec<(String, String)> {
        if let Some(q) = self.kb.query_layer() {
            let mut pairs = q.id_title_pairs(None);
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            return pairs;
        }
        let mut pairs: Vec<(String, String)> = self.kb.primary.all_id_title_pairs();
        let mut seen: std::collections::HashSet<String> =
            pairs.iter().map(|(id, _)| id.clone()).collect();

        for kb in self.kb.instances.values() {
            for (id, title) in kb.all_id_title_pairs() {
                if seen.insert(id.clone()) {
                    pairs.push((id, title));
                }
            }
        }
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        pairs
    }

    /// Collect all KB node (id, title, body) triples from local + federated instances.
    /// Used by KB palettes that need body content for search matching.
    /// Sorted according to `kb_search_sort` option: alphabetical (default/relevance),
    /// activity (recent first), or alphabetical.
    pub fn kb_all_node_triples(&self) -> Vec<(String, String, String)> {
        // Body truncated to 500 chars — only used for fuzzy search, not display.
        let mut triples: Vec<(String, String, String)> = if let Some(q) = self.kb.query_layer() {
            q.id_title_body_triples(None, 500)
        } else {
            self.kb.primary.all_id_title_body_triples()
        };
        let mut seen: std::collections::HashSet<String> =
            triples.iter().map(|(id, _, _)| id.clone()).collect();

        if self.kb.query_layer().is_none() {
            for kb in self.kb.instances.values() {
                for (id, title, body) in kb.all_id_title_body_triples() {
                    if seen.insert(id.clone()) {
                        triples.push((id, title, body));
                    }
                }
            }
        }

        if self.kb.search_sort == "activity" {
            let weights = mae_kb::activity::ActivityWeights {
                decay: self.kb.activity_decay,
                ..Default::default()
            };
            let today = today_ymd();
            triples.sort_by(|a, b| {
                let score_a = self.kb_activity_score_for_id(&a.0, &weights, today);
                let score_b = self.kb_activity_score_for_id(&b.0, &weights, today);
                score_b
                    .partial_cmp(&score_a)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.0.cmp(&b.0))
            });
        } else {
            triples.sort_by(|a, b| a.0.cmp(&b.0));
        }
        triples
    }

    /// Node-count signal for deciding the kb-find completion strategy. Uses the
    /// in-memory `primary` (+ instances) length — O(1), no allocation, safe to
    /// call per keystroke. (A Cozo-backed large KB with an empty `primary` falls
    /// back to the eager all-load path; the lazy window targets large in-memory
    /// KBs, which is the common at-scale case.)
    pub fn kb_loaded_node_count(&self) -> usize {
        self.kb.primary.len() + self.kb.instances.values().map(|k| k.len()).sum::<usize>()
    }

    /// Candidate triples (id, title, body≤500) for the kb-find/create palette.
    ///
    /// Small KBs (≤ `KB_FIND_LAZY_THRESHOLD`): return *all* nodes so the palette
    /// filters client-side (instant, no re-search). Large KBs: return a bounded,
    /// query-driven ranked window via `search_ranked` — full-KB-reachable (the
    /// ranker scans primary *and every federated instance*, mirroring
    /// `kb_federated_search_scoped`) yet capped, so per-keystroke work stays
    /// bounded instead of materializing every node. This is the lazy-at-scale
    /// path.
    pub fn kb_find_candidates(&self, query: &str) -> Vec<(String, String, String)> {
        if self.kb_loaded_node_count() <= Self::KB_FIND_LAZY_THRESHOLD {
            return self.kb_all_node_triples();
        }
        let limit = Self::KB_FIND_LAZY_LIMIT;
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut triples: Vec<(String, String, String)> = Vec::new();

        if self.kb.primary_thin() {
            if let Some(ql) = self.kb.query_layer() {
                for hit in ql.search(query, limit) {
                    if let Some(n) = ql.get(&hit.id) {
                        if seen.insert(n.id.clone()) {
                            let body: String = n.body.chars().take(500).collect();
                            triples.push((n.id.clone(), n.title.clone(), body));
                        }
                    }
                }
            }
        } else {
            for (id, _) in self.kb.primary.search_ranked(query, limit) {
                if let Some(n) = self.kb.primary.get(&id) {
                    if seen.insert(n.id.clone()) {
                        let body: String = n.body.chars().take(500).collect();
                        triples.push((n.id.clone(), n.title.clone(), body));
                    }
                }
            }
        }

        // Federated instances (kb-register'd directories) participate too —
        // this is the part `kb_find_candidates` used to skip entirely once a
        // large KB tipped it into the lazy branch, leaving federated content
        // permanently unreachable through kb-find regardless of query.
        if triples.len() < limit {
            for kb in self.kb.instances.values() {
                for (id, _) in kb.search_ranked(query, limit) {
                    if triples.len() >= limit {
                        break;
                    }
                    if let Some(n) = kb.get(&id) {
                        if seen.insert(n.id.clone()) {
                            let body: String = n.body.chars().take(500).collect();
                            triples.push((n.id.clone(), n.title.clone(), body));
                        }
                    }
                }
            }
        }

        triples
    }

    /// Re-derive the kb-find palette after its query changed: re-search a bounded
    /// ranked window for large KBs (lazy), else the standard client-side filter.
    /// A no-op for non-kb-find palettes beyond their usual `update_filter`.
    pub fn kb_find_palette_query_changed(&mut self) {
        use crate::command_palette::PalettePurpose;
        let (is_kb_find, query) = match self.command_palette.as_ref() {
            Some(p) => (p.purpose == PalettePurpose::KbFindOrCreate, p.query.clone()),
            None => return,
        };
        if is_kb_find && self.kb_loaded_node_count() > Self::KB_FIND_LAZY_THRESHOLD {
            let cands = self.kb_find_candidates(&query);
            if let Some(p) = self.command_palette.as_mut() {
                p.set_kb_find_entries(&cands);
            }
        } else if let Some(p) = self.command_palette.as_mut() {
            p.update_filter();
        }
    }

    /// Get activity score for a node by ID, searching local then federated KBs.
    pub fn kb_activity_score_for_id(
        &self,
        id: &str,
        weights: &mae_kb::activity::ActivityWeights,
        today: (i32, u32, u32),
    ) -> f64 {
        if let Some(q) = self.kb.query_layer() {
            if let Some(node) = q.get(id) {
                return mae_kb::activity::activity_score(&node.properties, weights, today);
            }
            return 0.0;
        }
        if let Some(node) = self.kb.primary.get(id) {
            return mae_kb::activity::activity_score(&node.properties, weights, today);
        }
        for kb in self.kb.instances.values() {
            if let Some(node) = kb.get(id) {
                return mae_kb::activity::activity_score(&node.properties, weights, today);
            }
        }
        0.0
    }

    /// Re-import a single file into the KB instance that covers its directory.
    /// Used after saving a file inside `kb_notes_dir` to keep the graph in sync.
    pub fn kb_reimport_file(&mut self, path: &std::path::Path) {
        for (uuid, inst) in self
            .kb
            .registry
            .instances
            .iter()
            .map(|i| (i.uuid.clone(), i.org_dir.clone()))
        {
            if path.starts_with(&inst) {
                let prev_ids = self
                    .kb
                    .watchers
                    .get(&uuid)
                    .and_then(|w| w.ids_for_path(path));
                let ids = match self.kb.instances.get_mut(&uuid) {
                    Some(kb) => kb.ingest_org_file(path),
                    None => return,
                };
                // Retract ids this path no longer produces (e.g. an in-place `:ID:`
                // edit followed by a save) — same class of fix as the watcher path.
                if let Some(prev_ids) = prev_ids {
                    for old_id in prev_ids.iter().filter(|id| !ids.contains(id)) {
                        if let Some(kb) = self.kb.instances.get_mut(&uuid) {
                            kb.remove(old_id);
                        }
                        self.kb_persist_instance_delete(&uuid, old_id);
                    }
                }
                // Phase 0b: persist the reimported nodes to the durable instance
                // store — parity with the watcher drain (0a); otherwise a save-driven
                // reimport is lost on restart.
                self.kb_persist_instance_ids(&uuid, &ids);
                // Keep the watcher's own path->ids map in sync too, so a subsequent
                // watcher-driven event for this same path diffs against the truth
                // rather than a stale pre-save mapping.
                if let Some(w) = self.kb.watchers.get(&uuid) {
                    w.record_ids(path, ids);
                }
                return;
            }
        }
    }

    /// Check if a path is inside a registered KB instance directory.
    pub fn kb_path_in_instance(&self, path: &std::path::Path) -> bool {
        self.kb
            .registry
            .instances
            .iter()
            .any(|i| path.starts_with(&i.org_dir))
    }

    /// Search across local KB and all federated instances.
    /// Returns (instance_name_or_none, node) pairs, deduplicated by node ID.
    /// Local results take priority over federated.
    /// Respects `kb_search_sort` option: "relevance" (default), "activity", "alphabetical".
    pub fn kb_federated_search(&self, query: &str) -> Vec<(Option<String>, mae_kb::Node)> {
        self.kb_federated_search_scoped(query, &mae_kb::KbScope::All)
    }

    /// Search across the primary KB and federated instances, restricted to the
    /// given `scope` (plan decision D4). `KbScope::All` reproduces
    /// `kb_federated_search` exactly. Local results always win on duplicates.
    /// Respects `kb_search_sort` ("relevance" default / "activity" /
    /// "alphabetical" / "recency"). "recency" ranks by relevance first, then
    /// stably re-sorts so session-visited nodes float to the top (most-recent
    /// first; unvisited nodes keep their relevance order below them).
    pub fn kb_federated_search_scoped(
        &self,
        query: &str,
        scope: &mae_kb::KbScope,
    ) -> Vec<(Option<String>, mae_kb::Node)> {
        use mae_kb::KbScope;
        let use_activity = self.kb.search_sort == "activity";
        let use_alpha = self.kb.search_sort == "alphabetical";
        let use_recency = self.kb.search_sort == "recency";
        let weights = mae_kb::activity::ActivityWeights {
            decay: self.kb.activity_decay,
            ..Default::default()
        };
        let today = if use_activity { today_ymd() } else { (0, 0, 0) };

        // Per-instance ranking, shared by primary + federated members.
        let rank = |kb: &mae_kb::KnowledgeBase| -> Vec<String> {
            if use_activity {
                kb.search_sorted_by_activity(query, &weights, today)
            } else if use_alpha {
                kb.search(query)
            } else {
                kb.search_ranked(query, self.kb.search_max_results)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect()
            }
        };

        let mut results: Vec<(Option<String>, mae_kb::Node)> = Vec::new();
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Does the primary participate under this scope? The primary's registry
        // name is matched for the Named case.
        let primary_name = self
            .kb
            .registry
            .instances
            .iter()
            .find(|i| i.primary)
            .map(|i| i.name.as_str());
        let include_primary = match scope {
            KbScope::All | KbScope::LocalOnly => true,
            KbScope::RemoteOnly => false,
            KbScope::Named(n) => primary_name == Some(n.as_str()),
        };

        if include_primary {
            if self.kb.primary_thin() {
                // Thin primary (Phase D): the in-memory mirror is empty; the daemon
                // holds the primary. Rank + fetch owned nodes via the query layer
                // (daemon LRU). Relevance order — the "activity" sort needs in-memory
                // scoring, so it degrades to relevance here (honest, not silent: the
                // daemon-hosted primary has no local activity log).
                if let Some(ql) = self.kb.query_layer() {
                    for hit in ql.search(query, self.kb.search_max_results) {
                        if let Some(node) = ql.get(&hit.id) {
                            if seen_ids.insert(node.id.clone()) {
                                results.push((None, node));
                            }
                        }
                    }
                }
            } else {
                for id in rank(&self.kb.primary) {
                    if let Some(node) = self.kb.primary.get(&id) {
                        if seen_ids.insert(node.id.clone()) {
                            results.push((None, node.clone()));
                        }
                    }
                }
            }
        }

        // Then each federated instance permitted by the scope (skip if seen).
        for (uuid, kb) in &self.kb.instances {
            let inst = self.kb.registry.find_by_uuid(uuid);
            let include = match scope {
                KbScope::All => true,
                KbScope::LocalOnly => false,
                KbScope::RemoteOnly => inst.is_some_and(|i| i.is_remote()),
                KbScope::Named(n) => inst.is_some_and(|i| &i.name == n),
            };
            if !include {
                continue;
            }
            let inst_name = inst.map(|i| i.name.clone());
            for id in rank(kb) {
                if let Some(node) = kb.get(&id) {
                    if seen_ids.insert(node.id.clone()) {
                        results.push((inst_name.clone(), node.clone()));
                    }
                }
            }
        }

        if use_alpha {
            results.sort_by(|a, b| a.1.id.cmp(&b.1.id));
        } else if use_recency {
            // Stable sort by descending visit ordinal: most-recently-visited
            // first; ties (incl. all unvisited at rank 0) keep relevance order.
            results.sort_by(|a, b| {
                self.kb
                    .visit_rank(&b.1.id)
                    .cmp(&self.kb.visit_rank(&a.1.id))
            });
        }

        results
    }

    /// Get a node by ID, searching local first then federated instances.
    pub fn kb_federated_get(&self, id: &str) -> Option<(Option<String>, &mae_kb::Node)> {
        if let Some(node) = self.kb.primary.get(id) {
            return Some((None, node));
        }
        for (uuid, kb) in &self.kb.instances {
            if let Some(node) = kb.get(id) {
                let name = self.kb.registry.find_by_uuid(uuid).map(|i| i.name.clone());
                return Some((name, node));
            }
        }
        None
    }

    /// Phase 1a: consume the background primary-store preload on an idle tick.
    ///
    /// The loader thread (spawned at startup) runs the O(n) `load_all` off the UI
    /// thread — a synchronous load on a large store (thousands of nodes) blocked the
    /// main thread long enough to trip the 10s startup watchdog. Here we drain the
    /// finished node set into the in-memory mirror. No-op until the loader completes;
    /// `Empty` means still loading. Idempotent (clears `pending_preload` when done).
    pub fn drain_kb_preload(&mut self) {
        if self.kb.pending_preload.is_none() {
            return;
        }
        let recv = self.kb.pending_preload.as_ref().map(|rx| rx.try_recv());
        match recv {
            Some(Ok(Ok(nodes))) => {
                let count = nodes.len();
                for node in nodes {
                    self.kb.primary.insert(node);
                }
                self.kb.pending_preload = None;
                if count > 0 {
                    self.set_status(format!("KB loaded: {} nodes", count));
                }
                tracing::debug!(count, "background KB preload complete");
            }
            Some(Ok(Err(e))) => {
                self.kb.pending_preload = None;
                self.set_status(format!("KB load failed: {}", e));
                tracing::warn!(error = %e, "background KB preload failed");
            }
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) => {
                // Still loading — check again next idle tick.
            }
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) | None => {
                self.kb.pending_preload = None;
                tracing::warn!("background KB preload thread disconnected before sending");
            }
        }
    }

    /// Phase 4: cross-instance freshness. When ANOTHER daemon-less process commits to
    /// the shared sqlite primary store, reload our in-memory mirror so search/find/get
    /// reflect it. Called on the idle tick. Reflects external adds + edits (upsert via
    /// the background loader); cross-instance deletes are not reflected until a full
    /// reload/restart. No-op when no store watcher is active (sled / daemon-hosted) or
    /// a preload is already in flight.
    pub fn drain_kb_store_watch(&mut self) {
        // Always drain the events (so ignored own-writes don't accumulate).
        let changed = match &self.kb.store_watcher {
            Some(w) => w.drain_changed(),
            None => return,
        };
        if !changed || self.kb.pending_preload.is_some() {
            return;
        }
        // Suppress reloads caused by our OWN recent writes: their file events are
        // drained above and ignored here, so we don't churn on local edits.
        if let Some(t) = self.kb.last_local_store_write {
            if t.elapsed() < std::time::Duration::from_millis(1500) {
                return;
            }
        }
        let Some(store) = self.kb.primary_cozo.clone() else {
            return;
        };
        // Reload off the UI thread (same path as the startup preload), drained by
        // `drain_kb_preload` on a later idle tick.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = store.load_all().map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        self.kb.pending_preload = Some(rx);
        tracing::debug!("external KB store change — reloading mirror in background");
    }

    /// Cross-process freshness for `kb-registry.toml`: if another `mae`
    /// process registered/unregistered a KB instance, pick it up here so
    /// `kb-find`/`SPC n f` sees it without this process needing to run a
    /// local KB operation first. Called on the idle tick, mirroring
    /// `drain_kb_store_watch` above. Unlike that primary-store watcher, this
    /// reloads synchronously — `kb-registry.toml` is a small TOML file, not
    /// a full KB store, so no background thread is needed.
    pub fn drain_kb_registry_watch(&mut self) {
        // Always drain the events (so ignored own-writes don't accumulate).
        let changed = match &self.kb.registry_watcher {
            Some(w) => w.drain_changed(),
            None => return,
        };
        if !changed {
            return;
        }
        // Suppress reloads caused by our OWN recent writes (KbRegistry::update
        // stamps this on every registry-mutating call in this process).
        if let Some(t) = self.kb.last_local_registry_write {
            if t.elapsed() < std::time::Duration::from_millis(1500) {
                return;
            }
        }
        let Some(data_dir) = self.mae_data_dir() else {
            return;
        };
        let fresh = mae_kb::federation::KbRegistry::load(&data_dir);

        let mut changed_any = false;
        for inst in fresh.instances.clone() {
            // Shared/joined instances (empty org_dir) are adopted via the
            // collab join flow, not by importing an org directory — skip.
            if inst.enabled
                && !inst.org_dir.as_os_str().is_empty()
                && !self.kb.instances.contains_key(&inst.uuid)
            {
                self.kb_adopt_instance(&inst.uuid, &inst.org_dir, Some(&inst.db_path));
                changed_any = true;
                tracing::info!(
                    name = %inst.name, uuid = %inst.uuid,
                    "picked up KB instance registered by another mae process"
                );
            }
        }
        let fresh_uuids: std::collections::HashSet<&str> =
            fresh.instances.iter().map(|i| i.uuid.as_str()).collect();
        let stale: Vec<String> = self
            .kb
            .instances
            .keys()
            .filter(|u| !fresh_uuids.contains(u.as_str()))
            .cloned()
            .collect();
        for uuid in stale {
            self.kb.instances.remove(&uuid);
            self.kb.instance_stores.remove(&uuid);
            self.kb.watchers.remove(&uuid);
            changed_any = true;
        }
        self.kb.registry = fresh;
        if changed_any {
            self.kb.rebuild_query_layer();
        }
    }

    /// Drain KB file watchers — apply changes from filesystem events.
    /// Called from `idle_work()` to pick up org file edits without `:kb-reimport`.
    ///
    /// Hardened with: debounce (skip if too recent), drain cap (max N events),
    /// time-boxing (50ms deadline), error tracking, and enable/disable toggle.
    pub fn drain_kb_watchers(&mut self) {
        // Early return if watchers disabled
        if !self.kb.watcher_enabled {
            return;
        }

        let drain_start = std::time::Instant::now();
        let debounce_dur = std::time::Duration::from_millis(self.kb.watcher_debounce_ms);
        let max_events = self.kb.max_drain_events;
        let deadline = drain_start + std::time::Duration::from_millis(50);

        let uuids: Vec<String> = self.kb.watchers.keys().cloned().collect();
        let mut changed = false;
        let mut total_processed: usize = 0;

        for uuid in uuids {
            // Debounce: skip if last drain was too recent
            if let Some(last) = self.kb.last_drain.get(&uuid) {
                if last.elapsed() < debounce_dur {
                    self.kb.watcher_stats.suppressed_debounce += 1;
                    continue;
                }
            }

            let changes = match self.kb.watchers.get(&uuid) {
                Some(w) => {
                    // Track watcher errors
                    let errs = w.error_count();
                    if errs > self.kb.watcher_stats.errors {
                        self.kb.watcher_stats.errors = errs;
                    }
                    w.drain()
                }
                None => continue,
            };
            if changes.is_empty() {
                continue;
            }

            // Update last drain timestamp
            self.kb
                .last_drain
                .insert(uuid.clone(), std::time::Instant::now());

            let skipped = changes.len().saturating_sub(max_events);
            if skipped > 0 {
                self.kb.watcher_stats.suppressed_timebox += skipped as u64;
            }

            for change in changes.into_iter().take(max_events) {
                // Time-boxing: break if we've exceeded the 50ms budget
                if std::time::Instant::now() > deadline {
                    self.kb.watcher_stats.suppressed_timebox += 1;
                    break;
                }

                match change {
                    mae_kb::watch::OrgChange::Upserted(path) => {
                        // Suppress events for paths MAE is currently writing
                        // (activity tracking, chain-fill) to prevent cascade.
                        if self.kb.write_guard.remove(&path) {
                            self.kb.watcher_stats.events_suppressed += 1;
                            total_processed += 1;
                            continue;
                        }
                        let prev_ids = self
                            .kb
                            .watchers
                            .get(&uuid)
                            .and_then(|w| w.ids_for_path(&path));
                        let ids = match self.kb.instances.get_mut(&uuid) {
                            Some(kb) => kb.ingest_org_file(&path),
                            None => continue,
                        };
                        // Retract ids this path no longer produces (e.g. an in-place
                        // `:ID:` edit) — otherwise the old id lingers as a ghost node
                        // in the index/search forever, since re-ingest only upserts.
                        if let Some(prev_ids) = prev_ids {
                            for old_id in prev_ids.iter().filter(|id| !ids.contains(id)) {
                                if let Some(kb) = self.kb.instances.get_mut(&uuid) {
                                    kb.remove(old_id);
                                }
                                self.kb_persist_instance_delete(&uuid, old_id);
                            }
                        }
                        // Phase 0a: write-through to the durable instance store BEFORE
                        // handing ownership of `ids` to the watcher record. Without this
                        // the watcher-ingested nodes live only in the in-memory mirror
                        // and are lost on restart (same class as the :kb-ingest bug).
                        self.kb_persist_instance_ids(&uuid, &ids);
                        if let Some(w) = self.kb.watchers.get(&uuid) {
                            w.record_ids(path, ids);
                        }
                        self.kb.watcher_stats.events_upserted += 1;
                        changed = true;
                        total_processed += 1;
                    }
                    mae_kb::watch::OrgChange::Removed(ids) => {
                        if self.kb.instances.contains_key(&uuid) {
                            if let Some(kb) = self.kb.instances.get_mut(&uuid) {
                                for id in &ids {
                                    kb.remove(id);
                                }
                            }
                            // Phase 0a: mirror the removals into the durable instance store.
                            for id in &ids {
                                self.kb_persist_instance_delete(&uuid, id);
                            }
                            self.kb.watcher_stats.events_removed += 1;
                            changed = true;
                            total_processed += 1;
                        }
                    }
                }
            }
        }

        // Record timing in both watcher stats and perf stats
        let elapsed_us = drain_start.elapsed().as_micros() as u64;
        self.kb.watcher_stats.last_drain_us = elapsed_us;
        self.kb.watcher_stats.last_drain_event_count = total_processed;
        if total_processed > 0 {
            self.kb.watcher_stats.drain_us_sum += elapsed_us;
            self.kb.watcher_stats.drain_count += 1;
            self.kb.watcher_stats.reimports_total +=
                self.kb.watcher_stats.events_upserted + self.kb.watcher_stats.events_removed;
        }
        self.perf_stats.kb_watcher_drain_us = elapsed_us;
        self.perf_stats.kb_watcher_events += total_processed as u64;

        if changed {
            self.fire_hook("after-kb-change");
        }
    }

    /// Validate links in the current buffer's KB node after save.
    /// Shows a status bar warning if broken links are found.
    /// Advisory only — does NOT block the save.
    pub fn validate_kb_links_on_save(&mut self) {
        let idx = self.active_buffer_idx();
        let buf = &self.buffers[idx];

        // Only validate KB-sourced buffers (have a source_file or daily: prefix)
        let node_id: Option<String> = buf.file_path().and_then(|path| {
            // Find a node whose source_file matches this path
            if let Some(q) = self.kb.query_layer() {
                q.list_ids(None).into_iter().find(|id| {
                    q.get(id)
                        .and_then(|n| n.source_file)
                        .map(|sf| sf.as_path() == path)
                        .unwrap_or(false)
                })
            } else {
                self.kb
                    .primary
                    .all_id_title_pairs()
                    .into_iter()
                    .find_map(|(id, _)| {
                        self.kb.primary.get(&id).and_then(|n| {
                            n.source_file
                                .as_ref()
                                .filter(|sf| sf.as_path() == path)
                                .map(|_| id.clone())
                        })
                    })
            }
        });

        // Also check dailies buffers
        let node_id = node_id.or_else(|| {
            let name = &self.buffers[self.active_buffer_idx()].name;
            if name.starts_with("daily:") {
                Some(name.clone())
            } else {
                None
            }
        });

        if let Some(id) = node_id {
            let missing: Vec<String> = if let Some(q) = self.kb.query_layer() {
                q.links_from(&id)
                    .into_iter()
                    .filter(|l| !q.contains(&l.dst))
                    .map(|l| l.dst)
                    .collect()
            } else {
                let m = self.kb.primary.validate_links(&id);
                // Also check federated instances for the targets
                m.into_iter()
                    .filter(|target| !self.kb.instances.values().any(|kb| kb.contains(target)))
                    .collect()
            };
            if !missing.is_empty() {
                self.set_status(format!(
                    "Warning: {} broken link(s) in this node",
                    missing.len()
                ));
            }
        }
    }

    /// Clean up orphan user notes (no links in or out).
    /// Preserves seed nodes (cmd:, concept:, lesson:, scheme:, option:).
    /// Returns the number of orphans removed.
    pub fn kb_cleanup_orphans(&mut self) -> usize {
        let seed_prefixes = ["cmd:", "concept:", "lesson:", "scheme:", "option:"];
        let orphan_ids: Vec<String> = if let Some(q) = self.kb.query_layer() {
            q.health_report().map(|r| r.orphan_ids).unwrap_or_default()
        } else {
            self.kb.primary.health_report().orphan_ids
        };
        let to_remove: Vec<String> = orphan_ids
            .into_iter()
            .filter(|id| !seed_prefixes.iter().any(|p| id.starts_with(p)))
            .collect();
        let count = to_remove.len();
        for id in &to_remove {
            self.kb.primary.remove(id);
        }
        if count > 0 {
            self.fire_hook("after-kb-change");
        }
        count
    }
}

/// Result of a dailies chain-fill operation.
pub struct ChainFillResult {
    pub stubs_created: Vec<(i32, u32, u32)>,
    pub links_inserted: usize,
    pub anchor_date: Option<(i32, u32, u32)>,
}

/// Current date as YYYY-MM-DD using proper calendar math.
fn today_str() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (y, m, d) = unix_secs_to_date(secs);
    mae_kb::activity::format_date(y, m, d)
}

/// Current date as (year, month, day). Used by dailies, activity sorting.
pub fn today_ymd() -> (i32, u32, u32) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    unix_secs_to_date(secs)
}

/// Convert Unix epoch seconds to (year, month, day).
/// Civil calendar conversion without chrono.
fn unix_secs_to_date(secs: u64) -> (i32, u32, u32) {
    // Algorithm from Howard Hinnant's civil_from_days
    let z = (secs / 86400) as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

/// Simple ISO-8601 timestamp without pulling in chrono.
fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Approximate: good enough for display purposes
    let days = secs / 86400;
    let years = 1970 + days / 365;
    let remainder_days = days % 365;
    let months = remainder_days / 30 + 1;
    let day = remainder_days % 30 + 1;
    format!("{:04}-{:02}-{:02}", years, months, day)
}

impl Editor {
    /// Record an access event for a KB node. Updates `:last-accessed:` in the
    /// source .org file (if any) and in-memory properties.
    pub fn kb_record_access(&mut self, node_id: &str) {
        if !self.kb.activity_tracking {
            return;
        }
        let today = today_str();
        self.kb_update_property_on_disk(node_id, "last-accessed", &today);
    }

    /// Record a modification event. Computes body hash, compares to stored
    /// `:hash:`, and updates `:last-modified:` + `:hash:` if changed.
    pub fn kb_record_modification(&mut self, path: &std::path::Path) {
        if !self.kb.activity_tracking {
            return;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        let new_hash = mae_kb::activity::body_hash(&content);
        // Find the node by source file path
        let node_id = self.kb_find_node_by_path(path).map(|n| n.id.clone());
        let Some(node_id) = node_id else {
            return;
        };
        // Check existing hash
        let old_hash = self
            .kb_find_node_by_path(path)
            .and_then(|n| n.properties.get("hash").cloned());
        if old_hash.as_deref() == Some(&new_hash) {
            return; // Content unchanged
        }
        let today = today_str();
        // Write hash and last-modified to file
        self.kb_update_property_in_file(path, "hash", &new_hash);
        self.kb_update_property_in_file(path, "last-modified", &today);
        // Update in-memory node properties
        if let Some(node) = self.kb_get_node_mut(&node_id) {
            node.properties.insert("hash".to_string(), new_hash);
            node.properties.insert("last-modified".to_string(), today);
        }
    }

    /// Record a link event for a target node. Updates `:last-linked:`.
    pub fn kb_record_link(&mut self, target_id: &str) {
        if !self.kb.activity_tracking {
            return;
        }
        let today = today_str();
        self.kb_update_property_on_disk(target_id, "last-linked", &today);
    }

    /// Update a single property in a node's source .org file on disk.
    /// Uses write-guard to prevent cascade.
    fn kb_update_property_on_disk(&mut self, node_id: &str, key: &str, value: &str) {
        // Find the source file for this node
        let source_path = self.kb_node_source_path(node_id);
        let Some(path) = source_path else {
            return;
        };
        self.kb_update_property_in_file(&path, key, value);
        // Update in-memory node properties
        if let Some(node) = self.kb_get_node_mut(node_id) {
            node.properties.insert(key.to_string(), value.to_string());
        }
    }

    /// Write a property to a .org file and reimport. Uses write-guard.
    fn kb_update_property_in_file(&mut self, path: &std::path::Path, key: &str, value: &str) {
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        let Some(updated) = mae_kb::org::update_property(&content, key, value) else {
            return;
        };
        // Guard the path to prevent watcher cascade
        self.kb.write_guard.insert(path.to_path_buf());
        if std::fs::write(path, &updated).is_ok() {
            // Reimport synchronously to keep in-memory KB in sync
            self.kb_reimport_file(path);
            self.kb.watcher_stats.reimports_total += 1;
        }
    }

    /// Find a node by its source file path (across all KB instances).
    fn kb_find_node_by_path(&self, path: &std::path::Path) -> Option<&mae_kb::Node> {
        for kb in self.kb.instances.values() {
            for id in kb.list_ids(None) {
                if let Some(node) = kb.get(&id) {
                    if node.source_file.as_deref() == Some(path) {
                        return Some(node);
                    }
                }
            }
        }
        None
    }

    /// Get the source file path for a node by ID.
    fn kb_node_source_path(&self, node_id: &str) -> Option<std::path::PathBuf> {
        for kb in self.kb.instances.values() {
            if let Some(node) = kb.get(node_id) {
                return node.source_file.clone();
            }
        }
        None
    }

    /// Get a mutable reference to a node by ID (across all KB instances).
    fn kb_get_node_mut(&mut self, node_id: &str) -> Option<&mut mae_kb::Node> {
        for kb in self.kb.instances.values_mut() {
            if let Some(node) = kb.get_mut(node_id) {
                return Some(node);
            }
        }
        None
    }

    // ── Audit ────────────────────────────────────────────────────────

    /// Show a comprehensive KB audit report in a buffer.
    pub fn show_kb_audit_report(&mut self) {
        let mut lines = Vec::new();
        lines.push("* KB Audit Report".to_string());
        lines.push(String::new());

        // 1. Basic health
        let total_nodes: usize = self.kb.instances.values().map(|kb| kb.len()).sum();
        let total_links: usize = self
            .kb
            .instances
            .values()
            .flat_map(|kb| kb.list_ids(None))
            .filter_map(|id| {
                self.kb
                    .instances
                    .values()
                    .find_map(|kb| kb.get(&id))
                    .map(|n| n.links().len())
            })
            .sum();
        lines.push(format!("** Node count: {}", total_nodes));
        lines.push(format!("** Link count: {}", total_links));
        lines.push(String::new());

        // 2. Stale node detection
        let mut stale_count = 0;
        for kb in self.kb.instances.values() {
            for id in kb.list_ids(None) {
                if let Some(node) = kb.get(&id) {
                    if let Some(ref sf) = node.source_file {
                        if !sf.exists() {
                            stale_count += 1;
                            lines.push(format!("  - STALE: {} (file: {})", id, sf.display()));
                        }
                    }
                }
            }
        }
        if stale_count > 0 {
            lines.insert(
                lines.len() - stale_count,
                format!("** Stale nodes: {}", stale_count),
            );
        } else {
            lines.push("** Stale nodes: 0".to_string());
        }
        lines.push(String::new());

        // 3. Dailies chain validation
        if let Some(dir) = self.kb_dailies_dir() {
            if dir.exists() {
                let mut daily_files: Vec<String> = std::fs::read_dir(&dir)
                    .map(|rd| {
                        rd.filter_map(|e| e.ok())
                            .filter_map(|e| {
                                e.path()
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .map(|s| s.to_string())
                            })
                            .filter(|s| mae_kb::activity::parse_date(s).is_some())
                            .collect()
                    })
                    .unwrap_or_default();
                daily_files.sort();
                let chain_gaps = daily_files
                    .windows(2)
                    .filter(|w| {
                        if let (Some(a), Some(b)) = (
                            mae_kb::activity::parse_date(&w[0]),
                            mae_kb::activity::parse_date(&w[1]),
                        ) {
                            mae_kb::activity::days_between(a, b) > 1
                        } else {
                            false
                        }
                    })
                    .count();
                lines.push(format!(
                    "** Dailies: {} files, {} chain gaps",
                    daily_files.len(),
                    chain_gaps
                ));
            } else {
                lines.push("** Dailies: directory not found".to_string());
            }
        } else {
            lines.push("** Dailies: not configured".to_string());
        }
        lines.push(String::new());

        // 4. Watcher stats
        let stats = &self.kb.watcher_stats;
        lines.push("** Watcher stats".to_string());
        lines.push(format!("   Upserted: {}", stats.events_upserted));
        lines.push(format!("   Removed: {}", stats.events_removed));
        lines.push(format!("   Suppressed: {}", stats.events_suppressed));
        lines.push(format!("   Reimports total: {}", stats.reimports_total));
        lines.push(format!("   Errors: {}", stats.errors));

        let content = lines.join("\n");
        let mut buf = crate::buffer::Buffer::new();
        buf.name = "*KB Audit*".to_string();
        buf.replace_contents(&content);
        buf.modified = false;
        buf.read_only = true;

        let buf_idx = self.buffers.len();
        self.buffers.push(buf);
        self.display_buffer(buf_idx);
    }

    // ── Dailies ─────────────────────────────────────────────────────

    /// Resolve the dailies directory. Explicit setting takes priority;
    /// falls back to `kb_notes_dir/daily`.
    pub fn kb_dailies_dir(&self) -> Option<std::path::PathBuf> {
        if let Some(ref dir) = self.kb.dailies_dir {
            return Some(dir.clone());
        }
        self.kb.notes_dir.as_ref().map(|d| d.join("daily"))
    }

    /// Path for a daily note file: `dailies_dir/YYYY-MM-DD.org`.
    fn kb_daily_path(&self, y: i32, m: u32, d: u32) -> Option<std::path::PathBuf> {
        self.kb_dailies_dir()
            .map(|dir| dir.join(format!("{}.org", mae_kb::activity::format_date(y, m, d))))
    }

    /// Canonical ID for a daily note.
    fn kb_daily_id(y: i32, m: u32, d: u32) -> String {
        format!("daily:{}", mae_kb::activity::format_date(y, m, d))
    }

    /// Check if a daily file exists on disk.
    fn kb_daily_exists(&self, y: i32, m: u32, d: u32) -> bool {
        self.kb_daily_path(y, m, d)
            .map(|p| p.exists())
            .unwrap_or(false)
    }

    /// Create a daily .org file stub with PROPERTIES drawer + title.
    /// Does NOT insert Previous: link (chain_fill does that).
    fn kb_create_daily_stub(
        &mut self,
        y: i32,
        m: u32,
        d: u32,
    ) -> Result<std::path::PathBuf, String> {
        let dir = self
            .kb_dailies_dir()
            .ok_or("No dailies directory configured")?;
        if !dir.exists() {
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("Failed to create dailies dir: {}", e))?;
        }
        let path = dir.join(format!("{}.org", mae_kb::activity::format_date(y, m, d)));
        if path.exists() {
            return Ok(path);
        }
        let id = Self::kb_daily_id(y, m, d);
        let date_str = mae_kb::activity::format_date(y, m, d);
        let content = format!(
            ":PROPERTIES:\n:ID: {}\n:END:\n#+title: {}\n\n",
            id, date_str
        );
        std::fs::write(&path, &content).map_err(|e| format!("Failed to write daily: {}", e))?;
        // Guard and reimport
        self.kb.write_guard.insert(path.clone());
        self.kb_reimport_file(&path);
        self.kb.watcher_stats.reimports_total += 1;
        Ok(path)
    }

    /// Find the nearest existing daily before/after a date.
    /// `direction`: -1 = backward, 1 = forward.
    fn kb_daily_find_nearest(
        &self,
        y: i32,
        m: u32,
        d: u32,
        direction: i32,
    ) -> Option<(i32, u32, u32)> {
        let max_search = self.kb.daily_chain_gap_max;
        let step = if direction < 0 {
            mae_kb::activity::prev_day
        } else {
            mae_kb::activity::next_day
        };
        let mut cur = step(y, m, d);
        for _ in 0..max_search {
            if self.kb_daily_exists(cur.0, cur.1, cur.2) {
                return Some(cur);
            }
            cur = step(cur.0, cur.1, cur.2);
        }
        None
    }

    /// Chain-fill: ensure target date's daily exists and is linked back to
    /// the most recent pre-existing daily. Creates stub files for gaps.
    pub fn kb_daily_chain_fill(
        &mut self,
        y: i32,
        m: u32,
        d: u32,
    ) -> Result<ChainFillResult, String> {
        let mut result = ChainFillResult {
            stubs_created: Vec::new(),
            links_inserted: 0,
            anchor_date: None,
        };

        // Ensure target date exists
        let target_path = self.kb_create_daily_stub(y, m, d)?;
        let _ = target_path; // used implicitly via reimport

        // Walk backwards to find the anchor (pre-existing daily)
        let max_gap = self.kb.daily_chain_gap_max;
        let mut cur = (y, m, d);
        let mut chain: Vec<(i32, u32, u32)> = vec![cur];

        for _ in 0..max_gap {
            let prev = mae_kb::activity::prev_day(cur.0, cur.1, cur.2);
            if self.kb_daily_exists(prev.0, prev.1, prev.2) {
                // This is a pre-existing daily — it's our anchor
                result.anchor_date = Some(prev);
                chain.push(prev);
                break;
            }
            // Create stub for the gap day
            self.kb_create_daily_stub(prev.0, prev.1, prev.2)?;
            result.stubs_created.push(prev);
            chain.push(prev);
            cur = prev;
        }

        // Now insert "Previous:" links from newest → oldest
        // chain is [target, ..., anchor] so we link chain[i] → chain[i+1]
        for i in 0..chain.len().saturating_sub(1) {
            let (cy, cm, cd) = chain[i];
            let (py, pm, pd) = chain[i + 1];
            let prev_id = Self::kb_daily_id(py, pm, pd);
            let prev_date_str = mae_kb::activity::format_date(py, pm, pd);
            let link_line = format!("Previous: [[id:{}][{}]]", prev_id, prev_date_str);

            // Insert "Previous:" link on chain[i] pointing to chain[i+1]
            if let Some(path) = self.kb_daily_path(cy, cm, cd) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if !content.contains("Previous:") {
                        let mut lines: Vec<&str> = content.lines().collect();
                        let insert_pos = lines
                            .iter()
                            .position(|l| l.starts_with("#+title:"))
                            .map(|i| i + 1)
                            .unwrap_or(lines.len());
                        lines.insert(insert_pos, &link_line);
                        let updated = lines.join("\n") + "\n";
                        self.kb.write_guard.insert(path.clone());
                        if std::fs::write(&path, &updated).is_ok() {
                            self.kb_reimport_file(&path);
                            self.kb.watcher_stats.reimports_total += 1;
                            result.links_inserted += 1;
                        }
                    }
                }
            }

            // Insert symmetric "Next:" link on chain[i+1] pointing to chain[i]
            let next_id = Self::kb_daily_id(cy, cm, cd);
            let next_date_str = mae_kb::activity::format_date(cy, cm, cd);
            let next_link_line = format!("Next: [[id:{}][{}]]", next_id, next_date_str);

            if let Some(prev_path) = self.kb_daily_path(py, pm, pd) {
                if let Ok(content) = std::fs::read_to_string(&prev_path) {
                    if !content.contains("Next:") {
                        let mut lines: Vec<&str> = content.lines().collect();
                        let insert_pos = lines
                            .iter()
                            .position(|l| l.starts_with("#+title:"))
                            .map(|i| i + 1)
                            .unwrap_or(lines.len());
                        lines.insert(insert_pos, &next_link_line);
                        let updated = lines.join("\n") + "\n";
                        self.kb.write_guard.insert(prev_path.clone());
                        if std::fs::write(&prev_path, &updated).is_ok() {
                            self.kb_reimport_file(&prev_path);
                            self.kb.watcher_stats.reimports_total += 1;
                            result.links_inserted += 1;
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    /// Open today's daily with chain-fill.
    pub fn kb_goto_daily_today(&mut self) -> Result<(), String> {
        let (y, m, d) = today_ymd();
        self.kb_daily_chain_fill(y, m, d)?;
        let path = self.kb_daily_path(y, m, d).ok_or("No dailies directory")?;
        self.open_file_at_path(&path);
        Ok(())
    }

    /// Open yesterday's daily.
    pub fn kb_goto_daily_yesterday(&mut self) -> Result<(), String> {
        let (y, m, d) = today_ymd();
        let (py, pm, pd) = mae_kb::activity::prev_day(y, m, d);
        if !self.kb_daily_exists(py, pm, pd) {
            self.kb_create_daily_stub(py, pm, pd)?;
        }
        let path = self
            .kb_daily_path(py, pm, pd)
            .ok_or("No dailies directory")?;
        self.open_file_at_path(&path);
        Ok(())
    }

    /// Navigate to previous daily from current buffer's date.
    pub fn kb_daily_prev(&mut self) -> Result<(), String> {
        let (y, m, d) = self.kb_daily_date_from_buffer()?;
        let (py, pm, pd) = self
            .kb_daily_find_nearest(y, m, d, -1)
            .ok_or("No previous daily found")?;
        let path = self
            .kb_daily_path(py, pm, pd)
            .ok_or("No dailies directory")?;
        self.open_file_at_path(&path);
        Ok(())
    }

    /// Navigate to next daily from current buffer's date.
    pub fn kb_daily_next(&mut self) -> Result<(), String> {
        let (y, m, d) = self.kb_daily_date_from_buffer()?;
        let (ny, nm, nd) = self
            .kb_daily_find_nearest(y, m, d, 1)
            .ok_or("No next daily found")?;
        let path = self
            .kb_daily_path(ny, nm, nd)
            .ok_or("No dailies directory")?;
        self.open_file_at_path(&path);
        Ok(())
    }

    /// Open a daily for a specific date string (YYYY-MM-DD).
    pub fn kb_goto_daily_date(&mut self, date_str: &str) -> Result<(), String> {
        let (y, m, d) = mae_kb::activity::parse_date(date_str)
            .ok_or_else(|| format!("Invalid date: '{}' (expected YYYY-MM-DD)", date_str))?;
        self.kb_daily_chain_fill(y, m, d)?;
        let path = self.kb_daily_path(y, m, d).ok_or("No dailies directory")?;
        self.open_file_at_path(&path);
        Ok(())
    }

    /// Extract a date from the current buffer's filename or title.
    fn kb_daily_date_from_buffer(&self) -> Result<(i32, u32, u32), String> {
        let buf = &self.buffers[self.active_buffer_idx()];
        // Try filename: YYYY-MM-DD.org
        if let Some(fp) = buf.file_path() {
            if let Some(stem) = fp.file_stem().and_then(|s| s.to_str()) {
                if let Some(date) = mae_kb::activity::parse_date(stem) {
                    return Ok(date);
                }
            }
        }
        // Try title line: #+title: YYYY-MM-DD
        let content = buf.text();
        for line in content.lines().take(10) {
            if let Some(rest) = line.strip_prefix("#+title:") {
                let trimmed = rest.trim();
                if let Some(date) = mae_kb::activity::parse_date(trimmed) {
                    return Ok(date);
                }
            }
        }
        Err("Current buffer is not a daily note".to_string())
    }

    /// Open a file at a given path (helper for dailies navigation).
    pub(crate) fn open_file_at_path(&mut self, path: &std::path::Path) {
        // Check if buffer already open
        for (i, buf) in self.buffers.iter().enumerate() {
            if buf.file_path().map(|p| p == path).unwrap_or(false) {
                self.display_buffer(i);
                return;
            }
        }
        // Open new buffer
        match crate::buffer::Buffer::from_file(path) {
            Ok(mut buf) => {
                buf.name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("daily")
                    .to_string();
                self.buffers.push(buf);
                let idx = self.buffers.len() - 1;

                // Language detection (same as open_file_hidden in file_ops.rs)
                let detected_lang = self.buffers[idx]
                    .file_path()
                    .and_then(|p| crate::syntax::language_for_buffer(p, &self.buffers[idx].text()));
                if let Some(lang) = detected_lang {
                    self.syntax.set_language(idx, lang);
                    self.buffers[idx]
                        .local_options
                        .apply_defaults(&lang.default_local_options());
                }

                self.display_buffer(idx);
            }
            Err(e) => {
                self.set_status(format!("Failed to open daily: {}", e));
            }
        }
    }

    // --- Graph KB dispatch helpers (CozoDB backend) ---

    /// Show text content in a read-only scratch buffer.
    fn show_scratch_buffer(&mut self, name: &str, content: &str) {
        let mut buf = crate::buffer::Buffer::new();
        buf.name = name.to_string();
        buf.replace_contents(content);
        buf.modified = false;
        buf.read_only = true;
        let buf_idx = self.buffers.len();
        self.buffers.push(buf);
        self.display_buffer(buf_idx);
    }

    /// Dispatch `:kb-agenda` with a filter string.
    pub fn dispatch_kb_agenda(&mut self, input: &str) {
        use mae_kb::AgendaFilter;

        let parts: Vec<&str> = input.splitn(2, ' ').collect();
        let filter = match parts[0] {
            "todo" => AgendaFilter::Todo(parts.get(1).map(|s| s.trim().to_string())),
            "priority" => {
                let ch = parts
                    .get(1)
                    .and_then(|s| s.trim().chars().next())
                    .unwrap_or('A');
                AgendaFilter::Priority(ch)
            }
            "tag" => match parts.get(1) {
                Some(t) => AgendaFilter::Tag(t.trim().to_string()),
                None => {
                    self.set_status("Usage: :kb-agenda tag <TAG>");
                    return;
                }
            },
            "orphan" => AgendaFilter::Orphan,
            "dead-end" | "deadend" => AgendaFilter::DeadEnd,
            "stale" => {
                let days = parts
                    .get(1)
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(30);
                AgendaFilter::Stale(days)
            }
            "custom" => match parts.get(1) {
                Some(q) => AgendaFilter::Custom(q.trim().to_string()),
                None => {
                    self.set_status("Usage: :kb-agenda custom <datalog-query>");
                    return;
                }
            },
            other => {
                self.set_status(format!(
                    "Unknown filter '{}'. Use: todo, priority, tag, orphan, dead-end, stale, custom",
                    other
                ));
                return;
            }
        };

        // Phase 3: route the agenda through the query layer so it resolves uniformly
        // in BOTH modes (daemon-less → local cozo store; daemon-hosted → daemon read
        // layer, closing part of the #118 thin-client gap). Fall back to the primary
        // store directly if no query layer is built yet.
        let nodes = if let Some(q) = self.kb.query_layer() {
            q.agenda(&filter)
        } else if let Some(ref store) = self.kb.store {
            store.agenda_query(&filter).unwrap_or_default()
        } else {
            self.set_status("No persistent KB store (CozoDB required)");
            return;
        };

        let mut lines = Vec::new();
        lines.push(format!("KB Agenda: {} results", nodes.len()));
        lines.push("=".repeat(40));
        lines.push(String::new());
        for node in &nodes {
            let todo = match &node.todo_state {
                Some(s) if !s.is_empty() => format!(" [{}]", s),
                _ => String::new(),
            };
            let prio = match node.priority {
                Some(c) => format!(" #{}", c),
                None => String::new(),
            };
            lines.push(format!("  {}{}{} — {}", node.id, todo, prio, node.title));
        }
        if nodes.is_empty() {
            lines.push("  (no matching nodes)".to_string());
        }
        self.show_scratch_buffer("*KB Agenda*", &lines.join("\n"));
    }

    /// Dispatch `:kb-history <node-id>`.
    pub fn dispatch_kb_history(&mut self, id: &str) {
        // Phase 3: route history through the query layer (uniform in both modes),
        // falling back to the primary store directly if no query layer is built.
        let versions = if let Some(q) = self.kb.query_layer() {
            q.history(id, 50)
        } else if let Some(ref store) = self.kb.store {
            store.node_history(id, 50).unwrap_or_default()
        } else {
            self.set_status("No persistent KB store (CozoDB required)");
            return;
        };

        let mut lines = Vec::new();
        lines.push(format!(
            "Version History: {} ({} versions)",
            id,
            versions.len()
        ));
        lines.push("=".repeat(50));
        lines.push(String::new());
        for v in &versions {
            let ts = if v.created_at > 0 {
                format!(" @{}", v.created_at)
            } else {
                String::new()
            };
            lines.push(format!(
                "  v{}: {} [{}]{} — {}",
                v.version, v.title, v.author, ts, v.change_summary
            ));
        }
        if versions.is_empty() {
            lines.push("  (no version history)".to_string());
        }
        self.show_scratch_buffer("*KB History*", &lines.join("\n"));
    }

    /// Dispatch `:kb-restore <node-id> <version>`.
    pub fn dispatch_kb_restore(&mut self, id: &str, version: i64) {
        let store = match &self.kb.store {
            Some(s) => s.clone(),
            None => {
                self.set_status("No persistent KB store (CozoDB required)");
                return;
            }
        };

        match store.restore_version(id, version) {
            Ok(()) => {
                self.set_status(format!("Restored '{}' to version {}", id, version));
            }
            Err(e) => {
                self.set_status(format!("Restore failed: {}", e));
            }
        }
    }

    /// Dispatch `:kb-raw-query <datalog>`.
    pub fn dispatch_kb_raw_query(&mut self, query: &str) {
        let store = match &self.kb.store {
            Some(s) => s.clone(),
            None => {
                self.set_status("No persistent KB store (CozoDB required)");
                return;
            }
        };

        match store.raw_query(query) {
            Ok((headers, rows)) => {
                let mut lines = Vec::new();
                lines.push(format!("Datalog Query Results ({} rows)", rows.len()));
                lines.push("=".repeat(50));
                lines.push(String::new());

                if !headers.is_empty() {
                    lines.push(format!("  {}", headers.join(" | ")));
                    lines.push(format!("  {}", "-".repeat(headers.len() * 15)));
                }

                for row in &rows {
                    lines.push(format!("  {}", row.join(" | ")));
                }
                if rows.is_empty() {
                    lines.push("  (no results)".to_string());
                }
                self.show_scratch_buffer("*KB Query*", &lines.join("\n"));
            }
            Err(e) => {
                self.set_status(format!("Query failed: {}", e));
            }
        }
    }

    // --- Meta-node narrow/widen editing (Phase 7) ---

    /// Narrow to a meta-node component for editing.
    ///
    /// If the current help buffer shows a meta-node, presents its members
    /// for selection. On selection, opens the member node's body in a
    /// new buffer for editing.
    pub fn kb_narrow_meta(&mut self) {
        // Get current KB view's node ID.
        let node_id = match self.buffers[self.active_buffer_idx()].kb_view() {
            Some(hv) => hv.current.clone(),
            None => {
                self.set_status("kb-narrow: not in a KB view");
                return;
            }
        };

        // Query meta-node members from the store.
        let members = if let Some(ref store) = self.kb.store {
            match store.meta_members(&node_id) {
                Ok(m) if !m.is_empty() => m,
                Ok(_) => {
                    self.set_status(format!("'{}' has no meta-members", node_id));
                    return;
                }
                Err(e) => {
                    self.set_status(format!("kb-narrow: {}", e));
                    return;
                }
            }
        } else {
            self.set_status("kb-narrow: no KB store available");
            return;
        };

        // Build completion list from members.
        let items: Vec<(String, String)> = members
            .iter()
            .map(|m| {
                let title = if let Some(q) = self.kb.query_layer() {
                    q.get(&m.member_id).map(|n| n.title)
                } else {
                    self.kb.primary.get(&m.member_id).map(|n| n.title.clone())
                }
                .unwrap_or_else(|| m.member_id.clone());
                (m.member_id.clone(), format!("{} ({})", title, m.role))
            })
            .collect();

        // For simplicity, if there's only one member, open it directly.
        // Otherwise, show first member (full completion UI deferred).
        let member_id = &items[0].0;
        self.kb_open_member_for_editing(&node_id, member_id);
    }

    /// Open a meta-node member for editing in a new buffer.
    ///
    /// Buffer name encodes both IDs: `*kb-narrow:META_ID:MEMBER_ID*`
    fn kb_open_member_for_editing(&mut self, meta_id: &str, member_id: &str) {
        let node = if let Some(q) = self.kb.query_layer() {
            q.get(member_id)
        } else {
            self.kb.primary.get(member_id).cloned()
        };
        let node = match node {
            Some(n) => n,
            None => {
                self.set_status(format!("Node '{}' not found", member_id));
                return;
            }
        };

        // Create an edit buffer with the node's body.
        let buf_name = format!("*kb-narrow:{}:{}*", meta_id, member_id);
        let mut buf = crate::Buffer::new();
        buf.name = buf_name;
        buf.insert_text_at(0, &node.body);
        buf.modified = false;

        self.buffers.push(buf);
        let idx = self.buffers.len() - 1;
        self.display_buffer(idx);
        self.set_status(format!(
            "Narrowed to '{}' — :kb-widen to save and return",
            member_id
        ));
    }

    /// Parse meta_id and member_id from a `*kb-narrow:META:MEMBER*` buffer name.
    fn parse_narrow_buffer_name(name: &str) -> Option<(String, String)> {
        let inner = name.strip_prefix("*kb-narrow:")?.strip_suffix('*')?;
        let colon = inner.find(':')?;
        let meta_id = &inner[..colon];
        let member_id = &inner[colon + 1..];
        if meta_id.is_empty() || member_id.is_empty() {
            return None;
        }
        Some((meta_id.to_string(), member_id.to_string()))
    }

    /// Save edits from a narrowed meta-node component and widen back.
    pub fn kb_widen_meta(&mut self) {
        let idx = self.active_buffer_idx();
        let buf_name = self.buffers[idx].name.clone();

        // Check if this is a narrowed KB buffer.
        let (meta_id, member_id) = match Self::parse_narrow_buffer_name(&buf_name) {
            Some(ids) => ids,
            None => {
                self.set_status("kb-widen: not in a narrowed KB buffer");
                return;
            }
        };

        // Extract edited content.
        let new_body = self.buffers[idx].text().to_string();

        // Update the node in the primary KB.
        if let Some(node) = self.kb.primary.get_mut(&member_id) {
            node.body.clone_from(&new_body);
        }

        // Update in the CozoDB store if available.
        if let Some(ref store) = self.kb.store {
            if let Some(node) = self.kb.primary.get(&member_id) {
                let _ = store.save_all(&[node]);
            }
            // Recompose the meta-node body.
            if let Ok(composed) = store.compose_meta_body(&meta_id) {
                if let Some(meta_node) = self.kb.primary.get_mut(&meta_id) {
                    meta_node.body = composed;
                }
            }
        }

        // Close the narrow buffer and return.
        self.buffers.remove(idx);
        for win in self.window_mgr.iter_windows_mut() {
            if win.buffer_idx >= idx {
                win.buffer_idx = win.buffer_idx.saturating_sub(1);
            }
        }
        let ret = idx.min(self.buffers.len().saturating_sub(1));
        self.display_buffer(ret);
        self.set_status(format!("Widened from '{}', changes saved", member_id));
    }
}

#[cfg(test)]
#[path = "kb_ops_tests.rs"]
mod tests;
