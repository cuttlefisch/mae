//! Every headline's properties, whether or not the headline is a KB node.
//!
//! [`crate::semantics`] lists only headlines with an `:ID:`, because those are
//! nodes. A page also needs the properties of the other headlines: a
//! `:VISIBILITY: folded` on a section that has only a `CUSTOM_ID`, for one.
//! This reads them from the same view the rest of the crate uses
//! ([`Headline::property_drawer`]), so a drawer Org does not read as the
//! headline's (one after a blank line, or further down the section) is not
//! listed here either. A lowercase `:properties:`…`:end:` drawer is the
//! headline's, as in Org (`org-entry-put` writes into one; see the
//! `CUSTOM_ID` corpus).
//!
//! Offsets are UTF-8 byte offsets into the text, like every span here.

use crate::custom_id::{heading_line, in_scope};
use crate::views::Headline;
use crate::{parse, ParseOptions, Span};

/// One `:KEY: value` line of a headline's property drawer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlineProperty {
    /// Canonical form: keys Org's case folding equates are equal (ASCII shown
    /// upper case). An appending key keeps its `+` (`MULTI+`).
    pub key: String,
    /// The key exactly as written.
    pub key_as_written: String,
    /// The value as Org reads it (trimmed).
    pub value: String,
}

/// A headline that has a property drawer, and that drawer's lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlineProperties {
    /// The headline line, without its line ending.
    pub heading: Span,
    /// The number of stars.
    pub level: usize,
    /// Every property line in file order, repeated keys included: Org reads
    /// the last occurrence of a key, and the caller can too.
    pub properties: Vec<HeadlineProperty>,
}

/// The headlines that have a property drawer, in document order, each with
/// its properties. With `ranges`, only headlines whose headline line starts
/// inside one (the start included, the end excluded), as in
/// [`crate::headings_missing_custom_id`]. A headline with no drawer is not
/// listed; one with an empty drawer is, with no properties.
pub fn headline_properties(
    text: &str,
    options: &ParseOptions,
    ranges: Option<&[Span]>,
) -> Vec<HeadlineProperties> {
    crate::views::headline_nodes(&parse(text, options).syntax())
        .into_iter()
        .filter_map(Headline::cast)
        .filter(|h| in_scope(h, ranges))
        .filter_map(|h| {
            let drawer = h.property_drawer()?;
            Some(HeadlineProperties {
                heading: heading_line(&h),
                level: h.level(),
                properties: drawer
                    .properties()
                    .into_iter()
                    .map(|p| HeadlineProperty {
                        key_as_written: p.key_as_written(),
                        key: p.key,
                        value: p.value,
                    })
                    .collect(),
            })
        })
        .collect()
}
