//! ADR-092 D3/D5 — editing a KB node as its normalized org source text.
//!
//! The failure this surface exists to prevent is **silent loss on a no-op save**:
//! open a node, change nothing, save, and lose whatever the serializer forgot.
//! So the load-bearing test here is the round trip through the *editor*, not just
//! through `mae-kb` — every field must come back.

use super::*;

fn editor_with_node() -> (Editor, TempDir) {
    let mut editor = Editor::new();
    let dirs = with_test_dirs(&mut editor);
    let mut node = mae_kb::Node::new(
        "note:editable",
        "Editable node",
        mae_kb::NodeKind::Concept,
        "Original body.\n",
    );
    node.tags = vec!["alpha".into()];
    node.todo_state = Some("TODO".into());
    node.priority = Some('B');
    node.aliases = vec!["nickname".into()];
    node.properties
        .insert("role".to_string(), "reference".to_string());
    editor.kb.primary.insert(node);
    (editor, dirs)
}

fn active_text(editor: &Editor) -> String {
    editor.buffers[editor.active_buffer_idx()].text()
}

/// Replace a buffer's whole contents — `Buffer` has no `set_text`, and the two
/// primitives it does have are what the editor itself uses.
fn replace_text(editor: &mut Editor, idx: usize, text: &str) {
    let len = editor.buffers[idx].rope().len_chars();
    editor.buffers[idx].delete_range(0, len);
    editor.buffers[idx].insert_text_at(0, text);
}

/// **Every field the source text carries must actually be WRITTEN.**
///
/// The first version of this test opened a node, saved it unchanged, and
/// asserted the fields were intact — and it **passed with the write of six of
/// them deleted**, because a field that is never overwritten is trivially
/// preserved. It measured nothing. The oracle has to be a field the buffer
/// *changed*, so only a real write can satisfy it.
#[test]
fn every_field_in_the_source_text_reaches_the_stored_node() {
    let (mut editor, _d) = editor_with_node();
    editor.kb_edit_node("note:editable").unwrap();
    let idx = editor.active_buffer_idx();

    let edited = active_text(&editor)
        .replace("#+title: Editable node", "#+title: Retitled")
        .replace(":KIND: concept", ":KIND: note")
        .replace("#+todo_state: TODO", "#+todo_state: DONE")
        .replace("#+priority: B", "#+priority: A")
        .replace("#+aliases: nickname", "#+aliases: renamed")
        .replace(":ROLE: reference", ":ROLE: primary")
        .replace("#+filetags: :alpha:", "#+filetags: :beta:")
        .replace("Original body.", "Rewritten body.");
    replace_text(&mut editor, idx, &edited);
    assert!(editor.kb_save_node_buffer(idx), "this IS a node buffer");

    let node = editor.kb.primary.get("note:editable").unwrap();
    assert_eq!(node.title, "Retitled", "title");
    assert!(
        node.body.contains("Rewritten body."),
        "body: {:?}",
        node.body
    );
    assert_eq!(node.tags, vec!["beta".to_string()], "tags");
    assert_eq!(node.kind, mae_kb::NodeKind::Note, "kind");
    assert_eq!(node.todo_state.as_deref(), Some("DONE"), "todo_state");
    assert_eq!(node.priority, Some('A'), "priority");
    assert_eq!(node.aliases, vec!["renamed".to_string()], "aliases");
    assert_eq!(
        node.properties.get("role").map(String::as_str),
        Some("primary"),
        "properties"
    );
}

/// ...and a save that changes nothing changes nothing — the round trip does not
/// mangle a field on its way through.
#[test]
fn a_no_op_save_leaves_every_field_as_it_was() {
    let (mut editor, _d) = editor_with_node();
    let before = editor.kb.primary.get("note:editable").unwrap().clone();

    editor.kb_edit_node("note:editable").unwrap();
    let idx = editor.active_buffer_idx();
    assert!(editor.kb_save_node_buffer(idx));

    let after = editor.kb.primary.get("note:editable").unwrap();
    assert_eq!(after.title, before.title);
    assert_eq!(after.body.trim(), before.body.trim());
    assert_eq!(after.tags, before.tags);
    assert_eq!(after.kind, before.kind);
    assert_eq!(after.todo_state, before.todo_state);
    assert_eq!(after.priority, before.priority);
    assert_eq!(after.aliases, before.aliases);
    assert_eq!(after.properties, before.properties);
}

/// And a real edit actually lands — otherwise "preserves every field" is
/// satisfied by a save that does nothing at all.
#[test]
fn an_edit_to_the_source_text_reaches_the_stored_node() {
    let (mut editor, _d) = editor_with_node();
    editor.kb_edit_node("note:editable").unwrap();

    let idx = editor.active_buffer_idx();
    let edited = active_text(&editor)
        .replace("#+title: Editable node", "#+title: Retitled")
        .replace("Original body.", "Rewritten body.");
    replace_text(&mut editor, idx, &edited);
    assert!(editor.kb_save_node_buffer(idx));

    let node = editor.kb.primary.get("note:editable").unwrap();
    assert_eq!(node.title, "Retitled");
    assert!(node.body.contains("Rewritten body."), "{:?}", node.body);
}

/// **The edit surface must not be able to destroy the node it opened.** A buffer
/// whose `:PROPERTIES:` drawer was deleted no longer identifies anything, and
/// writing it as an empty node is the worst available outcome.
#[test]
fn a_buffer_that_lost_its_id_drawer_is_refused_not_written() {
    let (mut editor, _d) = editor_with_node();
    editor.kb_edit_node("note:editable").unwrap();

    let idx = editor.active_buffer_idx();
    replace_text(&mut editor, idx, "just prose, no drawer at all\n");
    assert!(editor.kb_save_node_buffer(idx), "still a node buffer");

    let node = editor.kb.primary.get("note:editable").unwrap();
    assert_eq!(
        node.title, "Editable node",
        "the stored node must be untouched"
    );
    assert!(
        editor.status_msg.contains(":ID:"),
        "and the refusal must say why: {:?}",
        editor.status_msg
    );
}

/// Renaming the id is not an edit to content — it would orphan every link
/// pointing at the node, silently.
///
/// **The oracle is that nothing was written**, not that no new node appeared:
/// `kb_update_node_with` keys on the buffer's id, so removing the guard creates
/// no node either — it silently applies the *other* edits while discarding the
/// rename the user asked for, and tells them it saved. That is the failure.
#[test]
fn changing_the_id_in_the_buffer_is_refused_and_writes_nothing() {
    let (mut editor, _d) = editor_with_node();
    editor.kb_edit_node("note:editable").unwrap();

    let idx = editor.active_buffer_idx();
    let edited = active_text(&editor)
        .replace(":ID: note:editable", ":ID: note:something-else")
        .replace("#+title: Editable node", "#+title: Renamed and retitled");
    replace_text(&mut editor, idx, &edited);
    assert!(editor.kb_save_node_buffer(idx));

    assert!(
        editor.kb.primary.get("note:something-else").is_none(),
        "no node may be created by renaming an id in an edit buffer"
    );
    let node = editor.kb.primary.get("note:editable").unwrap();
    assert_eq!(
        node.title, "Editable node",
        "a refused save must write NOTHING — not silently apply the other edits"
    );
    assert!(
        editor.status_msg.contains("note:something-else"),
        "and must name what it refused: {:?}",
        editor.status_msg
    );
}

/// An ordinary buffer must be untouched by any of this — the save interception
/// has to be precisely scoped, or every `:w` in the editor routes into the KB.
#[test]
fn an_ordinary_buffer_is_not_a_node_buffer() {
    let (mut editor, _d) = editor_with_node();
    let idx = editor.active_buffer_idx();

    assert!(
        !editor.kb_save_node_buffer(idx),
        "the scratch buffer must fall through to the ordinary save path"
    );
}

/// Reopening the same node reuses its buffer. Two buffers over one node would
/// let a stale copy overwrite a fresh edit.
#[test]
fn reopening_a_node_reuses_its_buffer() {
    let (mut editor, _d) = editor_with_node();

    editor.kb_edit_node("note:editable").unwrap();
    let before = editor.buffers.len();
    editor.kb_edit_node("note:editable").unwrap();

    assert_eq!(editor.buffers.len(), before, "no second buffer");
}

/// The buffer-name encoding round-trips, including ids that contain colons —
/// which every namespaced MAE node id does.
#[test]
fn the_buffer_name_round_trips_a_namespaced_id() {
    let name = mae_kb_node_buffer_name("concept:window#3");
    assert_eq!(
        super::super::node_buffer::node_id_from_buffer_name(&name).as_deref(),
        Some("concept:window#3")
    );
    assert_eq!(
        super::super::node_buffer::node_id_from_buffer_name("*scratch*"),
        None,
        "an ordinary buffer name must not parse as a node buffer"
    );
}

fn mae_kb_node_buffer_name(id: &str) -> String {
    super::super::node_buffer::node_buffer_name(id)
}

/// **D5: the default reproduces today's behaviour exactly.**
#[test]
fn the_default_edit_surface_is_auto() {
    let (editor, _d) = editor_with_node();
    assert_eq!(
        editor.kb_edit_surface(),
        mae_core_edit_surface_auto(),
        "changing this default changes what :kb-edit-source opens for every \
         existing file-backed node"
    );
}

fn mae_core_edit_surface_auto() -> crate::editor::kb_ops::EditSurface {
    crate::editor::kb_ops::EditSurface::Auto
}

/// A node with no file has nowhere else to go, so `auto` must choose the buffer.
#[test]
fn auto_opens_the_node_buffer_when_there_is_no_source_file() {
    let (mut editor, _d) = editor_with_node();
    editor.open_help_at("note:editable");
    editor.help_edit_source();

    let name = editor.buffers[editor.active_buffer_idx()].name.clone();
    assert!(
        super::super::node_buffer::node_id_from_buffer_name(&name).is_some(),
        "expected a node edit buffer, got {name:?} (status: {:?})",
        editor.status_msg
    );
}

/// **The option must actually be readable.** It was registered before it had a
/// `get_option` arm, so `kb_edit_surface()` silently fell back to `Auto` and
/// setting it to `node` did nothing — a registered option with no consumer is
/// drift, not a feature (principle #7). The registry-reachability guard caught
/// it; this pins the behaviour rather than only the reachability.
#[test]
fn setting_the_edit_surface_option_actually_changes_the_surface() {
    let (mut editor, _d) = editor_with_node();
    assert_eq!(
        editor.kb_edit_surface(),
        crate::editor::kb_ops::EditSurface::Auto
    );

    editor.set_option("kb_edit_surface", "file").unwrap();
    assert_eq!(
        editor.kb_edit_surface(),
        crate::editor::kb_ops::EditSurface::File,
        "the option must round-trip through get_option, not fall back to the default"
    );
    assert_eq!(editor.get_option("kb_edit_surface").unwrap().0, "file");

    assert!(
        editor.set_option("kb_edit_surface", "nonsense").is_err(),
        "an invalid value must be rejected, not stored"
    );
}

/// With `file`, a file-less node is a dead end again — the escape hatch for a
/// deployment that wants exactly today's behaviour and nothing else.
#[test]
fn the_file_surface_still_refuses_a_node_with_no_file() {
    let (mut editor, _d) = editor_with_node();
    editor.set_option("kb_edit_surface", "file").unwrap();

    editor.open_help_at("note:editable");
    editor.help_edit_source();

    assert!(
        editor.status_msg.contains("No source file"),
        "got: {:?}",
        editor.status_msg
    );
}

/// **No image ever previewed while editing a node.** A node buffer has no
/// `file_path` by design (ADR-092: its content is the node, not a file), and
/// image resolution keyed on `file_path.parent()` — so `compute_image_regions`
/// got `None` for its base and returned nothing, even for an absolute path that
/// worked fine when the same content was opened as a file.
#[test]
fn a_node_buffer_has_an_image_base_dir_so_images_can_resolve() {
    let tmp = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    editor.kb.primary.insert(mae_kb::Node::new(
        "note:with-image",
        "With Image",
        mae_kb::NodeKind::Note,
        "[[file:diagram.png]]",
    ));

    editor.kb_edit_node("note:with-image").expect("edit opens");
    let idx = editor
        .buffers
        .iter()
        .position(|b| b.name.contains("with-image"))
        .expect("node buffer");

    assert!(
        editor.buffers[idx].file_path().is_none(),
        "a node buffer must still have no file — that is the ADR-092 design"
    );
    assert!(
        editor.buffers[idx].image_base_dir.is_some(),
        "but it must have SOMEWHERE to resolve images from, or none ever render"
    );
    let _ = tmp;
}
