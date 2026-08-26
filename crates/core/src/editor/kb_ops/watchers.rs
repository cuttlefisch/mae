//! KB filesystem watcher draining: preload, store watch, registry watch,
//! link validation, and orphan cleanup.

use super::*;

impl Editor {
    /// Every KB background drain, in one call — the set an event loop owes the
    /// KB once per tick, regardless of which frontend it is.
    ///
    /// **This exists because hand-rolling the set drifted, badly.** Three event
    /// loops each picked their own subset: the GUI (`idle_work`) called all
    /// four, the TUI called two, and the headless loops — including
    /// `--self-test`, and the mode the MCP server actually runs in — called
    /// none. Nothing enforced agreement, so the divergence was invisible.
    ///
    /// The headless consequence was not merely wasted work. `init_kb_federation`
    /// spawns the preload and parks the receiver; with nothing draining it,
    /// `kb.primary` stays empty, and because `primary_thin()` is false in that
    /// mode `kb_federated_search_scoped` ranks over the empty mirror. So
    /// `kb_search`/`kb_search_context`/`kb_vector_search` returned **zero of the
    /// user's own notes, forever, with no error**, while the query-layer-backed
    /// tools (`kb_list`, `kb_graph`, `kb_links_*`) saw them fine — two tools,
    /// opposite blind spots, same session.
    ///
    /// Every member is a cheap `Option`/`try_recv` check when there is nothing
    /// to do, so calling all four everywhere costs nothing measurable and
    /// removes the chance to pick a subset (principle #8).
    pub fn drain_kb_background(&mut self) {
        self.drain_kb_watchers();
        self.drain_kb_preload();
        self.drain_kb_system_stores();
        self.drain_kb_store_watch();
        self.drain_kb_registry_watch();
    }

    /// Adopt MAE's guidance corpora once the background provisioning thread
    /// finishes building/opening them, and rebuild the query layer so they
    /// become searchable.
    ///
    /// See [`crate::editor::kb_state::KbContext::pending_system_stores`] for why
    /// this is off the startup path at all (issue #713: it doubled GUI startup
    /// on Windows, which since #706 builds these corpora on every first launch).
    /// A failure inside the thread is logged there and simply yields fewer
    /// entries here — the same silent-no-op-if-absent behaviour guidance already
    /// had, rather than a new failure mode.
    pub fn drain_kb_system_stores(&mut self) {
        let Some(rx) = self.kb.pending_system_stores.as_ref() else {
            return;
        };
        let built = match rx.try_recv() {
            Ok(v) => v,
            // `Empty` means still building; `Disconnected` means the thread
            // ended without sending, which is already logged on its side.
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.kb.pending_system_stores = None;
                return;
            }
        };
        self.kb.pending_system_stores = None;
        if built.is_empty() {
            return;
        }
        for (name, store) in built {
            tracing::debug!(kb = %name, "system KB store ready");
            self.kb.system_stores.insert(name, store);
        }
        // Required, not cosmetic: the query layer joins `system_stores` as
        // labelled pseudo-instances, so a store adopted after startup is
        // invisible to every federated read until this runs.
        self.kb.rebuild_query_layer();
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
                // #79 third slice: genuinely async — drained on an idle tick well after
                // startup, the closest match to ADR-024's own motivating scenario (the
                // failure has nothing to do with whatever the user is doing when it
                // finally surfaces, so a status-line toast is easy to miss entirely).
                self.notify(
                    crate::notifications::Notification::error("kb", "KB background load failed")
                        .body(e.to_string())
                        .key("kb-background-preload-failed"),
                );
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
    /// Has this instance been drained too recently to drain again?
    ///
    /// Extracted alongside `kb_drop_detached_events` so the drain loop stays off
    /// the structural gate's per-function ceiling — both are self-contained
    /// skip-this-instance decisions.
    fn kb_debounced(&mut self, uuid: &str, debounce: std::time::Duration) -> bool {
        let too_recent = self
            .kb
            .last_drain
            .get(uuid)
            .is_some_and(|last| last.elapsed() < debounce);
        if too_recent {
            self.kb.watcher_stats.suppressed_debounce += 1;
        }
        too_recent
    }

    /// Drop a detached instance's watcher events instead of applying them.
    ///
    /// KB cutover, Phase 1: a detached instance's store IS the truth, so no
    /// `.org` event may write it. The events are still drained by the caller —
    /// dropping them here rather than skipping the instance keeps the watcher's
    /// queue from growing without bound while it is detached.
    ///
    /// @ai-caution: [kb-truth] This gate is the reason deleting a detached KB's
    /// stale archive is safe. Without it the drain's `Removed` arm reaches
    /// `kb_persist_instance_delete`, so the obvious post-cutover cleanup — "the
    /// archive is stale now, let me delete it" — silently destroyed the KB.
    /// `kb_reimport_file`'s chokepoint does NOT cover this path; its comment
    /// used to claim otherwise.
    fn kb_drop_detached_events(&mut self, uuid: &str, count: usize) -> bool {
        let detached = self
            .kb
            .registry
            .find_by_uuid(uuid)
            .is_some_and(|i| !i.allows_ingest());
        if detached {
            self.kb.watcher_stats.events_suppressed += count as u64;
            self.kb
                .last_drain
                .insert(uuid.to_string(), std::time::Instant::now());
        }
        detached
    }

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
            if self.kb_debounced(&uuid, debounce_dur) {
                continue;
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

            if self.kb_drop_detached_events(&uuid, changes.len()) {
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
                let ids = q.list_ids(None).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "kb list_ids failed while validating links on save");
                    Vec::new()
                });
                ids.into_iter().find(|id| {
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
                match q.links_from(&id) {
                    Ok(links) => links
                        .into_iter()
                        .filter(|l| !q.contains(&l.dst))
                        .map(|l| l.dst)
                        .collect(),
                    Err(e) => {
                        // A storage failure is surfaced to the user, not silently
                        // rendered as "no broken links" (ADR-086 read-side twin).
                        self.set_status(format!("KB link validation failed: {e}"));
                        Vec::new()
                    }
                }
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
    ///
    /// #485: orphan DETECTION (`orphan_ids` above) is already federation-reconciled
    /// (issue #474's fix — `FederatedQuery::health_report` sees links across every
    /// registered instance, not just primary), but each id must still be REMOVED from
    /// whichever store actually owns it, not unconditionally from `self.kb.primary`.
    /// A federated-instance orphan hitting `primary.remove(id)` was a silent no-op at
    /// best, or a wrong-node deletion at worst (two independently-authored KBs can
    /// legitimately reuse the same bare id — see `kb_owner_of`'s own doc comment).
    /// This transplants `kb_delete_node`'s (`kb_ops/nodes.rs`) owner-aware removal
    /// branch: resolve `kb_owner_of(id)` per id and delete from the resolved owner.
    pub fn kb_cleanup_orphans(&mut self) -> usize {
        let seed_prefixes = ["cmd:", "concept:", "lesson:", "scheme:", "option:"];
        let orphan_ids: Vec<String> = if let Some(q) = self.kb.query_layer() {
            match q.health_report() {
                Ok(Some(report)) => report.orphan_ids,
                Ok(None) => Vec::new(),
                Err(e) => {
                    tracing::warn!(error = %e, "kb health_report failed during orphan cleanup");
                    Vec::new()
                }
            }
        } else {
            self.kb.primary.health_report().orphan_ids
        };
        let to_remove: Vec<String> = orphan_ids
            .into_iter()
            .filter(|id| !seed_prefixes.iter().any(|p| id.starts_with(p)))
            .collect();
        let count = to_remove.len();
        for id in &to_remove {
            match self.kb_owner_of(id) {
                // Not found anywhere: shouldn't normally happen for something the
                // health report just reported as an orphan, but resolve it as a
                // graceful no-op rather than panicking or guessing a store.
                None => {}
                // Owned by primary (or genuinely unresolvable and defaulting to
                // primary, per `kb_owner_of`'s own fallback) — same primary branch
                // `kb_delete_node` uses: persist the deletion to the backing store,
                // then drop it from the in-memory mirror.
                Some(None) => {
                    self.kb_persist_delete(id);
                    self.kb.primary.remove(id);
                }
                // Owned by a federated instance — delete from that instance's own
                // persisted store AND its in-memory copy, exactly like
                // `kb_delete_node`'s federated branch. Missing store/instance entries
                // are handled gracefully (matching `kb_delete_node`'s own warn-and-
                // continue behavior), never panicking.
                Some(Some(uuid)) => {
                    if let Some(store) = self.kb.instance_stores.get(&uuid) {
                        if let Err(e) = store.delete_node(id) {
                            tracing::warn!(node_id = %id, error = %e, "KB instance store delete failed (orphan cleanup)");
                        }
                    }
                    if let Some(kb) = self.kb.instances.get_mut(&uuid) {
                        kb.remove(id);
                    }
                }
            }
        }
        if count > 0 {
            self.fire_hook("after-kb-change");
        }
        count
    }
}
