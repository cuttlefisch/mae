//! Guards on `kb_reimport`'s refusal cases.
//!
//! Reimport is a *destructive* operation — it replaces an instance's live
//! in-memory KB with whatever walking its `org_dir` produced. The cases where it
//! must refuse outright therefore each get an explicit test, in their own file
//! rather than appended to the general registry suite: these are data-loss
//! guards, and they should be findable as a group.

use super::*;

/// #631 — a dir-less instance must not be blanked by a reimport.
///
/// An empty `org_dir` is not a corner case: it is the documented convention for
/// every instance whose content does not come from disk (`federation.rs` states
/// it explicitly), which today means every JOINED COLLAB KB. Reimporting one
/// walks nothing, and the empty result was written straight back over the live
/// in-memory KB.
///
/// A guard for *system* KBs was added separately, but system KBs are only one
/// user of the dir-less convention — this covers the rest. The issue recorded
/// the blanking as plausible-but-unconfirmed; this test is what settles it.
#[test]
fn reimporting_a_dir_less_instance_does_not_blank_it() {
    let mut editor = Editor::new();
    let _tmp = with_test_dirs(&mut editor);

    // A joined collab KB: real content, no org directory, and NOT a system KB.
    let mut kb = mae_kb::KnowledgeBase::new();
    for id in ["shared:one", "shared:two"] {
        kb.insert(mae_kb::Node::new(
            id,
            id,
            mae_kb::NodeKind::Note,
            "content that exists only in the store",
        ));
    }
    editor.kb.instances.insert("uuid-joined".into(), kb);
    editor
        .kb
        .registry
        .instances
        .push(mae_kb::federation::KbInstance {
            uuid: "uuid-joined".into(),
            name: "joined-kb".into(),
            org_dir: std::path::PathBuf::new(), // the dir-less convention
            db_path: std::path::PathBuf::new(),
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: Some("joined-collab-id".into()),
            shared: true,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: mae_kb::federation::AiResidency::default(),
            project_root: None,
            project_key: None,
            kind: mae_kb::federation::KbInstanceKind::default(),
            ingest_policy: Default::default(),
            import_record: None,
            priority: 0,
            remote_hub: None,
        });

    editor.kb_reimport("joined-kb", None);

    let surviving = editor
        .kb
        .instances
        .get("uuid-joined")
        .map(|kb| kb.list_ids(None).len())
        .unwrap_or(0);
    assert_eq!(
        surviving, 2,
        "reimporting an instance with no org directory must be refused, not \
         allowed to replace its live content with the empty result of walking \
         nothing (#631)"
    );
}
