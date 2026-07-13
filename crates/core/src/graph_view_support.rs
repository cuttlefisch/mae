//! `shared_kb::NodeKind` <-> `mae_canvas::scene::NodeKind` conversion.
//!
//! Phase 0 prep for the native KB graph view (see the architecture plan's
//! "Part C" and its Phase 0 checklist): `mae-canvas` is deliberately kept a
//! leaf crate with no dependency on `mae-kb`, so its `NodeKind` is a
//! *structural* mirror of `shared_kb::NodeKind` (same 14 variants, same
//! names) rather than the same type. `mae-core` is the first crate in the
//! dependency graph that can see both `mae-kb` and `mae-canvas`, so the
//! conversion lives here rather than as a `From` impl in either leaf crate
//! (which the orphan rule would forbid anyway — neither crate owns both
//! types).
//!
//! No `GraphView` feature is built yet (that's Phase 1); this module exists
//! purely so a future caller (`crates/core/src/graph_view.rs`,
//! `build_kb_graph`'s eventual invoker) has a single, tested place to reach
//! for this conversion instead of writing it ad hoc.

/// Convert a `shared_kb::NodeKind` to its structural mirror in
/// `mae_canvas::scene::NodeKind`. A plain `match`, not a `From` impl, since
/// neither type is local to this crate (Rust's orphan rule).
pub fn shared_kind_to_canvas_kind(kind: mae_kb::NodeKind) -> mae_canvas::scene::NodeKind {
    use mae_canvas::scene::NodeKind as Canvas;
    use mae_kb::NodeKind as Kb;
    match kind {
        Kb::Index => Canvas::Index,
        Kb::Command => Canvas::Command,
        Kb::Concept => Canvas::Concept,
        Kb::Key => Canvas::Key,
        Kb::Note => Canvas::Note,
        Kb::Project => Canvas::Project,
        Kb::Category => Canvas::Category,
        Kb::Lesson => Canvas::Lesson,
        Kb::Tutorial => Canvas::Tutorial,
        Kb::Meta => Canvas::Meta,
        Kb::Block => Canvas::Block,
        Kb::SchemeApi => Canvas::SchemeApi,
        Kb::Task => Canvas::Task,
        Kb::View => Canvas::View,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `shared_kb::NodeKind` variant must round-trip through its
    /// exact-named canvas counterpart — the whole point of keeping the two
    /// enums structurally identical. If a future variant is added to one
    /// side without the other, this (and the compiler's non-exhaustive
    /// match error) catches the drift.
    #[test]
    fn every_kb_node_kind_maps_to_its_same_named_canvas_variant() {
        let cases = [
            (mae_kb::NodeKind::Index, "Index"),
            (mae_kb::NodeKind::Command, "Command"),
            (mae_kb::NodeKind::Concept, "Concept"),
            (mae_kb::NodeKind::Key, "Key"),
            (mae_kb::NodeKind::Note, "Note"),
            (mae_kb::NodeKind::Project, "Project"),
            (mae_kb::NodeKind::Category, "Category"),
            (mae_kb::NodeKind::Lesson, "Lesson"),
            (mae_kb::NodeKind::Tutorial, "Tutorial"),
            (mae_kb::NodeKind::Meta, "Meta"),
            (mae_kb::NodeKind::Block, "Block"),
            (mae_kb::NodeKind::SchemeApi, "SchemeApi"),
            (mae_kb::NodeKind::Task, "Task"),
            (mae_kb::NodeKind::View, "View"),
        ];
        for (kb_kind, expected_name) in cases {
            let canvas_kind = shared_kind_to_canvas_kind(kb_kind);
            assert_eq!(
                format!("{canvas_kind:?}"),
                expected_name,
                "mae_kb::NodeKind::{kb_kind:?} must map to mae_canvas::scene::NodeKind::{expected_name}"
            );
        }
    }
}
