//! The attachment directory of an `attachment:` link, as Emacs's
//! org-attach derives it (ADR-115 Amendment 1, E2), so the rule lives in
//! one place for every host.
//!
//! From `org-attach.el` (Emacs 30): `org-attach-dir` reads `DIR`, then
//! `ATTACH_DIR`, then `ID`, each with `org-attach-use-inheritance`, which
//! MAE fixes on (Emacs's default, `selective`, follows
//! `org-use-property-inheritance`, nil by default). So:
//! - the ID is the nearest `:ID:` up the outline from the link: its
//!   headline's, else an ancestor's, else the file's property drawer, else
//!   `#+PROPERTY: ID …` (one [`Inherited`] reading, the one babel uses);
//! - a `DIR` or `ATTACH_DIR` found the same way (at the link's headline or
//!   ANY ancestor, the file and `#+PROPERTY:` included, even above the ID's
//!   headline: Org reads it first) means no directory: v1 does not support
//!   them, and seeding refuses a node that has either (E2);
//! - the directory is `org-attach-id-dir` (`data/`) joined with the first of
//!   `org-attach-id-to-path-function-list` that applies: the UUID format
//!   `ID[0:2]/ID[2:]` for an ID longer than two characters (characters, as
//!   Emacs's `substring` counts them), else the fallback `__/ID[0]/ID`. The
//!   timestamp format (longer than six) never comes first, as any ID it
//!   accepts the UUID format accepts too.
//!
//! Deliberate differences (`conformance/semantics-divergences.toml`): Emacs
//! takes the first format whose folder already EXISTS on disk and, when
//! none does, exports the link as a file next to the Org file; MAE takes the
//! first that applies, so every peer derives the same directory with no
//! I/O. An empty `:ID:` (Emacs signals an error) gives no directory.
//!
//! **Path safety is the host's.** The directory is built from the ID as
//! written, and the link's target is reported verbatim: neither is
//! validated, normalised or joined here, so an ID or a NAME holding `..`,
//! `/` or `%2e%2e` reaches the host as written. A host that reads a file
//! must check the path it builds.

use super::SemanticLink;
use crate::inherit::Inherited;
use crate::link_grammar::LinkKind;
use crate::Document;

/// The scheme of an attachment link, as Org's link type is written: case
/// matters (`[[ATTACHMENT:x]]` is a link of another type to Org, which
/// `org-attach-expand-links` does not expand).
pub(crate) const ATTACHMENT_SCHEME: &str = "attachment:";

/// `org-attach-id-dir`'s default.
const ID_DIR: &str = "data/";

/// The attachment directory of the entry with ID `id`, relative to the Org
/// file's own directory, with a trailing `/`: `data/ab/cdef…/` for an ID
/// longer than two characters, `data/__/a/ab/` otherwise (ADR-115 E2).
/// `None` for an empty ID. Built from `id` as written: see the module doc
/// on path safety.
pub fn attachment_dir(id: &str) -> Option<String> {
    let mut chars = id.char_indices();
    let first = chars.next()?;
    match chars.nth(1) {
        // A third character: the UUID format.
        Some(_) => {
            let split = id.char_indices().nth(2).map_or(id.len(), |(i, _)| i);
            Some(format!("{ID_DIR}{}/{}/", &id[..split], &id[split..]))
        }
        None => {
            let c = &id[..first.0 + first.1.len_utf8()];
            Some(format!("{ID_DIR}__/{c}/{id}/"))
        }
    }
}

/// Sets `attach_id` and `attach_dir` on every `attachment:` link of
/// `links` (spans in `doc`).
pub(crate) fn fill(doc: &Document, links: &mut [SemanticLink]) {
    let mut inherited: Option<Inherited> = None;
    for l in links
        .iter_mut()
        .filter(|l| l.kind == LinkKind::Other && l.target.starts_with(ATTACHMENT_SCHEME))
    {
        let props = inherited.get_or_insert_with(|| Inherited::new(doc));
        let headline = props.headline_at(l.span.start);
        let unsupported = ["DIR", "ATTACH_DIR"]
            .iter()
            .any(|p| props.get(p, headline.as_ref()).is_some());
        if unsupported {
            continue;
        }
        let Some(id) = props.get("ID", headline.as_ref()) else {
            continue;
        };
        if let Some(dir) = attachment_dir(&id) {
            l.attach_dir = Some(dir);
            l.attach_id = Some(id);
        }
    }
}
