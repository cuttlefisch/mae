//! The explorer (P-39 to P-45): every published page, grouped by tag
//! (ADR-119 Amendment 3, owner's decision; a native KB has no folders, B1).
//!
//! One group per tag, in the tag index's order, each listing its pages A to Z
//! ([`crate::order`]); a page with several tags is in each of their groups,
//! and a page with none is in `untagged`, once. Every published page is
//! therefore listed at least once, and nothing else is.
//!
//! It lists the whole KB, so it is one of the bulk outputs a query-only
//! member is refused (D7); the caller decides who may have it.

use crate::derive::{PageRef, Site};
use crate::order::cmp_text;

/// The explorer of one derived KB.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Explorer {
    pub groups: Vec<Group>,
    /// The pages with no tag, A to Z.
    pub untagged: Vec<PageRef>,
}

/// One tag's pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub tag: String,
    pub pages: Vec<PageRef>,
}

/// The explorer of `site`.
pub fn explorer(site: &Site) -> Explorer {
    let groups = site
        .tags_in_order()
        .into_iter()
        .map(|(tag, pages)| Group {
            tag: tag.to_string(),
            pages: pages.to_vec(),
        })
        .collect();
    let mut untagged: Vec<PageRef> = site
        .pages
        .values()
        .filter(|p| p.tags.is_empty())
        .map(|p| PageRef {
            title: p.title.clone(),
            id: p.id.clone(),
            href: p.href.clone(),
        })
        .collect();
    untagged.sort_by(by_title);
    Explorer { groups, untagged }
}

/// Pages A to Z by title, then by ID: the order of every page list.
pub(crate) fn by_title(a: &PageRef, b: &PageRef) -> std::cmp::Ordering {
    cmp_text(&a.title, &b.title).then_with(|| a.cmp(b))
}
