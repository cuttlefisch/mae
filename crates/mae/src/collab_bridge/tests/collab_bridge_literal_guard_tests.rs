//! The body-format write guard (`mae_core` `kb_ops::literal_guard`) at the two
//! places the bridge puts a KB's local node documents on the wire **without**
//! going through an `Editor` mutator: the outbound drain (queued node updates
//! and manifest ops) and the share / set-encryption intents.
//!
//! Oracle: the `CollabCommand`s that actually reach the channel, i.e. what would
//! go to the daemon — not a status string, and not the queues one hop short of
//! the wire. Every refusal is paired with the same steps on a KB without the
//! marker, so a guard that blocked everything (or a fixture that could never
//! send) fails too.

use super::*;
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, BODY_FORMAT_ORG_LITERAL_V1};

const KB_ID: &str = "litkb-collab-91c2";
const NODE: &str = "lit:n";
const CREATED: &str = "lit:created";

fn collection(format: Option<&str>) -> Vec<u8> {
    let mut coll = KbCollectionDoc::new("litkb", "operator");
    coll.add_node(NODE, "Literal note");
    if let Some(f) = format {
        let _ = coll.set_body_format(f);
    }
    coll.encode_state()
}

/// The node as a literal-org seeder wrote it on the daemon.
fn seeder_doc() -> KbNodeDoc {
    let mut doc = KbNodeDoc::new_with_client_id(
        NODE,
        "Literal note",
        "See [[id:lit:other][the other note]].\n",
        &[],
        0x5eed,
    );
    let _ = doc.set_kind(Some("decision"));
    doc
}

fn join(editor: &mut Editor, format: Option<&str>) {
    let daemon = seeder_doc();
    handle_collab_event(
        editor,
        CollabEvent::KbJoined {
            kb_id: KB_ID.to_string(),
            collection_state: collection(format),
            nodes: vec![JoinedNode {
                id: NODE.to_string(),
                bytes: daemon.encode_state(),
                daemon_sv: Some(daemon.state_vector()),
            }],
        },
    );
}

/// `durable`: the SQLite pending queue (the production path whenever a store
/// exists) or the in-memory fallback.
fn editor(durable: bool) -> (Editor, tempfile::TempDir) {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut editor = Editor::new();
    editor.config_dir_override = Some(tmp.path().join("config"));
    editor.data_dir_override = Some(tmp.path().join("data"));
    editor.collab.kb_sync_mode = "on_save".into();
    if durable {
        editor.kb.store = Some(std::sync::Arc::new(
            mae_kb::CozoKbStore::open_mem().unwrap(),
        ));
    }
    (editor, tmp)
}

fn queued(editor: &Editor) -> usize {
    let durable = editor
        .kb
        .store
        .as_ref()
        .map_or(0, |s| s.count_pending_updates().unwrap());
    durable + editor.collab.pending_kb_updates.len() + editor.collab.pending_kb_manifest.len()
}

fn kb_name(editor: &Editor) -> String {
    editor
        .kb
        .registry
        .find_by_collab_id(KB_ID)
        .expect("joined KB is registered")
        .name
        .clone()
}

/// One real event-loop tick's worth of draining; every command that reached
/// the channel.
fn drain(editor: &mut Editor) -> Vec<CollabCommand> {
    let (tx, mut rx) = mpsc::channel(64);
    drain_collab_intents(editor, &tx);
    let mut out = Vec::new();
    while let Ok(cmd) = rx.try_recv() {
        out.push(cmd);
    }
    out
}

/// Commands that would carry this KB's node content or manifest to the daemon.
fn to_this_kb(cmds: &[CollabCommand]) -> Vec<&CollabCommand> {
    cmds.iter()
        .filter(|c| match c {
            CollabCommand::KbNodeUpdate { kb_id, .. }
            | CollabCommand::KbCollectionNode { kb_id, .. }
            | CollabCommand::ShareKb { kb_id, .. }
            | CollabCommand::KbSetEncryption { kb_id, .. } => kb_id == KB_ID,
            _ => false,
        })
        .collect()
}

fn withheld_notices(editor: &Editor) -> usize {
    let key = format!("kb-outbound-withheld:{KB_ID}");
    editor
        .notifications
        .feed()
        .filter(|n| n.key.as_deref() == Some(key.as_str()))
        .count()
}

/// Join, then fall into the fail-open window — a join by a build without the
/// guard, then a restart: no replica in memory and nothing recorded in the
/// KB's store (`Unknown`) — and edit offline there. Returns how many ops the
/// accepted edits queued.
fn queue_offline_edits_while_unknown(
    editor: &mut Editor,
    format: Option<&str>,
    case: &str,
) -> usize {
    join(editor, format);
    let uuid = editor
        .kb
        .registry
        .find_by_collab_id(KB_ID)
        .unwrap()
        .uuid
        .clone();
    editor.collab.kb_collection_state.remove(KB_ID);
    if let Some(store) = editor.kb.instance_stores.get(&uuid) {
        store.set_collection_body_format(None).unwrap();
    }
    editor
        .kb_update_node(NODE, Some("Offline edit"), None, None)
        .unwrap_or_else(|e| panic!("{case}: unknown format is fail-open: {e}"));
    let name = kb_name(editor);
    editor
        .kb_create_node_target(
            Some(&name),
            CREATED,
            "Created offline",
            "b",
            mae_kb::NodeKind::Note,
        )
        .unwrap_or_else(|e| panic!("{case}: {e}"));
    let queued = queued(editor);
    assert!(
        queued >= 2,
        "{case}: the offline edits are queued ({queued})"
    );
    queued
}

fn sent_a(cmds: &[&CollabCommand], update: bool) -> bool {
    cmds.iter().any(|c| match c {
        CollabCommand::KbNodeUpdate { .. } => update,
        CollabCommand::KbCollectionNode { add: true, .. } => !update,
        _ => false,
    })
}

/// **G1.** Edits accepted while the format was unknown (fail-open) are queued;
/// then the marker is learned by a re-join. New edits are refused — and the
/// edits ALREADY queued must not be sent either. They stay queued (the user's
/// edit is not thrown away), the user is told once, and the identical steps on
/// a KB without the marker still send.
#[test]
fn edits_queued_before_the_marker_is_learned_are_held_not_sent() {
    for durable in [true, false] {
        for literal in [true, false] {
            let case = format!("durable={durable} literal={literal}");
            let format = literal.then_some(BODY_FORMAT_ORG_LITERAL_V1);
            let (mut editor, _d) = editor(durable);
            let before = queue_offline_edits_while_unknown(&mut editor, format, &case);

            // The marker is learned: the daemon's collection, on re-join.
            join(&mut editor, format);
            let refused = editor.kb_update_node(NODE, Some("After the marker"), None, None);
            assert_eq!(refused.is_err(), literal, "{case}: new edits: {refused:?}");

            editor.collab.status = CollabStatus::Connected { peer_count: 1 };
            let sent = drain(&mut editor);
            let sent_here = to_this_kb(&sent);
            if !literal {
                assert!(
                    sent_a(&sent_here, true),
                    "{case}: node update sent: {sent:#?}"
                );
                assert!(
                    sent_a(&sent_here, false),
                    "{case}: manifest add sent: {sent:#?}"
                );
                assert_eq!(withheld_notices(&editor), 0, "{case}");
                continue;
            }
            assert!(
                sent_here.is_empty(),
                "{case}: an edit queued before the marker was learned went to the \
                 daemon: {sent_here:#?}"
            );
            assert_eq!(
                queued(&editor),
                before,
                "{case}: the held edits stay queued locally (quarantined, not purged)"
            );
            assert_eq!(withheld_notices(&editor), 1, "{case}: the user is told");
            // Every tick re-checks; nothing leaks later, and no notice spam.
            assert!(to_this_kb(&drain(&mut editor)).is_empty(), "{case}");
            assert_eq!(withheld_notices(&editor), 1, "{case}: told once");
            assert_eq!(queued(&editor), before, "{case}");
            // A hold, not a one-way latch: once the replica no longer declares
            // the format (the refusal cache is keyed by the replica), the held
            // edits go out.
            editor.kb_note_collection_state(KB_ID, collection(None));
            let released = drain(&mut editor);
            assert!(
                sent_a(&to_this_kb(&released), true),
                "{case}: the held edits are released when the format is lifted: {released:#?}"
            );
        }
    }
}

/// **G2.** A re-share and an encryption change both carry every node's LOCAL
/// document to the daemon, which merges a re-share into its own. On a literal
/// KB both are refused — from the human's command and from the agent's tool,
/// which lower onto the same intent — before any node document is touched; on
/// a KB without the marker both still produce their command.
#[test]
fn share_and_set_encryption_are_refused_on_a_literal_kb() {
    type Request = (&'static str, fn(&mut Editor, &str));
    let requests: &[Request] = &[
        ("human :kb-share <name>", |e, name| {
            e.dispatch_ex_command("kb-share", Some(name));
        }),
        ("agent kb_share", |e, name| {
            run_tool(e, "kb_share", serde_json::json!({ "kb_name": name }));
        }),
        ("human :kb-set-encryption <kb> e2e", |e, _| {
            e.dispatch_ex_command("kb-set-encryption", Some(&format!("{KB_ID} e2e")));
        }),
        ("agent kb_set_encryption", |e, _| {
            run_tool(
                e,
                "kb_set_encryption",
                serde_json::json!({ "kb_id": KB_ID, "mode": "e2e" }),
            );
        }),
    ];
    for (what, request) in requests {
        for literal in [true, false] {
            let case = format!("{what}, literal={literal}");
            let (mut editor, _d) = editor(true);
            join(&mut editor, literal.then_some(BODY_FORMAT_ORG_LITERAL_V1));
            let name = kb_name(&editor);
            let doc_before = editor
                .kb
                .instances
                .values()
                .find_map(|kb| kb.get(NODE).and_then(|n| n.crdt_doc.clone()));

            request(&mut editor, &name);
            assert!(
                editor.collab.pending_intent.is_some(),
                "{case}: precondition — the request reached the intent slot"
            );
            editor.collab.status = CollabStatus::Connected { peer_count: 1 };
            let sent = drain(&mut editor);
            let sent_here = to_this_kb(&sent);

            if literal {
                assert!(
                    sent_here.is_empty(),
                    "{case}: local node docs went to the daemon: {sent_here:#?}"
                );
                assert!(
                    editor.status_msg.contains(BODY_FORMAT_ORG_LITERAL_V1)
                        && editor.status_msg.contains(&name),
                    "{case}: the refusal names the KB and the format: {}",
                    editor.status_msg
                );
                let doc_after = editor
                    .kb
                    .instances
                    .values()
                    .find_map(|kb| kb.get(NODE).and_then(|n| n.crdt_doc.clone()));
                assert_eq!(doc_after, doc_before, "{case}: no node doc was rewritten");
            } else {
                assert!(
                    sent_here.iter().any(|c| matches!(
                        c,
                        CollabCommand::ShareKb { .. } | CollabCommand::KbSetEncryption { .. }
                    )),
                    "{case}: without the marker the request still goes out: {sent:#?} \
                     (status: {})",
                    editor.status_msg
                );
            }
        }
    }
}

/// Run one agent tool through the real dispatcher (what MCP and the embedded
/// agent call), with the permission layer opened so only the guard can stop it.
fn run_tool(editor: &mut Editor, name: &str, args: serde_json::Value) {
    let call = mae_ai::ToolCall {
        id: format!("call-{name}"),
        name: name.into(),
        arguments: args,
    };
    let mut tools = mae_ai::tools_from_registry(&mae_core::CommandRegistry::with_builtins());
    tools.extend(mae_ai::ai_specific_tools(&mae_core::OptionRegistry::new()));
    let policy = mae_ai::PermissionPolicy {
        auto_approve_up_to: mae_core::PermissionTier::Privileged,
        ..mae_ai::PermissionPolicy::default()
    };
    match mae_ai::execute_tool(editor, &call, &tools, &policy) {
        mae_ai::ExecuteResult::Immediate(r) => {
            assert!(
                r.success,
                "{name}: the tool itself must queue the request: {}",
                r.output
            )
        }
        _ => panic!("{name}: expected an immediate result"),
    }
}
