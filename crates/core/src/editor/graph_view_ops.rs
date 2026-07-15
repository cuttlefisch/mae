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
    flatten_scene_graph, kind_affinity_from_strength, node_render_radius, GraphColorTween,
    GraphLayoutIntent, GraphLayoutMode, GraphNavDirection, GraphStyleOptions, GraphView,
    ANIMATION_COOLING_FACTOR, ANIMATION_INITIAL_TEMPERATURE, ANIMATION_SETTLE_EPSILON,
    ANIMATION_TEMPERATURE_FLOOR,
};
use crate::visual_buffer::VisualBuffer;
use crate::window::WindowId;

use super::Editor;

/// Convert a Graph window's window-relative pixel click/drag position into
/// scene coordinates via `mae_canvas::interaction::viewport_to_scene` — the
/// exact inverse of `flatten_scene_graph`'s `scene_to_viewport` draw
/// transform. Looks up `win_id`'s own entry in `GraphView.viewports`
/// (issue #321 — each window showing this graph buffer has its own pan/
/// zoom/pixel-size, kept synced by `Editor::graph_view_reflatten_window`
/// before every render), falling back to `Viewport::default()` if this
/// window hasn't been reflattened yet (e.g. a just-split second window —
/// self-heals on the next `sync_open_graph_viewports` tick) — so a click
/// always resolves to the scene point actually under the cursor IN THAT
/// WINDOW, matching what's drawn there, with no separate/neutralized
/// convention to keep in sync by hand.
fn graph_scene_point(gv: &GraphView, win_id: WindowId, rel_x: f32, rel_y: f32) -> (f64, f64) {
    let viewport = gv.viewports.get(&win_id).copied().unwrap_or_default();
    mae_canvas::interaction::viewport_to_scene(&viewport, rel_x as f64, rel_y as f64)
}

/// Compute every node's hit-test radius (SCENE-space units, parallel to
/// `gv.scene.nodes`) for window `win_id`'s current zoom, for
/// `mae_canvas::interaction::hit_test`. The hit-testing analog of
/// `graph_scene_point`'s position conversion: each node's real SCREEN-space
/// render radius (`node_render_radius` — varies by degree and zoom, see
/// its doc comment) is converted back to scene-space by dividing by the
/// window's zoom, so the clickable circle always matches the drawn one
/// exactly, at any zoom/degree combination. Without this per-node,
/// zoom-aware conversion, a click's hit radius either stayed FIXED in
/// scene-space (the node's now-vestigial `width`/`height` fields, a
/// leftover from an earlier rectangular-node model) or FIXED in
/// screen-space (this codebase's own prior single-scalar-radius fix) while
/// the rendered circle varied by both zoom and degree — either way the two
/// drifted apart.
fn graph_scene_hit_radii(gv: &GraphView, win_id: WindowId, style: &GraphStyleOptions) -> Vec<f64> {
    let viewport = gv.viewports.get(&win_id).copied().unwrap_or_default();
    let zoom = viewport.zoom.max(f64::EPSILON);
    gv.scene
        .nodes
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let degree = gv.node_degrees.get(i).copied().unwrap_or(0);
            node_render_radius(style, degree, viewport.zoom) as f64 / zoom
        })
        .collect()
}

/// Part C Phase 4 (wheel-zoom): per-wheel-event zoom-factor tuning.
/// `GRAPH_ZOOM_SENSITIVITY` scales a raw wheel pixel delta (the same units
/// `gui_app.rs::handle_mouse_wheel` already computes for text scroll) down
/// to a fractional zoom step; `GRAPH_ZOOM_MAX_STEP` caps how far a single
/// wheel event can move the zoom factor so one fast/large scroll can't jump
/// straight to `canvas::interaction::zoom`'s 0.1x/10x clamp boundary.
/// Plain constants (not `OptionRegistry` entries), matching this module's
/// existing `ANIMATION_*` precedent in `graph_view.rs` for interaction/
/// physics tuning that isn't itself a user-facing behavior toggle.
const GRAPH_ZOOM_SENSITIVITY: f64 = 400.0;
const GRAPH_ZOOM_MAX_STEP: f64 = 0.3;

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

    /// Real pixel dimensions of window `win_id`, for centering the graph's
    /// viewport within it (via `Editor::graph_view_reflatten_window`). Falls
    /// back to `mae_canvas::scene::Viewport::default()`'s 800x600 if the
    /// window no longer resolves — e.g. the very first `populate_graph_
    /// buffer` call before `display_buffer` has created its window, or a
    /// window that closed mid-flight.
    fn graph_viewport_pixel_size(&self, win_id: WindowId) -> (f32, f32) {
        let default = mae_canvas::scene::Viewport::default();
        let fallback = (default.width as f32, default.height as f32);

        let Some((_, rect)) = self
            .window_mgr
            .layout_rects(self.last_layout_area)
            .into_iter()
            .find(|(id, _)| *id == win_id)
        else {
            return fallback;
        };
        (
            rect.width as f32 * self.gui_cell_width,
            rect.height as f32 * self.gui_cell_height,
        )
    }

    /// Re-flatten just ONE window's cached `VisualBuffer` entry (issue
    /// #321 — a graph buffer can be shown in more than one window, each
    /// with its own `Viewport`). First syncs that window's `Viewport`
    /// pixel size (see `graph_viewport_pixel_size`), THEN flattens through
    /// it — so `flatten_scene_graph`'s `scene_to_viewport` transform and
    /// `graph_scene_point`'s `viewport_to_scene` transform (used by
    /// hit-testing/drag/zoom) always agree on where a node actually is IN
    /// THIS WINDOW, driven by one synced `Viewport`, not two
    /// independently-maintained ideas of window size. Cheap — use for a
    /// change genuinely scoped to one window (zoom, resize). For a change
    /// to the shared topology/selection/hover (which every window showing
    /// this buffer must reflect), use `graph_view_reflatten_all_windows`
    /// instead, so viewport sizing/flattening can never be forgotten at a
    /// call site either way.
    fn graph_view_reflatten_window(&mut self, buf_idx: usize, win_id: WindowId) {
        let (width, height) = self.graph_viewport_pixel_size(win_id);
        let mut style = self.graph_style_options();
        if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
            // Merge in the active color tween's current eased color, if
            // any — `from_editor` has no per-`GraphView` knowledge, so
            // this is the call site that bridges `GraphView.color_tween`
            // into `GraphStyleOptions.color_override`.
            if let Some(tween) = &gv.color_tween {
                style.color_override = Some((tween.node_index, tween.current_color()));
            }
            let viewport = gv.viewports.entry(win_id).or_default();
            viewport.width = width as f64;
            viewport.height = height as f64;
            let viewport = *viewport;
            let elements = flatten_scene_graph(&gv.scene, &viewport, &style, &gv.node_degrees);
            gv.rendered.insert(win_id, VisualBuffer { elements });
            // Bump so the GUI's per-window render cache
            // (`WindowRenderCache::graph_render_epoch`) knows this
            // window's picture genuinely changed — `buf.generation`
            // (rope-based) never moves for a Graph buffer, so without
            // this the cache would have no way to invalidate on
            // pan/zoom/hover/selection/tween changes.
            *gv.render_epoch.entry(win_id).or_insert(0) += 1;
        }
        self.mark_full_redraw();
    }

    /// Re-flatten EVERY window currently showing graph buffer `buf_idx` —
    /// use after a change to the shared topology/selection/hover state
    /// (node position, selection, hover, a completed background layout),
    /// since that data is the SAME for every window showing it and every
    /// window's `rendered` cache must stay in sync. See `graph_view_
    /// reflatten_window` for the cheaper single-window variant (zoom,
    /// resize — genuinely scoped to one window).
    fn graph_view_reflatten_all_windows(&mut self, buf_idx: usize) {
        let win_ids: Vec<WindowId> = self
            .window_mgr
            .iter_windows()
            .filter(|w| w.buffer_idx == buf_idx)
            .map(|w| w.id)
            .collect();
        for win_id in win_ids {
            self.graph_view_reflatten_window(buf_idx, win_id);
        }
    }

    /// Self-healing resize sync: re-flatten every window on every open
    /// `BufferKind::Graph` buffer whose cached per-window `Viewport` pixel
    /// size no longer matches the real size of that window (issue #321 —
    /// iterates every window showing a graph buffer, not just the first
    /// one found, so a second window split onto the same buffer stays in
    /// sync too). Also prunes `viewports`/`rendered` entries for windows
    /// that no longer show this buffer (closed, or reassigned) — this is
    /// the only place that needs to, since it already walks every open
    /// graph buffer's live window set every tick; no dedicated
    /// window-close hook needed. Called every GUI event-loop iteration
    /// from `gui_app.rs`'s `about_to_wait` (before any redraw is
    /// requested) rather than hooked into every individual
    /// window-resize/split-resize command — `last_layout_area`/window-tree
    /// ratios are already correctly updated by those commands (each already
    /// triggers `mark_full_redraw`/a GUI redraw), so this single check
    /// self-heals on the next iteration after ANY layout change (full GUI
    /// window resize, split grow/shrink/balance/maximize, a new window
    /// split onto the buffer, or a sibling window closing) without needing
    /// to enumerate every call site that could change a window's rect.
    /// Returns whether anything was resynced, so the caller can set its
    /// own dirty flag accordingly (`graph_view_reflatten_window`'s
    /// `mark_full_redraw` only affects `Editor`'s own redraw-level
    /// tracking, not `GuiApp.dirty`, which is a separate field the GUI
    /// event loop manages itself).
    pub fn sync_open_graph_viewports(&mut self) -> bool {
        let mut changed = false;
        let graph_buf_indices: Vec<usize> = self
            .buffers
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind == BufferKind::Graph)
            .map(|(i, _)| i)
            .collect();

        for buf_idx in graph_buf_indices {
            let live_win_ids: std::collections::HashSet<WindowId> = self
                .window_mgr
                .iter_windows()
                .filter(|w| w.buffer_idx == buf_idx)
                .map(|w| w.id)
                .collect();

            if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
                gv.viewports.retain(|id, _| live_win_ids.contains(id));
                gv.rendered.retain(|id, _| live_win_ids.contains(id));
                gv.render_epoch.retain(|id, _| live_win_ids.contains(id));
            }

            for win_id in live_win_ids {
                let (w, h) = self.graph_viewport_pixel_size(win_id);
                let stale = match self.buffers[buf_idx]
                    .graph_view()
                    .and_then(|gv| gv.viewports.get(&win_id))
                {
                    Some(vp) => vp.width != w as f64 || vp.height != h as f64,
                    None => true,
                };
                if stale {
                    self.graph_view_reflatten_window(buf_idx, win_id);
                    changed = true;
                }
            }
        }
        changed
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
        // Bridge from `mae_kb::SubgraphLink` to `mae_canvas::kb_graph::
        // KbLinkInfo` — `mae-canvas` deliberately has no dependency on
        // `mae-kb` (see `KbNodeInfo`'s doc comment for the same pattern
        // applied to nodes), so this conversion lives here, the first
        // place in the dependency graph that can see both crates.
        let to_link_info = |l: &mae_kb::SubgraphLink| mae_canvas::kb_graph::KbLinkInfo {
            source: l.source.clone(),
            target: l.target.clone(),
            rel_type: l.rel_type.clone(),
            weight: l.weight,
        };
        let kb_links: Vec<mae_canvas::kb_graph::KbLinkInfo> =
            result.links.iter().map(to_link_info).collect();
        let kb_boundary_links: Vec<mae_canvas::kb_graph::KbLinkInfo> =
            result.boundary_links.iter().map(to_link_info).collect();

        let scene = mae_canvas::kb_graph::build_kb_graph_positions_only(
            &kb_nodes,
            &kb_links,
            &kb_boundary_links,
            std::slice::from_ref(&center),
            self.kb_graph_layout_spacing_scale as f64,
        );

        // Queue the background layout pass with a snapshot of the fresh
        // circular-layout scene BEFORE moving `scene` into the view below,
        // so the background pass refines the same data the view now holds
        // (not a stale prior scene). Part C Phase 3: when `kb_graph_animate`
        // is on, request the FIRST animation tick instead of a one-shot
        // `run()` — `apply_graph_layout_result` takes over queuing every
        // subsequent tick from there. When off (the default), this is
        // byte-for-byte the same one-shot `OneShot` request Phase 1/2 always
        // sent.
        let mode = if self.kb_graph_animate {
            GraphLayoutMode::Tick {
                temperature: ANIMATION_INITIAL_TEMPERATURE,
            }
        } else {
            GraphLayoutMode::OneShot {
                iterations: self.kb_graph_layout_iterations,
            }
        };
        // Computed once here and cached on `GraphView.layout_config` below
        // — every subsequent intent this view queues (including the Phase
        // 3 tick-requeue in `apply_graph_layout_result`) reads the cached
        // value instead of rebuilding one, so a later tick can never
        // silently drop back to `LayoutConfig::default()`.
        let layout_config = mae_canvas::layout::LayoutConfig {
            kind_affinity: kind_affinity_from_strength(self.kb_graph_layout_kind_clustering),
            spacing_scale: self.kb_graph_layout_spacing_scale as f64,
            ..mae_canvas::layout::LayoutConfig::default()
        };
        self.pending_graph_layout = Some(GraphLayoutIntent {
            buf_idx,
            scene: scene.clone(),
            mode,
            layout_config: layout_config.clone(),
        });

        if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
            gv.center_node = Some(center);
            gv.depth = depth;
            gv.kb_instance = kb_instance;
            gv.scene = scene;
            gv.node_degrees = crate::graph_view::node_degrees(&gv.scene);
            gv.animating = self.kb_graph_animate;
            gv.anim_temperature = ANIMATION_INITIAL_TEMPERATURE;
            gv.layout_config = layout_config;
        }
        self.graph_view_reflatten_all_windows(buf_idx);
    }

    /// Part C Phase 3: is there an open `BufferKind::Graph` buffer whose
    /// layout hasn't settled yet, with `kb_graph_animate` enabled? Read by
    /// `gui_app.rs`'s unified dirty/`ControlFlow::WaitUntil` loop condition
    /// (mirroring the existing scroll-inertia `any_inertia` term) to decide
    /// whether to keep the 60fps wake-up cadence alive so animation ticks
    /// keep flowing through `drain_graph_layout_intent`. Live-reads
    /// `kb_graph_animate` (not snapshotted) so toggling the option off stops
    /// the wake-up cadence on the very next check, same as it stops new
    /// ticks being queued in `apply_graph_layout_result`. Always `false`
    /// when the option is off, matching every `GraphView.animating` field it
    /// reads (see that field's doc comment).
    pub fn has_active_graph_animation(&self) -> bool {
        self.kb_graph_animate
            && self
                .buffers
                .iter()
                .any(|b| b.graph_view().map(|gv| gv.animating).unwrap_or(false))
    }

    /// Is any open `BufferKind::Graph` buffer mid hover/selection color
    /// tween? Mirrors `has_active_graph_animation` exactly — read by
    /// `gui_app.rs`'s `ControlFlow::WaitUntil` wake condition so a running
    /// tween keeps the 60fps cadence alive without a second animation
    /// scheduler. `GraphView.color_tween` is only ever `Some` when
    /// `kb_graph_color_tween_enabled` was on at the moment it started (see
    /// the start call sites), so no live option re-check is needed here.
    pub fn has_active_color_tween(&self) -> bool {
        self.buffers.iter().any(|b| {
            b.graph_view()
                .map(|gv| gv.color_tween.is_some())
                .unwrap_or(false)
        })
    }

    /// Advance every open graph buffer's in-flight color tween one tick:
    /// clears it (and reflattens once more so the final resting color
    /// renders) once `GraphColorTween::is_complete()`, otherwise just
    /// reflattens so the eased-interpolated `current_color()` actually
    /// shows up in this frame's render. Called every event-loop iteration
    /// from `drain_intents_and_lifecycle`, right after
    /// `drain_graph_layout_intent` — same hook neighborhood, no new call
    /// site invented.
    pub fn tick_graph_color_tweens(&mut self) {
        let buf_win_ids: Vec<(usize, Vec<WindowId>)> = self
            .buffers
            .iter()
            .enumerate()
            .filter(|(_, b)| {
                b.graph_view()
                    .map(|gv| gv.color_tween.is_some())
                    .unwrap_or(false)
            })
            .map(|(buf_idx, _)| {
                let win_ids = self
                    .window_mgr
                    .iter_windows()
                    .filter(|w| w.buffer_idx == buf_idx)
                    .map(|w| w.id)
                    .collect();
                (buf_idx, win_ids)
            })
            .collect();
        for (buf_idx, win_ids) in buf_win_ids {
            let completed = self.buffers[buf_idx]
                .graph_view()
                .and_then(|gv| gv.color_tween.as_ref())
                .map(|tween| tween.is_complete())
                .unwrap_or(false);
            if completed {
                if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
                    gv.color_tween = None;
                }
            }
            for win_id in win_ids {
                self.graph_view_reflatten_window(buf_idx, win_id);
            }
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
        // Display FIRST, then populate: `populate_graph_buffer` ends by
        // syncing the viewport to whatever window is currently showing
        // `buf_idx` (`graph_view_reflatten`/`graph_viewport_pixel_size`) —
        // doing this after the window exists means the very first render
        // is already centered on the real pane, not a size-unknown
        // fallback corrected on a later refresh.
        self.display_buffer(buf_idx);
        self.populate_graph_buffer(buf_idx, center, depth);
    }

    /// Structured, read-only introspection snapshot of the open graph view
    /// (hovered/selected node, every rendered node and edge) — `None` if no
    /// `BufferKind::Graph` buffer is open. Thin wrapper around
    /// `GraphView::describe_state`; the single shared data source for both
    /// the `kb_graph_view_state` MCP tool and the `(kb-graph-view-state)`
    /// Scheme primitive.
    pub fn kb_graph_view_state(&self) -> Option<crate::graph_view::GraphViewState> {
        self.buffers
            .iter()
            .find_map(|b| b.graph_view())
            .map(|gv| gv.describe_state())
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
        self.graph_view_reflatten_all_windows(idx);
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

    /// Issue #322: set the graph's zoom to an explicit level — the
    /// AI-appropriate equivalent of `kb_graph_view_zoom`'s pixel-focus-
    /// based wheel delta (which has no meaningful non-pointer input). No-op
    /// (returns `None`) if no `BufferKind::Graph` buffer is open. Applies
    /// to the focused window if it's currently showing the graph buffer
    /// (the common case: a human and the AI peer looking at the same
    /// window), else the first window found showing it (issue #321 made
    /// the viewport genuinely per-window, so SOME window must be chosen —
    /// an AI-driven call has no pixel focus point to disambiguate
    /// further). Returns the ACTUAL applied zoom (post-clamp — see
    /// `mae_canvas::interaction::set_zoom`'s 0.1-10.0 range), not simply
    /// `target` echoed back, so a caller reporting the result to a human or
    /// reasoning about its own action (the MCP tool / Scheme primitive)
    /// never claims an out-of-range value silently "worked" as requested.
    pub fn kb_graph_view_zoom_to(&mut self, target: f64) -> Option<f64> {
        let graph_idx = self
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)?;
        let focused_id = self.window_mgr.focused_id();
        let win_id = if self
            .window_mgr
            .window(focused_id)
            .is_some_and(|w| w.buffer_idx == graph_idx)
        {
            Some(focused_id)
        } else {
            self.window_mgr
                .iter_windows()
                .find(|w| w.buffer_idx == graph_idx)
                .map(|w| w.id)
        }?;
        let applied = self.buffers[graph_idx].graph_view_mut().map(|gv| {
            let viewport = gv.viewports.entry(win_id).or_default();
            mae_canvas::interaction::set_zoom(viewport, target);
            viewport.zoom
        })?;
        self.graph_view_reflatten_window(graph_idx, win_id);
        Some(applied)
    }

    /// Issue #322: pin or unpin a node by KB id, optionally repositioning
    /// it — the AI-appropriate equivalent of drag-to-pin (no drag gesture
    /// needed; `SceneNode.pinned` is already fully respected by
    /// `ForceLayout`, this is pure plumbing onto an existing concept). No-op
    /// if no graph is open or `id` isn't among the currently-rendered
    /// nodes (returns whether a node was found and updated, so callers —
    /// the Scheme primitive / MCP tool — can report a clear miss rather
    /// than a silent no-op). Reflattens EVERY window showing this buffer
    /// (`pinned`/position is shared topology, not per-window state).
    pub fn kb_graph_view_set_pinned(
        &mut self,
        id: &str,
        pinned: bool,
        pos: Option<(f64, f64)>,
    ) -> bool {
        let Some(graph_idx) = self
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
        else {
            return false;
        };
        let found = if let Some(gv) = self.buffers[graph_idx].graph_view_mut() {
            if let Some(node) = gv.scene.nodes.iter_mut().find(|n| n.id == id) {
                node.pinned = pinned;
                if let Some((x, y)) = pos {
                    node.x = x;
                    node.y = y;
                }
                true
            } else {
                false
            }
        } else {
            false
        };
        if found {
            self.graph_view_reflatten_all_windows(graph_idx);
        }
        found
    }

    /// Toggle the graph view between its normal tiled split-window pane and
    /// a full-frame modal overlay (dimmed background, drawn on top of the
    /// rest of the editor — see `render_common::overlay::ActiveOverlay::
    /// GraphView`). No-op (returns `false`, unchanged) if no `BufferKind::
    /// Graph` buffer is open — there's nothing to overlay. Returns the new
    /// state so callers (the Scheme primitive / MCP tool / keybinding) can
    /// report "overlay: on"/"overlay: off".
    pub fn kb_graph_view_toggle_overlay(&mut self) -> bool {
        if !self.buffers.iter().any(|b| b.kind == BufferKind::Graph) {
            self.set_status("No KB graph view is open".to_string());
            return false;
        }
        self.kb_graph_view_overlay_active = !self.kb_graph_view_overlay_active;
        self.mark_full_redraw();
        self.set_status(format!(
            "KB graph view overlay: {}",
            if self.kb_graph_view_overlay_active {
                "on"
            } else {
                "off"
            }
        ));
        self.kb_graph_view_overlay_active
    }

    /// Start a color tween (if `kb_graph_color_tween_enabled`) for
    /// `node_index` transitioning INTO `to_hex` (its new selected/hover
    /// highlight color) FROM its plain kind-color — the asymmetric design:
    /// only the newly-highlighted node tweens in; the previously-
    /// highlighted node's own return to its kind-color is an instant snap
    /// (simply no longer being the `color_tween`/`selection`/`hovered`
    /// target — no code needed for that side). No-op if the buffer/node
    /// index is invalid; overwrites any previous tween on this `GraphView`
    /// (one slot, not N concurrent tweens).
    fn maybe_start_color_tween(&mut self, buf_idx: usize, node_index: usize, to_hex: String) {
        if !self.kb_graph_color_tween_enabled {
            return;
        }
        let duration =
            std::time::Duration::from_millis(self.kb_graph_color_tween_duration_ms as u64);
        let style = self.graph_style_options();
        if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
            let Some(node) = gv.scene.nodes.get(node_index) else {
                return;
            };
            let from_hex = style.color_for_kind(node.kind).to_string();
            gv.color_tween = Some(GraphColorTween {
                node_index,
                from_hex,
                to_hex,
                started_at: std::time::Instant::now(),
                duration,
            });
        }
    }

    /// Hit-test a Graph window position and update the scene's selection —
    /// WITHOUT navigating the companion window. Factored out of
    /// `kb_graph_view_click_at` (Part C Phase 1 item 6) so Part C Phase 4's
    /// mouse-DOWN handler can do the identical hit-test/select step (for
    /// immediate visual feedback, and so a subsequent drag knows which node
    /// index it's moving) without prematurely navigating away — the press
    /// might turn into a drag rather than a click, and only a plain
    /// click-without-drag should navigate (see `kb_graph_view_click_at` and
    /// `gui_app.rs`'s press/move/release handlers).
    ///
    /// `graph_win_id`/`rel_x`/`rel_y` have the same contract as
    /// `kb_graph_view_click_at`. On a hit: sets `scene.selection`. On a miss
    /// (or an invalid/non-Graph window): clears `scene.selection` — an
    /// explicit deselect for a miss, a harmless no-op for an invalid
    /// window. Either way (except the invalid-window case, which mutates
    /// nothing), re-flattens `rendered` and marks a full redraw, matching
    /// `kb_graph_view_navigate`'s behavior. Returns the hit node's index, or
    /// `None` on a miss/invalid window.
    pub fn kb_graph_view_hit_test_and_select(
        &mut self,
        graph_win_id: WindowId,
        rel_x: f32,
        rel_y: f32,
    ) -> Option<usize> {
        let buf_idx = self.window_mgr.window(graph_win_id).map(|w| w.buffer_idx)?;
        if buf_idx >= self.buffers.len() || self.buffers[buf_idx].kind != BufferKind::Graph {
            return None;
        }

        let style = self.graph_style_options();
        let hit_result: Option<Option<usize>> = self.buffers[buf_idx].graph_view().map(|gv| {
            let (scene_x, scene_y) = graph_scene_point(gv, graph_win_id, rel_x, rel_y);
            let radii = graph_scene_hit_radii(gv, graph_win_id, &style);
            mae_canvas::interaction::hit_test(&gv.scene, scene_x, scene_y, &radii)
        });
        let node_hit = hit_result?;

        let previous_selection = self.buffers[buf_idx]
            .graph_view()
            .and_then(|gv| gv.scene.selection);
        if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
            gv.scene.selection = node_hit;
        }
        if let Some(newly_selected) = node_hit {
            if previous_selection != node_hit {
                self.maybe_start_color_tween(buf_idx, newly_selected, style.selected_color.clone());
            }
        }
        self.graph_view_reflatten_all_windows(buf_idx);

        node_hit
    }

    /// Real-time mouse hover (introspection/UX pass): update the hovered
    /// node for the Graph buffer shown in `graph_win_id`, structurally
    /// mirroring `kb_graph_view_hit_test_and_select` (same
    /// `graph_scene_point` + `mae_canvas::interaction::hit_test` call), but
    /// writing `gv.scene.hovered` instead of `gv.scene.selection` and never
    /// touching selection. Unlike hit-test-and-select (click-triggered,
    /// rare, always reflattens), this only reflattens when the hovered
    /// index actually CHANGES — `CursorMoved` fires far more often than
    /// clicks, and an unconditional reflatten on every pixel of mouse
    /// jitter would be wasteful. Not idle-debounced (unlike
    /// `KbPreviewPopup`): a lightweight color-highlight should feel
    /// immediate, not delayed. No-op (returns `false`) if `graph_win_id`
    /// doesn't resolve to an open `BufferKind::Graph` window. Returns
    /// whether the hovered node actually changed, so callers (`gui_app.rs`)
    /// can set their own dirty flag only when a redraw is actually needed,
    /// not on every pixel of mouse movement over the graph window.
    pub fn kb_graph_view_hover_at(
        &mut self,
        graph_win_id: WindowId,
        rel_x: f32,
        rel_y: f32,
    ) -> bool {
        let Some(buf_idx) = self.window_mgr.window(graph_win_id).map(|w| w.buffer_idx) else {
            return false;
        };
        if buf_idx >= self.buffers.len() || self.buffers[buf_idx].kind != BufferKind::Graph {
            return false;
        }

        let style = self.graph_style_options();
        let Some(node_hit) = self.buffers[buf_idx].graph_view().map(|gv| {
            let (scene_x, scene_y) = graph_scene_point(gv, graph_win_id, rel_x, rel_y);
            let radii = graph_scene_hit_radii(gv, graph_win_id, &style);
            mae_canvas::interaction::hit_test(&gv.scene, scene_x, scene_y, &radii)
        }) else {
            return false;
        };

        let changed = self.buffers[buf_idx]
            .graph_view()
            .is_some_and(|gv| gv.scene.hovered != node_hit);
        if !changed {
            return false;
        }
        if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
            gv.scene.hovered = node_hit;
        }
        if let Some(newly_hovered) = node_hit {
            // Selected always wins over hovered in the color-priority
            // chain, so don't start a tween toward hover_color for a node
            // that's already selected — color_override would otherwise
            // briefly override the correct selected_color mid-tween.
            let already_selected = self.buffers[buf_idx]
                .graph_view()
                .is_some_and(|gv| gv.scene.selection == Some(newly_hovered));
            if !already_selected {
                self.maybe_start_color_tween(buf_idx, newly_hovered, style.hover_color.clone());
            }
        }
        self.graph_view_reflatten_all_windows(buf_idx);
        true
    }

    /// Clear the hovered node on whichever `BufferKind::Graph` buffer
    /// currently has one set (there's normally at most one graph buffer —
    /// see `ensure_graph_buffer_idx` — so this doesn't need a window/buffer
    /// argument). Called from `gui_app.rs` both when the cursor moves off
    /// the graph window onto something else within the same OS window, and
    /// on `WindowEvent::CursorLeft` (the cursor leaving the OS window's
    /// client area entirely, after which winit stops delivering
    /// `CursorMoved` altogether — without this, a hover highlight could
    /// stay stuck lit indefinitely). Returns whether anything changed, so
    /// callers can set their own dirty flag accordingly. Reflattens only
    /// when something was actually cleared.
    pub fn kb_graph_view_clear_hover(&mut self) -> bool {
        let Some(buf_idx) = self
            .buffers
            .iter()
            .position(|b| b.graph_view().is_some_and(|gv| gv.scene.hovered.is_some()))
        else {
            return false;
        };
        if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
            gv.scene.hovered = None;
        }
        self.graph_view_reflatten_all_windows(buf_idx);
        true
    }

    /// @ai-caution: [architecture-debt] This method, `kb_graph_view_zoom`,
    /// `kb_graph_view_drag_node`, and `kb_graph_view_drag_end` are
    /// deliberately GUI-mouse-only — NOT exposed to Scheme or MCP, unlike
    /// every other `kb_graph_view_*` method in this file. They take raw
    /// pixel coordinates / wheel deltas, which have no meaningful AI-driven
    /// equivalent; the AI-relevant actions ("pick a node," "adjust hop
    /// radius," "re-center") are already covered by `navigate`/
    /// `select_current`/`set_depth`/`open`. If AI-driven zoom/pin is ever
    /// needed, see https://github.com/cuttlefisch/mae/issues/322 for the
    /// AI-appropriate (non-pixel) API shape to add instead of exposing
    /// these directly.
    ///
    /// Mouse click-to-navigate (Part C Phase 1 item 6): `graph_win_id` is
    /// the window a click just landed in (already confirmed by the caller —
    /// `gui_app.rs`'s `handle_mouse_button_pressed`/mouse-release path — to
    /// be showing a `BufferKind::Graph` buffer); `rel_x`/`rel_y` are the
    /// click's pixel position relative to that window's content rect
    /// (top-left origin), NOT raw screen pixels and NOT text cells — the
    /// graph is drawn via the Skia `VisualBuffer` pixel pipeline, not the
    /// text-cell layout pipeline.
    ///
    /// Delegates the hit-test/select step to
    /// `kb_graph_view_hit_test_and_select` (Part C Phase 4 extracted this
    /// so mouse-DOWN could reuse it without navigating). On a hit,
    /// additionally navigates the captured companion window to it via the
    /// same shared logic `kb_graph_view_select_current` uses. On a miss,
    /// this is otherwise a harmless no-op (no navigation, no panic) beyond
    /// the deselect `kb_graph_view_hit_test_and_select` already performed.
    ///
    /// Part C Phase 4 note: as of Phase 4, `gui_app.rs` calls this ONLY on
    /// mouse-release-without-drag (a plain click) — a press-drag-release
    /// instead calls `kb_graph_view_hit_test_and_select` on press and
    /// `kb_graph_view_drag_node`/`kb_graph_view_drag_end` during/after the
    /// drag, never this method, so a drag never navigates the companion
    /// window.
    pub fn kb_graph_view_click_at(&mut self, graph_win_id: WindowId, rel_x: f32, rel_y: f32) {
        let Some(node_idx) = self.kb_graph_view_hit_test_and_select(graph_win_id, rel_x, rel_y)
        else {
            return;
        };

        let Some(buf_idx) = self.window_mgr.window(graph_win_id).map(|w| w.buffer_idx) else {
            return;
        };
        let node_id = self.buffers[buf_idx]
            .graph_view()
            .and_then(|gv| gv.scene.nodes.get(node_idx))
            .map(|n| n.id.clone());
        if let Some(node_id) = node_id {
            self.navigate_companion_window_to_node(buf_idx, &node_id);
        }
    }

    /// Part C Phase 4 (drag-to-pin): update the dragged node's scene
    /// position to track the cursor. Called by `gui_app.rs` on every
    /// `CursorMoved` while a node-drag is in progress (mouse-DOWN hit a
    /// node via `kb_graph_view_hit_test_and_select`, button still held).
    /// Converts `rel_x`/`rel_y` to scene coordinates via the same
    /// `graph_scene_point` convention hit-testing uses, so the dragged node
    /// tracks the exact point the cursor is over.
    ///
    /// Sets `pinned = true` on EVERY call (not just at `kb_graph_view_drag_end`
    /// on release) — this used to be deferred to release only, reasoning
    /// that a layout/animation tick landing mid-drag "would just get
    /// overwritten by the next drag-move call anyway." That reasoning
    /// assumed a tick apply and the next `CursorMoved` always interleave
    /// tightly; in practice a tick can land while the user is mid-gesture
    /// but momentarily not moving the mouse (or the tick simply lands
    /// between two move events), and `apply_graph_layout_result` writes a
    /// fresh force-computed position for every unpinned node — visibly
    /// snapping the dragged node back until the next move event corrects
    /// it. Pinning from the first move closes that race outright:
    /// `ForceLayout::step` already skips pinned nodes unconditionally, so a
    /// concurrent tick can never touch this node's position for the
    /// duration of the drag. End-state (pinned at the drag's final
    /// position) is unchanged from before. No-op (never panics) if the
    /// window/buffer/node no longer exist — e.g. the graph was closed or
    /// the node's index no longer resolves because of a refresh mid-drag.
    pub fn kb_graph_view_drag_node(
        &mut self,
        graph_win_id: WindowId,
        node_index: usize,
        rel_x: f32,
        rel_y: f32,
    ) {
        let Some(buf_idx) = self.window_mgr.window(graph_win_id).map(|w| w.buffer_idx) else {
            return;
        };
        if buf_idx >= self.buffers.len() || self.buffers[buf_idx].kind != BufferKind::Graph {
            return;
        }
        let Some(scene_point) = self.buffers[buf_idx]
            .graph_view()
            .map(|gv| graph_scene_point(gv, graph_win_id, rel_x, rel_y))
        else {
            return;
        };
        let (scene_x, scene_y) = scene_point;
        let moved = if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
            if let Some(node) = gv.scene.nodes.get_mut(node_index) {
                node.x = scene_x;
                node.y = scene_y;
                node.pinned = true;
                true
            } else {
                false
            }
        } else {
            false
        };
        if !moved {
            return;
        }

        self.graph_view_reflatten_all_windows(buf_idx);
    }

    /// Part C Phase 4 (drag-to-pin): finish an in-progress node drag on
    /// mouse-release — pins the node (`SceneNode.pinned = true`) at
    /// wherever `kb_graph_view_drag_node` last placed it, so
    /// `mae_canvas::layout::ForceLayout::step`/`run` (both confirmed to
    /// skip pinned nodes when applying displacement) leave it exactly
    /// there on any subsequent layout/animation tick. No-op (never panics)
    /// if the window/buffer/node no longer exist.
    pub fn kb_graph_view_drag_end(&mut self, graph_win_id: WindowId, node_index: usize) {
        let Some(buf_idx) = self.window_mgr.window(graph_win_id).map(|w| w.buffer_idx) else {
            return;
        };
        if buf_idx >= self.buffers.len() || self.buffers[buf_idx].kind != BufferKind::Graph {
            return;
        }
        if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
            if let Some(node) = gv.scene.nodes.get_mut(node_index) {
                node.pinned = true;
            }
        }
        self.mark_full_redraw();
    }

    /// Part C Phase 4 (wheel-zoom): adjust a Graph window's viewport zoom by
    /// wheel `delta` (a raw pixel scroll amount — the same unit
    /// `gui_app.rs::handle_mouse_wheel` already computes for text scroll,
    /// reused directly rather than re-deriving a separate unit), focused at
    /// (`focus_x`, `focus_y`) — the cursor's window-relative pixel
    /// position — so the point under the cursor stays fixed, exactly as
    /// `mae_canvas::interaction::zoom`'s existing focus-preserving math
    /// already guarantees (including its 0.1x-10x clamp, applied
    /// unmodified). This method's only job is turning a wheel delta into
    /// the multiplicative `factor` argument `zoom()` expects and
    /// re-flattening afterward — `zoom()` itself is not duplicated.
    ///
    /// No-op if `graph_win_id` doesn't resolve to an open `BufferKind::
    /// Graph` window.
    pub fn kb_graph_view_zoom(
        &mut self,
        graph_win_id: WindowId,
        delta: f32,
        focus_x: f32,
        focus_y: f32,
    ) {
        let Some(buf_idx) = self.window_mgr.window(graph_win_id).map(|w| w.buffer_idx) else {
            return;
        };
        if buf_idx >= self.buffers.len() || self.buffers[buf_idx].kind != BufferKind::Graph {
            return;
        }

        // Positive delta (scroll up/away, MAE's existing convention —
        // matches `handle_mouse_wheel`'s positive-v_px-scrolls-content-up
        // treatment) zooms in; negative zooms out. Clamped per-event so one
        // fast/large scroll can't jump straight to zoom()'s clamp bounds.
        let factor = 1.0
            + (delta as f64 / GRAPH_ZOOM_SENSITIVITY)
                .clamp(-GRAPH_ZOOM_MAX_STEP, GRAPH_ZOOM_MAX_STEP);
        if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
            let viewport = gv.viewports.entry(graph_win_id).or_default();
            mae_canvas::interaction::zoom(viewport, factor, focus_x as f64, focus_y as f64);
        }
        // Viewport-only change, scoped to THIS window — other windows
        // showing the same buffer are unaffected, so only this one needs
        // reflattening.
        self.graph_view_reflatten_window(buf_idx, graph_win_id);
    }

    /// Click-and-drag-on-empty-canvas-space panning (the Obsidian/org-roam-
    /// ui-style gesture): shift a Graph window's viewport by a per-event
    /// screen-space pixel delta via `mae_canvas::interaction::pan` (the
    /// exact inverse of `scene_to_viewport`'s transform, already used
    /// nowhere else in this codebase until now — reused, not reimplemented,
    /// per CLAUDE.md principle #8). `dx_px`/`dy_px` are the delta since the
    /// LAST drag event (not since mouse-down) — `gui_app.rs`'s drag handler
    /// calls this every `CursorMoved` tick while panning, mirroring how
    /// `kb_graph_view_drag_node` is called every tick with an absolute
    /// target position; panning has no analogous "absolute" target, so it
    /// accumulates incrementally instead.
    ///
    /// No-op if `graph_win_id` doesn't resolve to an open `BufferKind::
    /// Graph` window.
    pub fn kb_graph_view_pan(&mut self, graph_win_id: WindowId, dx_px: f32, dy_px: f32) {
        let Some(buf_idx) = self.window_mgr.window(graph_win_id).map(|w| w.buffer_idx) else {
            return;
        };
        if buf_idx >= self.buffers.len() || self.buffers[buf_idx].kind != BufferKind::Graph {
            return;
        }
        if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
            let viewport = gv.viewports.entry(graph_win_id).or_default();
            mae_canvas::interaction::pan(viewport, dx_px as f64, dy_px as f64);
        }
        self.graph_view_reflatten_window(buf_idx, graph_win_id);
    }

    /// Apply a completed background layout (`mae::graph_layout_bridge`)
    /// back onto the owning `GraphView`, re-flattening for render and
    /// marking the buffer dirty. No-op if the buffer no longer exists or is
    /// no longer a `BufferKind::Graph` buffer (closed mid-flight).
    ///
    /// `max_displacement` is `None` for a completed `OneShot` `run()` pass
    /// (Phase 1/2 behavior, unchanged) and `Some(signal)` for a completed
    /// Phase 3 animation `Tick` — in the latter case, this is also where the
    /// NEXT tick gets queued (via the same `pending_graph_layout` field
    /// `graph_view_ops.rs`'s open/refresh paths use), continuing the chain
    /// until either the settlement signal drops below
    /// `ANIMATION_SETTLE_EPSILON` or `kb_graph_animate` has been turned off
    /// in the meantime.
    ///
    /// `scene` is a background-thread snapshot cloned when the PREVIOUS
    /// tick was queued, so a hover/selection change that landed after that
    /// snapshot was taken (e.g. the mouse moved to a new node mid-tick)
    /// would otherwise be silently reverted by this wholesale scene
    /// replace — `ForceLayout::step` only ever moves node positions, never
    /// touches `hovered`/`selection`, so it's always correct to carry the
    /// CURRENT values forward onto the incoming scene rather than the
    /// snapshot's stale ones.
    ///
    /// The same hazard applies to a node's position/`pinned` flag: `scene`
    /// can be a snapshot taken BEFORE a drag-to-pin gesture started (or
    /// before `kb_graph_view_drag_node` marked the node pinned), so the
    /// background pass computed it as a freely-movable node and the
    /// incoming `scene` carries a stale, force-computed position for it.
    /// Applying that wholesale would silently teleport the node the user
    /// is actively dragging back to wherever physics put it seconds ago —
    /// visible as a periodic snap-back mid-drag, worse the longer a tick
    /// takes to compute (large graphs) since more drag movement happens
    /// before the stale result lands. Fixed the same way as
    /// hovered/selection: for every node currently pinned in the LIVE
    /// scene, the live position wins over the incoming one, regardless of
    /// how stale the snapshot that produced `scene` was.
    pub fn apply_graph_layout_result(
        &mut self,
        buf_idx: usize,
        mut scene: mae_canvas::scene::SceneGraph,
        max_displacement: Option<f32>,
    ) {
        if buf_idx >= self.buffers.len() || self.buffers[buf_idx].kind != BufferKind::Graph {
            return;
        }
        if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
            scene.hovered = gv.scene.hovered;
            scene.selection = gv.scene.selection;
            for (i, live_node) in gv.scene.nodes.iter().enumerate() {
                if !live_node.pinned {
                    continue;
                }
                if let Some(incoming) = scene.nodes.get_mut(i) {
                    incoming.x = live_node.x;
                    incoming.y = live_node.y;
                    incoming.pinned = true;
                }
            }
            gv.scene = scene;
        }
        self.graph_view_reflatten_all_windows(buf_idx);

        if let Some(disp) = max_displacement {
            let settled = disp.abs() < ANIMATION_SETTLE_EPSILON;
            let still_animating = self.kb_graph_animate && !settled;
            if let Some(gv) = self.buffers[buf_idx].graph_view_mut() {
                gv.animating = still_animating;
                if still_animating {
                    gv.anim_temperature = (gv.anim_temperature * ANIMATION_COOLING_FACTOR)
                        .max(ANIMATION_TEMPERATURE_FLOOR);
                }
            }
            if still_animating {
                let next_scene = self.buffers[buf_idx]
                    .graph_view()
                    .map(|gv| gv.scene.clone());
                let next_temperature = self.buffers[buf_idx]
                    .graph_view()
                    .map(|gv| gv.anim_temperature);
                // Read the CACHED `layout_config` (set once by
                // `populate_graph_buffer`) rather than defaulting — see
                // `GraphView.layout_config`'s doc comment. Without this, a
                // kind-clustered graph would silently lose its clustering
                // on the second and every subsequent animation tick.
                let layout_config = self.buffers[buf_idx]
                    .graph_view()
                    .map(|gv| gv.layout_config.clone());
                if let (Some(scene), Some(temperature), Some(layout_config)) =
                    (next_scene, next_temperature, layout_config)
                {
                    self.pending_graph_layout = Some(GraphLayoutIntent {
                        buf_idx,
                        scene,
                        mode: GraphLayoutMode::Tick { temperature },
                        layout_config,
                    });
                }
            }
        }
    }

    /// Follow-current-node (Part C Phase 2): called at the end of
    /// `dispatch_builtin`'s `command-post` firing point, i.e. after EVERY
    /// dispatched command — not gated behind the coarser `buffer-switch`
    /// hook, because in-KB-buffer link-following (`Editor::help_follow_link`)
    /// updates the active buffer's `KbView.current` in place without
    /// necessarily changing the active buffer index (see that function's
    /// use of `kb_view_mut()`/`kb_view()`, both scoped to
    /// `active_buffer_idx()`). Re-centers the open `*KB Graph*` buffer, in
    /// place (never re-splits, matching `kb_graph_view_refresh_if_open`'s
    /// template), on whichever KB node the active buffer is currently
    /// showing.
    ///
    /// Short-circuits to a single `buffers.iter().position` scan (the same
    /// cost every sibling `kb_graph_view_*` method in this file already
    /// pays) when no `BufferKind::Graph` window exists — the common case,
    /// since most commands run with no graph view open. Reads
    /// `GraphView.follow_current` (snapshotted from the
    /// `kb_graph_follow_current_node` option when the graph was opened —
    /// see `kb_graph_view_open`) rather than re-reading the live option, so
    /// behavior is consistent with `GraphView.depth`'s existing
    /// snapshot-on-open pattern elsewhere in this file. Only calls
    /// `populate_graph_buffer` (the actual re-extract + re-flatten work)
    /// when the active KB node genuinely differs from `GraphView.center_
    /// node` — a no-op re-navigation (e.g. re-selecting the same node) or a
    /// command that doesn't touch a KB buffer at all costs nothing beyond
    /// that one scan plus a cheap field comparison.
    pub fn maybe_follow_kb_graph_view(&mut self) {
        let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
        else {
            return;
        };
        let Some((follow, center_node, depth)) = self.buffers[idx]
            .graph_view()
            .map(|gv| (gv.follow_current, gv.center_node.clone(), gv.depth))
        else {
            return;
        };
        if !follow {
            return;
        }
        let Some(current) = self.kb_view().map(|v| v.current.clone()) else {
            return;
        };
        if center_node.as_deref() == Some(current.as_str()) {
            return;
        }
        self.populate_graph_buffer(idx, current, depth);
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
    fn kb_graph_layout_spacing_scale_option_flows_into_the_cached_layout_config() {
        // Proves the option reaches `populate_graph_buffer`'s construction
        // site, not just the OptionRegistry — set a non-default value
        // BEFORE opening, then confirm the GraphView's cached
        // `layout_config` (the value every subsequent animation tick reuses,
        // per its own doc comment) actually carries it.
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_layout_spacing_scale = 7.5;
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        assert_eq!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .layout_config
                .spacing_scale,
            7.5,
            "the option's value must flow into the cached layout_config"
        );
    }

    #[test]
    fn tick_graph_color_tweens_clears_completed_tweens_and_reflattens() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        if let Some(gv) = editor.buffers[graph_idx].graph_view_mut() {
            gv.color_tween = Some(GraphColorTween {
                node_index: 0,
                from_hex: "#000000".to_string(),
                to_hex: "#ffffff".to_string(),
                started_at: std::time::Instant::now() - std::time::Duration::from_secs(10),
                duration: std::time::Duration::from_millis(100),
            });
        }
        editor.tick_graph_color_tweens();
        assert!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .color_tween
                .is_none(),
            "a tween well past its duration must be cleared"
        );
    }

    #[test]
    fn tick_graph_color_tweens_keeps_in_flight_tweens_and_reflattens() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        if let Some(gv) = editor.buffers[graph_idx].graph_view_mut() {
            gv.color_tween = Some(GraphColorTween {
                node_index: 0,
                from_hex: "#000000".to_string(),
                to_hex: "#ffffff".to_string(),
                started_at: std::time::Instant::now(),
                duration: std::time::Duration::from_secs(60),
            });
        }
        editor.tick_graph_color_tweens();
        assert!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .color_tween
                .is_some(),
            "a tween well within its duration must survive a tick"
        );
    }

    #[test]
    fn maybe_start_color_tween_is_a_no_op_when_disabled() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_color_tween_enabled = false;
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        editor.maybe_start_color_tween(graph_idx, 0, "#ffffff".to_string());
        assert!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .color_tween
                .is_none(),
            "must not start a tween when kb_graph_color_tween_enabled is off"
        );
    }

    /// Convert a node's SCENE position into the window-relative PIXEL
    /// position that would actually land a click on it — the exact inverse
    /// of `graph_scene_point`'s `viewport_to_scene`, driven by the same
    /// per-window `Viewport` (`gv.viewports[&win_id]`) the click/drag/
    /// hit-test methods under test read. Click/drag tests must click in
    /// pixel space (what real mouse events deliver), not raw scene
    /// coordinates — the default `Viewport` centers the scene origin at
    /// (width/2, height/2), so scene and pixel coordinates are NOT
    /// interchangeable.
    fn scene_to_click_px(gv: &GraphView, win_id: WindowId, x: f64, y: f64) -> (f32, f32) {
        let viewport = gv.viewports.get(&win_id).copied().unwrap_or_default();
        let (px, py) = mae_canvas::interaction::scene_to_viewport(&viewport, x, y);
        (px as f32, py as f32)
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
    fn state_is_none_when_no_graph_view_is_open() {
        let editor = Editor::new();
        assert!(editor.kb_graph_view_state().is_none());
    }

    #[test]
    fn state_reflects_open_graph_and_selection() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "[[concept:window]]");
        editor.kb.primary.insert(mae_kb::Node::new(
            "concept:window",
            "Window",
            mae_kb::NodeKind::Concept,
            "",
        ));
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), Some(1));

        let state = editor.kb_graph_view_state().expect("a graph view is open");
        assert_eq!(state.center_node.as_deref(), Some("concept:buffer"));
        assert_eq!(state.depth, 1);
        assert!(
            state.nodes.len() >= 2,
            "the center node + its one link should both be present at depth 1"
        );
        assert!(state.nodes.iter().any(|n| n.id == "concept:buffer"));
        assert!(state.nodes.iter().any(|n| n.id == "concept:window"));
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
    fn sync_open_graph_viewports_is_a_no_op_when_no_graph_buffer_is_open() {
        let mut editor = Editor::new();
        assert!(!editor.sync_open_graph_viewports());
    }

    #[test]
    fn sync_open_graph_viewports_is_a_no_op_when_size_is_unchanged() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        // kb_graph_view_open already reflattened against the current
        // last_layout_area/window rect — nothing has changed since.
        assert!(!editor.sync_open_graph_viewports());
    }

    #[test]
    fn sync_open_graph_viewports_reflattens_on_a_simulated_resize() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == idx)
            .map(|w| w.id)
            .unwrap();
        let (w0, h0) = {
            let gv = editor.buffers[idx].graph_view().unwrap();
            let vp = gv.viewports.get(&win_id).unwrap();
            (vp.width, vp.height)
        };

        // Simulate a GUI window resize: last_layout_area changes size
        // (mirrors gui_app.rs's WindowEvent::Resized handler), nothing else
        // touches the graph buffer.
        editor.last_layout_area = crate::window::Rect {
            x: 0,
            y: 0,
            width: editor.last_layout_area.width + 40,
            height: editor.last_layout_area.height + 20,
        };

        assert!(
            editor.sync_open_graph_viewports(),
            "a real size change must be detected and resynced"
        );
        let gv = editor.buffers[idx].graph_view().unwrap();
        let vp = gv.viewports.get(&win_id).unwrap();
        assert_ne!(vp.width, w0);
        assert_ne!(vp.height, h0);

        // Calling again with no further size change must be a no-op.
        assert!(!editor.sync_open_graph_viewports());
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
        assert!(
            after.rendered.values().any(|vb| !vb.elements.is_empty()),
            "at least one window's rendered cache must reflect the current scene"
        );
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

        // Read back node 0's REAL layout position and click exactly there
        // (converted from scene to pixel space — see `scene_to_click_px`).
        let (node_id, x, y) = {
            let gv = editor.buffers[graph_idx].graph_view().unwrap();
            assert!(
                gv.scene.nodes.len() >= 2,
                "a center node + its one link should both be present at depth 1"
            );
            let node = &gv.scene.nodes[0];
            let (x, y) = scene_to_click_px(gv, graph_win_id, node.x, node.y);
            (node.id.clone(), x, y)
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
            scene_to_click_px(gv, graph_win_id, node.x, node.y)
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

    // --- Hover introspection (resize-adaptivity + hover/selection
    // introspection parity pass) ---

    #[test]
    fn hover_at_a_node_sets_hovered_without_touching_selection() {
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
        let (x, y, selection_before) = {
            let gv = editor.buffers[graph_idx].graph_view().unwrap();
            let node = &gv.scene.nodes[0];
            let (x, y) = scene_to_click_px(gv, graph_win_id, node.x, node.y);
            (x, y, gv.scene.selection)
        };

        assert!(
            editor.kb_graph_view_hover_at(graph_win_id, x, y),
            "hovering a previously-unhovered node must report a change"
        );

        let gv = editor.buffers[graph_idx].graph_view().unwrap();
        assert_eq!(gv.scene.hovered, Some(0));
        assert_eq!(
            gv.scene.selection, selection_before,
            "hover must never mutate selection"
        );
    }

    #[test]
    fn hover_miss_clears_hovered() {
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
        editor.buffers[graph_idx]
            .graph_view_mut()
            .unwrap()
            .scene
            .hovered = Some(0);

        // Hover far away from any node's bounding box — a guaranteed miss.
        assert!(
            editor.kb_graph_view_hover_at(graph_win_id, 1_000_000.0, 1_000_000.0),
            "clearing a previously-hovered node via a miss must report a change"
        );

        assert_eq!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .scene
                .hovered,
            None
        );
    }

    #[test]
    fn hover_on_a_non_graph_window_or_stale_window_is_a_harmless_no_op() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let non_graph_win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx != graph_idx)
            .map(|w| w.id)
            .expect("opening the graph view should have split");

        // Must not panic, must report no change, and must not touch the
        // graph's hovered state.
        assert!(!editor.kb_graph_view_hover_at(non_graph_win_id, 0.0, 0.0));
        assert!(!editor.kb_graph_view_hover_at(999_999, 0.0, 0.0));

        assert_eq!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .scene
                .hovered,
            None
        );
    }

    #[test]
    fn clear_hover_returns_false_and_is_a_no_op_when_nothing_is_hovered() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        assert!(!editor.kb_graph_view_clear_hover());
    }

    #[test]
    fn clear_hover_clears_and_returns_true_when_something_was_hovered() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        editor.buffers[graph_idx]
            .graph_view_mut()
            .unwrap()
            .scene
            .hovered = Some(0);

        assert!(editor.kb_graph_view_clear_hover());
        assert_eq!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .scene
                .hovered,
            None
        );
        // Idempotent: calling again once already clear is a no-op.
        assert!(!editor.kb_graph_view_clear_hover());
    }

    // --- Part C Phase 4: drag-to-pin + wheel-zoom ---
    //
    // Same testing shape as the click_at coverage above (no literal winit
    // event loop reachable from this crate — tested at the `Editor` function
    // boundary that `gui_app.rs`'s press/move/release/wheel handlers call
    // into). Real post-layout node coordinates are read back and used as
    // click/drag positions throughout (not hand-picked "unicorn" values).

    #[test]
    fn hit_test_and_select_selects_without_navigating() {
        // The mouse-DOWN half of a click/drag gesture: selects the node for
        // immediate visual feedback but must NOT navigate the companion
        // window (that's release's job) — otherwise every drag would
        // spuriously navigate before the user even finishes dragging.
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
        let (x, y) = {
            let gv = editor.buffers[graph_idx].graph_view().unwrap();
            let node = &gv.scene.nodes[0];
            scene_to_click_px(gv, graph_win_id, node.x, node.y)
        };
        let kb_buf_count_before = editor
            .buffers
            .iter()
            .filter(|b| b.kind == BufferKind::Kb)
            .count();

        let hit = editor.kb_graph_view_hit_test_and_select(graph_win_id, x, y);

        assert_eq!(hit, Some(0), "pressing on node 0 must hit-test to index 0");
        assert_eq!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .scene
                .selection,
            Some(0),
            "hit-test-and-select must update scene.selection"
        );
        assert_eq!(
            editor
                .buffers
                .iter()
                .filter(|b| b.kind == BufferKind::Kb)
                .count(),
            kb_buf_count_before,
            "hit-test-and-select must NOT navigate the companion window"
        );
    }

    #[test]
    fn drag_start_move_release_pins_node_at_expected_position_without_navigating() {
        // The full drag gesture `gui_app.rs` drives: hit-test-and-select on
        // press, `kb_graph_view_drag_node` on each subsequent move, then
        // `kb_graph_view_drag_end` on release — must end with the node
        // pinned exactly where the drag left it, and must NEVER navigate
        // the companion window (that's the click-without-drag path only).
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
        let (scene_x0, scene_y0, press_x, press_y) = {
            let gv = editor.buffers[graph_idx].graph_view().unwrap();
            let node = &gv.scene.nodes[0];
            assert!(
                !node.pinned,
                "sanity: a freshly laid-out node must start unpinned"
            );
            let (px, py) = scene_to_click_px(gv, graph_win_id, node.x, node.y);
            (node.x, node.y, px, py)
        };
        let kb_buf_count_before = editor
            .buffers
            .iter()
            .filter(|b| b.kind == BufferKind::Kb)
            .count();
        let window_count_before = editor.window_mgr.iter_windows().count();

        // Press.
        let hit = editor.kb_graph_view_hit_test_and_select(graph_win_id, press_x, press_y);
        assert_eq!(hit, Some(0));

        // Drag across several intermediate PIXEL positions (real multi-step
        // movement, not a single teleport) to a final resting position 200
        // pixels away. With the default viewport's zoom of 1.0, a pixel
        // delta maps 1:1 to a scene delta (see `scene_to_click_px`'s
        // doc), so the expected scene position below is `scene_x0`/
        // `scene_y0` shifted by that same delta.
        let final_x = press_x + 200.0;
        let final_y = press_y - 150.0;
        for step in 1..=4 {
            let t = step as f32 / 4.0;
            let rx = press_x + (final_x - press_x) * t;
            let ry = press_y + (final_y - press_y) * t;
            editor.kb_graph_view_drag_node(graph_win_id, 0, rx, ry);
        }
        let expected_scene_x = scene_x0 as f32 + 200.0;
        let expected_scene_y = scene_y0 as f32 - 150.0;

        // Mid-drag: the node must already track the cursor AND already be
        // pinned (from the very first drag-move call, not deferred to
        // release) — this is what closes the race against a concurrent
        // animation tick landing mid-drag and snapping the node back to a
        // force-computed position; see `kb_graph_view_drag_node`'s doc
        // comment. No navigation should have happened either way.
        {
            let gv = editor.buffers[graph_idx].graph_view().unwrap();
            let node = &gv.scene.nodes[0];
            assert!(
                (node.x as f32 - expected_scene_x).abs() < 0.01,
                "node should track the cursor's scene-x during drag"
            );
            assert!(
                (node.y as f32 - expected_scene_y).abs() < 0.01,
                "node should track the cursor's scene-y during drag"
            );
            assert!(
                node.pinned,
                "must already be pinned mid-drag so a concurrent layout tick can't snap it back"
            );
        }

        // Release.
        editor.kb_graph_view_drag_end(graph_win_id, 0);

        let gv = editor.buffers[graph_idx].graph_view().unwrap();
        let node = &gv.scene.nodes[0];
        assert!(node.pinned, "release must pin the node");
        assert!(
            (node.x as f32 - expected_scene_x).abs() < 0.01,
            "pinned position must match the drag's final position (x)"
        );
        assert!(
            (node.y as f32 - expected_scene_y).abs() < 0.01,
            "pinned position must match the drag's final position (y)"
        );
        assert_eq!(
            editor
                .buffers
                .iter()
                .filter(|b| b.kind == BufferKind::Kb)
                .count(),
            kb_buf_count_before,
            "a drag-to-pin gesture must never navigate the companion window"
        );
        assert_eq!(
            editor.window_mgr.iter_windows().count(),
            window_count_before,
            "a drag-to-pin gesture must never open/close/split any window"
        );
    }

    #[test]
    fn stale_in_flight_layout_result_does_not_snap_back_a_node_pinned_mid_drag() {
        // Reproduces the reported bug directly: a background layout tick
        // can be queued from a scene snapshot taken BEFORE a drag starts.
        // By the time that tick's result comes back and is applied, the
        // user may have already dragged (and pinned) the node somewhere
        // else entirely. `apply_graph_layout_result` must not let the
        // stale incoming scene's un-pinned, force-computed position for
        // that node win over the live, actively-dragged one.
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

        // Snapshot the scene as it looked BEFORE the drag — this stands in
        // for what a background layout tick queued at that moment would
        // eventually compute from and return.
        let mut stale_scene = editor.buffers[graph_idx]
            .graph_view()
            .unwrap()
            .scene
            .clone();
        assert!(
            !stale_scene.nodes[0].pinned,
            "sanity: the pre-drag snapshot must show the node as unpinned"
        );
        // The background pass would have moved the (then-unpinned) node
        // based on forces — simulate that with an arbitrary displaced
        // position, distinct from both the original and the drag target.
        stale_scene.nodes[0].x = -999.0;
        stale_scene.nodes[0].y = -999.0;

        // Meanwhile, on the main thread, the user drags (and thereby pins,
        // per `kb_graph_view_drag_node`'s fix) the node somewhere else.
        editor.kb_graph_view_hit_test_and_select(graph_win_id, 0.0, 0.0);
        editor.kb_graph_view_drag_node(graph_win_id, 0, 12345.0, -6789.0);
        let (dragged_x, dragged_y) = {
            let node = &editor.buffers[graph_idx].graph_view().unwrap().scene.nodes[0];
            assert!(node.pinned, "sanity: drag must pin immediately");
            (node.x, node.y)
        };

        // The stale in-flight tick's result now lands.
        editor.apply_graph_layout_result(graph_idx, stale_scene, Some(1.0));

        let node = &editor.buffers[graph_idx].graph_view().unwrap().scene.nodes[0];
        assert!(
            node.pinned,
            "pinned flag must survive a stale layout-result apply"
        );
        assert_eq!(
            node.x, dragged_x,
            "a stale in-flight tick must not overwrite the live drag position (x)"
        );
        assert_eq!(
            node.y, dragged_y,
            "a stale in-flight tick must not overwrite the live drag position (y)"
        );
    }

    #[test]
    fn pinned_node_is_left_in_place_by_a_subsequent_force_layout_pass() {
        // Regression guard for the task's explicit requirement: "A
        // subsequent layout/animation tick ... must then leave this node in
        // place." Exercises the REAL `ForceLayout` (not a mock) against the
        // pinned node, mirroring how `apply_graph_layout_result` would fold
        // a background layout pass back in.
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

        editor.kb_graph_view_hit_test_and_select(graph_win_id, 0.0, 0.0);
        editor.kb_graph_view_drag_node(graph_win_id, 0, 12345.0, -6789.0);
        editor.kb_graph_view_drag_end(graph_win_id, 0);

        let pinned_x;
        let pinned_y;
        let mut scene = {
            let gv = editor.buffers[graph_idx].graph_view().unwrap();
            let node = &gv.scene.nodes[0];
            pinned_x = node.x;
            pinned_y = node.y;
            gv.scene.clone()
        };

        let layout =
            mae_canvas::layout::ForceLayout::new(mae_canvas::layout::LayoutConfig::default());
        layout.run(&mut scene.nodes, &scene.edges, 50);

        assert_eq!(
            scene.nodes[0].x, pinned_x,
            "a real ForceLayout pass must not move the pinned node (x)"
        );
        assert_eq!(
            scene.nodes[0].y, pinned_y,
            "a real ForceLayout pass must not move the pinned node (y)"
        );
    }

    #[test]
    fn plain_click_without_drag_still_navigates_companion_regression_guard() {
        // Regression guard: Phase 1b's "quick click-and-release navigates"
        // behavior must survive the Phase 4 press/release split.
        // `gui_app.rs`'s release handler calls `kb_graph_view_click_at`
        // (not the drag path) when no movement occurred between press and
        // release — simulate that exact sequence here.
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
        let (node_id, x, y) = {
            let gv = editor.buffers[graph_idx].graph_view().unwrap();
            let node = &gv.scene.nodes[0];
            let (x, y) = scene_to_click_px(gv, graph_win_id, node.x, node.y);
            (node.id.clone(), x, y)
        };

        // Press: hit-test-and-select only (mirrors gui_app.rs's press path).
        let hit = editor.kb_graph_view_hit_test_and_select(graph_win_id, x, y);
        assert_eq!(hit, Some(0));
        // Release without any drag_node calls in between: the release
        // handler falls back to a plain click at the press coordinates.
        editor.kb_graph_view_click_at(graph_win_id, x, y);

        let kb_idx = editor
            .buffers
            .iter()
            .position(|b| b.kb_view().map(|kv| kv.current == node_id).unwrap_or(false))
            .expect("a click-without-drag must still navigate to the clicked node's KB buffer");
        assert!(editor
            .window_mgr
            .iter_windows()
            .any(|w| w.buffer_idx == kb_idx));
        assert!(
            !editor.buffers[graph_idx].graph_view().unwrap().scene.nodes[0].pinned,
            "a plain click must never pin the node"
        );
    }

    #[test]
    fn drag_node_and_drag_end_on_stale_window_or_node_do_not_panic() {
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

        // Stale window id.
        editor.kb_graph_view_drag_node(999_999, 0, 1.0, 1.0);
        editor.kb_graph_view_drag_end(999_999, 0);
        // Valid window, out-of-range node index.
        editor.kb_graph_view_drag_node(graph_win_id, 999, 1.0, 1.0);
        editor.kb_graph_view_drag_end(graph_win_id, 999);
    }

    /// Read window `win_id`'s current zoom level on graph buffer `graph_idx`.
    fn zoom_of(editor: &Editor, graph_idx: usize, win_id: WindowId) -> f64 {
        editor.buffers[graph_idx]
            .graph_view()
            .unwrap()
            .viewports
            .get(&win_id)
            .unwrap()
            .zoom
    }

    #[test]
    fn zoom_changes_viewport_zoom_within_clamped_range_and_reflattens() {
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
        let zoom_before = zoom_of(&editor, graph_idx, graph_win_id);

        // Zoom in (positive delta).
        editor.kb_graph_view_zoom(graph_win_id, 100.0, 400.0, 300.0);
        let zoom_after_in = zoom_of(&editor, graph_idx, graph_win_id);
        assert!(
            zoom_after_in > zoom_before,
            "a positive wheel delta must increase zoom (before={zoom_before}, after={zoom_after_in})"
        );
        assert!(zoom_after_in <= 10.0, "zoom must respect the upper clamp");
        assert!(
            !editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .rendered
                .get(&graph_win_id)
                .unwrap()
                .elements
                .is_empty(),
            "zoom must re-flatten (rendered must reflect the current scene)"
        );

        // Zoom out repeatedly — must never cross below the 0.1x floor no
        // matter how many times it's applied.
        for _ in 0..200 {
            editor.kb_graph_view_zoom(graph_win_id, -100.0, 400.0, 300.0);
        }
        let zoom_after_out = zoom_of(&editor, graph_idx, graph_win_id);
        assert!(
            zoom_after_out >= 0.1,
            "zoom must respect the lower clamp even under sustained zoom-out, got {zoom_after_out}"
        );

        // Zoom in aggressively — must never cross above the 10x ceiling.
        for _ in 0..200 {
            editor.kb_graph_view_zoom(graph_win_id, 100.0, 400.0, 300.0);
        }
        let zoom_after_max = zoom_of(&editor, graph_idx, graph_win_id);
        assert!(
            zoom_after_max <= 10.0,
            "zoom must respect the upper clamp even under sustained zoom-in, got {zoom_after_max}"
        );
    }

    #[test]
    fn zoom_on_non_graph_or_stale_window_is_a_harmless_no_op() {
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
        let non_graph_win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx != graph_idx)
            .map(|w| w.id)
            .expect("opening the graph view should have split");

        // Must not panic, and must not affect the graph's own viewport.
        let zoom_before = zoom_of(&editor, graph_idx, graph_win_id);
        editor.kb_graph_view_zoom(non_graph_win_id, 100.0, 0.0, 0.0);
        editor.kb_graph_view_zoom(999_999, 100.0, 0.0, 0.0);
        assert_eq!(
            zoom_of(&editor, graph_idx, graph_win_id),
            zoom_before,
            "zooming a non-graph/stale window must not affect the graph's own viewport"
        );
    }

    /// Read window `win_id`'s current viewport center on graph buffer `graph_idx`.
    fn center_of(editor: &Editor, graph_idx: usize, win_id: WindowId) -> (f64, f64) {
        let vp = editor.buffers[graph_idx]
            .graph_view()
            .unwrap()
            .viewports
            .get(&win_id)
            .unwrap();
        (vp.center_x, vp.center_y)
    }

    #[test]
    fn pan_moves_viewport_center_opposite_the_drag_direction_and_reflattens() {
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
        let (cx_before, cy_before) = center_of(&editor, graph_idx, graph_win_id);

        // Dragging right (positive dx) reveals content further left —
        // "grab and pull" semantics, matching `mae_canvas::interaction::pan`.
        editor.kb_graph_view_pan(graph_win_id, 50.0, -20.0);
        let (cx_after, cy_after) = center_of(&editor, graph_idx, graph_win_id);
        assert!(
            cx_after < cx_before,
            "dragging right must decrease center_x (before={cx_before}, after={cx_after})"
        );
        assert!(
            cy_after > cy_before,
            "dragging up (negative dy) must increase center_y (before={cy_before}, after={cy_after})"
        );
        assert!(
            !editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .rendered
                .get(&graph_win_id)
                .unwrap()
                .elements
                .is_empty(),
            "pan must re-flatten (rendered must reflect the current scene)"
        );
    }

    #[test]
    fn pan_on_non_graph_or_stale_window_is_a_harmless_no_op() {
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
        let non_graph_win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx != graph_idx)
            .map(|w| w.id)
            .expect("opening the graph view should have split");

        let center_before = center_of(&editor, graph_idx, graph_win_id);
        editor.kb_graph_view_pan(non_graph_win_id, 50.0, 50.0);
        editor.kb_graph_view_pan(999_999, 50.0, 50.0);
        assert_eq!(
            center_of(&editor, graph_idx, graph_win_id),
            center_before,
            "panning a non-graph/stale window must not affect the graph's own viewport"
        );
    }

    // --- Per-window viewport isolation (issue #321) ---
    //
    // `WindowManager` has no one-window-per-buffer-kind guard, so a second
    // window can be split onto the SAME `*KB Graph*` buffer
    // (`:split-vertical`/`-horizontal` while it's focused). These tests
    // exercise that exact scenario directly, rather than assuming it can't
    // happen.

    #[test]
    fn two_windows_on_the_same_graph_buffer_have_independent_viewports() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let win_a = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == graph_idx)
            .map(|w| w.id)
            .unwrap();

        let area = editor.default_area();
        let win_b = editor
            .window_mgr
            .split(SplitDirection::Vertical, graph_idx, area)
            .expect("a second window on the same graph buffer must be allowed");
        assert_ne!(win_a, win_b);
        editor.sync_open_graph_viewports();

        assert!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .viewports
                .contains_key(&win_a),
            "window A must have its own viewport entry"
        );
        assert!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .viewports
                .contains_key(&win_b),
            "window B must have its own viewport entry"
        );

        // Zoom window A only.
        editor.kb_graph_view_zoom(win_a, 100.0, 400.0, 300.0);
        let zoom_a = zoom_of(&editor, graph_idx, win_a);
        let zoom_b = zoom_of(&editor, graph_idx, win_b);
        assert!(zoom_a > 1.0, "window A's zoom must have increased");
        assert_eq!(
            zoom_b, 1.0,
            "zooming window A must not affect window B's independent viewport"
        );
    }

    #[test]
    fn hit_test_in_one_window_does_not_use_another_windows_viewport() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let win_a = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == graph_idx)
            .map(|w| w.id)
            .unwrap();
        let area = editor.default_area();
        let win_b = editor
            .window_mgr
            .split(SplitDirection::Vertical, graph_idx, area)
            .unwrap();
        editor.sync_open_graph_viewports();

        // Zoom window A substantially (each call's step is clamped — see
        // GRAPH_ZOOM_MAX_STEP — so repeat it) so its scene<->pixel transform
        // genuinely diverges from window B's (still at the default 1.0x).
        for _ in 0..5 {
            editor.kb_graph_view_zoom(win_a, 300.0, 400.0, 300.0);
        }
        assert!(zoom_of(&editor, graph_idx, win_a) > 1.5);
        assert_eq!(zoom_of(&editor, graph_idx, win_b), 1.0);

        let (node_x, node_y) = {
            let gv = editor.buffers[graph_idx].graph_view().unwrap();
            let node = &gv.scene.nodes[0];
            (node.x, node.y)
        };

        // The pixel position that lands on the node UNDER WINDOW A's
        // (zoomed) viewport.
        let (px_a, py_a) = {
            let gv = editor.buffers[graph_idx].graph_view().unwrap();
            scene_to_click_px(gv, win_a, node_x, node_y)
        };
        let hit_in_a = editor.kb_graph_view_hit_test_and_select(win_a, px_a, py_a);
        assert_eq!(
            hit_in_a,
            Some(0),
            "clicking the node's own zoomed pixel position in window A must hit it"
        );

        // Clicking that SAME pixel position in window B (unzoomed) resolves
        // through a DIFFERENT viewport and must not land on the same node —
        // if it did, the two windows would still be sharing one viewport.
        let hit_in_b = editor.kb_graph_view_hit_test_and_select(win_b, px_a, py_a);
        assert_ne!(
            hit_in_b,
            Some(0),
            "the same pixel position must not hit the same node in window B, \
             which has an independent (unzoomed) viewport"
        );

        // Window B's OWN pixel position for the node (computed via B's own
        // viewport) must still hit correctly — proving B has a genuinely
        // correct, independent viewport, not just a "doesn't match A" one.
        let (px_b, py_b) = {
            let gv = editor.buffers[graph_idx].graph_view().unwrap();
            scene_to_click_px(gv, win_b, node_x, node_y)
        };
        let hit_in_b_own_coords = editor.kb_graph_view_hit_test_and_select(win_b, px_b, py_b);
        assert_eq!(
            hit_in_b_own_coords,
            Some(0),
            "clicking the node's own pixel position, computed via window B's own viewport, \
             must hit it in window B"
        );
    }

    #[test]
    fn graph_scene_hit_radii_matches_flatten_scene_graphs_render_radius() {
        // Parity guard, in the same spirit as `positions_only_and_full_
        // agree_on_topology`: hit-testing and rendering must never
        // disagree about how big a node is. Confirms
        // graph_scene_hit_radii(...) * zoom == node_render_radius(...) for
        // every node, at a non-default zoom (so a stale fixed-radius
        // regression can't hide behind zoom == 1.0's no-op scaling term).
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
        for _ in 0..3 {
            editor.kb_graph_view_zoom(graph_win_id, 300.0, 400.0, 300.0);
        }
        let zoom = zoom_of(&editor, graph_idx, graph_win_id);
        assert!(zoom > 1.0, "test setup: expected a non-default zoom");

        let style = editor.graph_style_options();
        let gv = editor.buffers[graph_idx].graph_view().unwrap();
        let hit_radii = graph_scene_hit_radii(gv, graph_win_id, &style);
        for (i, hit_radius) in hit_radii.iter().enumerate() {
            let degree = gv.node_degrees.get(i).copied().unwrap_or(0);
            let expected_render_radius = node_render_radius(&style, degree, zoom);
            let actual_render_radius = (hit_radius * zoom) as f32;
            assert!(
                (actual_render_radius - expected_render_radius).abs() < 0.01,
                "node {i}: hit radius * zoom ({actual_render_radius}) must match the render \
                 radius ({expected_render_radius})"
            );
        }
    }

    #[test]
    fn a_node_position_change_reflattens_every_window_showing_the_buffer() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let win_a = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == graph_idx)
            .map(|w| w.id)
            .unwrap();
        let area = editor.default_area();
        let win_b = editor
            .window_mgr
            .split(SplitDirection::Vertical, graph_idx, area)
            .unwrap();
        editor.sync_open_graph_viewports();

        let (press_x, press_y) = {
            let gv = editor.buffers[graph_idx].graph_view().unwrap();
            let node = &gv.scene.nodes[0];
            scene_to_click_px(gv, win_a, node.x, node.y)
        };
        editor.kb_graph_view_hit_test_and_select(win_a, press_x, press_y);
        editor.kb_graph_view_drag_node(win_a, 0, press_x + 300.0, press_y - 200.0);
        editor.kb_graph_view_drag_end(win_a, 0);

        // The node moved (shared topology) — BOTH windows' rendered caches
        // must reflect it, not just window A's.
        let gv = editor.buffers[graph_idx].graph_view().unwrap();
        assert!(gv.rendered.contains_key(&win_a));
        assert!(gv.rendered.contains_key(&win_b));
        assert!(!gv.rendered[&win_a].elements.is_empty());
        assert!(
            !gv.rendered[&win_b].elements.is_empty(),
            "dragging a node in window A must also reflatten window B, \
             since the node's new position is shared topology"
        );
    }

    #[test]
    fn sync_open_graph_viewports_prunes_a_closed_windows_entries() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let win_a = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == graph_idx)
            .map(|w| w.id)
            .unwrap();
        let area = editor.default_area();
        let win_b = editor
            .window_mgr
            .split(SplitDirection::Vertical, graph_idx, area)
            .unwrap();
        editor.sync_open_graph_viewports();
        assert!(editor.buffers[graph_idx]
            .graph_view()
            .unwrap()
            .viewports
            .contains_key(&win_b));

        editor.window_mgr.close(win_b);
        editor.sync_open_graph_viewports();

        let gv = editor.buffers[graph_idx].graph_view().unwrap();
        assert!(
            !gv.viewports.contains_key(&win_b),
            "a closed window's viewport entry must be pruned"
        );
        assert!(
            !gv.rendered.contains_key(&win_b),
            "a closed window's rendered entry must be pruned"
        );
        assert!(
            gv.viewports.contains_key(&win_a),
            "the surviving window's entry must remain"
        );
    }

    // --- AI-appropriate zoom-to/pin primitives (issue #322) ---

    #[test]
    fn zoom_to_sets_the_focused_windows_zoom_to_an_explicit_level() {
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
        editor.window_mgr.set_focused(graph_win_id);

        let applied = editor.kb_graph_view_zoom_to(2.5);

        assert_eq!(applied, Some(2.5), "must return the actual applied zoom");
        assert_eq!(zoom_of(&editor, graph_idx, graph_win_id), 2.5);
    }

    #[test]
    fn zoom_to_clamps_to_the_same_range_as_wheel_zoom() {
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
        editor.window_mgr.set_focused(graph_win_id);

        let applied_high = editor.kb_graph_view_zoom_to(999.0);
        assert_eq!(
            applied_high,
            Some(10.0),
            "the returned value must be the CLAMPED result, not the raw 999.0 request — \
             a caller reporting this to a human or reasoning about its own action must \
             never be told the out-of-range value \"worked\""
        );
        assert_eq!(zoom_of(&editor, graph_idx, graph_win_id), 10.0);

        let applied_low = editor.kb_graph_view_zoom_to(-5.0);
        assert_eq!(applied_low, Some(0.1));
        assert_eq!(zoom_of(&editor, graph_idx, graph_win_id), 0.1);
    }

    #[test]
    fn zoom_to_targets_a_window_showing_the_graph_when_focus_is_elsewhere() {
        // No focused-graph-window context (e.g. an AI-driven session with
        // focus on a text buffer) — must still resolve to SOME window
        // showing the graph, not silently no-op.
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
        let non_graph_win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx != graph_idx)
            .map(|w| w.id)
            .expect("opening the graph view should have split");
        editor.window_mgr.set_focused(non_graph_win_id);

        let applied = editor.kb_graph_view_zoom_to(3.0);

        assert_eq!(applied, Some(3.0));
        assert_eq!(zoom_of(&editor, graph_idx, graph_win_id), 3.0);
    }

    #[test]
    fn zoom_to_is_a_harmless_no_op_when_no_graph_is_open() {
        let mut editor = Editor::new();
        let applied = editor.kb_graph_view_zoom_to(2.0); // must not panic
        assert_eq!(applied, None);
        assert!(!editor.buffers.iter().any(|b| b.kind == BufferKind::Graph));
    }

    #[test]
    fn set_pinned_pins_a_node_by_id_and_optionally_repositions_it() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let node_id = editor.buffers[graph_idx].graph_view().unwrap().scene.nodes[0]
            .id
            .clone();
        assert!(
            !editor.buffers[graph_idx].graph_view().unwrap().scene.nodes[0].pinned,
            "sanity: a freshly laid-out node must start unpinned"
        );

        let found = editor.kb_graph_view_set_pinned(&node_id, true, Some((123.0, -45.0)));

        assert!(found);
        let node = &editor.buffers[graph_idx].graph_view().unwrap().scene.nodes[0];
        assert!(node.pinned);
        assert_eq!(node.x, 123.0);
        assert_eq!(node.y, -45.0);
    }

    #[test]
    fn set_pinned_without_a_position_leaves_the_node_wherever_it_currently_is() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let (node_id, x0, y0) = {
            let node = &editor.buffers[graph_idx].graph_view().unwrap().scene.nodes[0];
            (node.id.clone(), node.x, node.y)
        };

        editor.kb_graph_view_set_pinned(&node_id, true, None);

        let node = &editor.buffers[graph_idx].graph_view().unwrap().scene.nodes[0];
        assert!(node.pinned);
        assert_eq!(node.x, x0);
        assert_eq!(node.y, y0);
    }

    #[test]
    fn set_pinned_can_unpin() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let node_id = editor.buffers[graph_idx].graph_view().unwrap().scene.nodes[0]
            .id
            .clone();
        editor.kb_graph_view_set_pinned(&node_id, true, None);
        assert!(editor.buffers[graph_idx].graph_view().unwrap().scene.nodes[0].pinned);

        editor.kb_graph_view_set_pinned(&node_id, false, None);

        assert!(!editor.buffers[graph_idx].graph_view().unwrap().scene.nodes[0].pinned);
    }

    #[test]
    fn set_pinned_reports_a_miss_for_an_unknown_id() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);

        let found = editor.kb_graph_view_set_pinned("concept:does-not-exist", true, None);

        assert!(!found);
    }

    #[test]
    fn set_pinned_is_a_harmless_no_op_when_no_graph_is_open() {
        let mut editor = Editor::new();
        let found = editor.kb_graph_view_set_pinned("anything", true, None);
        assert!(!found);
    }

    #[test]
    fn set_pinned_reflattens_every_window_showing_the_buffer() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let area = editor.default_area();
        let win_b = editor
            .window_mgr
            .split(SplitDirection::Vertical, graph_idx, area)
            .unwrap();
        editor.sync_open_graph_viewports();
        let node_id = editor.buffers[graph_idx].graph_view().unwrap().scene.nodes[0]
            .id
            .clone();

        editor.kb_graph_view_set_pinned(&node_id, true, Some((10.0, 10.0)));

        let gv = editor.buffers[graph_idx].graph_view().unwrap();
        assert!(!gv.rendered[&win_b].elements.is_empty());
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
        editor.apply_graph_layout_result(idx, new_scene, None);

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
    fn apply_layout_result_preserves_hover_and_selection_across_the_scene_replace() {
        // Regression guard: apply_graph_layout_result's `scene` argument is
        // a background-thread snapshot cloned when the PREVIOUS tick was
        // queued — a hover/selection change that landed after that
        // snapshot must survive the wholesale `gv.scene = scene` replace,
        // not get silently reverted.
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "[[concept:window]]");
        editor.kb.primary.insert(mae_kb::Node::new(
            "concept:window",
            "Window",
            mae_kb::NodeKind::Concept,
            "",
        ));
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), Some(1));
        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();

        // Snapshot the scene BEFORE the hover/selection change below, as
        // the background layout thread would have.
        let stale_snapshot = editor.buffers[idx].graph_view().unwrap().scene.clone();
        assert_eq!(
            stale_snapshot.hovered, None,
            "sanity: nothing hovered in the stale snapshot"
        );

        // Hover/select node 1 AFTER the snapshot was taken (simulating a
        // mouse move / click landing mid-tick).
        {
            let gv = editor.buffers[idx].graph_view_mut().unwrap();
            gv.scene.hovered = Some(1);
            gv.scene.selection = Some(1);
        }

        editor.apply_graph_layout_result(idx, stale_snapshot, Some(0.5));

        let gv = editor.buffers[idx].graph_view().unwrap();
        assert_eq!(
            gv.scene.hovered,
            Some(1),
            "hover must survive the animation-tick scene replace"
        );
        assert_eq!(
            gv.scene.selection,
            Some(1),
            "selection must survive the animation-tick scene replace"
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
        editor.apply_graph_layout_result(idx, scene, None);
    }

    // --- maybe_follow_kb_graph_view (Part C Phase 2, follow-current-node) ---

    #[test]
    fn follow_recenters_graph_when_active_kb_node_changes() {
        let mut editor = ed_with_kb_node("concept:a", "A", "[[concept:b]]");
        editor.kb.primary.insert(mae_kb::Node::new(
            "concept:b",
            "B",
            mae_kb::NodeKind::Concept,
            "",
        ));

        // Focus a KB buffer showing concept:a.
        let kb_idx = editor.ensure_kb_buffer_idx("concept:a");
        editor.kb_populate_buffer(kb_idx);
        editor.switch_to_buffer(kb_idx);
        let kb_win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == kb_idx)
            .map(|w| w.id)
            .unwrap();

        // Open the graph centered on concept:a — this splits and focuses the
        // NEW graph window (`display_buffer_split`'s normal behavior).
        // Simulate the human refocusing the KB window afterward (e.g.
        // `focus-left`), which is the scenario this feature targets: the
        // graph is open in a side window while the human browses the KB
        // buffer in the other one.
        editor.kb_graph_view_open(Some("concept:a".to_string()), Some(1));
        editor.window_mgr.set_focused(kb_win_id);
        assert_eq!(
            editor.active_buffer_idx(),
            kb_idx,
            "refocusing the KB window must make it active again"
        );
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        assert_eq!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .center_node
                .as_deref(),
            Some("concept:a")
        );

        // Simulate in-place KB navigation the way `help_follow_link` does:
        // mutate the active buffer's `KbView.current` WITHOUT switching the
        // active buffer index.
        editor.kb_view_mut().unwrap().navigate_to("concept:b");
        assert_eq!(editor.active_buffer_idx(), kb_idx);

        editor.maybe_follow_kb_graph_view();

        assert_eq!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .center_node
                .as_deref(),
            Some("concept:b"),
            "the graph must re-center on the node the active KB buffer now shows"
        );
    }

    #[test]
    fn follow_is_a_noop_when_the_node_has_not_changed() {
        let mut editor = ed_with_kb_node("concept:a", "A", "[[concept:b]]");
        editor.kb.primary.insert(mae_kb::Node::new(
            "concept:b",
            "B",
            mae_kb::NodeKind::Concept,
            "",
        ));
        let kb_idx = editor.ensure_kb_buffer_idx("concept:a");
        editor.kb_populate_buffer(kb_idx);
        editor.switch_to_buffer(kb_idx);
        let kb_win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == kb_idx)
            .map(|w| w.id)
            .unwrap();
        editor.kb_graph_view_open(Some("concept:a".to_string()), Some(1));
        editor.window_mgr.set_focused(kb_win_id);

        // First call: no navigation happened since open, so the center node
        // already matches — must be a no-op (no populate/layout work).
        editor.pending_graph_layout = None;
        editor.maybe_follow_kb_graph_view();
        assert!(
            editor.pending_graph_layout.is_none(),
            "re-checking an unchanged node must not queue a re-populate/layout pass"
        );

        // Navigate once (genuine change) to confirm the harness actually
        // detects changes, then call again with no further navigation.
        editor.kb_view_mut().unwrap().navigate_to("concept:b");
        editor.pending_graph_layout = None;
        editor.maybe_follow_kb_graph_view();
        assert!(
            editor.pending_graph_layout.is_some(),
            "sanity check: a genuine node change DOES queue a populate/layout pass"
        );

        // Reset and call again with the node still "concept:b" — no-op.
        editor.pending_graph_layout = None;
        editor.maybe_follow_kb_graph_view();
        assert!(
            editor.pending_graph_layout.is_none(),
            "calling again with the same current node must not redo any work"
        );
        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        assert_eq!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .center_node
                .as_deref(),
            Some("concept:b")
        );
    }

    #[test]
    fn follow_is_a_noop_when_disabled_on_the_graph_view() {
        let mut editor = ed_with_kb_node("concept:a", "A", "[[concept:b]]");
        editor.kb.primary.insert(mae_kb::Node::new(
            "concept:b",
            "B",
            mae_kb::NodeKind::Concept,
            "",
        ));
        let kb_idx = editor.ensure_kb_buffer_idx("concept:a");
        editor.kb_populate_buffer(kb_idx);
        editor.switch_to_buffer(kb_idx);
        let kb_win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == kb_idx)
            .map(|w| w.id)
            .unwrap();
        editor.kb_graph_view_open(Some("concept:a".to_string()), Some(1));
        editor.window_mgr.set_focused(kb_win_id);

        let graph_idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        editor.buffers[graph_idx]
            .graph_view_mut()
            .unwrap()
            .follow_current = false;

        editor.kb_view_mut().unwrap().navigate_to("concept:b");
        editor.pending_graph_layout = None;
        editor.maybe_follow_kb_graph_view();

        assert!(
            editor.pending_graph_layout.is_none(),
            "disabled follow must not queue any populate/layout work"
        );
        assert_eq!(
            editor.buffers[graph_idx]
                .graph_view()
                .unwrap()
                .center_node
                .as_deref(),
            Some("concept:a"),
            "disabled follow must leave the graph centered where it was"
        );
    }

    #[test]
    fn follow_is_a_harmless_noop_when_no_graph_view_is_open() {
        let mut editor = ed_with_kb_node("concept:a", "A", "[[concept:b]]");
        editor.kb.primary.insert(mae_kb::Node::new(
            "concept:b",
            "B",
            mae_kb::NodeKind::Concept,
            "",
        ));
        let kb_idx = editor.ensure_kb_buffer_idx("concept:a");
        editor.kb_populate_buffer(kb_idx);
        editor.switch_to_buffer(kb_idx);
        editor.kb_view_mut().unwrap().navigate_to("concept:b");

        // No `*KB Graph*` buffer exists at all — must return immediately
        // without creating one or touching any state.
        let buf_count_before = editor.buffers.len();
        editor.maybe_follow_kb_graph_view();
        assert_eq!(editor.buffers.len(), buf_count_before);
        assert!(!editor.buffers.iter().any(|b| b.kind == BufferKind::Graph));
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
        match pending.mode {
            GraphLayoutMode::OneShot { iterations } => {
                assert_eq!(iterations, editor.kb_graph_layout_iterations);
            }
            GraphLayoutMode::Tick { .. } => {
                panic!("kb_graph_animate defaults to false — open() must queue OneShot, not Tick")
            }
        }
    }

    // --- Part C Phase 3: `kb_graph_animate` physics-animation ticking ---

    #[test]
    fn animate_disabled_is_byte_for_byte_the_old_one_shot_path() {
        // The default (`kb_graph_animate = false`) must behave exactly like
        // Phase 1/2: a single OneShot request, `GraphView.animating` never
        // set, and `has_active_graph_animation` always false.
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        assert!(!editor.kb_graph_animate, "sanity: animate defaults to off");
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);

        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        assert!(!editor.buffers[idx].graph_view().unwrap().animating);
        assert!(!editor.has_active_graph_animation());

        // Simulate the OneShot request completing (no settlement signal) —
        // must not start animating or queue another request.
        let scene = editor.buffers[idx].graph_view().unwrap().scene.clone();
        editor.pending_graph_layout = None;
        editor.apply_graph_layout_result(idx, scene, None);
        assert!(!editor.buffers[idx].graph_view().unwrap().animating);
        assert!(editor.pending_graph_layout.is_none());
        assert!(!editor.has_active_graph_animation());
    }

    #[test]
    fn animate_enabled_open_queues_a_tick_not_a_one_shot() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_animate = true;
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);

        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        assert!(
            editor.buffers[idx].graph_view().unwrap().animating,
            "opening with kb_graph_animate=true should mark the view as animating"
        );
        assert!(editor.has_active_graph_animation());

        let pending = editor
            .pending_graph_layout
            .as_ref()
            .expect("open should queue the first animation tick");
        match pending.mode {
            GraphLayoutMode::Tick { temperature } => {
                assert_eq!(temperature, ANIMATION_INITIAL_TEMPERATURE);
            }
            GraphLayoutMode::OneShot { .. } => {
                panic!("kb_graph_animate=true — open() must queue a Tick, not OneShot")
            }
        }
    }

    #[test]
    fn animate_tick_with_large_displacement_requeues_another_tick() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_animate = true;
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();

        let scene = editor.buffers[idx].graph_view().unwrap().scene.clone();
        editor.pending_graph_layout = None;
        // Well above ANIMATION_SETTLE_EPSILON — still moving.
        editor.apply_graph_layout_result(idx, scene, Some(10.0));

        assert!(
            editor.buffers[idx].graph_view().unwrap().animating,
            "a large settlement signal must keep animating=true"
        );
        assert!(editor.has_active_graph_animation());
        let pending = editor
            .pending_graph_layout
            .as_ref()
            .expect("an unsettled tick result must queue the next tick");
        assert!(matches!(pending.mode, GraphLayoutMode::Tick { .. }));
    }

    #[test]
    fn animate_tick_with_small_displacement_settles_and_stops_requeuing() {
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_animate = true;
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();

        let scene = editor.buffers[idx].graph_view().unwrap().scene.clone();
        editor.pending_graph_layout = None;
        // Well below ANIMATION_SETTLE_EPSILON — converged.
        editor.apply_graph_layout_result(idx, scene, Some(0.01));

        assert!(
            !editor.buffers[idx].graph_view().unwrap().animating,
            "a small settlement signal must clear animating"
        );
        assert!(!editor.has_active_graph_animation());
        assert!(
            editor.pending_graph_layout.is_none(),
            "a settled tick result must NOT queue another tick"
        );
    }

    #[test]
    fn animate_disabled_mid_flight_stops_requeuing_even_with_large_displacement() {
        // If the user toggles kb_graph_animate off while a tick chain is in
        // flight, the in-flight result must not resurrect the chain.
        let mut editor = ed_with_kb_node("concept:buffer", "Buffer", "");
        editor.kb_graph_animate = true;
        editor.kb_graph_view_open(Some("concept:buffer".to_string()), None);
        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Graph)
            .unwrap();
        let scene = editor.buffers[idx].graph_view().unwrap().scene.clone();

        editor.kb_graph_animate = false;
        editor.pending_graph_layout = None;
        editor.apply_graph_layout_result(idx, scene, Some(10.0));

        assert!(!editor.buffers[idx].graph_view().unwrap().animating);
        assert!(!editor.has_active_graph_animation());
        assert!(editor.pending_graph_layout.is_none());
    }
}
