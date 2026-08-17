//! ADR-105 Stage 3 (D4): KB identity — a KB syncs under a minted collab id, not
//! its display name.
//!
//! Split from `collab_bridge_kb_sync_tests.rs` to stay under the structural
//! ceiling. Uses that module's `share_request` helper, which performs the REQUEST
//! half of a share and returns the id the confirmation will carry.

use super::collab_bridge_kb_sync_tests::share_request;
use super::*;

/// ADR-105 finding F, the bug Stage 3 exists to fix: **two editors must both be
/// able to share their primary KB.**
///
/// Every editor's primary is called "default". While that name was also its
/// collab id, the first tenant to connect to a shared daemon claimed `kbc:default`
/// permanently — `kb/unregister` removes only metadata, so the collection survives
/// — and every later tenant's `kb/share` was accepted and then denied on every
/// subsequent operation. A KB that looks shared and does nothing.
///
/// The oracle is the ids themselves, not a status message: two editors, each
/// sharing the KB they both call "default", must present DIFFERENT ids to the
/// daemon. Asserted per-editor with independent data dirs, because the whole
/// point is that neither knows about the other.
#[test]
fn two_editors_can_both_share_their_primary_kb() {
    fn editor_sharing_primary(tag: &str) -> (String, std::path::PathBuf) {
        let tmp = std::env::temp_dir().join(format!(
            "mae-adr105-f-{}-{}-{}",
            tag,
            std::process::id(),
            // Distinct per editor so neither reads the other's registry — two
            // tenants on one daemon are two machines, not one shared data dir.
            mae_kb::federation::generate_uuid()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let mut editor = Editor::new();
        editor.data_dir_override = Some(tmp.clone());
        editor.kb.primary.insert(mae_kb::Node::new(
            // The SAME node id in both KBs (H5): two tenants picking
            // `concept:architecture` is ordinary, and a test using distinct ids
            // is structurally unable to observe a collision.
            "concept:architecture",
            "Architecture",
            mae_kb::NodeKind::Note,
            format!("{tag}-body"),
        ));
        let kb_id = share_request(&mut editor, "default");
        handle_collab_event(
            &mut editor,
            CollabEvent::KbShared {
                kb_id: kb_id.clone(),
                node_count: 1,
                collection_state: Vec::new(),
            },
        );
        assert!(
            editor.kb.registry.primary_shared,
            "{tag}: the share must stamp the durable primary marker"
        );
        assert_eq!(
            editor.kb.registry.primary_collab_id.as_deref(),
            Some(kb_id.as_str()),
            "{tag}: the stamped id must be the one the share used"
        );
        assert!(
            !editor.collab.shared_kbs[&kb_id].is_empty(),
            "{tag}: an empty node set means later edits match nothing and sync \
             stops silently (I-9/ADR-086)"
        );
        (kb_id, tmp)
    }

    let (alice, alice_dir) = editor_sharing_primary("alice");
    let (bob, bob_dir) = editor_sharing_primary("bob");

    assert_ne!(
        alice, bob,
        "two editors' primaries claimed the SAME collab id — finding F. On a \
         shared daemon the second tenant's KB is accepted and then denied on \
         every operation."
    );
    for id in [&alice, &bob] {
        assert!(
            !mae_kb::PRIMARY_NAME_ALIASES.contains(&id.as_str()),
            "a primary must not sync under its display name: {id}"
        );
        assert!(
            mae_sync::kb_id_is_addressable(id),
            "the id becomes part of every node's document address (D3): {id}"
        );
    }

    let _ = std::fs::remove_dir_all(&alice_dir);
    let _ = std::fs::remove_dir_all(&bob_dir);
}

/// ADR-105 D4, and the sharpest edge Stage 3 exposed: enabling E2E on a shared
/// primary must carry that KB's nodes so they get RE-SEALED (#171).
///
/// `KbSetEncryption` carries a collab *id*, but `kb_share_node_states` resolved a
/// *name*. Those were the same string for the primary right up until D4 minted
/// real ids — at which point the lookup silently returned an EMPTY list. The
/// failure is not a missing feature: the KB would be marked encrypted while every
/// node's content stayed plaintext, which is #573's shape (a KB that reports E2E
/// and is not) reached from the other direction.
///
/// The oracle is the node states on the wire, because that is the thing whose
/// emptiness is invisible: the command is still produced, the KB still flips to
/// e2e, and nothing anywhere reports that zero nodes were sealed.
#[test]
fn enabling_encryption_on_a_shared_primary_carries_its_nodes_for_resealing() {
    let tmp = std::env::temp_dir().join(format!("mae-adr105-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let mut editor = Editor::new();
    editor.data_dir_override = Some(tmp.clone());
    editor.kb.primary.insert(mae_kb::Node::new(
        "concept:architecture",
        "Architecture",
        mae_kb::NodeKind::Note,
        "secret body",
    ));

    let kb_id = share_request(&mut editor, "default");
    handle_collab_event(
        &mut editor,
        CollabEvent::KbShared {
            kb_id: kb_id.clone(),
            node_count: 1,
            collection_state: Vec::new(),
        },
    );

    let cmd = crate::collab_bridge::events_kb::kb_intent_to_command(
        &mut editor,
        CollabIntent::KbSetEncryption {
            kb_id: kb_id.clone(),
            mode: "e2e".to_string(),
        },
    )
    .expect("set-encryption on a shared KB must produce a command");

    match cmd {
        CollabCommand::KbSetEncryption { node_states, .. } => assert!(
            !node_states.is_empty(),
            "no node states carried, so E2E would be enabled with NOTHING re-sealed \
             — the KB reports encrypted while its content stays plaintext"
        ),
        other => panic!("expected KbSetEncryption, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

/// The control: an id this editor does not own must be REFUSED, not silently
/// treated as "a KB with no nodes". Both produce an empty re-seal set; only one
/// of them tells the user.
#[test]
fn enabling_encryption_on_an_unknown_kb_is_refused() {
    let mut editor = Editor::new();
    let cmd = crate::collab_bridge::events_kb::kb_intent_to_command(
        &mut editor,
        CollabIntent::KbSetEncryption {
            kb_id: "not-a-kb-of-mine".to_string(),
            mode: "e2e".to_string(),
        },
    );
    assert!(
        cmd.is_none(),
        "an unresolvable KB must not produce a set-encryption command — it would \
         enable E2E with an empty re-seal set"
    );
}

/// ADR-105 D4/D5: a KB id that was minted but never confirmed must be
/// RECOVERABLE, not permanent.
///
/// This is the root cause behind "what if two uuids collide", and randomness
/// alone does not touch it. `collab_id_for_share` deliberately returns an
/// existing id unchanged — changing a live KB's id destroys its signed
/// membership (finding A) — so a refused id is re-presented on every retry
/// forever and the KB is unshareable until someone hand-edits
/// `kb-registry.toml`. The same trap catches a restored or copied registry, and
/// a client supplying an id it did not mint, both likelier than a clock
/// collision.
#[test]
fn a_refused_share_id_is_reminted_so_the_kb_stays_shareable() {
    let tmp = std::env::temp_dir().join(format!("mae-adr105-remint-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let mut editor = Editor::new();
    editor.data_dir_override = Some(tmp.clone());
    editor.kb.primary.insert(mae_kb::Node::new(
        "concept:architecture",
        "Architecture",
        mae_kb::NodeKind::Note,
        "body",
    ));

    let taken = share_request(&mut editor, "default");
    assert_eq!(
        editor.kb.registry.primary_collab_id.as_deref(),
        Some(taken.as_str()),
        "precondition: the minted id is persisted before confirmation"
    );
    assert!(
        !editor.kb.registry.primary_shared,
        "precondition: nothing is confirmed yet — that is what makes re-minting safe"
    );

    handle_collab_event(
        &mut editor,
        CollabEvent::KbShareIdConflict {
            kb_id: taken.clone(),
            detail: "already shared by a different owner".into(),
        },
    );

    let fresh = editor
        .kb
        .registry
        .primary_collab_id
        .clone()
        .expect("the primary must still have an id");
    assert_ne!(
        fresh, taken,
        "the refused id was kept, so every retry presents the same taken id and the \
         KB can never be shared"
    );

    // Recovery is not just a new id — the share must actually be re-issued, or the
    // user is left with a silently different id and no share.
    assert!(
        matches!(
            editor.collab.pending_intent,
            Some(CollabIntent::ShareKb { .. })
        ),
        "a re-mint must re-issue the share, got: {:?}",
        editor.collab.pending_intent
    );

    // And the retry must actually present the NEW id.
    let retried = share_request(&mut editor, "default");
    assert_eq!(
        retried, fresh,
        "the retry must go out under the re-minted id"
    );

    // Persisted, not just in memory: a re-mint lost on restart re-poisons the KB.
    let reloaded = mae_kb::federation::KbRegistry::load(&tmp);
    assert_eq!(reloaded.primary_collab_id.as_deref(), Some(fresh.as_str()));

    let _ = std::fs::remove_dir_all(&tmp);
}

/// The control, and the more dangerous half: a KB with a CONFIRMED share must
/// NEVER be re-minted, however the daemon answers.
///
/// "Owned by another" on a KB we genuinely own is not a collision — we are
/// pointed at the wrong daemon, or the id was taken over. Re-minting there would
/// destroy the KB's signed membership and make an E2E KB read as plaintext
/// (finding A), which is far worse than a failed share. The confirmed-share
/// marker is the discriminator, because it is stamped only on a confirmed share.
#[test]
fn a_confirmed_kbs_id_is_never_reminted_even_when_the_daemon_disowns_it() {
    let tmp = std::env::temp_dir().join(format!("mae-adr105-noremint-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let mut editor = Editor::new();
    editor.data_dir_override = Some(tmp.clone());
    editor.kb.primary.insert(mae_kb::Node::new(
        "concept:architecture",
        "Architecture",
        mae_kb::NodeKind::Note,
        "body",
    ));

    let kb_id = share_request(&mut editor, "default");
    handle_collab_event(
        &mut editor,
        CollabEvent::KbShared {
            kb_id: kb_id.clone(),
            node_count: 1,
            collection_state: Vec::new(),
        },
    );
    assert!(
        editor.kb.registry.primary_shared,
        "precondition: the share is confirmed, so this KB's id is signature-bound"
    );

    handle_collab_event(
        &mut editor,
        CollabEvent::KbShareIdConflict {
            kb_id: kb_id.clone(),
            detail: "already shared by a different owner".into(),
        },
    );

    assert_eq!(
        editor.kb.registry.primary_collab_id.as_deref(),
        Some(kb_id.as_str()),
        "a confirmed KB's id was re-minted — that destroys its membership and reads \
         an E2E KB as plaintext (finding A)"
    );
    assert!(
        editor.collab.pending_intent.is_none(),
        "no silent re-share: this state needs a human, not a retry"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// A re-mint must preserve HOST-ONLY sharing (ADR-029 Phase D + ADR-105 D4/D5).
///
/// `daemon_host_pending` is keyed by the id a share went out under. Re-issuing
/// under a new id without re-keying leaves the confirmation unable to match, so
/// the retry is treated as a peer share and stamps the durable `primary_shared`
/// marker — a runtime-only hosting gate leaking into a later daemon-less launch.
/// The same defect the mint path had to avoid, reachable through recovery.
#[test]
fn reminting_a_host_only_share_does_not_turn_it_into_a_peer_share() {
    let tmp = std::env::temp_dir().join(format!("mae-adr105-hostremint-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let mut editor = Editor::new();
    editor.data_dir_override = Some(tmp.clone());
    editor.kb.primary.insert(mae_kb::Node::new(
        "concept:architecture",
        "Architecture",
        mae_kb::NodeKind::Note,
        "body",
    ));

    // A host-only share in flight: enqueued under an id, marked host-pending.
    let taken = share_request(&mut editor, "default");
    editor.collab.daemon_host_pending.insert(taken.clone());

    handle_collab_event(
        &mut editor,
        CollabEvent::KbShareIdConflict {
            kb_id: taken.clone(),
            detail: "already shared by a different owner".into(),
        },
    );
    let fresh = editor.kb.registry.primary_collab_id.clone().unwrap();
    assert_ne!(fresh, taken, "precondition: the id was re-minted");
    assert!(
        editor.collab.daemon_host_pending.contains(&fresh),
        "the host-only marker did not follow the re-mint"
    );

    // The oracle is the outcome, not the marker: confirming the retry must NOT
    // stamp the durable peer-share marker.
    handle_collab_event(
        &mut editor,
        CollabEvent::KbShared {
            kb_id: fresh.clone(),
            node_count: 1,
            collection_state: Vec::new(),
        },
    );
    assert!(
        !editor.kb.registry.primary_shared,
        "a re-minted HOST share stamped the durable peer-share marker — hosting is \
         runtime-only and must not survive into a daemon-less launch"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// ADR-105 plan Verification 5: creating a node still works end to end **through
/// the real drain**, and everything it emits is addressed to the KB's minted id.
///
/// Driven through `drain_collab_intents` — the function the event loop actually
/// calls — rather than by inspecting the pending queues, because the queues are
/// where every unit test above already stops. What Stage 3 changed is which id
/// reaches the wire, and the queues are one hop short of the wire.
///
/// Also pins **D7's ordering contract**, which nothing asserted: the manifest add
/// must precede the node update. The plan states it as "node creation is ordered
/// manifest-first", and it drains that way today (`events_kb.rs` takes
/// `pending_kb_manifest` before `pending_kb_updates`) purely as a consequence of
/// statement order — one edit away from silently inverting. D6, the check that
/// would have made an inversion fail loudly, was withdrawn in Stage 2, so an
/// inversion now costs a projection that arrives before its manifest entry and is
/// deferred, which is a *quiet* wrong answer.
#[test]
fn creating_a_node_drains_manifest_then_update_under_the_minted_id() {
    let tmp = std::env::temp_dir().join(format!("mae-adr105-v5-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let mut editor = Editor::new();
    editor.data_dir_override = Some(tmp.clone());
    editor.collab.kb_sync_mode = "on_save".into();

    // A shared primary, through the real request→confirm pair.
    let kb_id = share_request(&mut editor, "default");
    handle_collab_event(
        &mut editor,
        CollabEvent::KbShared {
            kb_id: kb_id.clone(),
            node_count: 0,
            collection_state: Vec::new(),
        },
    );
    assert!(
        !mae_kb::PRIMARY_NAME_ALIASES.contains(&kb_id.as_str()),
        "precondition: a minted id, or this test passes through the old conflation"
    );

    editor.collab.status = CollabStatus::Connected { peer_count: 1 };
    editor
        .kb_create_node(
            "concept:architecture",
            "Architecture",
            "body",
            mae_kb::NodeKind::Note,
        )
        .expect("create must succeed");

    let (tx, mut rx) = mpsc::channel(16);
    drain_collab_intents(&mut editor, &tx);

    let mut drained = Vec::new();
    while let Ok(cmd) = rx.try_recv() {
        drained.push(cmd);
    }
    assert!(
        !drained.is_empty(),
        "creating a node on a shared KB must reach the wire — an empty drain is \
         how sync silently stops"
    );

    let manifest_at = drained.iter().position(|c| {
        matches!(c, CollabCommand::KbCollectionNode { kb_id: k, node_id, add, .. }
                 if k == &kb_id && node_id == "concept:architecture" && *add)
    });
    let update_at = drained.iter().position(|c| {
        matches!(c, CollabCommand::KbNodeUpdate { kb_id: k, node_id, .. }
                 if k == &kb_id && node_id == "concept:architecture")
    });

    let manifest_at = manifest_at
        .unwrap_or_else(|| panic!("no manifest add addressed to '{kb_id}' in {drained:#?}"));
    let update_at = update_at
        .unwrap_or_else(|| panic!("no node update addressed to '{kb_id}' in {drained:#?}"));
    assert!(
        manifest_at < update_at,
        "D7: the manifest add must precede the node update, else the projector sees \
         a node its KB's manifest does not list yet and defers it"
    );

    // Nothing may go out under the display name — that is the whole of D4, and it
    // is checked over EVERY drained command rather than the two matched above, so a
    // third command addressed the old way cannot slip past.
    for cmd in &drained {
        let addressed = match cmd {
            CollabCommand::KbNodeUpdate { kb_id, .. }
            | CollabCommand::KbCollectionNode { kb_id, .. } => Some(kb_id),
            _ => None,
        };
        if let Some(k) = addressed {
            assert_eq!(
                k, &kb_id,
                "a command went out under a different id than the KB's minted one: {cmd:?}"
            );
        }
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

/// The re-mint recovery must be BOUNDED. It re-issues the share, and the
/// re-issued share can be refused again — unbounded, that is a livelock: share →
/// refuse → re-mint → persist → share, a network request and a registry disk
/// write every round, for as long as the daemon keeps saying no.
///
/// With 122-bit random ids a second genuine collision is not the worry. A daemon
/// fault that reports every collection as foreign-owned is, and it drives the
/// identical loop — which is why the ceiling is about the daemon, not the odds.
#[test]
fn repeated_share_refusals_stop_reminting_instead_of_looping_forever() {
    let tmp = std::env::temp_dir().join(format!("mae-adr105-bound-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let mut editor = Editor::new();
    editor.data_dir_override = Some(tmp.clone());

    let mut seen = std::collections::HashSet::new();
    let mut current = share_request(&mut editor, "default");
    seen.insert(current.clone());

    // Refuse every id it offers. A correct implementation gives up; a broken one
    // spins here forever, so the loop is capped well above the real ceiling and
    // asserts it stopped early.
    let mut rounds = 0;
    for _ in 0..25 {
        rounds += 1;
        handle_collab_event(
            &mut editor,
            CollabEvent::KbShareIdConflict {
                kb_id: current.clone(),
                detail: "already shared by a different owner".into(),
            },
        );
        let next = editor.kb.registry.primary_collab_id.clone().unwrap();
        if next == current {
            break; // gave up: the id stopped changing
        }
        assert!(
            seen.insert(next.clone()),
            "a re-mint handed back an id already tried: {next}"
        );
        current = next;
    }

    assert!(
        rounds < 25,
        "the editor never stopped re-minting — that is an unbounded share/refuse \
         loop doing a registry disk write per round"
    );
    assert!(
        editor.collab.pending_intent.is_none(),
        "after giving up there must be no queued retry, or the loop continues via \
         the drain instead"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// The budget is per-episode, not per-lifetime: a confirmed share clears it.
/// Otherwise three unrelated conflicts spread across a long session would exhaust
/// the ceiling and refuse a recovery that would have worked.
#[test]
fn a_successful_share_clears_the_remint_budget() {
    let tmp = std::env::temp_dir().join(format!("mae-adr105-budget-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let mut editor = Editor::new();
    editor.data_dir_override = Some(tmp.clone());

    let first = share_request(&mut editor, "default");
    handle_collab_event(
        &mut editor,
        CollabEvent::KbShareIdConflict {
            kb_id: first,
            detail: "taken".into(),
        },
    );
    assert!(
        !editor.collab.share_id_remint_attempts.is_empty(),
        "precondition: the conflict spent budget"
    );

    let second = editor.kb.registry.primary_collab_id.clone().unwrap();
    handle_collab_event(
        &mut editor,
        CollabEvent::KbShared {
            kb_id: second,
            node_count: 0,
            collection_state: Vec::new(),
        },
    );
    assert!(
        editor.collab.share_id_remint_attempts.is_empty(),
        "a confirmed share must return the budget, or unrelated conflicts later in \
         the session inherit a spent one"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
