//! ADR-040 §4: an identity rotation's second half — switching the session to the new key.
//!
//! `collab-rotate-identity` ships each KB's `Rebind` on the session it already has, the one
//! authenticated by the OLD key: the old key signs the `Rebind` and is still the member it
//! retires. Once the daemon accepts it, that key has no seat left, and a hub that verifies
//! ops refuses that session everything — its own ops, ops the successor signed, and the
//! owner's self re-wrap alike. So the rotation waits here for the daemon's answer to every
//! `Rebind` ([`PendingRotation`]), then [`finish`] swaps the session's one identity
//! (`ClientTransport::rekey`) and the network loop reconnects under it through its ordinary
//! reconnect path, which re-subscribes every shared doc and KB. What only the new key may
//! send waits in the outbox for that session ([`flush_outbox`]).
//!
//! @ai-caution: [collab-identity] Never send an op signed by the new key on the old key's
//! session, and never let the signing key and the transport's key diverge: the hub's rule is
//! "a retired key gets nothing" (the daemon's `a_retired_keys_session_gets_nothing_after_rotation`).

use super::*;

/// A rotation whose `Rebind`s were sent on the old key's session and are not all answered.
pub(crate) struct PendingRotation {
    new_id: std::sync::Arc<mae_mcp::identity::Identity>,
    owned: usize,
    member: usize,
    /// Plans whose `Rebind` reached the wire, in send order.
    plans: Vec<RotationPlan>,
    /// Request id of each unanswered `Rebind` → its index in `plans`.
    awaiting: std::collections::HashMap<u64, usize>,
    /// Plans the daemon refused (index), with its reason, by KB.
    refused: std::collections::BTreeMap<usize, String>,
    /// KBs whose `Rebind` never reached the wire: they stay on the old key.
    pub(super) unsent: Vec<String>,
}

impl PendingRotation {
    pub(super) fn new(
        new_id: std::sync::Arc<mae_mcp::identity::Identity>,
        owned: usize,
        member: usize,
    ) -> Self {
        Self {
            new_id,
            owned,
            member,
            plans: Vec::new(),
            awaiting: std::collections::HashMap::new(),
            refused: std::collections::BTreeMap::new(),
            unsent: Vec::new(),
        }
    }

    /// Record `plan`, whose old-session deltas went out as requests `ids` — or, when `ids`
    /// is empty, did not reach the wire at all.
    pub(super) fn track(&mut self, plan: RotationPlan, ids: Vec<u64>) {
        if ids.is_empty() {
            self.unsent.push(plan.kb_id);
            return;
        }
        let idx = self.plans.len();
        for id in ids {
            self.awaiting.insert(id, idx);
        }
        self.plans.push(plan);
    }

    /// Every plan failed to reach the wire (and there was at least one).
    pub(super) fn nothing_sent(&self) -> bool {
        self.plans.is_empty() && !self.unsent.is_empty()
    }

    /// The daemon has answered every `Rebind` that was sent.
    pub(super) fn settled(&self) -> bool {
        self.awaiting.is_empty()
    }

    /// Consume `text` if it is the daemon's answer to one of this rotation's `Rebind`s,
    /// recording a refusal. `false` for any other message, which the caller handles as usual.
    pub(super) fn ack(&mut self, text: &str) -> bool {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(text) else {
            return false;
        };
        if val.get("method").is_some() {
            return false;
        }
        let Some(idx) = val
            .get("id")
            .and_then(|v| v.as_u64())
            .and_then(|id| self.awaiting.remove(&id))
        else {
            return false;
        };
        if let Some(err) = val.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("refused")
                .to_string();
            self.refused.insert(idx, msg);
        }
        true
    }
}

/// The connection was lost before the daemon answered every `Rebind`. Nothing local moves:
/// the session keeps the old key. Whether the daemon applied a `Rebind` it never answered is
/// unknown, so say so rather than report either outcome.
pub(super) fn abandon(rot: PendingRotation, evt_tx: &mpsc::Sender<CollabEvent>) {
    warn!(
        unanswered = rot.awaiting.len(),
        new_fp = %rot.new_id.fingerprint(),
        "rotate-identity: connection lost before the daemon answered — identity NOT rotated"
    );
    try_send_evt(
        evt_tx,
        CollabEvent::Error {
            message: format!(
                "Identity rotation interrupted — the connection dropped before the daemon answered {} Rebind(s). Your identity was NOT changed here; if the daemon applied one, that KB now expects the abandoned key {}.",
                rot.awaiting.len(),
                rot.new_id.fingerprint()
            ),
        },
    );
}

/// Every `Rebind` is answered. If the daemon accepted any, adopt those KBs' rotated
/// replicas, queue what only the new key may send, persist the new key and make it the
/// session's identity; `true` tells the caller to drop the old key's connection and
/// reconnect. `false`: nothing was accepted, and nothing changed.
pub(super) fn finish(
    rot: PendingRotation,
    transport: &mut ClientTransport,
    kb_collections: &mut std::collections::HashMap<String, Vec<u8>>,
    outbox: &mut Vec<(String, Vec<u8>)>,
    evt_tx: &mpsc::Sender<CollabEvent>,
) -> bool {
    let PendingRotation {
        new_id,
        owned,
        member,
        plans,
        refused,
        unsent,
        ..
    } = rot;
    let refused_kbs: Vec<String> = refused
        .iter()
        .map(|(i, why)| format!("{} ({why})", plans[*i].kb_id))
        .collect();
    let accepted: Vec<RotationPlan> = plans
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !refused.contains_key(i))
        .map(|(_, p)| p)
        .collect();
    if accepted.is_empty() && !(refused_kbs.is_empty() && unsent.is_empty()) {
        warn!(refused = ?refused_kbs, unsent = ?unsent, "rotate-identity: the daemon accepted no Rebind — identity NOT rotated");
        try_send_evt(
            evt_tx,
            CollabEvent::Error {
                message: format!(
                    "Identity rotation failed — the daemon accepted no Rebind (refused: {}; not sent: {}). Your identity was NOT changed.",
                    refused_kbs.join(", "),
                    unsent.join(", ")
                ),
            },
        );
        return false;
    }
    // Persist the successor key (the old key already signed the Rebinds).
    if let Some(dir) = mae_mcp::identity::default_collab_dir() {
        if let Err(e) = new_id.save(&dir) {
            warn!(error = %e, "rotate-identity: failed to persist the new key");
        }
    }
    let rotated = accepted.len();
    for plan in accepted {
        // Later ops chain on the rotated state, and the new key derives its content key.
        kb_collections.insert(plan.kb_id.clone(), plan.new_replica.clone());
        // B2: persist so the rotated op-log survives a restart.
        persist_collection(&plan.kb_id, &plan.new_replica);
        outbox.extend(
            plan.on_successor
                .into_iter()
                .map(|d| (plan.kb_id.clone(), d)),
        );
    }
    let new_fp = new_id.fingerprint();
    transport.rekey(new_id);
    info!(rotated, owned, member, refused = refused_kbs.len(), unsent = unsent.len(), new_fp = %new_fp, "rotate-identity: rotation shipped");
    let mut lines = vec![
        format!("Identity rotated across {rotated} KB(s) ({owned} owned, {member} as member)."),
        format!("New fingerprint: {new_fp}"),
        "Reconnecting under the new key. The daemon refuses it until its operator authorizes it (`mae-daemon authorize`); the reconnect retries until then.".to_string(),
        // The new key has a new ADR-023 client-id, so the FIRST edit to an existing node
        // after rotation trips the epoch fence once and must rebase.
        "Your first edit to an existing node may rebase once (the rotated key has a new write lineage) — set `collab-fence-resolution` to `auto` to handle it silently.".to_string(),
    ];
    if !unsent.is_empty() || !refused_kbs.is_empty() {
        lines.push(format!(
            "WARNING: rotation did not reach {} KB(s) — still on the OLD key there: {}",
            unsent.len() + refused_kbs.len(),
            unsent
                .iter()
                .cloned()
                .chain(refused_kbs)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    try_send_evt(evt_tx, CollabEvent::StatusReport { lines });
    true
}

/// Send what only the new key's session may send, now that it is the session. A write
/// that fails leaves it, and everything after it, queued for the next connection.
pub(super) async fn flush_outbox<W: tokio::io::AsyncWrite + Unpin>(
    w: &mut W,
    outbox: &mut Vec<(String, Vec<u8>)>,
    next_id: &mut u64,
    pending: &mut std::collections::HashMap<u64, PendingResponseKind>,
    timeout: std::time::Duration,
) {
    while let Some((kb_id, delta)) = outbox.first() {
        let req_id = *next_id;
        *next_id += 1;
        let req = mae_sync::wire::kb_collection_op_request(
            req_id,
            kb_id,
            &mae_sync::encoding::update_to_base64(delta),
        );
        let Ok(body) = serde_json::to_vec(&req) else {
            outbox.remove(0);
            continue;
        };
        if mae_mcp::write_framed(w, &body, timeout).await.is_err() {
            return;
        }
        pending.insert(
            req_id,
            PendingResponseKind::RotationSuccessorOp {
                kb_id: kb_id.clone(),
            },
        );
        outbox.remove(0);
    }
}
