//! KB node CRUD: create/update/delete/promote/adopt, and share-lineage prep.

use super::*;

impl Editor {
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
    ///  - Resets `source` to `NodeSource::Promoted` and clears `crdt_doc`.
    ///    Leaving the original `Federation` marker in place used to make
    ///    `kb_owner_of`'s #76 stranded-node guard mistake the fresh primary
    ///    copy for pre-ADR-019 leftover cruft and keep routing every future
    ///    update/delete/adopt/remote-CRDT-update back to the stale instance
    ///    copy — promotion was a no-op for CRUD purposes. Leaving the old
    ///    `crdt_doc` in place would also let a later re-share
    ///    (`kb_prepare_share_lineage`) reuse a lineage tied to the WRONG
    ///    KB/epoch, since that function only mints a fresh one when
    ///    `crdt_doc.is_none()`.
    ///  - Stamps `promoted_from_{uuid,org_dir,path}`/`promoted_at` into
    ///    `node.properties` (already durably persisted as `properties_json`
    ///    — no schema migration) so provenance isn't silently lost.
    ///  - The node's id is UNCHANGED — nothing elsewhere in the KB graph
    ///    needs link-rewriting, since resolution is by id string.
    ///  - Leaves the original org file on disk untouched. The federated
    ///    instance's own copy of the node is deduplicated away immediately
    ///    (`kb_dedup_promoted_instance_copy`) when its content still
    ///    matches what was just promoted; if it has since diverged, both
    ///    copies are kept and surfaced via notification for manual review.
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
        node.source = Some(mae_kb::NodeSource::Promoted);
        node.crdt_doc = None;
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
        // Snapshot before the branch below potentially moves `node` into
        // `primary.insert` — the dedup check needs it afterward.
        let promoted_snapshot = node.clone();
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

        let dedup = self.kb_dedup_promoted_instance_copy(node_id, &owner_uuid, &promoted_snapshot);

        let status = match dedup {
            PromoteDedup::Removed => format!(
                "Promoted '{}' to the primary KB (instance copy removed)",
                node_id
            ),
            PromoteDedup::KeptDiverged => format!(
                "Promoted '{}' to the primary KB (instance copy diverges — kept both, see notifications)",
                node_id
            ),
        };
        self.set_status(status);
        Ok(KbPromoteResult {
            node_id: node_id.to_string(),
            promoted_from_uuid: owner_uuid,
            promoted_from_org_dir: instance.org_dir,
            dedup,
        })
    }

    /// Deduplicate the origin instance's now-redundant copy of a node that
    /// was just promoted into primary (`kb_promote_node`). Inverse polarity
    /// of `kb_migrate_stranded_federation_nodes`: there, the INSTANCE copy
    /// is correct and a stale `primary` copy is cruft to remove; here, the
    /// freshly-promoted PRIMARY copy is correct and the INSTANCE copy is
    /// the leftover. Content is checked (not assumed) to still match what
    /// was just promoted — normally true immediately post-copy, but this
    /// stays correct if something else raced in between.
    ///
    /// `pub(super)` (not private) so tests can drive the divergence branch
    /// directly — that path isn't reachable through `kb_promote_node`'s own
    /// synchronous call sequence in v1 (content always matches immediately
    /// post-copy), but must stay correct if something ever races.
    pub(super) fn kb_dedup_promoted_instance_copy(
        &mut self,
        node_id: &str,
        owner_uuid: &str,
        promoted: &mae_kb::Node,
    ) -> PromoteDedup {
        let Some(instance_node) = self
            .kb
            .instances
            .get(owner_uuid)
            .and_then(|kb| kb.get(node_id))
            .cloned()
        else {
            // Already gone — nothing to dedup.
            return PromoteDedup::Removed;
        };

        if !super::sync::kb_content_equal(promoted, &instance_node) {
            tracing::warn!(target: "kb_sync", node_id = %node_id, instance = %owner_uuid, "promoted copy diverges from its origin instance copy — preserved, surfacing for review");
            self.notify(
                crate::notifications::Notification::action_required(
                    "kb",
                    format!(
                        "KB '{node_id}': the promoted copy diverges from its origin instance copy"
                    ),
                )
                .key(format!("kb:promote-diverge:{node_id}"))
                .body(
                    "This node was just promoted to the primary KB, but its content no longer \
                     matches the copy still in its origin federated instance. Both copies were \
                     kept — review and delete the stale one manually (`:kb-find` or the KB \
                     graph view) once you've confirmed which is correct.",
                ),
            );
            return PromoteDedup::KeptDiverged;
        }

        if let Some(store) = self.kb.instance_stores.get(owner_uuid) {
            if let Err(e) = store.delete_node(node_id) {
                tracing::warn!(node_id = %node_id, error = %e, "KB instance store delete failed during promote dedup");
            }
        }
        if let Some(kb) = self.kb.instances.get_mut(owner_uuid) {
            kb.remove(node_id);
        }

        let instance_owner = Some(owner_uuid.to_string());
        if let Some(kb_id) = self.kb_sync_target(&instance_owner) {
            self.kb_enqueue_manifest_op(&kb_id, node_id, "", false);
        }

        PromoteDedup::Removed
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
    pub(super) fn kb_update_node_with(
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
            self.kb.capture_state = Some(crate::CaptureState {
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
    pub(super) fn kb_insert_to_notes_instance(
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
}
