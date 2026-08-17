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
