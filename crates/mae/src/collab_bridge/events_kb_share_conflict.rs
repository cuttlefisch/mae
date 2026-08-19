//! ADR-105 D4/D5: recovery when `kb/share` is refused because the id belongs to
//! a different owner.
//!
//! Split from `events_kb.rs` to stay under the structural ceiling. Kept as its own
//! module because it is the one place that may CHANGE a KB's collab id, and the
//! rule governing that — safe only while the KB has no confirmed share — is worth
//! having a single, named home rather than a branch inside the share lifecycle.

use super::*;

/// ADR-105 D4/D5: `kb/share` was refused because this id belongs to a different
/// owner. Recover by discarding the id and minting a fresh one.
///
/// Two states reach here and they must be handled oppositely:
///
///   * **The KB has no confirmed share.** Then the id was minted locally and never
///     accepted by anyone, so it is not ours and nothing is bound to it. Re-mint and
///     re-issue the share. Without this the KB is permanently unshareable —
///     `collab_id_for_share` deliberately returns an existing id unchanged, so the
///     refused id is re-presented on every retry forever and the only fix is
///     hand-editing `kb-registry.toml`. This is the path a genuine id collision
///     takes, and also the likelier ones: a restored or copied registry, or an id
///     supplied by a client that did not mint it.
///
///   * **The KB HAS a confirmed share under that id.** Then this is not a collision
///     at all — a KB we own is being reported as someone else's, meaning we are
///     pointed at the wrong daemon or the id was taken over. Re-minting would
///     destroy our own signed membership and make an E2E KB read as plaintext
///     (finding A), so `remint_unconfirmed_collab_id` refuses and we surface it
///     loudly instead. A silent recovery here would be far worse than the failure.
pub(super) fn handle_kb_share_id_conflict(editor: &mut Editor, kb_id: String, detail: String) {
    let Some(target) = editor.kb.registry.target_of_collab_id(&kb_id) else {
        warn!(kb = %kb_id, "share id conflict for a KB this editor does not own");
        editor.set_status(format!("Share refused for unknown KB '{kb_id}': {detail}"));
        return;
    };

    // Bound the recovery. The re-issued share can itself be refused, and without a
    // ceiling that is a livelock: share → refuse → re-mint → persist → share, each
    // round a network request and a registry disk write, for as long as the daemon
    // keeps saying no. A second genuine collision is not the concern with 122-bit
    // random ids; a daemon fault that reports every collection as foreign-owned is,
    // and it drives the identical loop. Keyed by the KB, not the id — the id is
    // what changes each attempt.
    let attempt_key = remint_attempt_key(&target);
    let attempts = editor
        .collab
        .share_id_remint_attempts
        .entry(attempt_key.clone())
        .or_insert(0);
    *attempts += 1;
    if *attempts > MAX_SHARE_ID_REMINTS {
        error!(
            kb = %kb_id, attempts = *attempts,
            "giving up re-minting this KB's collab id; the daemon refuses every id"
        );
        editor.notify(
            mae_core::notifications::Notification::error(
                "collab",
                format!("Could not share KB '{kb_id}' after {MAX_SHARE_ID_REMINTS} new ids"),
            )
            .body(format!(
                "Every id this editor minted was refused as already owned. That is not \
                 an id collision at this point — the daemon is reporting every \
                 collection as belonging to someone else, so check that you are \
                 connected to the right daemon and that its store is intact.\n\n\
                 Daemon said: {detail}"
            ))
            .key(format!("kb-share-remint-exhausted:{attempt_key}")),
        );
        // "Gave up" must mean nothing is still queued. In practice the drain has
        // already consumed the previous round's retry by the time this event
        // arrives, so this is usually a no-op — but relying on that ordering is
        // what would turn a bounded recovery back into an unbounded one the moment
        // an event arrives before its drain. Only a share of THIS KB is cleared;
        // an unrelated queued intent is not ours to discard.
        let queued_for_this_kb = matches!(
            &editor.collab.pending_intent,
            Some(CollabIntent::ShareKb { kb_name, .. })
                if editor.kb.registry.target_of_name(kb_name) == Some(target.clone())
        );
        if queued_for_this_kb {
            editor.collab.pending_intent = None;
        }
        editor.set_status(format!("Gave up sharing '{kb_id}' → *Messages*"));
        return;
    }

    let reminted = match editor.mae_data_dir() {
        Some(dir) => {
            let (registry, id, saved) = mae_kb::federation::KbRegistry::update(&dir, |reg| {
                reg.remint_unconfirmed_collab_id(&target)
            });
            if let Err(e) = saved {
                error!(kb = %kb_id, error = %e, "failed to persist re-minted KB collab id");
                editor.set_status(format!(
                    "Share refused for '{kb_id}' and the new id \
                                           could not be saved: {e}"
                ));
                return;
            }
            editor.kb.registry = registry;
            editor.kb.last_local_registry_write = Some(std::time::Instant::now());
            id
        }
        None => editor.kb.registry.remint_unconfirmed_collab_id(&target),
    };

    let Some(fresh) = reminted else {
        // Confirmed share + "owned by another" — see the doc comment. Loud, and
        // deliberately NOT recovered from.
        error!(
            kb = %kb_id,
            "a KB with a CONFIRMED share is reported as owned by another principal; \
             refusing to re-mint (that would destroy its signed membership)"
        );
        editor.notify(
            mae_core::notifications::Notification::error(
                "collab",
                format!("KB '{kb_id}' is reported as owned by someone else"),
            )
            .body(format!(
                "This KB has a confirmed share under that id, so this is not an id \
                 collision — you may be connected to a different daemon than the one \
                 it was shared with. Its id was NOT changed: re-minting would destroy \
                 the KB's membership and its encryption.\n\nDaemon said: {detail}"
            ))
            .key(format!("kb-share-conflict:{kb_id}")),
        );
        editor.set_status(format!(
            "KB '{kb_id}' is owned by another peer → *Messages*"
        ));
        return;
    };

    // Carry the host-only marker across the re-mint. A daemon-host share
    // (ADR-029 Phase D) is keyed in `daemon_host_pending` by the id it went out
    // under; re-issuing under a NEW id without re-keying leaves the confirmation
    // unable to match, so the retry would be treated as a peer share and stamp the
    // durable `primary_shared` marker hosting must never stamp — a runtime-only
    // gate leaking into a later daemon-less launch. Same defect the mint itself had
    // to avoid, reachable here by a second route.
    let was_host_share = editor.collab.daemon_host_pending.remove(&kb_id);
    if was_host_share {
        editor.collab.daemon_host_pending.insert(fresh.clone());
    }

    // Retry under the fresh id. The intent carries the KB's NAME, which
    // `kb_intent_to_command` resolves back to this same target and which then picks
    // up the id just minted.
    let kb_name = match &target {
        mae_kb::KbTarget::Primary => mae_core::KB_DEFAULT_NAME.to_string(),
        mae_kb::KbTarget::Instance(uuid) => editor
            .kb
            .registry
            .find_by_uuid(uuid)
            .map(|i| i.name.clone())
            .unwrap_or_else(|| uuid.clone()),
    };
    info!(old = %kb_id, new = %fresh, kb = %kb_name, "re-minted KB collab id after a share conflict");
    editor.collab.pending_intent = Some(CollabIntent::ShareKb {
        kb_name: kb_name.clone(),
        node_ids: vec![],
    });
    editor.set_status(format!(
        "KB id was already taken — retrying share of '{kb_name}' under a new id"
    ));
}

/// How many fresh ids to try before concluding the daemon, not the id, is the
/// problem. Three is enough that a genuine collision (already vanishingly
/// unlikely with 122-bit random ids) never exhausts it, and small enough that a
/// misbehaving daemon costs three registry writes rather than an unbounded loop.
const MAX_SHARE_ID_REMINTS: u8 = 3;

/// A stable key for `share_id_remint_attempts`.
///
/// Deliberately NOT the collab id: that is the value being replaced on every
/// attempt, so keying by it would give each retry a fresh counter and the bound
/// above would never be reached — a cap that silently caps nothing.
pub(super) fn remint_attempt_key(target: &mae_kb::KbTarget) -> String {
    match target {
        mae_kb::KbTarget::Primary => "primary".to_string(),
        mae_kb::KbTarget::Instance(uuid) => format!("instance:{uuid}"),
    }
}
