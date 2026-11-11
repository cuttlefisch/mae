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
        (
            &stranger,
            OperatorMemberOp::Add {
                member: &member,
                role: Role::Viewer,
                label: "",
            },
            "owned by",
        ),
        (
            &daemon,
            OperatorMemberOp::Add {
                member: &member,
                role: Role::Owner,
                label: "",
            },
            "viewer or editor",
        ),
        (
            &daemon,
            OperatorMemberOp::Add {
                member: &owner_fp,
                role: Role::Editor,
                label: "",
            },
            "owner",
        ),
        (
            &daemon,
            OperatorMemberOp::Remove { member: &owner_fp },
            "owner",
        ),
        (
            &daemon,
            OperatorMemberOp::Add {
                member: "alice",
                role: Role::Viewer,
                label: "",
            },
            "fingerprint",
        ),
    ];
    for (op_id, op, want) in cases {
        let e = operator_change_membership(&store, op_id, KB, op)
            .await
            .unwrap_err();
        assert!(e.contains(want), "{want}: {e}");
    }
    let e = operator_change_membership(
        &store,
        &daemon,
        "no-such-kb",
        OperatorMemberOp::Add {
            member: &member,
            role: Role::Viewer,
            label: "",
        },
    )
    .await
    .unwrap_err();
    assert!(e.contains("no-such-kb"), "{e}");
    assert_eq!(operator_list_members(&store, KB).await.unwrap(), before);
}
