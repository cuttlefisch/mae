//! KB join lifecycle, CRDT sync gating, and remote-update application.

use super::*;

impl Editor {
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
    pub(super) fn kb_collab_id_of(&self, owner: &Option<String>) -> Option<String> {
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
    pub(super) fn kb_sync_target(&self, owner: &Option<String>) -> Option<String> {
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
    pub(super) fn kb_enqueue_node_crdt(
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
    pub(super) fn kb_enqueue_manifest_op(
        &mut self,
        kb_id: &str,
        node_id: &str,
        title: &str,
        add: bool,
    ) {
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
    pub(super) fn kb_ensure_node_loaded(&mut self, id: &str) {
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
    pub(super) fn kb_persist_node_in(&self, owner: &Option<String>, node: &mae_kb::Node) {
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
    pub(super) fn kb_persist_delete(&self, id: &str) {
        if let Some(ref store) = self.kb.store {
            if let Err(e) = store.delete_node(id) {
                tracing::warn!(node_id = %id, error = %e, "KB store delete failed");
            }
        }
    }
}
