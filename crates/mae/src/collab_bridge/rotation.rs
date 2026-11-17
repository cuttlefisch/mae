//! ADR-040 §4: an identity rotation, from the first `Rebind` to the session running on the
//! new key — and every way the connection can fail in between.
//!
//! `collab-rotate-identity` ships each KB's `Rebind` on the session it already has, the one
//! authenticated by the OLD key: the old key signs the `Rebind` and is still the member it
//! retires. Once the daemon accepts it, that key has no seat left, and a hub that verifies
//! ops refuses that session everything. So the session moves to the new key only when the
//! daemon's verdict on every KB is known, and only KBs that accepted the `Rebind` adopt it.
//!
//! The verdict can be lost: the connection may drop after the daemon applied a `Rebind` and
//! before its answer arrived. So the rotation is durable ([`record`]): the new key and each
//! KB's `Rebind` are written to disk BEFORE anything is sent, and the record is deleted only
//! when no KB can still hold a `Rebind` the editor has not adopted. A KB whose answer was
//! lost is resolved on the next connection, or the next start:
//!
//! 1. Re-send the same signed `Rebind`, byte for byte, on the old key's session. A yes means
//!    the daemon holds it (a duplicate is a no-op there).
//! 2. A no to the re-send is ambiguous: the daemon refuses a re-send both when it never will
//!    accept the `Rebind` and when it already did (the old key is then retired and gets
//!    nothing). Only the new key can tell, so the editor asks as the new key, on a separate
//!    short connection that does nothing else (`kb/query.capabilities`, a read gated on
//!    membership — never a join, which a permissive KB would answer by admitting the new key
//!    and so fake the answer). A member: applied. `not_member`: refused.
//! 3. Until then the key is kept. A KB the daemon may have accepted is never given up.
//!
//! The order matters for one race: the daemon may still be applying the lost original when
//! the editor reconnects. The re-send goes first, so a refusal that follows it is already
//! after the original in the daemon's log (the same bytes, the same checks), and the
//! membership question asked after that cannot be answered too early.
//!
//! The rotation lives in [`Slot`], inside `ClientTransport`, beside the identity it will
//! replace: the one place that owns the session identity.
//!
//! @ai-caution: [collab-identity] Never send an op signed by the new key on the old key's
//! session, never let the signing key and the transport's key diverge, and never adopt the
//! new key for a KB the daemon did not accept: the hub's rule is "a retired key gets
//! nothing" (the daemon's `a_retired_keys_session_gets_nothing_after_rotation`).

use super::*;
use mae_mcp::identity::Identity;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod record;
use record::{KbEntry, Outcome, Record};

/// The session's rotation state. Held by `ClientTransport` in key mode.
pub(crate) struct Slot {
    /// The directory of the identity key, where the pending record lives too. `None`:
    /// nowhere to keep a new key until the daemon answers, so no rotation may start.
    dir: Option<PathBuf>,
    pending: Option<PendingRotation>,
}

/// A rotation whose outcome is not settled on every KB, or (`switched`) whose re-wraps the
/// new key's session has not sent yet.
pub(crate) struct PendingRotation {
    record: Record,
    new_id: Arc<Identity>,
    /// Request id of each unanswered `Rebind` delta → (KB index, is it a re-send).
    awaiting: HashMap<u64, (usize, bool)>,
    /// The connection the `Unknown` KBs were last sent on.
    sent_gen: u64,
    /// When the new key may next be asked about membership; `None`: now.
    retry_at: Option<Instant>,
    /// Switched: what only the new key's session may send.
    outbox: Vec<(String, Vec<u8>)>,
    /// The new key could not be saved, and the user was told.
    commit_reported: bool,
}

impl Slot {
    pub(crate) fn new(dir: Option<PathBuf>) -> Self {
        Self { dir, pending: None }
    }

    /// Start-up: pick up a rotation an earlier run left pending for `current`, the identity
    /// just loaded from `dir`.
    pub(crate) fn load(dir: PathBuf, current: &Identity) -> Self {
        let pending = match record::load(&dir) {
            Ok(Some(rec)) => resume(rec, current, &dir),
            Ok(None) => None,
            Err(e) => {
                warn!(error = %e, path = %record::path(&dir).display(), "rotate-identity: unreadable pending-rotation record; left in place, not used");
                None
            }
        };
        Self {
            dir: Some(dir),
            pending,
        }
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// When the network loop should wake to ask the new key again.
    pub(super) fn wake_at(&self) -> Option<Instant> {
        let r = self.pending.as_ref()?;
        let open = r.record.any(Outcome::Unknown) || r.record.any(Outcome::Ambiguous);
        if r.record.switched || !r.awaiting.is_empty() || !open {
            return None;
        }
        r.retry_at
    }

    /// Consume `text` if it is the daemon's answer to one of this rotation's `Rebind`s,
    /// recording the outcome durably. `false` for any other message.
    pub(super) fn ack(&mut self, text: &str) -> bool {
        let Some(rot) = self.pending.as_mut() else {
            return false;
        };
        let Ok(val) = serde_json::from_str::<serde_json::Value>(text) else {
            return false;
        };
        if val.get("method").is_some() {
            return false;
        }
        let Some((idx, resend)) = val
            .get("id")
            .and_then(|v| v.as_u64())
            .and_then(|id| rot.awaiting.remove(&id))
        else {
            return false;
        };
        let e = &mut rot.record.kbs[idx];
        match val.get("error") {
            None if e.outcome == Outcome::Unknown => e.outcome = Outcome::Applied,
            None => {}
            Some(err) => {
                let why = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("refused")
                    .to_string();
                // A refusal of the original, on the connection that carried it, is final
                // only when it is one the daemon makes before applying anything. A refusal
                // of a re-send is not: the original may be what made it. Any other error —
                // an internal or storage fault, a quota, an unkinded one — may come after
                // the daemon applied the `Rebind`, so it is settled by asking the new key.
                if !resend && e.outcome == Outcome::Unknown && refused_before_applying(err) {
                    e.outcome = Outcome::Refused;
                    e.reason = Some(why);
                } else {
                    e.outcome = Outcome::Ambiguous;
                    rot.retry_at = None;
                }
            }
        }
        if let Some(dir) = &self.dir {
            persist(dir, &rot.record);
        }
        true
    }
}

/// The `data.kind`s of the daemon's `kb/collection_op` refusals that it returns BEFORE it
/// applies anything (`handle_kb_collection_op`): the access gate (not a member, wrong role,
/// not shared over this transport, no such KB), a malformed or oversized request, and an op
/// whose signature or author does not verify. Only these settle a `Rebind` as refused.
/// Everything else — `internal` (persisting failed, possibly after the update was applied),
/// `collection_unavailable`, `tenant_quota_exceeded`, a kind this build does not know, or
/// none — leaves it `Ambiguous`, so the new key's membership decides.
const REFUSED_BEFORE_APPLYING: &[&str] = &[
    "not_member",
    "role_forbidden",
    "transport_forbidden",
    "kb_not_found",
    "invalid_params",
    "update_too_large",
    "request_too_large",
    "signature_invalid",
    "signature_required",
    "not_author",
    "history_holds_plaintext",
];

fn refused_before_applying(err: &serde_json::Value) -> bool {
    err.pointer("/data/kind")
        .and_then(|k| k.as_str())
        .is_some_and(|k| REFUSED_BEFORE_APPLYING.contains(&k))
}

impl PendingRotation {
    fn open_kbs(&self) -> Vec<String> {
        self.record
            .kbs
            .iter()
            .filter(|k| matches!(k.outcome, Outcome::Unknown | Outcome::Ambiguous))
            .map(|k| k.kb_id.clone())
            .collect()
    }
}

/// Resume a record found at start-up for the identity `current`.
fn resume(mut rec: Record, current: &Identity, dir: &Path) -> Option<PendingRotation> {
    let Some(new_id) = rec.new_identity() else {
        warn!(path = %record::path(dir).display(), "rotate-identity: the pending-rotation record does not hold the key it names; left in place, not used");
        return None;
    };
    let cur = current.fingerprint();
    let mut outbox = Vec::new();
    if cur == rec.new_fp {
        // The identity file already holds the new key: the rotation committed, and only
        // the re-wraps of the KBs that accepted it can be left.
        rec.kbs.retain(|k| k.outcome == Outcome::Applied);
        outbox = successor_ops(&rec.kbs);
        if outbox.is_empty() {
            if let Err(e) = record::remove(dir) {
                warn!(error = %e, "rotate-identity: could not remove a completed rotation's record");
            }
            return None;
        }
        rec.switched = true;
    } else if cur == rec.old_fp {
        // Not committed: whatever was in flight is resolved on the next connection.
        rec.switched = false;
        info!(new_fp = %rec.new_fp, "rotate-identity: a rotation is pending from an earlier run; resolving it on connect");
    } else {
        warn!(path = %record::path(dir).display(), "rotate-identity: a pending-rotation record for another identity; left in place, not used");
        return None;
    }
    Some(PendingRotation {
        record: rec,
        new_id: Arc::new(new_id),
        awaiting: HashMap::new(),
        sent_gen: 0,
        retry_at: None,
        outbox,
        commit_reported: false,
    })
}

fn successor_ops(kbs: &[KbEntry]) -> Vec<(String, Vec<u8>)> {
    kbs.iter()
        .flat_map(|k| {
            k.on_successor.iter().filter_map(move |b| {
                mae_sync::encoding::base64_to_update(b)
                    .ok()
                    .map(|d| (k.kb_id.clone(), d))
            })
        })
        .collect()
}

fn persist(dir: &Path, rec: &Record) {
    if let Err(e) = record::save(dir, rec) {
        warn!(error = %e, "rotate-identity: could not update the pending-rotation record");
    }
}

/// Start a rotation to `new_id` over `plans`: record it durably FIRST. `Err` (nothing sent,
/// nothing changed) when there is no directory to keep the new key in or the write fails.
pub(super) fn begin(
    slot: &mut Slot,
    old_fp: &str,
    new_id: Arc<Identity>,
    plans: Vec<RotationPlan>,
    conn_gen: u64,
) -> Result<(), String> {
    if slot.pending.is_some() {
        return Err("a rotation is already pending".to_string());
    }
    let dir = slot
        .dir
        .clone()
        .ok_or_else(|| "there is no data directory to keep the new key in".to_string())?;
    // A record this run is not driving — one start-up could not use, or another editor's on
    // the same directory — may be the only copy of a key a daemon accepted. Never write over
    // it, and never show what is in it.
    if record::present(&dir) {
        return Err(format!(
            "a pending-rotation record is already on disk at {}; it may hold the only copy of a key a daemon accepted, so resolve it (restart MAE with the identity it names, or move it aside once you know no daemon accepted its key) before rotating again",
            record::path(&dir).display()
        ));
    }
    let b64 = |d: &Vec<u8>| mae_sync::encoding::update_to_base64(d);
    let kbs = plans
        .into_iter()
        .map(|p| KbEntry {
            rebind: p.deltas.iter().map(b64).collect(),
            on_successor: p.on_successor.iter().map(b64).collect(),
            kb_id: p.kb_id,
            outcome: Outcome::Unknown,
            reason: None,
        })
        .collect();
    let record = Record::new(old_fp, &new_id, kbs);
    record::create(&dir, &record).map_err(|e| format!("the new key could not be saved ({e})"))?;
    slot.pending = Some(PendingRotation {
        record,
        new_id,
        awaiting: HashMap::new(),
        sent_gen: conn_gen,
        retry_at: None,
        outbox: Vec::new(),
        commit_reported: false,
    });
    Ok(())
}

/// Ship every `Unknown` KB's `Rebind` on `w`, the old key's session. A KB whose write fails
/// stays `Unknown` (its bytes may still have reached the daemon). Returns how many KBs were
/// not written; writing stops at the first failure.
async fn send_unknown<W: tokio::io::AsyncWrite + Unpin>(
    rot: &mut PendingRotation,
    w: &mut W,
    next_id: &mut u64,
    timeout: Duration,
    resend: bool,
) -> usize {
    let mut failed = 0;
    for (idx, e) in rot.record.kbs.iter().enumerate() {
        if e.outcome != Outcome::Unknown {
            continue;
        }
        if failed > 0 {
            failed += 1;
            continue;
        }
        let mut ids = Vec::new();
        for b64 in &e.rebind {
            let id = *next_id;
            *next_id += 1;
            let req = mae_sync::wire::kb_collection_op_request(id, &e.kb_id, b64);
            let sent = match serde_json::to_vec(&req) {
                Ok(body) => mae_mcp::write_framed(w, &body, timeout).await.is_ok(),
                Err(_) => false,
            };
            if !sent {
                ids.clear();
                failed += 1;
                break;
            }
            ids.push(id);
        }
        for id in ids {
            rot.awaiting.insert(id, (idx, resend));
        }
    }
    failed
}

/// The first send of a rotation [`begin`] recorded.
pub(super) async fn ship<W: tokio::io::AsyncWrite + Unpin>(
    slot: &mut Slot,
    w: &mut W,
    next_id: &mut u64,
    timeout: Duration,
    evt_tx: &mpsc::Sender<CollabEvent>,
) {
    let Some(rot) = slot.pending.as_mut() else {
        return;
    };
    let failed = send_unknown(rot, w, next_id, timeout, false).await;
    if failed > 0 {
        warn!(failed, new_fp = %rot.record.new_fp, "rotate-identity: Rebind write failed; outcome unknown, kept pending");
        try_send_evt(
            evt_tx,
            CollabEvent::Error {
                message: format!(
                    "Identity rotation not confirmed — the Rebind for {failed} KB(s) could not be written to the daemon. Your identity was NOT changed; the new key {} is kept as a pending rotation and MAE finds out what the daemon did when it reconnects.",
                    rot.record.new_fp
                ),
            },
        );
    }
}

/// What [`drive`] may use of the network loop's state.
pub(super) struct Net<'a> {
    pub connected: bool,
    /// Bumped on every new connection.
    pub conn_gen: u64,
    pub writer: Option<&'a mut BoxWriter>,
    pub addr: Option<&'a str>,
    pub next_id: &'a mut u64,
    pub timeout: Duration,
    /// How long to wait before asking the new key again.
    pub retry: Duration,
}

/// Move a pending rotation on, once per pass of the network loop: note a lost connection,
/// re-send on a new one, ask the new key about what the re-send could not settle, and switch
/// the session once every KB is settled. `true`: the session's identity is now the new key,
/// and the caller must drop the old key's connection and reconnect.
pub(super) async fn drive(
    transport: &mut ClientTransport,
    net: Net<'_>,
    kb_collections: &mut HashMap<String, Vec<u8>>,
    evt_tx: &mpsc::Sender<CollabEvent>,
) -> bool {
    let Some(slot) = transport.rotation_mut() else {
        return false;
    };
    let Some(dir) = slot.dir.clone() else {
        return false;
    };
    let Some(mut rot) = slot.pending.take() else {
        return false;
    };
    if rot.record.switched {
        put_back(transport, rot);
        return false;
    }
    if !net.connected && !rot.awaiting.is_empty() {
        interrupted(&rot, evt_tx);
        rot.awaiting.clear();
        rot.retry_at = None;
    }
    if net.connected && rot.sent_gen != net.conn_gen && rot.record.any(Outcome::Unknown) {
        if let Some(w) = net.writer {
            rot.sent_gen = net.conn_gen;
            send_unknown(&mut rot, w, net.next_id, net.timeout, true).await;
        }
    }
    let due = rot.retry_at.is_none_or(|t| Instant::now() >= t);
    let open = rot.open_kbs();
    if rot.awaiting.is_empty() && due && !open.is_empty() {
        if let (Some(addr), Some(as_new)) = (net.addr, transport.probe_transport(&rot.new_id)) {
            let found = probe(addr, &as_new, &open, net.timeout).await;
            settle_by_membership(&mut rot, &found);
            rot.retry_at = Some(Instant::now() + net.retry);
            persist(&dir, &rot.record);
        }
    }
    if !rot.awaiting.is_empty() || !rot.open_kbs().is_empty() {
        put_back(transport, rot);
        return false;
    }
    finish(rot, transport, &dir, kb_collections, evt_tx)
}

fn put_back(transport: &mut ClientTransport, rot: PendingRotation) {
    if let Some(slot) = transport.rotation_mut() {
        slot.pending = Some(rot);
    }
}

/// What the new key's membership says. A member: the daemon holds the `Rebind` (nothing
/// else can have admitted a key generated for this rotation). Not a member: final only for
/// an `Ambiguous` KB, whose refused re-send came after the original; an `Unknown` KB's
/// original may still be in flight.
fn settle_by_membership(rot: &mut PendingRotation, found: &HashMap<String, bool>) {
    for e in &mut rot.record.kbs {
        match (found.get(&e.kb_id), e.outcome) {
            (Some(true), Outcome::Unknown | Outcome::Ambiguous) => e.outcome = Outcome::Applied,
            (Some(false), Outcome::Ambiguous) => {
                e.outcome = Outcome::Refused;
                e.reason = Some(
                    "the daemon refused the re-sent Rebind and does not count the new key as a member"
                        .to_string(),
                );
            }
            _ => {}
        }
    }
}

/// Ask the daemon, as the new key, which of `kbs` it is a member of. Absent from the answer:
/// unknown (no connection, the new key not authorized yet, any other refusal).
async fn probe(
    addr: &str,
    as_new: &ClientTransport,
    kbs: &[String],
    timeout: Duration,
) -> HashMap<String, bool> {
    let mut found = HashMap::new();
    let _ = tokio::time::timeout(
        timeout * 2,
        probe_into(addr, as_new, kbs, timeout, &mut found),
    )
    .await;
    found
}

async fn probe_into(
    addr: &str,
    as_new: &ClientTransport,
    kbs: &[String],
    timeout: Duration,
    found: &mut HashMap<String, bool>,
) -> Option<()> {
    let (mut r, mut w) = establish_connection(addr, as_new).await.ok()?;
    send_initialize(&mut w, &mut r, timeout).await?;
    let mut ids = HashMap::new();
    for (i, kb) in kbs.iter().enumerate() {
        let id = i as u64 + 1;
        let req = serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "kb/query.capabilities",
            "params": { "kb_id": kb },
        });
        let body = serde_json::to_vec(&req).ok()?;
        mae_mcp::write_framed(&mut w, &body, timeout).await.ok()?;
        ids.insert(id, kb.as_str());
    }
    while !ids.is_empty() {
        let text = mae_mcp::read_message_limited(&mut r, mae_mcp::MAX_DAEMON_RESPONSE_SIZE)
            .await
            .ok()??;
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(kb) = v
            .get("id")
            .and_then(|i| i.as_u64())
            .and_then(|i| ids.remove(&i))
        else {
            continue;
        };
        if v.get("result").is_some() {
            found.insert(kb.to_string(), true);
        } else if v.pointer("/error/data/kind").and_then(|k| k.as_str()) == Some("not_member") {
            found.insert(kb.to_string(), false);
        }
    }
    Some(())
}

/// The connection dropped with `Rebind`s unanswered. Nothing moves, and nothing is
/// forgotten: the record keeps the new key until the daemon's verdict is known.
fn interrupted(rot: &PendingRotation, evt_tx: &mpsc::Sender<CollabEvent>) {
    warn!(
        unanswered = rot.awaiting.len(),
        new_fp = %rot.record.new_fp,
        "rotate-identity: connection lost before the daemon answered; kept pending"
    );
    try_send_evt(
        evt_tx,
        CollabEvent::Error {
            message: format!(
                "Identity rotation interrupted — the connection dropped before the daemon answered {} Rebind(s). Your identity was NOT changed yet. The new key {} is kept as a pending rotation: MAE finds out what the daemon did when it reconnects, and adopts the key only for KBs that accepted it.",
                rot.awaiting.len(),
                rot.record.new_fp
            ),
        },
    );
}

/// Every KB is settled. If none accepted, drop the record: nothing changed. Otherwise adopt
/// the accepted KBs' rotated replicas, commit the new key to the identity file, and make it
/// the session's identity; `true` tells the caller to reconnect.
fn finish(
    mut rot: PendingRotation,
    transport: &mut ClientTransport,
    dir: &Path,
    kb_collections: &mut HashMap<String, Vec<u8>>,
    evt_tx: &mpsc::Sender<CollabEvent>,
) -> bool {
    let refused: Vec<String> = rot
        .record
        .kbs
        .iter()
        .filter(|k| k.outcome == Outcome::Refused)
        .map(|k| format!("{} ({})", k.kb_id, k.reason.as_deref().unwrap_or("refused")))
        .collect();
    let applied: Vec<KbEntry> = rot
        .record
        .kbs
        .iter()
        .filter(|k| k.outcome == Outcome::Applied)
        .cloned()
        .collect();
    if applied.is_empty() && !rot.record.kbs.is_empty() {
        if let Err(e) = record::remove(dir) {
            warn!(error = %e, "rotate-identity: could not remove a refused rotation's record");
        }
        warn!(refused = ?refused, "rotate-identity: the daemon accepted no Rebind — identity NOT rotated");
        try_send_evt(
            evt_tx,
            CollabEvent::Error {
                message: format!(
                    "Identity rotation failed — the daemon accepted no Rebind (refused: {}). Your identity was NOT changed.",
                    refused.join(", ")
                ),
            },
        );
        return false;
    }
    // Adopt: later ops chain on the rotated state, and the new key derives its content key.
    for k in &applied {
        let deltas = k.rebind.iter().chain(&k.on_successor);
        let decoded: Vec<Vec<u8>> = deltas
            .filter_map(|b| mae_sync::encoding::base64_to_update(b).ok())
            .collect();
        let Some(mut coll) = kb_collections
            .get(&k.kb_id)
            .and_then(|b| mae_sync::kb::KbCollectionDoc::from_bytes(b).ok())
        else {
            warn!(kb = %k.kb_id, "rotate-identity: no local replica to adopt the rotation into");
            continue;
        };
        for d in &decoded {
            let _ = coll.apply_update(d);
        }
        let state = coll.encode_state();
        // B2: persist so the rotated op-log survives a restart.
        persist_collection(&k.kb_id, &state);
        kb_collections.insert(k.kb_id.clone(), state);
    }
    // Commit: from here the identity file holds the new key.
    if let Err(e) = rot.new_id.save(dir) {
        warn!(error = %e, "rotate-identity: could not save the new key; still pending");
        if !rot.commit_reported {
            rot.commit_reported = true;
            try_send_evt(
                evt_tx,
                CollabEvent::Error {
                    message: format!("Identity rotation accepted by the daemon, but the new key could not be saved ({e}). It stays pending and MAE keeps retrying; do not delete {}.", record::path(dir).display()),
                },
            );
        }
        put_back(transport, rot);
        return false;
    }
    rot.record.switched = true;
    rot.record.kbs = applied
        .iter()
        .filter(|k| !k.on_successor.is_empty())
        .cloned()
        .collect();
    rot.outbox = successor_ops(&rot.record.kbs);
    if rot.outbox.is_empty() {
        if let Err(e) = record::remove(dir) {
            warn!(error = %e, "rotate-identity: could not remove a completed rotation's record");
        }
    } else {
        persist(dir, &rot.record);
    }
    let new_fp = rot.record.new_fp.clone();
    transport.rekey(rot.new_id.clone());
    if !rot.outbox.is_empty() {
        put_back(transport, rot);
    }
    info!(rotated = applied.len(), refused = refused.len(), new_fp = %new_fp, "rotate-identity: rotation shipped, and committed to the new key");
    let mut lines = vec![
        format!("Identity rotated across {} KB(s).", applied.len()),
        format!("New fingerprint: {new_fp}"),
        "Reconnecting under the new key. The daemon refuses it until its operator authorizes it (`mae-daemon authorize`); the reconnect retries until then.".to_string(),
        // The new key has a new ADR-023 client-id, so the FIRST edit to an existing node
        // after rotation trips the epoch fence once and must rebase.
        "Your first edit to an existing node may rebase once (the rotated key has a new write lineage) — set `collab-fence-resolution` to `auto` to handle it silently.".to_string(),
    ];
    if !refused.is_empty() {
        lines.push(format!(
            "WARNING: rotation did not reach {} KB(s) — still on the OLD key there: {}",
            refused.len(),
            refused.join(", ")
        ));
    }
    try_send_evt(evt_tx, CollabEvent::StatusReport { lines });
    true
}

/// Send what only the new key's session may send, once it is the session. A write that
/// fails leaves it, and everything after it, queued for the next connection; once nothing
/// is left the rotation is complete and its record is removed.
pub(super) async fn flush_outbox<W: tokio::io::AsyncWrite + Unpin>(
    slot: &mut Slot,
    w: &mut W,
    next_id: &mut u64,
    pending: &mut std::collections::HashMap<u64, PendingResponseKind>,
    timeout: Duration,
) {
    let Some(rot) = slot.pending.as_mut().filter(|r| r.record.switched) else {
        return;
    };
    while let Some((kb_id, delta)) = rot.outbox.first() {
        let req_id = *next_id;
        *next_id += 1;
        let req = mae_sync::wire::kb_collection_op_request(
            req_id,
            kb_id,
            &mae_sync::encoding::update_to_base64(delta),
        );
        let Ok(body) = serde_json::to_vec(&req) else {
            rot.outbox.remove(0);
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
        rot.outbox.remove(0);
    }
    if let Some(dir) = &slot.dir {
        if let Err(e) = record::remove(dir) {
            warn!(error = %e, "rotate-identity: could not remove a completed rotation's record");
        }
    }
    slot.pending = None;
}
