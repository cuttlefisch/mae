//! The remaining object parsers. Each returns `None` until implemented; the
//! dispatcher in `mod.rs` calls them in `org-element--object-lex`'s order.

#![allow(unused_variables)]

use super::{InlineCx, Obj};

pub(crate) fn radio_link(cx: &InlineCx, i: usize, end: usize, floor: usize) -> Option<Obj> {
    None
}
pub(crate) fn table_cell(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    None
}
pub(crate) fn citation_reference(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    None
}
pub(crate) fn inline_babel_call(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    None
}
pub(crate) fn inline_src_block(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    None
}
pub(crate) fn superscript(cx: &InlineCx, i: usize, end: usize, floor: usize) -> Option<Obj> {
    None
}
pub(crate) fn subscript(cx: &InlineCx, i: usize, end: usize, floor: usize) -> Option<Obj> {
    None
}
pub(crate) fn export_snippet(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    None
}
pub(crate) fn macro_(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    None
}
pub(crate) fn latex_fragment(cx: &InlineCx, i: usize, end: usize, floor: usize) -> Option<Obj> {
    None
}
pub(crate) fn radio_target(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    None
}
pub(crate) fn target(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    None
}
pub(crate) fn timestamp(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    None
}
pub(crate) fn line_break(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    None
}
pub(crate) fn entity(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    None
}
pub(crate) fn footnote_reference(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    None
}
pub(crate) fn citation(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    None
}
pub(crate) fn statistics_cookie(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    None
}
