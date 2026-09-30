//! Which KB owns a filesystem path — one resolver for every caller that asks.
//!
//! @ai-caution: [kb-truth] This question used to be answered separately in four
//! places (dailies' backing, capture's notes-dir owner, the stale-archive label
//! and the new-`.org` refusal), each with its own copy of "an instance whose
//! non-empty `org_dir` is a prefix of the path". All four went blind the moment
//! a KB was RETIRED, because retirement clears `org_dir` (that is what makes a
//! KB native, ADR-110). The consequence was not cosmetic: `kb-notes-dir` still
//! pointed at a retired KB's origin, dailies asked "who owns this dir?", got
//! "nobody", fell back to the primary's file-backed policy — and wrote hollow
//! `Previous:`/`Next:` stub files into the retired origin, every day, which a
//! later attach or re-register would ingest over the real store (#825).
//!
//! A retired KB still records where it came from (`import_record.origin`), so
//! the ownership is not lost, only unasked. Ask here, and say how far to look.

use mae_kb::paths::canonical_lenient;
use std::path::Path;

use super::*;

/// How far [`Editor::kb_dir_owner`] looks when deciding which KB owns a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirOwnership {
    /// Only a live `org_dir`: the KB's current ingest source, attached or
    /// migrating. The human-facing archive guards use this — a retired origin
    /// is no longer the KB's source, and refusing a person's edits in what may
    /// be an ordinary project repo is a decision this resolver must not make
    /// for them.
    Source,
    /// Also a retired KB's recorded origin. For writes MAE ORIGINATES ITSELF
    /// (dailies, capture) into a configured notes directory: if that directory
    /// is a retired KB's former home, the note belongs to that KB, in its store
    /// — never in a file no ingest will read.
    SourceOrRetiredOrigin,
}

/// Is `path` at or under `root`? False for an empty root — an empty `org_dir`
/// or `origin` means "none", and every path starts with the empty path.
fn is_under(path: &Path, path_canon: &Path, root: &Path) -> bool {
    !root.as_os_str().is_empty()
        && (path.starts_with(root) || path_canon.starts_with(canonical_lenient(root)))
}

impl Editor {
    /// The KB instance that owns `path`, if any — the MOST SPECIFIC one when
    /// roots nest, so a KB registered inside another KB's directory owns its
    /// own files.
    pub(crate) fn kb_dir_owner(
        &self,
        path: &Path,
        scope: DirOwnership,
    ) -> Option<&mae_kb::federation::KbInstance> {
        let canon = canonical_lenient(path);
        let depth = |p: &Path| p.components().count();
        let instances = &self.kb.registry.instances;

        let by_source = instances
            .iter()
            .filter(|i| is_under(path, &canon, &i.org_dir))
            .max_by_key(|i| depth(&i.org_dir));
        if by_source.is_some() || scope == DirOwnership::Source {
            return by_source;
        }

        // Retired: store is truth AND `org_dir` cleared — the native state
        // retirement produces. A detached-but-unretired KB still has its
        // `org_dir` and was matched above.
        instances
            .iter()
            .filter(|i| !i.ingest_policy.allows_ingest() && i.org_dir.as_os_str().is_empty())
            .filter_map(|i| Some((i, &i.import_record.as_ref()?.origin)))
            .filter(|(_, origin)| is_under(path, &canon, origin))
            .max_by_key(|(_, origin)| depth(origin))
            .map(|(i, _)| i)
    }

    /// Is the STORE the source of truth for whichever KB owns `dir`?
    ///
    /// The directory-addressed twin of [`Editor::kb_store_is_truth_for`], which
    /// answers the same question for a node id. Capture and dailies need this
    /// one, because what they hold is a PATH (`kb_notes_dir` /
    /// `kb_dailies_dir`), not an id.
    ///
    /// @ai-caution: [kb-policy] Ask about the OWNER of the directory, never
    /// about the primary: `kb_insert_to_notes_instance` treats "a registered
    /// instance covers `kb_notes_dir`" as the normal case. And the owner
    /// includes a RETIRED KB's origin — see this module's header for what
    /// happened when it did not.
    pub(crate) fn kb_dir_store_is_truth(&self, dir: &Path) -> bool {
        match self.kb_dir_owner(dir, DirOwnership::SourceOrRetiredOrigin) {
            Some(inst) => !inst.ingest_policy.allows_ingest(),
            // No registered instance covers it, so it belongs to the primary,
            // whose policy lives on the registry rather than on any row.
            None => self.kb.primary_store_is_truth(),
        }
    }

    /// The uuid of the KB that a MAE-originated write into `dir` belongs to.
    pub fn kb_dir_write_owner(&self, dir: &Path) -> Option<String> {
        self.kb_dir_owner(dir, DirOwnership::SourceOrRetiredOrigin)
            .map(|i| i.uuid.clone())
    }
}
