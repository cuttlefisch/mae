//! One place that decides what a tool may do with a **detached** KB's `.org`
//! files.
//!
//! When a KB is detached (`IngestPolicy::StoreIsTruth`), its `.org` directory
//! stops being read by any ingest. The files stay on disk, but nothing keeps
//! them current and nothing reads them back:
//!
//! - **Reading** one returns content that may be arbitrarily old while looking
//!   authoritative. An agent answers confidently and wrongly, with nothing in
//!   the response to signal it (this is what Story C closed for `file_read`).
//! - **Writing** one is worse, and is what this module adds. The bytes land in
//!   a directory no ingest will ever read, so the content is invisible to
//!   `kb_search`, `kb_get`, the graph and the agenda — while the tool reports
//!   success, byte count and all. A silent write into a black hole is the
//!   failure mode `federation.rs`'s own `@ai-caution` warns about: *"a path
//!   that skips this check silently reverts a detached KB to text."*
//!
//! @ai-caution: [architecture-debt] `Editor::kb_stale_archive_instance` had
//! exactly ONE consumer for the whole read side and none at all on the write
//! side. Any NEW tool that reads or writes a filesystem path must call one of
//! the two helpers here — not re-derive the rule, and not skip it because the
//! tool "isn't an ingest path". Neither `create_file` nor `open_file` looked
//! like an ingest path either.
//!
//! @stability: experimental

use mae_core::ArchiveAccess;
use mae_core::Editor;
use std::path::Path;

/// Refuse a READ of a detached KB's stale archive.
///
/// Worded as a consequence, and granting the file tools jurisdiction
/// elsewhere: R10 measured that aggressive prohibitions in tool descriptions
/// roughly triple the wrong-tool rate, and the shape that works states the
/// consequence and says where the tool IS right. This is an execution error,
/// which the MCP spec says carries "actionable feedback that language models
/// can use to self-correct".
pub(crate) fn refuse_read(editor: &Editor, path: &str, tool: &str) -> Option<String> {
    editor.kb_archive_refusal(Path::new(path), ArchiveAccess::Read, tool)
}

/// Refuse a WRITE into a detached KB's stale archive.
pub(crate) fn refuse_write(editor: &Editor, path: &str, tool: &str) -> Option<String> {
    editor.kb_archive_refusal(Path::new(path), ArchiveAccess::Write, tool)
}

/// Fixtures shared by every stale-archive test, in one place so a second test
/// module cannot drift from the first on what "detached" means.
#[cfg(test)]
pub(crate) mod test_support {
    use mae_core::Editor;

    pub(crate) fn instance(
        name: &str,
        dir: &std::path::Path,
        policy: mae_kb::federation::IngestPolicy,
    ) -> mae_kb::federation::KbInstance {
        mae_kb::federation::KbInstance {
            uuid: format!("uuid-{name}"),
            name: name.into(),
            org_dir: dir.to_path_buf(),
            db_path: dir.join("kb.db"),
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: mae_kb::federation::AiResidency::default(),
            project_root: None,
            project_key: None,
            kind: mae_kb::federation::KbInstanceKind::default(),
            ingest_policy: policy,
            priority: 0,
            remote_hub: None,
        }
    }

    /// A detached KB whose store records `recorded` as imported source files.
    ///
    /// Recording is not optional decoration: the guard only claims a file the
    /// KB actually imported, because a KB's `org_dir` is often a whole project
    /// repo. A fixture that skips this models a KB that imported nothing, and
    /// every "is refused" assertion would pass vacuously.
    pub(crate) fn editor_with_detached_kb_recording(
        dir: &std::path::Path,
        recorded: &[&std::path::Path],
    ) -> Editor {
        let mut editor = Editor::new();
        let inst = instance(
            "Detached",
            dir,
            mae_kb::federation::IngestPolicy::StoreIsTruth,
        );
        let store = mae_kb::CozoKbStore::open_mem().expect("in-memory store");
        for p in recorded {
            store
                .record_source_file(&p.to_string_lossy(), "hash-for-test", 0, &[])
                .expect("record source file");
        }
        editor
            .kb
            .instance_stores
            .insert(inst.uuid.clone(), std::sync::Arc::new(store));
        editor.kb.registry.instances.push(inst);
        editor
    }

    /// Convenience for the common single-file case.
    pub(crate) fn editor_with_detached_kb(dir: &std::path::Path) -> Editor {
        editor_with_detached_kb_recording(dir, &[&dir.join("note.org")])
    }

    pub(crate) fn editor_with_attached_kb(dir: &std::path::Path) -> Editor {
        let mut editor = Editor::new();
        editor.kb.registry.instances.push(instance(
            "Attached",
            dir,
            mae_kb::federation::IngestPolicy::FromOrgDir,
        ));
        editor
    }
}
