//! One place that decides what may be done with a **detached** KB's source
//! files, for every caller — human and agent alike.
//!
//! When a KB is detached (`IngestPolicy::StoreIsTruth`) its `.org` files stop
//! being read by any ingest. They stay on disk, but nothing keeps them current
//! and nothing reads them back:
//!
//! - **Reading** one returns content that may be arbitrarily old while looking
//!   authoritative.
//! - **Writing** one is worse: the bytes land where no ingest will ever read
//!   them, so the content is invisible to `kb_search`, `kb_get`, the graph and
//!   the agenda — while the write reports success.
//!
//! @ai-caution: [kb-truth] This lived in `crates/ai/src/tool_impls/` and
//! therefore covered the AGENT paths only. Every human path — `:e`, the file
//! picker, `:w`, autosave, the file-tree dialogs — reached the same primitives
//! with no check at all, so the human got the silent-lost-edit the agent was
//! protected from. Principle #3: the human and the AI call the same
//! primitives, so the rule belongs where both reach it. Do not add a second
//! copy anywhere; add a caller here.

use super::super::Editor;
use std::path::Path;

/// What a caller wants to do with the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveAccess {
    Read,
    Write,
}

impl Editor {
    /// The refusal for touching a detached KB's source file, or `None` if this
    /// path is not one.
    ///
    /// `surface` names the caller (`file_read`, `:w`, `create_file`, …) so the
    /// message can say where the tool or command *is* still right — R10
    /// measured that a bare prohibition roughly triples the wrong-tool rate,
    /// and that the shape which works states the consequence and grants
    /// jurisdiction elsewhere.
    pub fn kb_archive_refusal(
        &self,
        path: &Path,
        access: ArchiveAccess,
        surface: &str,
    ) -> Option<String> {
        let kb = self.kb_stale_archive_instance(path)?;
        let consequence = match access {
            ArchiveAccess::Read => {
                "Reading it would return content that may be arbitrarily out of date."
            }
            ArchiveAccess::Write => {
                "Writing here would report success while the content stayed invisible to \
                 kb_search, kb_get and the graph."
            }
        };
        let redirect = match access {
            // Both spellings, because both audiences read this: a human types
            // `:kb-search`, an agent calls `kb_search`. One message for one
            // rule (principle #3) beats two that can drift apart.
            ArchiveAccess::Read => "Use :kb-search / kb_search or kb_get for this KB's content",
            ArchiveAccess::Write => {
                "Use :kb-create / kb_create or kb_update to add content to this KB"
            }
        };
        Some(format!(
            "'{}' is inside KB '{kb}', which is detached: its store is the source of truth \
             and these .org files are a stale archive no ingest reads. {consequence} \
             {redirect}; {surface} remains correct for source code and files outside a \
             detached KB.",
            path.display()
        ))
    }

    /// The banner shown above a detached KB's source file opened for reading.
    ///
    /// Deliberately not a refusal: the archive is still the only copy of things
    /// the store lost at ingest (external link markup), so it must stay
    /// readable. It is opened read-only instead, so an edit cannot be silently
    /// stranded.
    pub fn kb_archive_banner(kb: &str) -> String {
        format!(
            "This is NOT the KB — it is the former source text of '{kb}', kept as an archive. \
             The KB is authoritative and is not updated by editing this file. Open the real \
             node with :kb-search, or retire this archive with :kb-retire-archive {kb}."
        )
    }

    /// Refuse CREATING a new `.org` file inside a detached KB's directory.
    ///
    /// A distinct question from [`Editor::kb_archive_refusal`], and it needs a
    /// distinct rule. That one asks "is this file the KB's former source?",
    /// answered from `source_files` — which a brand-new file can never be in,
    /// so it would never fire here.
    ///
    /// A new `.org` dropped into a detached KB's directory looks exactly like
    /// adding a note and is not one: no ingest will read it, ever. A new
    /// `.tf`/`.yml`/`.md` in the same directory is an ordinary project file and
    /// is left alone — which is why this keys on the extension rather than the
    /// directory alone. The directory is routinely a whole project repo.
    pub fn kb_orphan_org_target(&self, path: &Path) -> Option<String> {
        if path.extension().and_then(|e| e.to_str()) != Some("org") {
            return None;
        }
        let inst = self.kb.registry.instances.iter().find(|i| {
            !i.ingest_policy.allows_ingest()
                && !i.org_dir.as_os_str().is_empty()
                && path.starts_with(&i.org_dir)
        })?;
        Some(format!(
            "'{}' would be created inside KB '{}', which is detached: its store is \
             the source of truth and no ingest reads this directory. The file would \
             exist on disk and be invisible to the KB. Use :kb-create to add a note \
             to '{}'.",
            path.display(),
            inst.name,
            inst.name
        ))
    }

    /// Refuse removing or renaming a detached KB's archived source file.
    ///
    /// Not because the KB would break — the watcher is already gated, so the
    /// node survives — but because that file is the archive, and the archive is
    /// currently the ONLY copy of what the store lost at ingest (external link
    /// markup is flattened and unrecoverable). `:kb-retire-archive` is the
    /// sanctioned way to remove it: it verifies the store represents every file
    /// first, and moves rather than deletes.
    pub fn kb_archive_removal_refusal(&self, path: &Path, surface: &str) -> Option<String> {
        let kb = self.kb_stale_archive_instance(path)?;
        Some(format!(
            "'{}' is an archived source file of KB '{kb}'. It is the only copy of what the \
             store lost at ingest, so {surface} will not remove it. Use \
             :kb-retire-archive {kb} — it verifies the store holds every file first, and \
             moves the archive aside instead of deleting it.",
            path.display()
        ))
    }

    /// Refuse a save that would land in a detached KB's archive.
    ///
    /// Shared by `save_current_buffer` and `save_all_modified_buffers` so
    /// autosave cannot do silently what an explicit `:w` is refused.
    pub(crate) fn refuse_save_into_stale_archive(&self, idx: usize) -> Result<(), String> {
        let Some(path) = self.buffers[idx].file_path().map(|p| p.to_path_buf()) else {
            return Ok(());
        };
        // TWO rules, because a save can be either shape and only one of them
        // was covered. `kb_archive_refusal` asks "is this the KB's former
        // source?", answered from `source_files` — which a brand-new file is
        // never in. So `:e newnote.org` inside a detached KB's directory,
        // typed into and saved, wrote an orphan the KB would never see and
        // reported success. `kb_orphan_org_target` is the rule for that shape,
        // and it was only wired into the file-tree dialogs.
        if let Some(msg) = self.kb_archive_refusal(&path, ArchiveAccess::Write, ":w") {
            return Err(msg);
        }
        if let Some(msg) = self.kb_orphan_org_target(&path) {
            return Err(msg);
        }
        Ok(())
    }
}
