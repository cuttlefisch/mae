//! `mae-daemon kb member` (the operator's bridge until ADR-098): membership of
//! a KB this daemon owns, through the same write path as `kb/add_member`,
//! signed as the daemon. A real seeded KB; reads checked through `kb/query`,
//! the surface a member actually uses.

use mae_mcp::identity::Identity;
use mae_sync::kb::Role;
use serde_json::json;

use super::apply_tests::{fixture_files, fixture_repo, seeded, KB};
use crate::collab_handler::{operator_change_membership, operator_list_members, OperatorMemberOp};
use crate::doc_store::DocStore;
use crate::kb_query::{self, KbQueryLimits};

const NODE: &str = "0a000000-0000-4000-8000-000000000001";

async fn reads(store: &DocStore, who: &Identity) -> bool {
    let params = json!({"kb_id": KB, "node_id": NODE});
    let fp = who.fingerprint();
    kb_query::dispatch(
        "kb/query.get",
        &params,
        store,
        Some(&fp),
        KbQueryLimits::default(),
    )
    .await
    .is_ok()
}

/// Add as viewer, then remove: the member reads exactly while a member, the
/// change is in the signed op-log both times, and `list` shows it. Kills: a
/// change that never reaches the roles, or a removal that does not revoke.
#[tokio::test]
async fn a_member_reads_exactly_while_a_member() {
    let r = fixture_repo(&fixture_files());
    let (store, daemon, _) = seeded(&r).await;
    let editor_a = Identity::generate("editor-a");
    let fp = editor_a.fingerprint();
    assert!(!reads(&store, &editor_a).await, "not yet a member");

    let add = OperatorMemberOp::Add {
        member: &fp,
        role: Role::Viewer,
        label: "editor-a",
    };
    let out = operator_change_membership(&store, &daemon, KB, add)
        .await
        .unwrap();
    assert_eq!(out["signed_oplog"], true);
    assert!(reads(&store, &editor_a).await, "a viewer reads");
    let (owner, members) = operator_list_members(&store, KB).await.unwrap();
    assert_eq!(owner, daemon.fingerprint());
    assert!(
        members.contains(&(fp.clone(), "viewer".to_string())),
        "{members:?}"
    );

    let out = operator_change_membership(
        &store,
        &daemon,
        KB,
        OperatorMemberOp::Remove { member: &fp },
    )
    .await
    .unwrap();
    assert_eq!(out["signed_oplog"], true);
    assert!(!reads(&store, &editor_a).await, "removed, no longer reads");
}

/// An add with no label: the shape every refusal case uses.
fn add(member: &str, role: Role) -> OperatorMemberOp<'_> {
    OperatorMemberOp::Add {
        member,
        role,
        label: "",
    }
}

/// Every refusal writes nothing. Kills: acting on a KB another identity owns,
/// transferring or touching ownership, accepting a non-fingerprint member.
#[tokio::test]
async fn refusals_write_nothing() {
    let r = fixture_repo(&fixture_files());
    let (store, daemon, _) = seeded(&r).await;
    let before = operator_list_members(&store, KB).await.unwrap();
    let stranger = Identity::generate("not-the-owner");
    let member = Identity::generate("m").fingerprint();
    let owner_fp = daemon.fingerprint();
    let cases: Vec<(&Identity, OperatorMemberOp<'_>, &str)> = vec![
        (&stranger, add(&member, Role::Viewer), "owned by"),
        (&daemon, add(&member, Role::Owner), "viewer or editor"),
        (&daemon, add(&owner_fp, Role::Editor), "owner"),
        (
            &daemon,
            OperatorMemberOp::Remove { member: &owner_fp },
            "owner",
        ),
        (&daemon, add("alice", Role::Viewer), "fingerprint"),
        (&daemon, add("SHA256:", Role::Viewer), "fingerprint"),
        (&daemon, add("SHA256:abc def", Role::Viewer), "fingerprint"),
        (
            &daemon,
            OperatorMemberOp::Remove { member: &member },
            "not a member",
        ),
    ];
    for (op_id, op, want) in cases {
        let e = operator_change_membership(&store, op_id, KB, op)
            .await
            .unwrap_err();
        assert!(e.contains(want), "{want}: {e}");
    }
    let e = operator_change_membership(&store, &daemon, "no-such-kb", add(&member, Role::Viewer))
        .await
        .unwrap_err();
    assert!(e.contains("no-such-kb"), "{e}");
    assert_eq!(operator_list_members(&store, KB).await.unwrap(), before);
}

/// The recovery the error message promises: re-running an add or a remove
/// completes it (signed op-log included) and changes nothing else. Kills a
/// re-run that fails, demotes, or leaves the member's access wrong.
#[tokio::test]
async fn a_rerun_completes_and_changes_nothing_else() {
    let r = fixture_repo(&fixture_files());
    let (store, daemon, _) = seeded(&r).await;
    let editor_a = Identity::generate("editor-a");
    let fp = editor_a.fingerprint();
    for _ in 0..2 {
        let add = OperatorMemberOp::Add {
            member: &fp,
            role: Role::Editor,
            label: "a",
        };
        let out = operator_change_membership(&store, &daemon, KB, add)
            .await
            .unwrap();
        assert_eq!(out["signed_oplog"], true);
    }
    let (_, members) = operator_list_members(&store, KB).await.unwrap();
    let mine: Vec<_> = members.iter().filter(|(m, _)| m == &fp).collect();
    assert_eq!(
        mine,
        vec![&(fp.clone(), "editor".to_string())],
        "once, as editor"
    );
    assert!(reads(&store, &editor_a).await);
    operator_change_membership(
        &store,
        &daemon,
        KB,
        OperatorMemberOp::Remove { member: &fp },
    )
    .await
    .unwrap();
    let e = operator_change_membership(
        &store,
        &daemon,
        KB,
        OperatorMemberOp::Remove { member: &fp },
    )
    .await
    .unwrap_err();
    assert!(
        e.contains("not a member"),
        "a second remove is refused, not re-signed: {e}"
    );
    assert!(!reads(&store, &editor_a).await);
}

/// Every real fingerprint is accepted, so the shape check never refuses a
/// genuine member (64 generated keys); near-misses are refused.
#[tokio::test]
async fn the_fingerprint_shape_accepts_every_real_key() {
    let r = fixture_repo(&fixture_files());
    let (store, daemon, _) = seeded(&r).await;
    for i in 0..64 {
        let fp = Identity::generate(&format!("k{i}")).fingerprint();
        let add = OperatorMemberOp::Add {
            member: &fp,
            role: Role::Viewer,
            label: "",
        };
        operator_change_membership(&store, &daemon, KB, add)
            .await
            .unwrap_or_else(|e| panic!("{fp}: {e}"));
    }
    let real = Identity::generate("x").fingerprint();
    for bad in [
        format!("{real}A"),
        real[..real.len() - 1].to_string(),
        real.replace('S', "s"),
    ] {
        let add = OperatorMemberOp::Add {
            member: &bad,
            role: Role::Viewer,
            label: "",
        };
        assert!(
            operator_change_membership(&store, &daemon, KB, add)
                .await
                .is_err(),
            "{bad}"
        );
    }
}
