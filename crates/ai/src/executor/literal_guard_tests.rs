//! Principle #3 for the body-format write guard: the agent's tools and the
//! human's commands reach the SAME refusal, because both lower onto the same
//! `Editor` mutators (see mae-core `kb_ops::literal_guard`).
//!
//! The agent half goes through `execute_tool` — the dispatcher the MCP server
//! and the embedded agent both call — not the `execute_kb_*` functions
//! directly, so a refusal added (or lost) anywhere on that route is observed.

use super::*;
use crate::tools::{tools_from_registry, PermissionPolicy};
use mae_core::Editor;
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, BODY_FORMAT_ORG_LITERAL_V1};

const UUID: &str = "uuid-lit";
const NAME: &str = "litkb";
const KB_ID: &str = "litkb-collab-7f3a";
const NODE: &str = "lit:n";

fn all_tools() -> Vec<ToolDefinition> {
    let mut t = tools_from_registry(&mae_core::CommandRegistry::with_builtins());
    t.extend(crate::tools::ai_specific_tools(
        &mae_core::OptionRegistry::new(),
    ));
    t
}

/// Auto-approve everything: the point is the guard, so the permission layer
/// must not be what stops the call.
fn allow_all() -> PermissionPolicy {
    PermissionPolicy {
        auto_approve_up_to: PermissionTier::Privileged,
        ..PermissionPolicy::default()
    }
}

/// A joined KB with one seeder-written node; the collection replica declares
/// `format` (`None` = no key).
fn editor_with_kb(format: Option<&str>) -> (Editor, tempfile::TempDir) {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut editor = Editor::new();
    editor.config_dir_override = Some(tmp.path().join("config"));
    editor.data_dir_override = Some(tmp.path().join("data"));
    editor.collab.kb_sync_mode = "on_save".into();

    let mut inst = mae_kb::federation::KbInstance::joined(
        UUID.into(),
        KB_ID,
        std::path::PathBuf::new(),
        "2026-11-09T00:00:00Z".into(),
    );
    inst.name = NAME.into();
    editor.kb.registry.instances.push(inst);

    let mut doc = KbNodeDoc::new_with_client_id(
        NODE,
        "Literal note",
        "See [[id:lit:other][the other note]].\n",
        &[],
        0x5eed,
    );
    let _ = doc.set_kind(Some("decision"));
    let node =
        mae_kb::Node::from_crdt_doc(&doc, mae_kb::NodeKind::Note, mae_kb::NodeSource::Federation);
    let mut kb = mae_kb::KnowledgeBase::new();
    kb.insert(node);
    editor.kb.instances.insert(UUID.into(), kb);

    let mut coll = KbCollectionDoc::new(NAME, "operator");
    if let Some(f) = format {
        let _ = coll.set_body_format(f);
    }
    editor
        .collab
        .kb_collection_state
        .insert(KB_ID.into(), coll.encode_state());
    (editor, tmp)
}

fn doc_bytes(editor: &Editor) -> Vec<u8> {
    editor.kb.instances[UUID]
        .get(NODE)
        .and_then(|n| n.crdt_doc.clone())
        .expect("node and its CRDT doc")
}

/// Run one tool through the real dispatcher; `Ok(output)` / `Err(output)`.
fn run_tool(editor: &mut Editor, name: &str, args: serde_json::Value) -> Result<String, String> {
    let call = ToolCall {
        id: format!("call-{name}"),
        name: name.into(),
        arguments: args,
    };
    match execute_tool(editor, &call, &all_tools(), &allow_all()) {
        ExecuteResult::Immediate(r) if r.success => Ok(r.output),
        ExecuteResult::Immediate(r) => Err(r.output),
        ExecuteResult::Deferred { .. } => panic!("{name}: unexpectedly deferred"),
        ExecuteResult::NeedsApproval(_) => {
            panic!("{name}: the policy asked; the guard was not reached")
        }
    }
}

fn agent_calls() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        (
            "kb_update",
            serde_json::json!({"id": NODE, "title": "Retitled"}),
        ),
        (
            "kb_set_role",
            serde_json::json!({"id": NODE, "role": "atom"}),
        ),
        (
            "kb_add_link",
            serde_json::json!({"src": NODE, "dst": "lit:other", "rel_type": "references", "kb": NAME}),
        ),
        ("kb_delete", serde_json::json!({"id": NODE})),
        (
            "kb_create",
            serde_json::json!({"id": "lit:new", "title": "New", "body": "b", "kb": NAME}),
        ),
        ("kb_promote", serde_json::json!({"id": NODE})),
    ]
}

/// Every agent write tool is refused on a literal KB, nothing is written or
/// queued — and each of the same calls succeeds on a KB without the marker, so
/// the refusal is the guard's and not a broken fixture's.
#[test]
fn agent_kb_write_tools_are_refused_on_a_literal_kb_and_work_without_it() {
    for (tool, args) in agent_calls() {
        let (mut editor, _d) = editor_with_kb(Some(BODY_FORMAT_ORG_LITERAL_V1));
        let before = doc_bytes(&editor);
        let err = run_tool(&mut editor, tool, args.clone()).expect_err(tool);
        assert!(
            err.contains(BODY_FORMAT_ORG_LITERAL_V1) && err.contains(NAME),
            "{tool}: {err}"
        );
        assert_eq!(doc_bytes(&editor), before, "{tool}: doc must be unchanged");
        assert!(editor.collab.pending_kb_updates.is_empty(), "{tool}");
        assert!(editor.collab.pending_kb_manifest.is_empty(), "{tool}");

        let (mut editor, _d) = editor_with_kb(None);
        run_tool(&mut editor, tool, args).unwrap_or_else(|e| panic!("{tool} without marker: {e}"));
        assert!(
            !editor.collab.pending_kb_updates.is_empty()
                || !editor.collab.pending_kb_manifest.is_empty(),
            "{tool} without marker: the write must reach the outbound queue"
        );
    }
}

/// The agent's `kb_update` and the human's `:kb-update` produce the same
/// refusal, word for word — one rule, not two that happen to agree today.
#[test]
fn the_agent_and_the_human_get_the_same_refusal() {
    let (mut agent_editor, _d1) = editor_with_kb(Some(BODY_FORMAT_ORG_LITERAL_V1));
    let agent = run_tool(
        &mut agent_editor,
        "kb_update",
        serde_json::json!({"id": NODE, "body": "edited"}),
    )
    .unwrap_err();

    let (mut human_editor, _d2) = editor_with_kb(Some(BODY_FORMAT_ORG_LITERAL_V1));
    let before = doc_bytes(&human_editor);
    human_editor.dispatch_ex_command("kb-update", Some(&format!("{NODE} edited")));
    assert_eq!(
        human_editor.status_msg, agent,
        "same refusal on both surfaces"
    );
    assert_eq!(doc_bytes(&human_editor), before);

    // And the human's own edit surface: the node buffer.
    human_editor.kb_edit_node(NODE).unwrap();
    let idx = human_editor.active_buffer_idx();
    human_editor.execute_command("save");
    assert!(
        human_editor.status_msg.ends_with(&agent),
        "node-buffer save: {}",
        human_editor.status_msg
    );
    assert_eq!(human_editor.active_buffer_idx(), idx);
    assert_eq!(doc_bytes(&human_editor), before);
}
