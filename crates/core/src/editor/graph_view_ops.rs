//! Native KB graph view operations (`BufferKind::Graph`) — Part C Phase 1.
//!
//! Mirrors `debug_panel_ops.rs`'s relationship to `BufferKind::Debug`:
//! find-or-create the dedicated buffer, populate it from live state,
//! refresh in place without re-splitting or stealing focus. The graph
//! view's data source is `KnowledgeBase::extract_subgraph` (not the
//! `kb-graph`/`kb_graph` BFS primitives — those answer raw queries; this is
//! the view's actual backing data, per the architecture plan). Layout is
//! computed off the main thread by `mae::graph_layout_bridge`; this file
//! only ever produces the CHEAP initial circular layout inline
//! (`build_kb_graph_positions_only`) and queues the force-directed refine
//! pass via `Editor::pending_graph_layout`.

use crate::buffer::{Buffer, BufferKind};
use crate::graph_view::{
    flatten_scene_graph, GraphLayoutIntent, GraphNavDirection, GraphStyleOptions,
};
use crate::visual_buffer::VisualBuffer;
use crate::window::WindowId;

use super::Editor;

impl Editor {
    /// Find or create the `*KB Graph*` buffer. Returns buffer index.
    fn ensure_graph_buffer_idx(&mut self) -> usize {
        if let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
        {
            return idx;
        }
        self.buffers.push(Buffer::new_graph());
        self.buffers.len() - 1
    }

    /// Resolve the node id to center the graph on: an explicit `center`,
    /// else whichever KB node the (first) open `*KB*` buffer is currently
    /// displaying, else `"index"`.
    fn resolve_graph_center(&self, center: Option<String>) -> String {
        center
            .or_else(|| {
                self.buffers
                    .iter()
                    .find_map(|b| b.kb_view().map(|kv| kv.current.clone()))
            })
            .unwrap_or_else(|| "index".to_string())
    }

    /// Build `GraphStyleOptions` from the current option values + theme.
    /// `pub(crate)` — used by `graph_view_ops.rs` itself and exercised
    /// directly by unit tests in this file.
    pub(crate) fn graph_style_options(&self) -> GraphStyleOptions {
        GraphStyleOptions::from_editor(self)
    }

    /// Rebuild `GraphView.scene`/`rendered` for `buf_idx` from a fresh
    /// `extract_subgraph` around `center` at `depth` hops, resolving which
    /// KB instance owns `center` via `kb_owner_of` (federated KB scoping —
    /// graph queries never cross instances, so this always resolves to
    /// exactly one). Queues the force-directed layout refinement pass on
    /// the background bridge rather than running it inline.
    fn populate_graph_buffer(&mut self, buf_idx: usize, center: String, depth: usize) {
        let spec = mae_kb::SubgraphSpec {
            starter_nodes: vec![center.clone()],
            max_depth: depth,
            include_backlinks: self.kb_graph_include_backlinks,
        };
        let owner = self.kb_owner_of(&center);
        let empty_result = || mae_kb::SubgraphResult {
            nodes: Vec::new(),
            links: Vec::new(),
            boundary_links: Vec::new(),
        };
        let result = match &owner {
            Some(None) => self.kb.primary.extract_subgraph(&spec),
            Some(Some(uuid)) => self
                .kb
                .instances
                .get(uuid)
                .map(|kb| kb.extract_subgraph(&spec))
                .unwrap_or_else(empty_result),
            None => empty_result(),
        };
        let kb_instance = match owner {
            Some(Some(uuid)) => Some(uuid),
            _ => None,
        };

        let kb_nodes: Vec<mae_canvas::kb_graph::KbNodeInfo> = result
            .nodes
            .iter()
            .map(|n| mae_canvas::kb_graph::KbNodeInfo {
                id: n.id.clone(),
                title: n.title.clone(),
                kind: crate::graph_view_support::shared_kind_to_canvas_kind(n.kind),
            })
            .collect();

        let scene = mae_canvas::kb_graph::build_kb_graph_positions_only(
            &kb_nodes,
            &result.links,
            &result.boundary_links,
            std::slice::from_ref(&center),
        );

        // Queue the background layout refine pass with a snapshot of the
        // fresh circular-layout scene BEFORE moving `scene` into the view
        // below, so the background pass refines the same data the view now
        // holds (not a stale prior scene).
        self.pending_graph_layout = Some(GraphLayoutIntent {
            buf_idx,
            scene: scene.clone(),
            iterations: self.kb_graph_layout_iterations,
        });

        let style = self.graph_style_options();
        let rendered = VisualBuffer {
            elements: flatten_scene_graph(&scene, &style),
        };

        if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
            gv.center_node = Some(center);
            gv.depth = depth;
            gv.kb_instance = kb_instance;
            gv.scene = scene;
            gv.rendered = rendered;
        }
    }

    /// Open the KB graph view, centered on `center` (default: whichever KB
    /// node the `*KB*` buffer is currently showing, else `"index"`), at
    /// `depth` hops (default: `kb_graph_default_depth`). Reuses the
    /// existing `*KB Graph*` window if one is already open (via
    /// `display_buffer`'s `ReuseOrSplit` policy), never re-splits.
    pub fn kb_graph_view_open(&mut self, center: Option<String>, depth: Option<usize>) {
        let center = self.resolve_graph_center(center);
        let depth = depth.unwrap_or(self.kb_graph_default_depth);
        let buf_idx = self.ensure_graph_buffer_idx();
        if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
            gv.follow_current = self.kb_graph_follow_current_node;
        }
        self.populate_graph_buffer(buf_idx, center, depth);
        self.display_buffer(buf_idx);
    }

    /// Close the graph view buffer (mirrors `close_debug_panel`).
    pub fn kb_graph_view_close(&mut self) {
        let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
        else {
            return;
        };

        let alt = self.vi.alternate_buffer_idx.unwrap_or(0);
        let target = if alt < self.buffers.len() && alt != idx {
            alt
        } else {
            self.buffers
                .iter()
                .position(|b| b.kind != BufferKind::Graph)
                .unwrap_or(0)
        };
        self.switch_to_buffer(target);

        self.buffers.remove(idx);
        self.notify_buffer_removed(idx);

        for win in self.window_mgr.iter_windows_mut() {
            if win.buffer_idx == idx {
                win.buffer_idx = target.min(self.buffers.len().saturating_sub(1));
            } else if win.buffer_idx > idx {
                win.buffer_idx -= 1;
            }
        }
    }

    /// Refresh the graph view buffer in place if it exists — same center
    /// node/depth, freshly re-extracted data. Template:
    /// `debug_panel_refresh_if_open`. Never re-splits or steals focus.
    pub fn kb_graph_view_refresh_if_open(&mut self) {
        let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
        else {
            return;
        };
        let Some(center) = self.buffers[idx]
            .graph_view()
            .and_then(|gv| gv.center_node.clone())
        else {
            return;
        };
        let depth = self.buffers[idx]
            .graph_view()
            .map(|gv| gv.depth)
            .unwrap_or(self.kb_graph_default_depth);
        self.populate_graph_buffer(idx, center, depth);
        self.mark_full_redraw();
    }

    /// Change the graph view's hop radius and refresh in place. No-op if
    /// the graph view isn't open.
    pub fn kb_graph_view_set_depth(&mut self, depth: usize) {
        let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
        else {
            return;
        };
        let Some(center) = self.buffers[idx]
            .graph_view()
            .and_then(|gv| gv.center_node.clone())
        else {
            return;
        };
        self.populate_graph_buffer(idx, center, depth);
        self.mark_full_redraw();
    }

    /// Move the graph's selection cursor toward `dir` (wraps
    /// `canvas::interaction::navigate_direction`) and re-flatten so the
    /// selection highlight is visible without a full data refetch.
    pub fn kb_graph_view_navigate(&mut self, dir: GraphNavDirection) {
        let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
        else {
            return;
        };
        if let Some(gv) = self.buffers[idx].graph_view_mut() {
            mae_canvas::interaction::navigate_direction(&mut gv.scene, dir.into());
        }
        let style = self.graph_style_options();
        if let Some(gv) = self.buffers[idx].graph_view_mut() {
            gv.rendered = VisualBuffer {
                elements: flatten_scene_graph(&gv.scene, &style),
            };
        }
        self.mark_full_redraw();
    }

    /// Navigate the graph's captured companion window (Part A
    /// `DrivenWindow`) to `node_id`'s KB buffer. Shared by
    /// `kb_graph_view_select_current` (keyboard/Scheme/MCP: "select the
    /// currently-highlighted node") and `kb_graph_view_click_at` (mouse:
    /// "navigate to the clicked node") — both resolve *which* node
    /// differently but then must do the exact same navigation, so this is
    /// pure code motion out of what was previously
    /// `kb_graph_view_select_current`'s body, not new logic. Uses the direct
    /// window-buffer-write idiom, bypassing `display_buffer`'s
    /// reuse-or-split policy entirely — falls back to a normal
    /// `display_buffer` split if no valid companion window is captured yet,
    /// and that new window becomes the companion for next time.
    fn navigate_companion_window_to_node(&mut self, graph_idx: usize, node_id: &str) {
        self.kb.record_visit(node_id);
        let kb_buf_idx = self.ensure_kb_buffer_idx(node_id);
        self.kb_populate_buffer(kb_buf_idx);

        let companion_valid = self.buffers[graph_idx]
            .graph_view()
            .and_then(|gv| gv.companion_window.get_valid(&self.window_mgr));

        if let Some(win_id) = companion_valid {
            if let Some(win) = self.window_mgr.window_mut(win_id) {
                win.buffer_idx = kb_buf_idx;
                win.cursor_row = 0;
                win.cursor_col = 0;
            }
            self.mark_full_redraw();
        } else {
            self.display_buffer(kb_buf_idx);
            let new_win_id = self
                .window_mgr
                .iter_windows()
                .find(|w| w.buffer_idx == kb_buf_idx)
                .map(|w| w.id);
            if let Some(gv) = self.buffers[graph_idx].graph_view_mut() {
                gv.companion_window.set(new_win_id);
            }
        }
    }

    /// Navigate the captured companion window (Part A `DrivenWindow`) to
    /// the currently-selected graph node's KB buffer. See
    /// `navigate_companion_window_to_node` for the shared navigation logic.
    pub fn kb_graph_view_select_current(&mut self) {
        let Some(graph_idx) = self
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
        else {
            return;
        };
        let Some(node_id) = self.buffers[graph_idx]
            .graph_view()
            .and_then(|gv| gv.scene.selected_node())
            .map(|n| n.id.clone())
        else {
            return;
        };
        self.navigate_companion_window_to_node(graph_idx, &node_id);
    }

    /// Mouse click-to-navigate (Part C Phase 1 item 6): `graph_win_id` is
    /// the window a click just landed in (already confirmed by the caller —
    /// `gui_app.rs`'s `handle_mouse_button_pressed` — to be showing a
    /// `BufferKind::Graph` buffer); `rel_x`/`rel_y` are the click's pixel
    /// position relative to that window's content rect (top-left origin),
    /// NOT raw screen pixels and NOT text cells — the graph is drawn via
    /// the Skia `VisualBuffer` pixel pipeline, not the text-cell layout
    /// pipeline.
    ///
    /// Converts to scene coordinates and hit-tests against the graph's
    /// `SceneGraph`. On a hit: selects that node (mirroring keyboard-nav's
    /// selection-highlight behavior) and navigates the captured companion
    /// window to it via the same shared logic `kb_graph_view_select_current`
    /// uses. On a miss (click landed on empty canvas): clears the
    /// selection — an explicit deselect, since the user visibly clicked
    /// empty space — and is otherwise a harmless no-op (no navigation, no
    /// panic). Either way, `rendered` is re-flattened so the (de)selection
    /// highlight is visible immediately, matching `kb_graph_view_navigate`'s
    /// behavior.
    ///
    /// NOTE: `flatten_scene_graph` (the current GUI renderer for
    /// `BufferKind::Graph`, see `crates/gui/src/lib.rs`'s
    /// `render_visual_buffer_with_bg`) does not yet apply
    /// `SceneGraph.viewport`'s pan/zoom transform to node positions — it
    /// draws `SceneNode.x/y` directly as window-relative pixel offsets.
    /// Phase 4 (drag-to-pin/wheel-zoom, explicitly out of scope here) is
    /// where a real camera transform gets wired into the renderer and this
    /// hit-test path together. Until then, `viewport_to_scene` is called
    /// with the viewport's width/height zeroed out (neutralizing its
    /// centering term, `(screen - dimension/2)/zoom`) so hit-testing stays
    /// consistent with what's actually drawn on screen today — using the
    /// viewport's `zoom`/`center` unchanged keeps this forward-compatible
    /// with whenever Phase 4 wires pan/zoom into both sides together.
    pub fn kb_graph_view_click_at(&mut self, graph_win_id: WindowId, rel_x: f32, rel_y: f32) {
        let Some(buf_idx) = self.window_mgr.window(graph_win_id).map(|w| w.buffer_idx) else {
            return;
        };
        if buf_idx >= self.buffers.len() || self.buffers[buf_idx].kind != BufferKind::Graph {
            return;
        }

        let hit_result: Option<Option<usize>> = self.buffers[buf_idx].graph_view().map(|gv| {
            let vp = &gv.scene.viewport;
            let neutral_vp = mae_canvas::scene::Viewport {
                width: 0.0,
                height: 0.0,
                zoom: vp.zoom,
                center_x: vp.center_x,
                center_y: vp.center_y,
            };
            let (scene_x, scene_y) =
                mae_canvas::interaction::viewport_to_scene(&neutral_vp, rel_x as f64, rel_y as f64);
            mae_canvas::interaction::hit_test(&gv.scene, scene_x, scene_y)
        });
        let Some(node_hit) = hit_result else {
            return;
        };

        if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
            gv.scene.selection = node_hit;
        }
        let style = self.graph_style_options();
        if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
            gv.rendered = VisualBuffer {
                elements: flatten_scene_graph(&gv.scene, &style),
            };
        }
        self.mark_full_redraw();

        if let Some(node_idx) = node_hit {
            let node_id = self.buffers[buf_idx]
                .graph_view()
                .and_then(|gv| gv.scene.nodes.get(node_idx))
                .map(|n| n.id.clone());
            if let Some(node_id) = node_id {
                self.navigate_companion_window_to_node(buf_idx, &node_id);
            }
        }
    }

    /// Apply a completed background layout (`mae::graph_layout_bridge`)
    /// back onto the owning `GraphView`, re-flattening for render and
    /// marking the buffer dirty. No-op if the buffer no longer exists or is
    /// no longer a `BufferKind::Graph` buffer (closed mid-flight).
    pub fn apply_graph_layout_result(
        &mut self,
        buf_idx: usize,
        scene: mae_canvas::scene::SceneGraph,
    ) {
        if buf_idx >= self.buffers.len() || self.buffers[buf_idx].kind != BufferKind::Graph {
            return;
        }
        let style = self.graph_style_options();
        if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
            gv.scene = scene;
            gv.rendered = VisualBuffer {
                elements: flatten_scene_graph(&gv.scene, &style),
            };
        }
        self.mark_full_redraw();
    }

    /// Focus-capture hook (Part A `DrivenWindow::follow_focus_away_from`):
    /// call whenever editor focus changes to any window — keyboard/Scheme-
    /// driven (`dispatch_builtin`'s wrapper) or mouse-driven
    /// (`focus_window_at`), the two points that cover "every focus change"
    /// without a single shared low-level `set_focused` call site (keyboard
    /// window-nav commands call `WindowManager::focus_direction` directly;
    /// mouse clicks call `Editor::focus_window_at` directly — see those
    /// call sites' comments). Updates the graph view's captured companion
    /// window when a `BufferKind::Graph` window exists and focus moved to a
    /// DIFFERENT window than it; cheap no-op otherwise.
    pub(crate) fn capture_graph_companion_focus(&mut self, newly_focused: WindowId) {
        let Some(graph_buf_idx) = self
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
        else {
            return;
        };
        let Some(graph_win_id) = self
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == graph_buf_idx)
            .map(|w| w.id)
        else {
            return;
        };
        if let Some(gv) = self.buffers[graph_buf_idx].graph_view_mut() {
            gv.companion_window
                .follow_focus_away_from(newly_focused, graph_win_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::SplitDirection;

    fn ed_with_kb_node(id: &str, title: &str, body: &str) -> Editor {
        let mut editor = Editor::new();
        editor.kb.primary.insert(mae_kb::Node::new(
            id,
            title,
            mae_kb::NodeKind::Concept,
            body,
        ));
        editor
    }

    #[test]
    fn open_creates_graph_buffer_and_displays_it() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "[[concept:window]]");
        editor.kb.primary.insert(mae_kb::Node::new(
            "concept:window",
            "Window",
            mae_kb::NodeKind::Concept,
            "",
        ));

        assert!(!editor.buffers.iter().any(|b| b.kind == BufferKind::Graph));
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), Some(1));

        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .expect("graph buffer should exist");
        assert_eq!(
            editor.buffers[idx]
                .graph_view()
                .unwrap()
                .center_node
                .as_deref(),
            Some("concept:buffer")
        );
        assert_eq!(editor.buffers[idx].graph_view().unwrap().depth, 1);
        // Displayed somewhere.
        assert!(editor
            .window_mgr
            .iter_windows()
            .any(|w| w.buffer_idx == idx));
        // At least the center node is present.
        assert!(!editor.buffers[idx]
            .graph_view()
            .unwrap()
            .scene
            .nodes
            .is_empty());
    }

    #[test]
    fn open_defaults_center_to_open_kb_view() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        let kb_idx = editor.buffers.len();
        editor.buffers.push(Buffer::new_kb("concept:buffer"));
        let _ = kb_idx;

        editor.kb_graph_view_open(None, None);

        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        assert_eq!(
            editor.buffers[idx]
                .graph_view()
                .unwrap()
                .center_node
                .as_deref(),
            Some("concept:buffer")
        );
    }

    #[test]
    fn open_defaults_center_to_index_when_no_kb_view() {
        let mut editor = Editor::new();
        editor.kb_graph_view_open(None, None);
        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        assert_eq!(
            editor.buffers[idx]
                .graph_view()
                .unwrap()
                .center_node
                .as_deref(),
            Some("index")
        );
    }

    #[test]
    fn open_twice_reuses_the_same_buffer_not_a_new_split() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb.primary.insert(mae_kb::Node::new(
            "concept:other",
            "Other",
            mae_kb::NodeKind::Concept,
            "",
        ));

        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let window_count_after_first = editor.window_mgr.iter_windows().count();
        let graph_buf_count_after_first = editor
            .buffers
            .iter()
            .filter(|b| b.kind == BufferKind::Graph)
            .count();

        editor.kb_graph_view_open(Some("concept:other".to_string()), None);
        let window_count_after_second = editor.window_mgr.iter_windows().count();
        let graph_buf_count_after_second = editor
            .buffers
            .iter()
            .filter(|b| b.kind == BufferKind::Graph)
            .count();

        assert_eq!(graph_buf_count_after_first, 1);
        assert_eq!(
            graph_buf_count_after_first, graph_buf_count_after_second,
            "opening again must not create a second *KB Graph* buffer"
        );
        assert_eq!(
            window_count_after_first, window_count_after_second,
            "opening again must reuse the existing window, not split again"
        );
        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        assert_eq!(
            editor.buffers[idx]
                .graph_view()
                .unwrap()
                .center_node
                .as_deref(),
            Some("concept:other"),
            "reopening must update the center node in place"
        );
    }

    #[test]
    fn close_removes_the_graph_buffer() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        assert!(editor.buffers.iter().any(|b| b.kind == BufferKind::Graph));

        editor.kb_graph_view_close();
        assert!(!editor.buffers.iter().any(|b| b.kind == BufferKind::Graph));
    }

    #[test]
    fn close_when_not_open_is_a_harmless_no_op() {
        let mut editor = Editor::new();
        let buf_count_before = editor.buffers.len();
        editor.kb_graph_view_close();
        assert_eq!(editor.buffers.len(), buf_count_before);
    }

    #[test]
    fn refresh_if_open_repopulates_without_resplitting() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let window_count_before = editor.window_mgr.iter_windows().count();

        // Simulate the underlying KB changing between opens.
        editor.kb.primary.insert(mae_kb::Node::new(
            "concept:new",
            "New",
            mae_kb::NodeKind::Concept,
            "[[concept:buffer]]",
        ));
        editor.kb_graph_view_refresh_if_open();

        assert_eq!(
            editor.window_mgr.iter_windows().count(),
            window_count_before,
            "refresh must never open/close windows"
        );
        assert_eq!(
            editor
                .buffers
                .iter()
                .filter(|b| b.kind == BufferKind::Graph)
                .count(),
            1,
            "refresh must never create a second graph buffer"
        );
        // Same buffer index, still centered the same.
        assert_eq!(
            editor.buffers[idx]
                .graph_view()
                .unwrap()
                .center_node
                .as_deref(),
            Some("concept:buffer")
        );
    }

    #[test]
    fn refresh_if_open_is_a_no_op_when_not_open() {
        let mut editor = Editor::new();
        // Must not panic even though no graph buffer/center exists.
        editor.kb_graph_view_refresh_if_open();
        assert!(!editor.buffers.iter().any(|b| b.kind == BufferKind::Graph));
    }

    #[test]
    fn set_depth_updates_depth_and_refreshes_in_place() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), Some(1));
        let window_count_before = editor.window_mgr.iter_windows().count();

        editor.kb_graph_view_set_depth(3);

        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        assert_eq!(editor.buffers[idx].graph_view().unwrap().depth, 3);
        assert_eq!(
            editor.window_mgr.iter_windows().count(),
            window_count_before
        );
    }

    #[test]
    fn navigate_moves_selection_and_reflattens() {
        let mut editor = ed_with_kb_node("concept:a", "A", "[[concept:b]]");
        editor.kb.primary.insert(mae_kb::Node::new(
            "concept:b",
            "B",
            mae_kb::NodeKind::Concept,
            "",
        ));
        editor.kb_graph_view_open(Some("concept:a".to_string()), Some(1));

        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let before = editor.buffers[idx].graph_view().unwrap().scene.selection;
        editor.kb_graph_view_navigate(GraphNavDirection::Right);
        // Selection is either unchanged (no node in that direction) or
        // moved — either way, `rendered` must reflect the current scene
        // (non-empty for a 2-node graph).
        let after = editor.buffers[idx].graph_view().unwrap();
        assert!(before.is_some() || after.scene.selection.is_some());
        assert!(!after.rendered.elements.is_empty());
    }

    #[test]
    fn navigate_when_not_open_is_a_harmless_no_op() {
        let mut editor = Editor::new();
        editor.kb_graph_view_navigate(GraphNavDirection::Down);
        assert!(!editor.buffers.iter().any(|b| b.kind == BufferKind::Graph));
    }

    #[test]
    fn select_current_falls_back_to_a_split_when_no_companion_captured() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);

        editor.kb_graph_view_select_current();

        let kb_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Kb)
            .expect("a KB buffer should have been created for the selected node");
        assert!(editor
            .window_mgr
            .iter_windows()
            .any(|w| w.buffer_idx == kb_idx));

        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        // The fallback split becomes the captured companion for next time.
        assert!(editor.buffers[graph_idx]
            .graph_view()
            .unwrap()
            .companion_window
            .get_valid(&editor.window_mgr)
            .is_some());
    }

    #[test]
    fn select_current_reuses_captured_companion_without_resplitting() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);

        // Manually split once to create a second window, and capture it as
        // the companion — simulating what the focus-capture hook would have
        // done had the human previously focused a non-graph window.
        let area = editor.default_area();
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let text_idx = editor.buffers.len();
        editor.buffers.push(Buffer::new());
        let companion_win_id = editor
            .window_mgr
            .split(SplitDirection::Vertical, text_idx, area)
            .expect("split should succeed");
        editor.buffers[graph_idx]
            .graph_view_mut()
            .unwrap()
            .companion_window
            .set(Some(companion_win_id));

        let window_count_before = editor.window_mgr.iter_windows().count();
        editor.kb_graph_view_select_current();
        let window_count_after = editor.window_mgr.iter_windows().count();

        assert_eq!(
            window_count_before, window_count_after,
            "reusing the captured companion must never split"
        );
        let companion_win = editor.window_mgr.window(companion_win_id).unwrap();
        let kb_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Kb)
            .unwrap();
        assert_eq!(
            companion_win.buffer_idx, kb_idx,
            "the captured companion window must now show the selected node's KB buffer"
        );
    }

    // --- kb_graph_view_click_at (Part C Phase 1 item 6, mouse click-to-navigate) ---
    //
    // No literal GUI event-loop / winit glue is exercised here (that lives in
    // `crates/mae/src/gui_app.rs::handle_mouse_button_pressed`, which isn't
    // reachable from this crate's test harness) — these test at the
    // `Editor`-level function boundary instead, exactly like the rest of
    // this file's `select_current`/`navigate` coverage, and like Part B's
    // idle-dispatch work handled the same class of limitation. The
    // pixel->scene conversion itself is exercised end-to-end here by reading
    // back a node's REAL post-layout `(x, y)` from the scene and clicking at
    // that exact coordinate — not a hand-picked "unicorn" value that happens
    // to work, but the actual position the current circular layout produces.

    #[test]
    fn click_hits_node_selects_it_and_navigates_companion() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "[[concept:window]]");
        editor.kb.primary.insert(mae_kb::Node::new(
            "concept:window",
            "Window",
            mae_kb::NodeKind::Concept,
            "",
        ));
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), Some(1));

        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let graph_win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == graph_idx)
            .map(|w| w.id)
            .unwrap();

        // Read back node 0's REAL layout position and click exactly there.
        let (node_id, x, y) = {
            let gv = editor.buffers[graph_idx].graph_view().unwrap();
            assert!(
                gv.scene.nodes.len() >= 2,
                "a center node + its one link should both be present at depth 1"
            );
            let node = &gv.scene.nodes[0];
            (node.id.clone(), node.x as f32, node.y as f32)
        };

        editor.kb_graph_view_click_at(graph_win_id, x, y);

        assert_eq!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .scene
                .selection,
            Some(0),
            "clicking a node must select it, mirroring keyboard-nav selection"
        );

        let kb_idx = editor
            .buffers
            .iter()
            .position(|b| b.kb_view().map(|kv| kv.current == node_id).unwrap_or(false))
            .expect("a KB buffer for the clicked node should have been created");
        assert!(
            editor
                .window_mgr
                .iter_windows()
                .any(|w| w.buffer_idx == kb_idx),
            "the clicked node's KB buffer must be displayed in the companion window"
        );
    }

    #[test]
    fn click_reuses_captured_companion_without_resplitting() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);

        let area = editor.default_area();
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let graph_win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == graph_idx)
            .map(|w| w.id)
            .unwrap();
        let text_idx = editor.buffers.len();
        editor.buffers.push(Buffer::new());
        let companion_win_id = editor
            .window_mgr
            .split(SplitDirection::Vertical, text_idx, area)
            .expect("split should succeed");
        editor.buffers[graph_idx]
            .graph_view_mut()
            .unwrap()
            .companion_window
            .set(Some(companion_win_id));

        let (x, y) = {
            let gv = editor.buffers[graph_idx].graph_view().unwrap();
            let node = &gv.scene.nodes[0];
            (node.x as f32, node.y as f32)
        };

        let window_count_before = editor.window_mgr.iter_windows().count();
        editor.kb_graph_view_click_at(graph_win_id, x, y);
        let window_count_after = editor.window_mgr.iter_windows().count();

        assert_eq!(
            window_count_before, window_count_after,
            "reusing the captured companion via a click must never split"
        );
        let companion_win = editor.window_mgr.window(companion_win_id).unwrap();
        let kb_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Kb)
            .unwrap();
        assert_eq!(
            companion_win.buffer_idx, kb_idx,
            "the captured companion window must now show the clicked node's KB buffer"
        );
    }

    #[test]
    fn click_miss_deselects_and_does_not_navigate() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let graph_win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == graph_idx)
            .map(|w| w.id)
            .unwrap();
        // Pre-select a node, as keyboard nav would have left it.
        editor.buffers[graph_idx]
            .graph_view_mut()
            .unwrap()
            .scene
            .selection = Some(0);

        let kb_buf_count_before = editor
            .buffers
            .iter()
            .filter(|b| b.kind == BufferKind::Kb)
            .count();
        let window_count_before = editor.window_mgr.iter_windows().count();

        // Click far away from any node's bounding box — a guaranteed miss.
        editor.kb_graph_view_click_at(graph_win_id, 1_000_000.0, 1_000_000.0);

        assert_eq!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .scene
                .selection,
            None,
            "a miss must clear the selection (explicit deselect)"
        );
        assert_eq!(
            editor
                .buffers
                .iter()
                .filter(|b| b.kind == BufferKind::Kb)
                .count(),
            kb_buf_count_before,
            "a miss must not create/navigate any KB buffer"
        );
        assert_eq!(
            editor.window_mgr.iter_windows().count(),
            window_count_before,
            "a miss must not open/close/split any window"
        );
    }

    #[test]
    fn click_on_a_non_graph_window_is_a_harmless_no_op() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        // `kb_graph_view_open` splits, so there should be a non-graph window too.
        let non_graph_win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx != graph_idx)
            .map(|w| w.id)
            .expect("opening the graph view should have split");

        let buf_count_before = editor.buffers.len();
        let window_count_before = editor.window_mgr.iter_windows().count();

        editor.kb_graph_view_click_at(non_graph_win_id, 0.0, 0.0);

        assert_eq!(editor.buffers.len(), buf_count_before);
        assert_eq!(
            editor.window_mgr.iter_windows().count(),
            window_count_before
        );
    }

    #[test]
    fn click_with_a_stale_window_id_does_not_panic() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        // 999_999 was never allocated by this WindowManager.
        editor.kb_graph_view_click_at(999_999, 0.0, 0.0);
    }

    #[test]
    fn capture_focus_updates_companion_on_focus_change_away_from_graph() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let graph_win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == graph_idx)
            .map(|w| w.id)
            .unwrap();
        let other_win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.id != graph_win_id)
            .map(|w| w.id)
            .expect("opening the graph view should have split, giving 2 windows");

        editor.capture_graph_companion_focus(other_win_id);

        assert_eq!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .companion_window
                .get_valid(&editor.window_mgr),
            Some(other_win_id)
        );
    }

    #[test]
    fn capture_focus_ignores_focus_moving_to_the_graph_window_itself() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let graph_win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == graph_idx)
            .map(|w| w.id)
            .unwrap();
        let other_win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.id != graph_win_id)
            .map(|w| w.id)
            .unwrap();

        editor.capture_graph_companion_focus(other_win_id);
        editor.capture_graph_companion_focus(graph_win_id); // focus moves TO the graph — ignored

        assert_eq!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .companion_window
                .get_valid(&editor.window_mgr),
            Some(other_win_id),
            "focus moving to the graph window itself must not overwrite the captured companion"
        );
    }

    #[test]
    fn apply_layout_result_updates_scene_and_rendered() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();

        let mut new_scene = editor.buffers[idx].graph_view().unwrap().scene.clone();
        if let Some(n) = new_scene.nodes.first_mut() {
            n.x = 12345.0;
        }
        editor.apply_graph_layout_result(idx, new_scene);

        assert_eq!(
            editor.buffers[idx]
                .graph_view()
                .unwrap()
                .scene
                .nodes
                .first()
                .unwrap()
                .x,
            12345.0
        );
    }

    #[test]
    fn apply_layout_result_is_a_no_op_after_the_buffer_is_closed() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let scene = editor.buffers[idx].graph_view().unwrap().scene.clone();
        editor.kb_graph_view_close();

        // Must not panic even though `idx` now points at a different (or
        // out-of-range) buffer.
        editor.apply_graph_layout_result(idx, scene);
    }

    #[test]
    fn open_queues_a_background_layout_request() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        assert!(editor.pending_graph_layout.is_none());
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let pending = editor
            .pending_graph_layout
            .as_ref()
            .expect("open should queue a layout request");
        assert_eq!(pending.iterations, editor.kb_graph_layout_iterations);
    }
}
