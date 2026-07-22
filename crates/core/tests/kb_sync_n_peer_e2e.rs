//! ADR-022 — N-peer KB-sync e2e harness (editor-logic altitude).
//!
//! This harness reproduces the manual two-machine validation matrix (T1–T3c) as
//! fast, deterministic, in-process tests — but it drives the **real editor CRDT
//! path** (`mae_kb::KnowledgeBase::{upsert_with_crdt, apply_remote_update,
//! adopt_remote_node, reconcile_remote_node}` + `derive_kb_client_id`), NOT a
//! hand-rolled parallel implementation. That is the whole point: the six-bug
//! chain (B-8..B-16) and the T3c crash-clobber all lived in this layer, and the
//! anti-pattern that hid them was tests that used stand-in values (`client_id=1`)
//! or serialization the production path never produces. Here:
//!
//! * Each peer's `client_id` comes from the production `derive_kb_client_id`
//!   over a distinct identity fingerprint — a hardcoded-stand-in regression
//!   (e.g. everyone collapsing to `client_id=1`) makes the concurrent-edit test
//!   diverge and fail.
//! * The hub (daemon stand-in) holds the authoritative per-node CRDT doc and
//!   exchanges **state-vector diffs** exactly as `DocStore::encode_diff_and_sv`
//!   does, so the reconcile is the same math the daemon runs.
//!
//! The crash-safety contrast is captured by two sibling tests:
//! `lost_row_adopt_clobbers_documents_the_bug` (blind adopt loses the durable
//! edit) and `lost_row_reconcile_converges` (ADR-022 reconcile recovers it).

//! ## Manual T-matrix cross-reference
//!
//! These in-process tests map onto the manual two-machine validation steps so a
//! regression here points straight at the live scenario it stands in for:
//!
//! * **T1 (share → bidirectional propagate):** `share_join_bidirectional_{2,3,5}_peers`
//! * **T2 (concurrent disjoint edits merge):** `concurrent_edits_converge_{2,3,5}_peers`
//! * **T3 (offline edit → reconnect flush):** `offline_edit_merges_on_reconnect`
//! * **T3b (offline edit survives restart):** `offline_edit_survives_restart_then_reconnects`
//! * **T3c (crash-lost row clobber vs ADR-022 reconcile):**
//!   `lost_row_adopt_clobbers_documents_the_bug` / `lost_row_reconcile_converges`
//! * **T4 (divergent independent same-id lineage repair):** `divergent_lineage_detected_and_reconciled`
//! * **T5 (distinct, wire-safe per-peer client ids):** `derived_client_ids_are_distinct_and_safe`
//!
//! The complementary **real-daemon** convergence check (the same T1/T2 concurrent
//! merge, but end-to-end over TCP framing + base64 + the daemon's authoritative
//! per-node doc) lives in `crates/mae/tests/collab_tcp_e2e.rs::tcp_kb_two_peers_concurrent_converge`
//! (MAE_TCP_E2E-gated). T6/T7 (daemon power-loss durability, mDNS/LAN discovery)
//! stay manual — see `docs/collab-testing-plan.md`.

use mae_core::editor::derive_kb_client_id;
use mae_kb::{KnowledgeBase, Node, NodeKind, ReconcileAction};

/// A collaborating editor: a real `KnowledgeBase` + a production-derived
/// per-peer `client_id` (distinct lineage, the B-16 prerequisite).
struct Peer {
    name: String,
    client_id: u64,
    kb: KnowledgeBase,
}

impl Peer {
    fn new(name: &str) -> Self {
        // Mirror how the editor derives its KB client id from the collab identity
        // fingerprint at startup — distinct peers ⇒ distinct, non-{0,1} ids.
        let fingerprint = format!("ed25519:fingerprint-for-{name}");
        Peer {
            name: name.to_string(),
            client_id: derive_kb_client_id(&fingerprint, 0),
            kb: KnowledgeBase::new(),
        }
    }

    /// Create a node locally (a fresh CRDT lineage rooted at this peer).
    fn create(&mut self, id: &str, title: &str, body: &str) {
        let node = Node::new(
            id.to_string(),
            title.to_string(),
            NodeKind::Note,
            body.to_string(),
        );
        self.kb.upsert_with_crdt(node, self.client_id);
    }

    /// Edit an existing node's fields in place (chains onto its lineage — B-15).
    fn edit(&mut self, id: &str, title: &str, body: &str) {
        let mut node = self
            .kb
            .get(id)
            .cloned()
            .unwrap_or_else(|| panic!("{} has no node {id} to edit", self.name));
        node.title = title.to_string();
        node.body = body.to_string();
        self.kb.upsert_with_crdt(node, self.client_id);
    }

    fn full_state(&self, id: &str) -> Vec<u8> {
        self.kb
            .get(id)
            .unwrap_or_else(|| panic!("{} has no node {id}", self.name))
            .to_crdt_doc()
            .unwrap()
            .encode_state()
    }

    fn state_vector(&self, id: &str) -> Option<Vec<u8>> {
        self.kb.node_state_vector(id)
    }

    fn title(&self, id: &str) -> String {
        self.kb.get(id).map(|n| n.title.clone()).unwrap_or_default()
    }

    fn body(&self, id: &str) -> String {
        self.kb.get(id).map(|n| n.body.clone()).unwrap_or_default()
    }

    fn has(&self, id: &str) -> bool {
        self.kb.contains(id)
    }

    /// Simulate an editor restart: the durable artifact is each node's persisted
    /// `crdt_doc` bytes (content), so drop the in-memory KB and rebuild it from
    /// those bytes — faithful to the disk-first startup loader. The in-flight
    /// sync intent (pending-queue row) is deliberately NOT modeled here; that's
    /// what the "lost row" tests exercise. Enumerates nodes via the real
    /// `KnowledgeBase::iter` API (no harness-side id bookkeeping).
    fn restart(&mut self) {
        let saved: Vec<(String, Vec<u8>)> = self
            .kb
            .iter()
            .filter_map(|(id, n)| n.crdt_doc.clone().map(|bytes| (id.clone(), bytes)))
            .collect();
        let mut kb = KnowledgeBase::new();
        for (id, bytes) in saved {
            kb.apply_remote_update(&id, &bytes).unwrap();
        }
        self.kb = kb;
    }
}

/// The collaboration mesh: an authoritative hub (daemon stand-in) plus N peers.
/// Implements the real protocol surface — share, (re)join via reconcile, live
/// edit broadcast — using only `KnowledgeBase` CRDT primitives.
struct Mesh {
    hub: KnowledgeBase,
    peers: Vec<Peer>,
}

impl Mesh {
    fn new(peer_names: &[&str]) -> Self {
        Mesh {
            hub: KnowledgeBase::new(),
            peers: peer_names.iter().map(|n| Peer::new(n)).collect(),
        }
    }

    fn idx(&self, name: &str) -> usize {
        self.peers
            .iter()
            .position(|p| p.name == name)
            .unwrap_or_else(|| panic!("no peer named {name}"))
    }

    fn peer(&self, name: &str) -> &Peer {
        &self.peers[self.idx(name)]
    }

    /// Owner publishes a node to the hub, establishing the canonical lineage all
    /// members will share (B-16). The hub adopts the owner's lineage verbatim.
    fn share(&mut self, owner: &str, node_id: &str) {
        let state = self.peer(owner).full_state(node_id);
        self.hub.apply_remote_update(node_id, &state).unwrap();
    }

    /// Hub-side diff for a member's state vector — exactly the daemon's
    /// `encode_diff_and_sv`: `(ops the member lacks, hub state vector)`.
    fn hub_diff_for(&self, node_id: &str, member_sv: &Option<Vec<u8>>) -> (Vec<u8>, Vec<u8>) {
        let doc = self
            .hub
            .get(node_id)
            .unwrap_or_else(|| panic!("hub has no node {node_id}"))
            .to_crdt_doc()
            .unwrap();
        let remote_sv = doc.state_vector();
        let remote_diff = match member_sv {
            Some(sv) => doc.encode_diff(sv).unwrap(),
            // Member has never seen the node — send full state (encode_diff vs an
            // empty SV). Reuse encode_state to avoid materializing an empty SV.
            None => doc.encode_state(),
        };
        (remote_diff, remote_sv)
    }

    /// Deliver the hub's current authoritative state of `node_id` to every peer
    /// except `origin` (live broadcast / catch-up).
    fn broadcast(&mut self, node_id: &str, origin: &str) {
        let state = self
            .hub
            .get(node_id)
            .unwrap()
            .to_crdt_doc()
            .unwrap()
            .encode_state();
        for p in self.peers.iter_mut() {
            if p.name != origin && p.kb.contains(node_id) {
                p.kb.apply_remote_update(node_id, &state).unwrap();
            }
        }
    }

    /// ADR-022 (re)join: bidirectional state-vector reconcile. Returns the
    /// classification so tests can assert merge-vs-adopt-vs-divergent.
    fn join_reconcile(&mut self, member: &str, node_id: &str) -> ReconcileAction {
        let i = self.idx(member);
        let member_sv = self.peers[i].state_vector(node_id);
        let (remote_diff, remote_sv) = self.hub_diff_for(node_id, &member_sv);

        let outcome = self.peers[i]
            .kb
            .reconcile_remote_node(node_id, &remote_diff, &remote_sv)
            .unwrap();

        // Divergent legacy lineage: the caller establishes a shared lineage by
        // adopting the hub's full state (the documented fallback). Post-B-16 this
        // is vanishingly rare — only independently-constructed same-id nodes hit it.
        if outcome.action == ReconcileAction::DivergentLineage {
            let full = self
                .hub
                .get(node_id)
                .unwrap()
                .to_crdt_doc()
                .unwrap()
                .encode_state();
            self.peers[i].kb.adopt_remote_node(node_id, &full).unwrap();
        } else if let Some(local_ahead) = outcome.local_ahead {
            // Push our durable-but-unsynced edits up; the hub merges (no clobber),
            // then everyone else catches up. This is the crash-safety path.
            self.hub.apply_remote_update(node_id, &local_ahead).unwrap();
            self.broadcast(node_id, member);
        }
        outcome.action
    }

    /// Pre-ADR-022 (re)join: blind full-snapshot adopt (replace). Kept ONLY to
    /// characterize the clobber it causes — production no longer uses it for an
    /// existing node.
    fn join_adopt(&mut self, member: &str, node_id: &str) {
        let i = self.idx(member);
        let full = self
            .hub
            .get(node_id)
            .unwrap()
            .to_crdt_doc()
            .unwrap()
            .encode_state();
        self.peers[i].kb.adopt_remote_node(node_id, &full).unwrap();
    }

    /// A connected peer edits a node and the edit propagates live (hub merge +
    /// broadcast to the other peers).
    fn edit_live(&mut self, editor: &str, node_id: &str, title: &str, body: &str) {
        let i = self.idx(editor);
        self.peers[i].edit(node_id, title, body);
        let state = self.peers[i].full_state(node_id);
        self.hub.apply_remote_update(node_id, &state).unwrap();
        self.broadcast(node_id, editor);
    }

    fn assert_all_converged_to(&self, node_id: &str, title: &str, body: &str) {
        let hub_doc = self.hub.get(node_id).unwrap().to_crdt_doc().unwrap();
        assert_eq!(hub_doc.title(), title, "hub title for {node_id}");
        assert_eq!(hub_doc.body(), body, "hub body for {node_id}");
        for p in &self.peers {
            assert_eq!(p.title(node_id), title, "{} title for {node_id}", p.name);
            assert_eq!(p.body(node_id), body, "{} body for {node_id}", p.name);
        }
    }

    /// All peers (and hub) agree on the materialized content — used for
    /// concurrent edits where the converged value is a deterministic CRDT merge,
    /// not a value we predict.
    fn assert_all_agree(&self, node_id: &str) -> (String, String) {
        let hub_doc = self.hub.get(node_id).unwrap().to_crdt_doc().unwrap();
        let (title, body) = (hub_doc.title(), hub_doc.body());
        for p in &self.peers {
            assert_eq!(
                p.title(node_id),
                title,
                "{} disagrees on title for {node_id}: {:?} != hub {:?}",
                p.name,
                p.title(node_id),
                title
            );
            assert_eq!(
                p.body(node_id),
                body,
                "{} disagrees on body for {node_id}",
                p.name
            );
        }
        (title, body)
    }
}

fn peer_names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("peer{i}")).collect()
}

// ---------------------------------------------------------------------------
// T1/T2 — share → join → bidirectional propagation, N ∈ {2, 3, 5}
// ---------------------------------------------------------------------------

fn run_share_join_bidirectional(n: usize) {
    let names = peer_names(n);
    let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    let mut mesh = Mesh::new(&refs);
    let node = "collab:overview";

    // peer0 owns + shares; everyone else joins via reconcile (first join = Created).
    mesh.peers[0].create(node, "Overview v1", "body v1");
    mesh.share("peer0", node);
    for name in &refs[1..] {
        let action = mesh.join_reconcile(name, node);
        assert_eq!(
            action,
            ReconcileAction::Created,
            "{name} first join should create the node"
        );
        assert!(mesh.peer(name).has(node), "{name} should have the node");
    }
    mesh.assert_all_converged_to(node, "Overview v1", "body v1");

    // Owner edits → all members see it live.
    mesh.edit_live("peer0", node, "Overview v2", "body v2");
    mesh.assert_all_converged_to(node, "Overview v2", "body v2");

    // A non-owner member edits → propagates back through the hub to everyone
    // (the B-8/B-13/B-16 round-trip that was broken end-to-end).
    let last = refs[n - 1];
    mesh.edit_live(last, node, "Overview v3", "edited by member");
    mesh.assert_all_converged_to(node, "Overview v3", "edited by member");
}

#[test]
fn share_join_bidirectional_2_peers() {
    run_share_join_bidirectional(2);
}

#[test]
fn share_join_bidirectional_3_peers() {
    run_share_join_bidirectional(3);
}

#[test]
fn share_join_bidirectional_5_peers() {
    run_share_join_bidirectional(5);
}

// ---------------------------------------------------------------------------
// #303 follow-up — a promoted node shares/joins like any other primary node
// ---------------------------------------------------------------------------

#[test]
fn promoted_node_shares_and_joins_peer_materializes_as_federation() {
    // A promoted primary node (source=Promoted, crdt_doc=None per
    // kb_promote_node, provenance stamped in properties) must share and be
    // joined by a peer exactly like any other primary node -- no special
    // casing needed, since the CRDT wire payload (KbNodeDoc) only carries
    // id/title/body/tags/links/meta. This locks in, explicitly rather than
    // as an implicit surprise: NodeSource/kind/properties are NOT part of
    // that payload, so the joining peer's materialized copy always comes
    // back as NodeSource::Federation with none of the promoted_from_*
    // properties -- pre-existing, uniform-across-all-nodes CRDT-schema
    // behavior, not Promoted-specific, but never explicitly tested before.
    let mut mesh = Mesh::new(&["owner", "peer1"]);
    let node_id = "test:promoted-share";

    let mut node = Node::new(node_id, "Promoted Note", NodeKind::Note, "promoted body");
    node.source = Some(mae_kb::NodeSource::Promoted);
    node.properties
        .insert("promoted_from_uuid".to_string(), "origin-uuid".to_string());
    node.properties.insert(
        "promoted_from_org_dir".to_string(),
        "/home/user/notes".to_string(),
    );
    node.properties.insert(
        "promoted_at".to_string(),
        "2026-07-20T14:03:00Z".to_string(),
    );
    let owner_client_id = mesh.peers[0].client_id;
    mesh.peers[0].kb.upsert_with_crdt(node, owner_client_id);

    mesh.share("owner", node_id);
    let action = mesh.join_reconcile("peer1", node_id);
    assert_eq!(
        action,
        ReconcileAction::Created,
        "first join for this id should create the node on peer1"
    );
    mesh.assert_all_converged_to(node_id, "Promoted Note", "promoted body");

    let peer_node = mesh.peer("peer1").kb.get(node_id).unwrap();
    assert_eq!(
        peer_node.source,
        Some(mae_kb::NodeSource::Federation),
        "the joining peer's materialized copy is always Federation-sourced -- \
         NodeSource is not part of the CRDT wire payload"
    );
    assert!(
        !peer_node.properties.contains_key("promoted_from_uuid"),
        "promoted_from_* properties are not part of the CRDT wire payload either"
    );

    // The owner's own copy, unaffected by the peer's materialization,
    // still carries its original provenance.
    assert_eq!(
        mesh.peer("owner").kb.get(node_id).unwrap().source,
        Some(mae_kb::NodeSource::Promoted)
    );
}

// ---------------------------------------------------------------------------
// T4 — concurrent same-node edits converge (distinct client_ids — the B-16 guard)
// ---------------------------------------------------------------------------

fn run_concurrent_edits_converge(n: usize) {
    let names = peer_names(n);
    let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    let mut mesh = Mesh::new(&refs);
    let node = "collab:concurrent";

    mesh.peers[0].create(node, "base", "base body");
    mesh.share("peer0", node);
    for name in &refs[1..] {
        mesh.join_reconcile(name, node);
    }

    // Every peer edits the SAME node locally, offline (no propagation yet). With
    // distinct derived client_ids these merge; a hardcoded shared client_id would
    // collide and the peers would diverge below.
    for (k, name) in refs.iter().enumerate() {
        let i = mesh.idx(name);
        mesh.peers[i].edit(node, &format!("title-from-{name}"), &format!("body-{k}"));
    }

    // Now everyone reconnects and reconciles (order matters not — CRDT).
    for name in &refs {
        mesh.join_reconcile(name, node);
    }
    // Second pass so late local-ahead pushes fan out to everyone.
    for name in &refs {
        mesh.join_reconcile(name, node);
    }

    let (title, body) = mesh.assert_all_agree(node);
    // The merge must reflect every peer's contribution survived in SOME form
    // (no silent last-writer-wins drop): each peer's id fragment is present.
    for name in &refs {
        assert!(
            title.contains(&format!("from-{name}")),
            "converged title {title:?} dropped {name}'s concurrent edit"
        );
    }
    assert!(!body.is_empty());
}

#[test]
fn concurrent_edits_converge_2_peers() {
    run_concurrent_edits_converge(2);
}

#[test]
fn concurrent_edits_converge_3_peers() {
    run_concurrent_edits_converge(3);
}

#[test]
fn concurrent_edits_converge_5_peers() {
    run_concurrent_edits_converge(5);
}

// ---------------------------------------------------------------------------
// T3 — offline edit merges on reconnect (no full-snapshot stampede)
// ---------------------------------------------------------------------------

#[test]
fn offline_edit_merges_on_reconnect() {
    let mut mesh = Mesh::new(&["alice", "bob"]);
    let node = "collab:offline";

    mesh.peers[0].create(node, "v1", "shared");
    mesh.share("alice", node);
    mesh.join_reconcile("bob", node);
    mesh.assert_all_converged_to(node, "v1", "shared");

    // bob edits while offline (queued, not pushed). Meanwhile alice edits the body
    // live so the two edits are genuinely concurrent on different fields.
    let bi = mesh.idx("bob");
    mesh.peers[bi].edit(node, "v2-bob-title", "shared");
    mesh.edit_live("alice", node, "v1", "alice-edited-body");

    // bob reconnects → reconcile merges both directions: bob keeps its title,
    // gains alice's body; everyone converges.
    let action = mesh.join_reconcile("bob", node);
    assert_eq!(action, ReconcileAction::Merged);
    let (title, body) = mesh.assert_all_agree(node);
    assert_eq!(title, "v2-bob-title", "bob's offline title must survive");
    assert_eq!(
        body, "alice-edited-body",
        "alice's concurrent body must survive"
    );
}

// ---------------------------------------------------------------------------
// T3b — offline edit survives an editor restart, then reconnects
// ---------------------------------------------------------------------------

#[test]
fn offline_edit_survives_restart_then_reconnects() {
    let mut mesh = Mesh::new(&["alice", "bob"]);
    let node = "collab:restart";

    mesh.peers[0].create(node, "v1", "body");
    mesh.share("alice", node);
    mesh.join_reconcile("bob", node);

    // bob edits offline, then restarts the editor (durable content reloaded from
    // crdt_doc bytes; the pending-sync row, if any, still present in this model).
    let bi = mesh.idx("bob");
    mesh.peers[bi].edit(node, "v2-after-restart", "body");
    mesh.peers[bi].restart();
    assert_eq!(
        mesh.peer("bob").title(node),
        "v2-after-restart",
        "durable edit must survive restart"
    );

    // Reconnect → reconcile pushes the surviving edit up; alice converges.
    mesh.join_reconcile("bob", node);
    mesh.assert_all_converged_to(node, "v2-after-restart", "body");
}

// ---------------------------------------------------------------------------
// T3c — the crash-safety crux: a durable edit whose sync intent was LOST.
//   * adopt path  → clobbers it (documents the bug ADR-022 fixes)
//   * reconcile   → recovers it (the fix + permanent regression guard)
// ---------------------------------------------------------------------------

/// Shared setup: bob makes a durable edit that never propagated (its pending
/// queue row was lost in a `kill -9`), modeled as "edit locally, do not push".
fn lost_row_setup() -> (Mesh, &'static str) {
    let mut mesh = Mesh::new(&["alice", "bob"]);
    let node = "collab:crash";
    mesh.peers[0].create(node, "v1", "body");
    mesh.share("alice", node);
    mesh.join_reconcile("bob", node);
    mesh.assert_all_converged_to(node, "v1", "body");

    // bob edits durably; crash drops the sync intent → NOT pushed. Restart keeps
    // the content (durable crdt_doc) but there is no queued row to replay.
    let bi = mesh.idx("bob");
    mesh.peers[bi].edit(node, "v2-unsynced", "body");
    mesh.peers[bi].restart();
    (mesh, node)
}

#[test]
fn lost_row_adopt_clobbers_documents_the_bug() {
    let (mut mesh, node) = lost_row_setup();
    // Pre-ADR-022 behavior: rejoin blindly adopts the hub's older snapshot.
    mesh.join_adopt("bob", node);
    // The durable local edit is silently lost — this is the data-loss ADR-022
    // exists to prevent. Asserting it here pins the regression: if someone
    // reintroduces adopt-on-rejoin, the reconcile test below also starts failing.
    assert_eq!(
        mesh.peer("bob").title(node),
        "v1",
        "blind adopt is expected to clobber bob's durable edit (the bug)"
    );
}

#[test]
fn lost_row_reconcile_converges() {
    let (mut mesh, node) = lost_row_setup();
    // ADR-022: rejoin reconciles. The hub is behind bob, so it sends a no-op diff
    // (bob keeps v2-unsynced); bob computes its local-ahead and pushes it up; the
    // hub merges and broadcasts. No clobber, no dependence on the pending queue.
    let action = mesh.join_reconcile("bob", node);
    assert_eq!(action, ReconcileAction::Merged);
    mesh.assert_all_converged_to(node, "v2-unsynced", "body");
}

// ---------------------------------------------------------------------------
// Divergent-lineage guard (B-14): two INDEPENDENTLY-constructed same-id docs.
// This is the false-confidence case W3 calls out — both peers build the node
// from scratch (different lineages), so a naive merge no-ops. Reconcile must
// detect divergence and the caller's adopt fallback must converge them.
// ---------------------------------------------------------------------------

#[test]
fn divergent_lineage_detected_and_reconciled() {
    let mut mesh = Mesh::new(&["alice", "bob"]);
    let node = "collab:divergent";

    // Both peers independently create a node with the SAME id but different
    // lineage and content (e.g. both imported the same org fixture pre-share).
    mesh.peers[0].create(node, "alice-version", "alice body");
    mesh.peers[1].create(node, "bob-version", "bob body");

    // alice shares hers; bob already has an independent same-id node.
    mesh.share("alice", node);

    // bob reconciles: the hub's ops can't merge into bob's parallel lineage →
    // DivergentLineage → the harness's adopt fallback establishes the shared
    // (alice's) lineage. They converge on the canonical owner content.
    let action = mesh.join_reconcile("bob", node);
    assert_eq!(
        action,
        ReconcileAction::DivergentLineage,
        "independently-constructed same-id docs must be flagged divergent"
    );
    assert_eq!(mesh.peer("bob").title(node), "alice-version");

    // After adoption they share a lineage — subsequent edits merge cleanly.
    mesh.edit_live("bob", node, "bob-version-2", "merged body");
    mesh.assert_all_converged_to(node, "bob-version-2", "merged body");
}

// ---------------------------------------------------------------------------
// Distinct-client-id guarantee: the production derivation must not collapse
// peers onto one lineage (the latent hardcoded-`client_id=1` trap, B-16).
// ---------------------------------------------------------------------------

#[test]
fn derived_client_ids_are_distinct_and_safe() {
    let peers: Vec<Peer> = peer_names(8).iter().map(|n| Peer::new(n)).collect();
    let mut seen = std::collections::HashSet::new();
    for p in &peers {
        assert!(
            p.client_id != 0 && p.client_id != 1,
            "{} got reserved client_id {}",
            p.name,
            p.client_id
        );
        assert!(
            seen.insert(p.client_id),
            "duplicate derived client_id {} for {}",
            p.client_id,
            p.name
        );
    }
}
