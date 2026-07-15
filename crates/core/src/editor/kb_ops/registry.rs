//! KB instance registry: register/unregister/reimport, instance store
//! adoption, and instance-persistence plumbing.

use super::*;

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
    pub(super) fn mae_config_dir(&self) -> Option<PathBuf> {
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
    pub(super) fn kb_adopt_instance(
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
    pub(super) fn kb_persist_node(&self, node: &mae_kb::Node) {
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
    pub(super) fn kb_persist_instance_ids(&mut self, uuid: &str, ids: &[String]) -> usize {
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
    pub(super) fn kb_persist_instance_delete(&self, uuid: &str, id: &str) {
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

    /// Same resolution as `kb_owner_of`, but honors `kb.search_scope` when
    /// it names a specific registered instance (set via
    /// `:kb-set-search-scope` / `(set-option! "kb_search_scope" ...)`):
    /// if that instance ALSO contains `id`, it wins over the default
    /// primary-first order. This is what lets the graph view (or anything
    /// else resolving a generic id like "index") target a specific
    /// registered KB's own root once the user has scoped to it, instead of
    /// `kb_owner_of` always finding primary's node of the same id first.
    ///
    /// Falls through to plain `kb_owner_of` (byte-identical result) when
    /// the scope is a keyword (`"all"`/`"local"`/`"remote"`, including the
    /// default empty/"all") or when the named instance doesn't actually
    /// contain `id` — this is deliberately a narrowing preference, never a
    /// way to make a resolvable id become unresolvable.
    pub(crate) fn kb_owner_of_scoped(&self, id: &str) -> Option<Option<String>> {
        let scope = self.kb.search_scope.trim();
        let is_keyword = matches!(
            scope.to_ascii_lowercase().as_str(),
            "" | "all" | "local" | "local-only" | "remote" | "remote-only"
        );
        if !is_keyword {
            if let Some(entry) = self.kb.registry.find(scope) {
                if let Some(kb) = self.kb.instances.get(&entry.uuid) {
                    if kb.contains(id) {
                        return Some(Some(entry.uuid.clone()));
                    }
                }
            }
        }
        self.kb_owner_of(id)
    }

    /// Look up a KB node by id, checking the query layer first (when
    /// present) and falling through to the in-memory KB
    /// (`kb.primary`/`kb.instances`) when the query layer misses.
    ///
    /// The query layer (when CozoDB-backed) is a deterministic PROJECTION
    /// of the in-memory/CRDT truth (ADR-029), not the truth itself, and
    /// can legitimately lag behind it. A miss there must never
    /// short-circuit to "doesn't exist" when the in-memory KB — always
    /// current — might still have it; `kb_owner_of` already resolves
    /// existence this way (in-memory-first, no query layer involved at
    /// all). This is the single source of truth for "does this KB contain
    /// X, and if so what is it" that every other call site — including
    /// `crates/ai`'s `help_open` tool implementation, a separate crate —
    /// should build on, rather than each reimplementing the same
    /// query-layer-then-in-memory fallback order independently (which is
    /// exactly how this bug reproduced three times: `kb_contains_any`/
    /// `kb_resolve_title` in this same crate, and a third, divergent copy
    /// in `mae-ai`, each had the fallback missing).
    pub fn kb_get_node_anywhere(&self, id: &str) -> Option<mae_kb::Node> {
        if let Some(q) = self.kb.query_layer() {
            if let Some(n) = q.get(id) {
                return Some(n);
            }
        }
        if let Some(n) = self.kb.primary.get(id) {
            return Some(n.clone());
        }
        self.kb
            .instances
            .values()
            .find_map(|kb| kb.get(id).cloned())
    }
}

#[cfg(test)]
mod scoped_owner_tests {
    use crate::editor::Editor;

    fn editor_with_a_registered_instance_sharing_an_id_with_primary() -> Editor {
        let mut editor = Editor::new();
        editor.kb.primary.insert(mae_kb::Node::new(
            "index",
            "Primary Index",
            mae_kb::NodeKind::Index,
            "primary body",
        ));
        let mut inst = mae_kb::KnowledgeBase::new();
        inst.insert(mae_kb::Node::new(
            "index",
            "Notes Index",
            mae_kb::NodeKind::Index,
            "instance body",
        ));
        editor.kb.instances.insert("uuid-notes".into(), inst);
        editor
            .kb
            .registry
            .instances
            .push(mae_kb::federation::KbInstance {
                uuid: "uuid-notes".into(),
                name: "notes".into(),
                org_dir: std::path::PathBuf::from("/tmp/notes"),
                db_path: std::path::PathBuf::from("/tmp/notes.db"),
                primary: false,
                enabled: true,
                last_import: None,
                collab_id: None,
                shared: false,
                remote_peers: Vec::new(),
                last_sync: None,
                ai_residency: mae_kb::federation::AiResidency::default(),
            });
        editor
    }

    #[test]
    fn kb_owner_of_scoped_prefers_the_named_instance_over_primary_when_both_contain_the_id() {
        let mut editor = editor_with_a_registered_instance_sharing_an_id_with_primary();
        // Default scope ("all") behaves exactly like the unscoped lookup —
        // primary wins, since kb_owner_of always checks primary first.
        assert_eq!(editor.kb_owner_of_scoped("index"), Some(None));

        editor.kb.search_scope = "notes".to_string();
        assert_eq!(
            editor.kb_owner_of_scoped("index"),
            Some(Some("uuid-notes".to_string())),
            "scoping to a named instance that also has this id must prefer it over primary"
        );
    }

    #[test]
    fn kb_owner_of_scoped_falls_back_to_unscoped_when_the_named_instance_lacks_the_id() {
        let mut editor = editor_with_a_registered_instance_sharing_an_id_with_primary();
        editor.kb.primary.insert(mae_kb::Node::new(
            "concept:only-in-primary",
            "Only In Primary",
            mae_kb::NodeKind::Concept,
            "",
        ));
        editor.kb.search_scope = "notes".to_string();
        // "notes" doesn't contain this id — must still resolve via the
        // normal primary-first search, not silently fail to resolve.
        assert_eq!(
            editor.kb_owner_of_scoped("concept:only-in-primary"),
            Some(None)
        );
    }

    #[test]
    fn kb_owner_of_scoped_matches_unscoped_for_keyword_scopes() {
        let mut editor = editor_with_a_registered_instance_sharing_an_id_with_primary();
        for scope in ["all", "local", "remote", ""] {
            editor.kb.search_scope = scope.to_string();
            assert_eq!(
                editor.kb_owner_of_scoped("index"),
                editor.kb_owner_of("index"),
                "keyword scope '{scope}' must behave identically to the unscoped lookup"
            );
        }
    }
}
