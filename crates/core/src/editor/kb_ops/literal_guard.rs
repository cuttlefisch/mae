//! Refuse MAE writes to a KB whose collection declares a `body_format` MAE does
//! not write — today, `org-literal-v1`.
//!
//! # Why
//!
//! A literal-org KB stores each node's org body byte-for-byte, with typed fields
//! (title, tags, properties in source order, the raw `kind` string) lifted out of
//! the header. Clients that edit through `mae-org` keep that format. MAE's node
//! writer does not: `Node` → `write_v2_fields` folds any `kind` outside
//! `NodeKind` to `note`, the property map loses source order and key case, and
//! the node-buffer round trip rewrites `[[id:X][d]]` to `[[X|d]]`. The result
//! is then broadcast to every peer. Until the mae-org write path lands
//! (ADR-112 O4), MAE must not write such a KB at all. Reads are unaffected.
//!
//! # Where
//!
//! [`Editor::kb_refuse_foreign_body_format`] is called from the `Editor`
//! mutators themselves — `kb_update_node_with_target` (ADR-092's sole content
//! mutator, behind `kb_update`/`kb_set_role`/`kb_add_link`, the node-buffer
//! save, `:kb-update`, Scheme `(kb-update)`, keep-mine re-authoring),
//! `kb_create_node_in`, `kb_delete_node_target`, `kb_promote_node` and
//! `kb_reimport` — so the human's commands and the agent's tools reach the same
//! refusal (principle #3: one rule for both). The join path additionally
//! withholds local-ahead ops ([`Editor::kb_withhold_local_ahead`]).
//!
//! # What the editor knows, and when
//!
//! The marker's truth is the KB's collection CRDT (`kbc:`). The editor holds a
//! replica of it in memory (`collab.kb_collection_state`) from join/share until
//! leave or exit. Every bridge path that writes that map also records the
//! declared format in the KB's own instance store
//! ([`Editor::kb_note_collection_state`], or [`Editor::kb_record_body_format`]
//! on join, once the store exists). That durable copy is what still answers after a
//! restart, before (or without) a reconnect, which is exactly when an offline
//! edit would otherwise be queued and later shipped to every peer.

use super::*;
use mae_sync::kb::{KbCollectionDoc, BODY_FORMAT_ORG_LITERAL_V1};

/// What this editor can tell about a KB's body format.
enum BodyFormat {
    /// No format declared: MAE's own, the only one MAE writes.
    Mae,
    /// A declared format MAE must not write.
    Declared(String),
    /// There is something to read, and it could not be read.
    Unreadable(String),
    /// Nothing recorded anywhere this editor can look.
    Unknown,
}

impl Editor {
    /// Refuse a write to the KB that `owner` names (`None` = the primary) when
    /// that KB declares a body format MAE does not write.
    ///
    /// # Fail-open on `Unknown`, fail-closed on `Unreadable`
    ///
    /// - **No collab id at all** (a KB never shared or joined): allowed. The
    ///   marker lives only in a collection CRDT, so such a KB has no marker, and
    ///   everything in it was written by MAE's own ingest or editor.
    /// - **A collab id but no replica and no recorded format** (`Unknown`):
    ///   allowed. A literal KB is created on the daemon with the marker in its
    ///   genesis collection, and a MAE editor can only acquire one by joining,
    ///   which seeds the in-memory replica and records the format in the store
    ///   before any of its nodes can be edited here. What is left is a KB
    ///   joined by a build without this guard and not re-joined since; the next
    ///   join closes it. Refusing instead would block offline editing of every
    ///   ordinary shared KB after a restart — the durable pending queue
    ///   (ADR-020) exists precisely to allow that — on the strength of no
    ///   evidence at all.
    /// - **A replica or record that cannot be read** (`Unreadable`): refused.
    ///   That is evidence the KB has a format declaration this editor failed to
    ///   see, and a transient read error must not be what lets a write through.
    pub(crate) fn kb_refuse_foreign_body_format(
        &self,
        owner: &Option<String>,
    ) -> Result<(), String> {
        let Some(kb_id) = self.kb_format_collab_id(owner) else {
            return Ok(());
        };
        let name = match owner {
            None => "local".to_string(),
            Some(uuid) => self
                .kb
                .registry
                .find_by_uuid(uuid)
                .map_or_else(|| uuid.clone(), |i| i.name.clone()),
        };
        self.kb_refuse_foreign_body_format_of(&kb_id, owner, &name)
    }

    /// [`Self::kb_refuse_foreign_body_format`] for a KB named by its collab id —
    /// for the join path, which runs before the registry row exists.
    fn kb_refuse_foreign_body_format_of(
        &self,
        kb_id: &str,
        owner: &Option<String>,
        name: &str,
    ) -> Result<(), String> {
        match self.kb_body_format(kb_id, owner) {
            BodyFormat::Mae | BodyFormat::Unknown => Ok(()),
            BodyFormat::Declared(f) if f == BODY_FORMAT_ORG_LITERAL_V1 => Err(format!(
                "KB '{name}' stores literal org bodies ({f}); MAE cannot edit it until the \
                 mae-org write path lands (ADR-112 O4). Reads are unaffected."
            )),
            BodyFormat::Declared(f) => Err(format!(
                "KB '{name}' stores node bodies as '{f}', a format this build of MAE does \
                 not write; it is read-only here. Reads are unaffected."
            )),
            BodyFormat::Unreadable(e) => Err(format!(
                "KB '{name}': could not read its collection to confirm MAE may write it \
                 ({e}); nothing was written. Reconnect to the daemon and retry."
            )),
        }
    }

    /// On a (re)join, drop the local-ahead ops reconcile found if the KB declares
    /// a format MAE does not write. Those ops were authored by a MAE build
    /// without this guard (or in its fail-open window), and re-sending them
    /// would carry MAE's rewrite to every peer. They stay in the local doc,
    /// unsent. Runs before the registry row exists, hence keyed by `kb_id`.
    pub(super) fn kb_withhold_local_ahead(
        &self,
        kb_id: &str,
        uuid: &str,
        local_ahead: &mut Vec<(String, Vec<u8>)>,
    ) {
        if local_ahead.is_empty() {
            return;
        }
        let owner = Some(uuid.to_string());
        if let Err(why) = self.kb_refuse_foreign_body_format_of(kb_id, &owner, kb_id) {
            tracing::warn!(
                target: "kb_sync", kb_id = %kb_id, count = local_ahead.len(), reason = %why,
                "join: NOT re-syncing local-ahead edit(s) to a KB MAE does not write"
            );
            local_ahead.clear();
        }
    }

    /// The collab id the owning KB is — or was — shared under, whether or not it
    /// syncs right now. Deliberately not `kb_collab_id_of`, which is the
    /// broadcast gate and drops a KB the user has left: a left literal KB's local
    /// copy still must not take MAE edits, because a later re-join pushes any
    /// local-ahead ops to every peer (ADR-022).
    fn kb_format_collab_id(&self, owner: &Option<String>) -> Option<String> {
        match owner {
            None => self
                .kb
                .registry
                .primary_collab_id
                .clone()
                .or_else(|| self.kb_collab_id_of(&None)),
            Some(uuid) => self
                .kb
                .registry
                .find_by_uuid(uuid)
                .and_then(|i| i.collab_id.clone()),
        }
    }

    /// The live replica first (it is current); the instance store's durable
    /// record only when there is no replica.
    fn kb_body_format(&self, kb_id: &str, owner: &Option<String>) -> BodyFormat {
        // An EMPTY replica is no replica: a join answered by a daemon that sent
        // no collection state is stored as `[]`, which is not a decode failure
        // to fail closed on but an absence of evidence.
        let replica = self
            .collab
            .kb_collection_state
            .get(kb_id)
            .filter(|b| !b.is_empty());
        if let Some(bytes) = replica {
            return match KbCollectionDoc::from_bytes(bytes) {
                Ok(coll) => coll
                    .body_format()
                    .map_or(BodyFormat::Mae, BodyFormat::Declared),
                Err(e) => BodyFormat::Unreadable(e.to_string()),
            };
        }
        let store = owner
            .as_ref()
            .and_then(|uuid| self.kb.instance_stores.get(uuid));
        match store.map(|s| s.collection_body_format()) {
            Some(Ok(Some(f))) => BodyFormat::Declared(f),
            Some(Ok(None)) | None => BodyFormat::Unknown,
            Some(Err(e)) => BodyFormat::Unreadable(e.to_string()),
        }
    }

    /// Install `state` as this editor's replica of KB `kb_id`'s collection, and
    /// record its declared body format durably (see the module doc).
    ///
    /// The bridge's share and live-`kbc:`-broadcast handlers route through here
    /// rather than inserting into `kb_collection_state` directly, so the durable
    /// record cannot fall behind the replica. (Join inserts first and calls
    /// [`Self::kb_record_body_format`] after its instance store is opened.)
    pub fn kb_note_collection_state(&mut self, kb_id: &str, state: Vec<u8>) {
        self.collab
            .kb_collection_state
            .insert(kb_id.to_string(), state);
        self.kb_record_body_format(kb_id);
    }

    /// Copy the replica's declared body format into the owning instance store.
    /// A no-op until both exist (a first join records it once its store is
    /// opened), and when the store already holds the same value.
    pub fn kb_record_body_format(&self, kb_id: &str) {
        let Some(bytes) = self.collab.kb_collection_state.get(kb_id) else {
            return;
        };
        let Ok(coll) = KbCollectionDoc::from_bytes(bytes) else {
            return;
        };
        let format = coll.body_format();
        let Some(store) = self
            .kb
            .registry
            .find_by_collab_id(kb_id)
            .and_then(|i| self.kb.instance_stores.get(&i.uuid))
        else {
            return;
        };
        if matches!(store.collection_body_format(), Ok(ref cur) if *cur == format) {
            return;
        }
        if let Err(e) = store.set_collection_body_format(format.as_deref()) {
            tracing::warn!(
                target: "kb_sync", kb_id = %kb_id, error = %e,
                "failed to record the KB's body format durably"
            );
        }
    }
}
