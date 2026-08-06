//! KB instance registry: register/unregister/reimport, instance store
//! adoption, and instance-persistence plumbing.

use std::collections::HashSet;

use super::*;

impl Editor {
    /// Above this loaded-node count, kb-find switches from eager all-load +
    /// client-filter to a bounded, query-driven ranked window (lazy at scale).
    /// Sits above the bundled manual (~870) so the default UX is unchanged.
    pub const KB_FIND_LAZY_THRESHOLD: usize = 2000;
    /// Size of the lazy ranked window fetched per query for large KBs.
    pub const KB_FIND_LAZY_LIMIT: usize = 200;

    /// Resolve the MAE data directory (~/.local/share/mae).
    /// Checks `data_dir_override` first (for test isolation).
    ///
    /// @ai-caution: [test-safety] This is the sole resolver for everything
    /// under `~/.local/share/mae` — the KB registry, the project list, collab
    /// collections and content keys. The `$XDG_DATA_HOME`/`$HOME` fallback is
    /// *ambient*, so a test that forgets `data_dir_override` silently reads and
    /// rewrites the contributor's real data: `cargo test -p mae-ai --lib` was
    /// observed overwriting `kb-registry.toml` (their registered KB list),
    /// `projects.toml`, and writing real `transcripts/`, while a full workspace
    /// run also wrote a `collab/content_keys/*.key`.
    ///
    /// The override existed and worked — it was just opt-in, and forgetting it
    /// failed *silently toward the real directory*. Under the effect sandbox
    /// the fallback is refused instead, so forgetting now fails toward `None`,
    /// which every caller already handles (`let Some(dir) = … else { return }`).
    /// See [`crate::effect_sandbox`].
    pub fn mae_data_dir(&self) -> Option<PathBuf> {
        if let Some(ref dir) = self.data_dir_override {
            return Some(dir.clone());
        }
        if crate::external_effects_blocked!() {
            return None;
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
        let mut engine = self.kb.storage_engine.clone();

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
                    if msg.contains("inotify")
                        || msg.contains("No space left")
                        || msg.contains("Too many open files")
                    {
                        // Deliberately does NOT tell the user to raise
                        // fs.inotify.max_user_instances/max_user_watches: MAE
                        // now spends ONE instance per process no matter how
                        // many KBs are registered (see mae_kb::watch's
                        // @ai-caution), so hitting the cap means something
                        // else on the machine is monopolising it — most often
                        // leftover mae processes from a crashed session.
                        // Raising the cap would hide that, and the cap exists
                        // precisely to stop one application starving the rest.
                        self.set_status(
                            "KB watcher failed: the system inotify limit is exhausted. \
                             Check for stray processes holding watches \
                             (`ls -l /proc/*/fd | grep inotify`), \
                             or set `kb_watcher_enabled=false` to run without live updates.",
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

    /// Register a `Project`-kind KB instance scoped to `root` (or the current project root
    /// if `root` is `None`) — ADR-058 Phase B, the always-available explicit path
    /// (`:kb-init-project` / `kb_init_project` MCP tool) that `maybe_suggest_project_kb_provisioning`'s
    /// notification action also invokes. A thin wrapper around `kb_register`, reusing its
    /// full registration/import/adoption logic (principle #8) rather than reimplementing it —
    /// the only addition is deriving a deterministic, project-scoped `org_dir`
    /// (`<project_root>/.mae-kb`) and patching the newly-registered instance's `kind`/
    /// `project_root` fields (Phase A), which `kb_register` predates and doesn't set.
    ///
    /// Idempotent: if a `Project`-kind instance already covers `root`, returns it rather than
    /// erroring or creating a duplicate — this, combined with `org_dir` being deterministically
    /// derived from `root` (so two concurrent callers for the same root compute the identical
    /// path) and `kb_register`'s own org_dir-based dedup running inside `KbRegistry::update`'s
    /// file-lock-serialized critical section, is what makes a race between multiple sessions
    /// provisioning the same project converge to exactly one instance rather than duplicates.
    pub fn kb_init_project(&mut self, root: Option<PathBuf>) -> Result<KbImportResult, String> {
        let root = root
            .or_else(|| self.active_project_root().map(|p| p.to_path_buf()))
            .ok_or_else(|| {
                "No project root detected — pass an explicit path or open a file inside a project"
                    .to_string()
            })?;
        let canonical_root = root
            .canonicalize()
            .map_err(|e| format!("cannot resolve project root {}: {e}", root.display()))?;

        if let Some(existing) = self
            .kb
            .registry
            .instances
            .iter()
            .find(|i| i.matches_project_root(&canonical_root))
        {
            return Ok(KbImportResult {
                name: existing.name.clone(),
                uuid: existing.uuid.clone(),
                report: ImportReport::default(),
                health: ImportHealth::default(),
            });
        }

        let project_name = canonical_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project")
            .to_string();
        let org_dir = canonical_root.join(".mae-kb");
        std::fs::create_dir_all(&org_dir)
            .map_err(|e| format!("failed to create {}: {e}", org_dir.display()))?;

        let result = self.kb_register(&project_name, &org_dir).ok_or_else(|| {
            "kb_register failed — see status line for the specific error".to_string()
        })?;

        let Some(data_dir) = self.mae_data_dir() else {
            return Err("cannot determine data directory".to_string());
        };
        let uuid = result.uuid.clone();
        let (registry, _, saved) = mae_kb::federation::KbRegistry::update(&data_dir, |reg| {
            if let Some(inst) = reg.instances.iter_mut().find(|i| i.uuid == uuid) {
                inst.kind = mae_kb::federation::KbInstanceKind::Project;
                inst.project_root = Some(canonical_root.clone());
            }
        });
        if let Err(e) = saved {
            tracing::warn!(error = %e, "failed to persist KB registry (kind/project_root patch)");
        }
        self.kb.registry = registry;

        Ok(result)
    }

    /// Record a decline of project-KB provisioning for `root` (or the current project root)
    /// — ADR-058 Phase E. Persisted via the same concurrent-safe `KbRegistry::update` every
    /// other registry mutation uses, so it survives a restart and a decline recorded by one
    /// session is visible to another.
    pub fn kb_decline_project_provisioning(&mut self, root: Option<PathBuf>) -> Result<(), String> {
        let root = root
            .or_else(|| self.active_project_root().map(|p| p.to_path_buf()))
            .ok_or_else(|| "No project root detected".to_string())?;
        let canonical_root = root
            .canonicalize()
            .map_err(|e| format!("cannot resolve project root {}: {e}", root.display()))?;
        let Some(data_dir) = self.mae_data_dir() else {
            return Err("cannot determine data directory".to_string());
        };
        let (registry, _, saved) = mae_kb::federation::KbRegistry::update(&data_dir, |reg| {
            reg.decline_project(canonical_root.clone());
        });
        if let Err(e) = saved {
            tracing::warn!(error = %e, "failed to persist declined-project-provisioning marker");
        }
        self.kb.registry = registry;
        Ok(())
    }

    /// The opt-in-by-default provisioning trigger (ADR-058 Phase B). Call from a KB-touching
    /// entry point (wired into `kb_exec::dispatch`, the AI/MCP tool-dispatch chokepoint) to
    /// check whether the current project should be offered its own KB instance, and raise a
    /// deduped, non-blocking notification if so. Cheap no-op in the common case (no
    /// detectable project root, already provisioned, or already declined) — every check here
    /// is an in-memory comparison against already-loaded state, no filesystem/network I/O
    /// beyond the one `canonicalize()` call.
    ///
    /// Never silently auto-creates by default — the notification's "Register" action still
    /// requires an explicit user/agent act — **unless** `kb_auto_register` is explicitly set
    /// (wiring up that previously-dead option, per CLAUDE.md principle #15: a registered,
    /// gettable/settable option with no consumer is drift, not a feature). Even then, a
    /// failure is logged, not surfaced as a nagging notification on every subsequent call.
    pub fn maybe_suggest_project_kb_provisioning(&mut self) {
        let Some(root) = self.active_project_root().map(|p| p.to_path_buf()) else {
            return;
        };
        let Ok(canonical_root) = root.canonicalize() else {
            return;
        };
        if self
            .kb
            .registry
            .instances
            .iter()
            .any(|i| i.matches_project_root(&canonical_root))
        {
            return;
        }
        if self.kb.registry.has_declined_project(&canonical_root) {
            return;
        }

        if self.kb.auto_register {
            if let Err(e) = self.kb_init_project(Some(canonical_root)) {
                tracing::warn!(error = %e, "kb_auto_register: failed to auto-provision project KB");
            }
            return;
        }

        let key = format!("kb-init-project:{}", canonical_root.display());
        self.notify(
            crate::notifications::Notification::action_required(
                "kb",
                "Register a KB for this project?",
            )
            .key(key)
            .body(format!(
                "No knowledge base is registered for {}. MAE can maintain one automatically, \
                 kept separate from your other KBs.",
                canonical_root.display()
            ))
            .action(
                "Register project KB",
                crate::notifications::NotifCommand::Command("kb-init-project".to_string()),
            )
            .action(
                "Don't ask again",
                crate::notifications::NotifCommand::Command(
                    "kb-decline-project-provisioning".to_string(),
                ),
            ),
        );
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
        // #76: pre-ADR-019 KB joins dumped nodes straight into `primary`;
        // the ADR-019 join path (`kb_register_joined_instance`) now creates
        // a proper federated instance instead, but never migrates those old
        // copies OUT of `primary` — they're permanently stranded there. A
        // node id can therefore legitimately exist in both `primary` (the
        // stale stranded copy) and a `kb.instances` entry (the correct,
        // actively-synced copy) at once. Checking `primary` unconditionally
        // first means the stale copy always wins, permanently shadowing the
        // correct one for every future read/write/CRDT-apply that resolves
        // through this function — a live correctness bug, not just cosmetic
        // leftover data. `NodeSource::Federation` is the marker every
        // federation-derived node carries (stamped by
        // `apply_remote_update`/`adopt_remote_node` regardless of which
        // store they were called against, including the old pre-ADR-019
        // path) — a `primary` node with that marker is exactly the stranded
        // shape, so prefer an `instances` match over it when one exists.
        // This doesn't attempt to migrate/remove the stranded copy (that
        // needs attribution to the node's correct originating instance,
        // real design work tracked separately) — it just stops the stale
        // copy from winning once a correct one exists.
        let primary_hit = self.kb.primary.get(id);
        let primary_is_stranded_federation_node = matches!(
            primary_hit.and_then(|n| n.source),
            Some(mae_kb::NodeSource::Federation)
        );
        if primary_hit.is_some() && !primary_is_stranded_federation_node {
            return Some(None);
        }
        if let Some((uuid, _)) = self.kb.instances.iter().find(|(_, kb)| kb.contains(id)) {
            return Some(Some(uuid.clone()));
        }
        // No instance match — fall back to the (possibly stranded) primary
        // copy if that's all there is, rather than resolving to nothing.
        if primary_hit.is_some() {
            return Some(None);
        }
        None
    }

    /// Same resolution as `kb_owner_of`, but honors `kb.search_scope` when
    /// it names a specific registered instance (set via
    /// `:kb-set-scope` / `(set-option! "kb_search_scope" ...)`):
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
        self.kb_resolve_anywhere(id).map(|(node, _)| node)
    }

    /// Split a subgraph's boundary links (`SubgraphResult::boundary_links`)
    /// into "same instance (or unresolvable)" vs "genuinely crosses into a
    /// DIFFERENT registered KB instance" — the multi-KB chord view's (#462)
    /// building block for turning a fringe/boundary stub into a real
    /// cross-diagram edge. `owner_instance` is the KB instance the subgraph
    /// was extracted FROM (`None` = primary, `Some(uuid)` = federated —
    /// matches `GraphView.kb_instance`'s convention), so a link whose
    /// target resolves to that SAME instance is correctly kept as a plain
    /// boundary stub (it's just outside the depth/cap cutoff, not a
    /// cross-KB relationship).
    ///
    /// @ai-caution: [correctness] "Not found anywhere" (`kb_owner_of`
    /// returns `None` — a genuinely dead/unresolvable link) and "found, but
    /// in the SAME instance the subgraph was extracted from" (truncated by
    /// `max_depth`/`node_cap`, not actually missing) are deliberately NOT
    /// distinguished by this split — both land in the plain `SubgraphLink`
    /// bucket returned as `.0`. A future feature that needs to tell a dead
    /// link apart from a same-instance link merely hidden by today's BFS
    /// truncation must NOT assume this two-way split is exhaustive; it
    /// isn't — a third bucket would be needed for that.
    pub(crate) fn partition_boundary_links_by_instance(
        &self,
        owner_instance: Option<&str>,
        boundary_links: Vec<mae_kb::SubgraphLink>,
    ) -> (Vec<mae_kb::SubgraphLink>, Vec<mae_kb::CrossInstanceLink>) {
        let mut same_or_dead = Vec::with_capacity(boundary_links.len());
        let mut cross = Vec::new();
        for link in boundary_links {
            let owner = self.kb_owner_of(&link.target);
            let is_cross_instance = matches!(&owner, Some(o) if o.as_deref() != owner_instance);
            if is_cross_instance {
                cross.push(mae_kb::CrossInstanceLink {
                    source: link.source,
                    target: link.target,
                    rel_type: link.rel_type,
                    weight: link.weight,
                    // Safe to unwrap: `is_cross_instance` only matched the
                    // `Some(o)` arm above.
                    target_instance: owner.unwrap(),
                    // The instance this BATCH of boundary links was
                    // extracted from — see Phase A2's doc comment on
                    // `CrossInstanceLink::source_instance` for why this can
                    // no longer be assumed to always be the seed once this
                    // function is called per-diagram.
                    source_instance: owner_instance.map(str::to_string),
                });
            } else {
                same_or_dead.push(link);
            }
        }
        (same_or_dead, cross)
    }

    /// Phase B1 (#462 full-corpus retrieval): every node id in `instance`'s
    /// own KB (`None` = primary, `Some(uuid)` = a federated instance) that
    /// has at least one outgoing link whose target resolves to a
    /// DIFFERENT registered KB instance — i.e. every node that would ever
    /// produce a `CrossInstanceLink` if this instance's boundary links were
    /// classified via `partition_boundary_links_by_instance`. This is the
    /// "bridge" half of `KnowledgeBase::extract_full_corpus`'s `protected`
    /// set: cutting one of these nodes under a full-corpus node cap would
    /// silently sever the only connection between two rendered diagrams,
    /// which is exactly the failure mode `extract_full_corpus`'s exemption
    /// mechanism exists to prevent.
    ///
    /// Deliberately lives here (`mae-core`), not on `KnowledgeBase` itself
    /// (`shared/kb`) — resolving "which instance owns this link's target"
    /// requires `kb_owner_of` and `self.kb.instances`, neither of which
    /// `mae-kb` has access to (it has no notion of a federation registry at
    /// all). `extract_full_corpus` only knows "here is a protected id set,
    /// exempt it from truncation"; this is where that set gets computed.
    ///
    /// Cost: O(nodes-in-instance × avg-out-degree), each outgoing link
    /// resolved via one `kb_owner_of` call (O(#registered instances) each,
    /// not O(#nodes) — bounded, cheap even for a many-thousand-node
    /// instance). Meant to be called ONCE per instance per
    /// `populate_graph_buffer` call, not once per node-cap truncation.
    pub(crate) fn kb_cross_instance_link_sources(&self, instance: Option<&str>) -> HashSet<String> {
        let kb: &mae_kb::KnowledgeBase = match instance {
            None => &self.kb.primary,
            Some(uuid) => match self.kb.instances.get(uuid) {
                Some(kb) => kb,
                None => return HashSet::new(),
            },
        };
        let mut sources = HashSet::new();
        for (id, node) in kb.iter() {
            for (target, _rel_type, _weight) in node.links_typed() {
                if let Some(owner) = self.kb_owner_of(&target) {
                    if owner.as_deref() != instance {
                        sources.insert(id.clone());
                        break;
                    }
                }
            }
        }
        sources
    }

    /// Like [`Self::kb_get_node_anywhere`], but also reports WHICH tier the
    /// node resolved through (query layer / in-memory primary / a specific
    /// federated instance by uuid). Callers that need more than just the
    /// node — e.g. its links, fetched from that SAME tier — use this instead
    /// of re-deriving the query-layer-then-in-memory fallback order a third
    /// time (see `kb_get_node_anywhere`'s doc comment for the history: this
    /// exact fallback order had already been reimplemented independently
    /// three times before that consolidation; a fourth divergent copy, in
    /// `mae-ai`'s `node_json`/`execute_kb_links_from`/`execute_kb_links_to`,
    /// surfaced later — see the mae-audit remediation pass that added this).
    pub fn kb_resolve_anywhere(&self, id: &str) -> Option<(mae_kb::Node, KbResolution)> {
        if let Some(q) = self.kb.query_layer() {
            if let Some(n) = q.get(id) {
                return Some((n, KbResolution::Query));
            }
        }
        if let Some(n) = self.kb.primary.get(id) {
            return Some((n.clone(), KbResolution::Primary));
        }
        self.kb.instances.iter().find_map(|(uuid, kb)| {
            kb.get(id)
                .map(|n| (n.clone(), KbResolution::Instance(uuid.clone())))
        })
    }
}

/// Which KB tier a [`Editor::kb_resolve_anywhere`] lookup resolved through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KbResolution {
    /// Resolved via the CozoDB query layer (the common case when a daemon or
    /// local query layer is available).
    Query,
    /// Resolved via the in-memory primary `KnowledgeBase` (query-layer miss,
    /// or no query layer configured at all).
    Primary,
    /// Resolved via a federated instance, identified by its uuid.
    Instance(String),
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
                project_root: None,
                kind: mae_kb::federation::KbInstanceKind::default(),
                priority: 0,
                remote_hub: None,
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

#[cfg(test)]
mod partition_boundary_links_by_instance_tests {
    use crate::editor::Editor;
    use mae_kb::{KnowledgeBase, Node, NodeKind, SubgraphLink};

    fn link(source: &str, target: &str) -> SubgraphLink {
        SubgraphLink {
            source: source.to_string(),
            target: target.to_string(),
            rel_type: "references".to_string(),
            weight: 1.0,
        }
    }

    fn register_instance<'a>(
        editor: &'a mut Editor,
        uuid: &str,
        name: &str,
    ) -> &'a mut KnowledgeBase {
        editor
            .kb
            .registry
            .instances
            .push(mae_kb::federation::KbInstance {
                uuid: uuid.to_string(),
                name: name.to_string(),
                org_dir: std::path::PathBuf::from(format!("/tmp/{name}")),
                db_path: std::path::PathBuf::from(format!("/tmp/{name}.db")),
                primary: false,
                enabled: true,
                last_import: None,
                collab_id: None,
                shared: false,
                remote_peers: Vec::new(),
                last_sync: None,
                ai_residency: mae_kb::federation::AiResidency::default(),
                project_root: None,
                kind: mae_kb::federation::KbInstanceKind::default(),
                priority: 0,
                remote_hub: None,
            });
        editor.kb.instances.entry(uuid.to_string()).or_default()
    }

    /// Three registered instances (primary + two siblings, "alpha" and
    /// "beta"), fanning boundary links out to BOTH siblings from a primary
    /// seed — the adversarial N-way case (CLAUDE.md #14: not just a single
    /// cherry-picked sibling).
    fn three_instance_editor() -> Editor {
        let mut editor = Editor::new();
        editor.kb.primary.insert(Node::new(
            "concept:seed",
            "Seed",
            NodeKind::Concept,
            "seed body",
        ));
        editor.kb.primary.insert(Node::new(
            "concept:same-instance-truncated",
            "Truncated",
            NodeKind::Concept,
            "",
        ));
        let alpha = register_instance(&mut editor, "uuid-alpha", "alpha");
        alpha.insert(Node::new(
            "concept:alpha-target",
            "Alpha Target",
            NodeKind::Concept,
            "",
        ));
        let beta = register_instance(&mut editor, "uuid-beta", "beta");
        beta.insert(Node::new(
            "concept:beta-target",
            "Beta Target",
            NodeKind::Concept,
            "",
        ));
        editor
    }

    #[test]
    fn fans_out_to_two_distinct_sibling_instances_correctly_attributed() {
        let editor = three_instance_editor();
        let boundary = vec![
            link("concept:seed", "concept:alpha-target"),
            link("concept:seed", "concept:beta-target"),
        ];
        let (same_or_dead, cross) = editor.partition_boundary_links_by_instance(None, boundary);
        assert!(
            same_or_dead.is_empty(),
            "both links genuinely cross into a different instance"
        );
        assert_eq!(cross.len(), 2);
        let alpha = cross
            .iter()
            .find(|l| l.target == "concept:alpha-target")
            .expect("alpha-target link must survive");
        assert_eq!(alpha.target_instance.as_deref(), Some("uuid-alpha"));
        let beta = cross
            .iter()
            .find(|l| l.target == "concept:beta-target")
            .expect("beta-target link must survive");
        assert_eq!(beta.target_instance.as_deref(), Some("uuid-beta"));
    }

    #[test]
    fn same_instance_truncated_link_stays_in_the_boundary_bucket_unchanged() {
        let editor = three_instance_editor();
        let boundary = vec![link("concept:seed", "concept:same-instance-truncated")];
        let (same_or_dead, cross) =
            editor.partition_boundary_links_by_instance(None, boundary.clone());
        assert!(cross.is_empty());
        assert_eq!(same_or_dead.len(), 1);
        assert_eq!(same_or_dead[0].target, boundary[0].target);
    }

    #[test]
    fn dead_unresolvable_link_stays_in_the_boundary_bucket_not_dropped_not_misclassified() {
        let editor = three_instance_editor();
        let boundary = vec![link("concept:seed", "concept:nowhere")];
        let (same_or_dead, cross) = editor.partition_boundary_links_by_instance(None, boundary);
        assert!(
            cross.is_empty(),
            "an unresolvable target is never cross-instance"
        );
        assert_eq!(
            same_or_dead.len(),
            1,
            "an unresolvable target must not be silently dropped"
        );
    }

    #[test]
    fn federated_source_pointing_back_into_primary_resolves_the_none_direction() {
        // Adversarial (#14): verify the `target_instance: None` direction
        // specifically, not just the `Some(uuid)` direction exercised by
        // the other tests above — a federated instance's own boundary link
        // pointing back into PRIMARY must promote to `CrossInstanceLink`
        // with `target_instance: None`.
        let mut editor = three_instance_editor();
        let alpha = editor.kb.instances.get_mut("uuid-alpha").unwrap();
        alpha.insert(Node::new(
            "concept:alpha-seed",
            "Alpha Seed",
            NodeKind::Concept,
            "",
        ));
        let boundary = vec![link("concept:alpha-seed", "concept:seed")];
        let (same_or_dead, cross) =
            editor.partition_boundary_links_by_instance(Some("uuid-alpha"), boundary);
        assert!(same_or_dead.is_empty());
        assert_eq!(cross.len(), 1);
        assert_eq!(
            cross[0].target_instance, None,
            "a link resolving back to PRIMARY must carry target_instance: None, not be \
             mistaken for unresolvable"
        );
    }

    #[test]
    fn unregistering_the_target_instance_between_detection_and_a_later_render_never_panics() {
        // Simulates the narrow race the plan calls out: detect a
        // cross-instance link while the target instance is still
        // registered, then the instance is unregistered before a
        // hypothetical re-render reads the stale `CrossInstanceLink`.
        // Nothing in this crate dereferences `target_instance` back into
        // the registry without a fallible lookup, so this must simply not
        // panic — a live TOCTOU-shaped scenario, not just a static check.
        let mut editor = three_instance_editor();
        let boundary = vec![link("concept:seed", "concept:alpha-target")];
        let (_, cross) = editor.partition_boundary_links_by_instance(None, boundary);
        assert_eq!(cross.len(), 1);
        let stale_uuid = cross[0].target_instance.clone();

        // Unregister "alpha" entirely.
        editor.kb.instances.remove("uuid-alpha");
        editor
            .kb
            .registry
            .instances
            .retain(|i| i.uuid != "uuid-alpha");

        // A later lookup against the now-stale uuid must resolve to
        // nothing, not panic.
        assert!(editor
            .kb
            .registry
            .find(stale_uuid.as_deref().unwrap())
            .is_none());
        assert!(!editor
            .kb
            .instances
            .contains_key(stale_uuid.as_deref().unwrap()));
    }

    // --- kb_cross_instance_link_sources (Phase B1, #462 full-corpus retrieval) ---

    #[test]
    fn kb_cross_instance_link_sources_finds_a_node_with_a_real_cross_instance_link() {
        let mut editor = three_instance_editor();
        // Re-upsert "concept:seed" (primary) with a real link into alpha —
        // KnowledgeBase::insert overwrites in place, rebuilding indexes.
        editor.kb.primary.insert(Node::new(
            "concept:seed",
            "Seed",
            NodeKind::Concept,
            "see [[concept:alpha-target]]",
        ));
        let sources = editor.kb_cross_instance_link_sources(None);
        assert!(
            sources.contains("concept:seed"),
            "a node with a real cross-instance link must be in the protected set: {sources:?}"
        );
    }

    #[test]
    fn kb_cross_instance_link_sources_excludes_a_node_with_only_same_instance_links() {
        let mut editor = three_instance_editor();
        editor.kb.primary.insert(Node::new(
            "concept:seed",
            "Seed",
            NodeKind::Concept,
            "see [[concept:same-instance-truncated]]",
        ));
        let sources = editor.kb_cross_instance_link_sources(None);
        assert!(
            !sources.contains("concept:seed"),
            "a node whose only links stay within the SAME instance must not be \
             misclassified as a cross-instance bridge: {sources:?}"
        );
    }

    #[test]
    fn kb_cross_instance_link_sources_works_for_a_federated_instance_too_not_just_primary() {
        // Adversarial (#14): the pre-pass must work uniformly for a
        // federated instance's own KB, not only primary — the Multi-mode
        // wiring calls this for the seed AND every related instance.
        let mut editor = three_instance_editor();
        let alpha = editor.kb.instances.get_mut("uuid-alpha").unwrap();
        alpha.insert(Node::new(
            "concept:alpha-seed",
            "Alpha Seed",
            NodeKind::Concept,
            "see [[concept:seed]]", // points back into primary
        ));
        let sources = editor.kb_cross_instance_link_sources(Some("uuid-alpha"));
        assert!(
            sources.contains("concept:alpha-seed"),
            "a federated instance's own bridge node must be found too: {sources:?}"
        );
    }

    #[test]
    fn kb_cross_instance_link_sources_is_empty_for_an_unregistered_or_unloaded_instance() {
        let editor = three_instance_editor();
        let sources = editor.kb_cross_instance_link_sources(Some("uuid-does-not-exist"));
        assert!(
            sources.is_empty(),
            "an unloaded/unregistered instance must yield an empty protected set, not panic"
        );
    }
}
