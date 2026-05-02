use ropey::Rope;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::buffer_view::BufferView;
use crate::conversation::Conversation;
use crate::debug_view::DebugView;
use crate::file_tree::FileTree;
use crate::git_status::GitStatusView;
use crate::help_view::HelpView;
use crate::visual_buffer::VisualBuffer;
use crate::window::Window;

/// What kind of content this buffer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    /// Normal text editing buffer (backed by rope).
    Text,
    /// AI conversation buffer (backed by structured entries, not rope).
    Conversation,
    /// Rendered preview of org/markdown (read-only).
    Preview,
    /// In-editor log viewer (*Messages* buffer). Read-only, live view.
    Messages,
    /// Knowledge-base viewer (`*Help*`). Body rendered live from the KB.
    Help,
    /// Terminal emulator buffer. Rendering is driven by an external
    /// `ShellTerminal` (lives in `mae` binary, not in core).
    Shell,
    /// DAP debug panel — read-only dashboard showing threads, stack frames,
    /// scopes, and variables from `DebugState`.
    Debug,
    /// Startup dashboard — read-only buffer that shows the splash screen.
    /// Unlike `Text` scratch, this buffer always renders the splash overlay.
    Dashboard,
    /// Git status "porcelain" UI (Phase 6 M5).
    GitStatus,
    /// Visual scene-graph buffer (Phase 1 Visual Debugger).
    Visual,
    /// File tree sidebar — project-level directory browser.
    FileTree,
    /// AI-generated unified diff view (read-only).
    Diff,
}

/// A single edit operation, stored for undo/redo.
///
/// Emacs lesson: undo.c uses an unbounded cons-list truncated at GC time.
/// We use explicit action records with bounded stacks and standard undo/redo
/// semantics (redo stack cleared on new edit).
#[derive(Debug, Clone)]
pub enum EditAction {
    InsertChar {
        pos: usize,
        ch: char,
    },
    DeleteChar {
        pos: usize,
        ch: char,
    },
    InsertRange {
        pos: usize,
        text: String,
    },
    DeleteRange {
        pos: usize,
        text: String,
    },
    /// A group of actions that undo/redo as a single unit.
    Group(Vec<EditAction>),
}

/// Per-buffer option overrides (Emacs buffer-local variables / Vim setlocal).
///
/// Each field is `Option<T>`: `None` means "use the global Editor default",
/// `Some(v)` means this buffer overrides the global value. Access via
/// `Editor::effective_word_wrap()` and friends, never read `editor.word_wrap`
/// directly when a per-buffer check is needed.
#[derive(Debug, Clone, Default)]
pub struct BufferLocalOptions {
    pub word_wrap: Option<bool>,
    pub line_numbers: Option<bool>,
    pub relative_line_numbers: Option<bool>,
    pub break_indent: Option<bool>,
    pub show_break: Option<String>,
    pub heading_scale: Option<bool>,
    pub link_descriptive: Option<bool>,
    pub render_markup: Option<bool>,
}

impl BufferLocalOptions {
    /// Merge defaults into this set, only filling in fields that are currently None.
    pub fn apply_defaults(&mut self, defaults: &BufferLocalOptions) {
        if self.word_wrap.is_none() {
            self.word_wrap = defaults.word_wrap;
        }
        if self.line_numbers.is_none() {
            self.line_numbers = defaults.line_numbers;
        }
        if self.relative_line_numbers.is_none() {
            self.relative_line_numbers = defaults.relative_line_numbers;
        }
        if self.break_indent.is_none() {
            self.break_indent = defaults.break_indent;
        }
        if self.show_break.is_none() {
            self.show_break = defaults.show_break.clone();
        }
        if self.heading_scale.is_none() {
            self.heading_scale = defaults.heading_scale;
        }
        if self.link_descriptive.is_none() {
            self.link_descriptive = defaults.link_descriptive;
        }
        if self.render_markup.is_none() {
            self.render_markup = defaults.render_markup;
        }
    }
}

/// Rope-backed text buffer with undo history.
///
/// Emacs lesson: point (cursor) is per-window, not per-buffer. Two windows can
/// view the same buffer at different positions. Cursor state lives on `Window`.
///
/// Design: lean struct, pure state mutation, no I/O dependencies beyond std::fs.
/// All operations are designed to be called programmatically by an AI agent.
pub struct Buffer {
    rope: Rope,
    file_path: Option<PathBuf>,
    pub modified: bool,
    pub name: String,
    pub kind: BufferKind,
    /// Read-only buffers reject all edit operations. Set for Help, Messages.
    pub read_only: bool,
    /// Mode-specific state. Replaces 6 scattered Option<T> fields.
    pub view: BufferView,
    undo_stack: Vec<EditAction>,
    redo_stack: Vec<EditAction>,
    /// When non-None, edits accumulate here instead of the undo stack directly.
    /// `end_undo_group()` flushes them as a single `EditAction::Group`.
    undo_group_acc: Option<Vec<EditAction>>,
    /// Last known modification time of the backing file on disk.
    /// Used by auto-reload to detect external changes.
    pub file_mtime: Option<SystemTime>,
    /// Project root associated with this buffer, detected from its file path.
    /// When set, `Editor::active_project_root()` prefers this over the
    /// editor-wide `project` field, enabling per-buffer project context.
    pub project_root: Option<PathBuf>,
    /// Whether this is an AI agent shell (spawned by `open-ai-agent`).
    /// Agent shells are auto-closed when the process exits.
    pub agent_shell: bool,
    /// Line indices that are currently folded (hidden).
    /// NOTE: Fold boundaries are NOT adjusted on line insert/delete.
    /// After structural edits, refresh folds with `zx`. A proper fix
    /// (Emacs-style overlays with anchor tracking) is deferred.
    pub folded_ranges: Vec<(usize, usize)>,
    /// Monotonic counter incremented on every rope mutation. Used by
    /// `SyntaxMap` to detect stale cached spans without external
    /// invalidation calls.
    pub generation: u64,
    /// Per-buffer mode persistence (evil-mode pattern).  When switching away
    /// from a buffer the editor saves its current mode here; switching back
    /// restores it so that e.g. a Shell buffer in Normal mode stays Normal.
    pub saved_mode: Option<crate::Mode>,
    /// Narrowed view: when set, only lines in `[start, end)` are visible.
    /// Rendering and cursor movement are clamped to this range.
    pub narrowed_range: Option<(usize, usize)>,
    /// Line indices modified since the last save. Used by gutter rendering
    /// to show change markers. Cleared on `save()`.
    pub changed_lines: HashSet<usize>,
    /// Per-line git diff status (vs HEAD). Populated on file open/save.
    pub git_diff_lines: HashMap<usize, crate::render_common::gutter::GitLineStatus>,
    /// Detected link spans in the buffer content. Populated lazily by
    /// the renderer for conversation and shell buffers.
    pub link_spans: Vec<crate::link_detect::LinkSpan>,
    /// Global fold cycle state: 0 = SHOW ALL, 1 = OVERVIEW, 2 = CONTENTS.
    /// Cycled by Shift-TAB in org/markdown buffers (Doom Emacs pattern).
    pub global_fold_state: u8,
    /// Per-buffer option overrides (Emacs buffer-local / Vim setlocal).
    pub local_options: BufferLocalOptions,
    /// Display regions: byte ranges with display overrides (link concealment, etc.).
    /// Rebuilt lazily when `display_regions_gen != generation`.
    pub display_regions: Vec<crate::display_region::DisplayRegion>,
    /// Generation at which `display_regions` were last computed.
    pub display_regions_gen: u64,
    /// Cursor byte offset for org-appear reveal. When the cursor is inside a
    /// display region, that region is suppressed so raw text is visible.
    /// Set per-frame from the focused window's cursor position. `None` = no reveal.
    pub display_reveal_cursor: Option<usize>,
    /// Swap file state for crash recovery (Emacs-style autosave).
    pub swap: crate::swap::SwapState,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Buffer {
    pub fn new() -> Self {
        Buffer {
            rope: Rope::new(),
            file_path: None,
            modified: false,
            name: String::from("[scratch]"),
            kind: BufferKind::Text,
            read_only: false,
            view: BufferView::None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            undo_group_acc: None,
            file_mtime: None,
            project_root: None,
            agent_shell: false,
            folded_ranges: Vec::new(),
            generation: 0,
            saved_mode: None,
            narrowed_range: None,
            changed_lines: HashSet::new(),
            git_diff_lines: HashMap::new(),
            link_spans: Vec::new(),
            global_fold_state: 0,
            local_options: BufferLocalOptions::default(),
            display_regions: Vec::new(),
            display_regions_gen: u64::MAX, // force initial compute
            display_reveal_cursor: None,
            swap: crate::swap::SwapState::default(),
        }
    }

    /// Recompute display regions for link concealment.
    /// Called when buffer generation changes or `link_descriptive` toggles.
    pub fn recompute_display_regions(&mut self, link_descriptive: bool) {
        self.display_regions.clear();
        self.display_regions_gen = self.generation;

        if !link_descriptive {
            return;
        }

        // Only text buffers have link concealment.
        if self.kind != BufferKind::Text {
            return;
        }

        let ext = self
            .file_path
            .as_ref()
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str());

        let source: String = self.rope.chars().collect();
        self.display_regions = crate::display_region::compute_link_regions(&source, true, ext);
    }

    /// Create a dashboard buffer (startup splash screen).
    pub fn new_dashboard() -> Self {
        Buffer {
            name: String::from("[dashboard]"),
            kind: BufferKind::Dashboard,
            read_only: true,
            ..Self::new()
        }
    }

    /// Create a conversation buffer (AI interaction pane).
    /// Word-wrap is enabled by default — prose reads better wrapped.
    pub fn new_conversation(name: impl Into<String>) -> Self {
        Buffer {
            name: name.into(),
            kind: BufferKind::Conversation,
            view: BufferView::Conversation(Box::default()),
            local_options: BufferLocalOptions {
                word_wrap: Some(true),
                ..Default::default()
            },
            ..Self::new()
        }
    }

    /// Create a messages buffer (live view of the in-editor log).
    /// Word-wrap is enabled by default — log messages are prose.
    pub fn new_messages() -> Self {
        Buffer {
            name: String::from("*Messages*"),
            kind: BufferKind::Messages,
            read_only: true,
            local_options: BufferLocalOptions {
                word_wrap: Some(true),
                ..Default::default()
            },
            ..Self::new()
        }
    }

    /// Create a help buffer viewing a KB node.
    /// Word-wrap is enabled by default — help text is prose.
    pub fn new_help(start_node_id: impl Into<String>) -> Self {
        Buffer {
            name: String::from("*Help*"),
            kind: BufferKind::Help,
            read_only: true,
            view: BufferView::Help(Box::new(HelpView::new(start_node_id.into()))),
            local_options: BufferLocalOptions {
                word_wrap: Some(true),
                ..Default::default()
            },
            ..Self::new()
        }
    }

    /// Create a shell (terminal emulator) buffer.
    pub fn new_shell(name: impl Into<String>) -> Self {
        Buffer {
            name: name.into(),
            kind: BufferKind::Shell,
            read_only: true,
            ..Self::new()
        }
    }

    /// Create a file tree sidebar buffer.
    pub fn new_file_tree(root: &std::path::Path) -> Self {
        Buffer {
            name: String::from(" File Tree "),
            kind: BufferKind::FileTree,
            read_only: true,
            view: BufferView::FileTree(Box::new(FileTree::open(root))),
            ..Self::new()
        }
    }

    /// Create a debug panel buffer.
    pub fn new_debug() -> Self {
        Buffer {
            name: String::from("*Debug*"),
            kind: BufferKind::Debug,
            read_only: true,
            view: BufferView::Debug(Box::default()),
            ..Self::new()
        }
    }

    pub fn from_file(path: &Path) -> std::io::Result<Self> {
        let content = fs::read_to_string(path)?;
        let rope = Rope::from_str(&content);
        let mtime = fs::metadata(path).and_then(|m| m.modified()).ok();
        let project_root = crate::project::detect_project_root(path);
        Ok(Buffer {
            rope,
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            file_path: Some(path.to_path_buf()),
            file_mtime: mtime,
            project_root,
            ..Self::new()
        })
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        if let Some(ref path) = self.file_path {
            // Atomic save: write to a temp file in the same directory, then
            // rename. This prevents data loss if the write is interrupted
            // (disk full, crash, etc.). rename(2) is atomic on POSIX.
            let parent = path.parent().unwrap_or(Path::new("."));
            let tmp_path = parent.join(format!(".mae-save-{}.tmp", std::process::id()));
            fs::write(&tmp_path, self.rope.to_string())?;
            if let Err(e) = fs::rename(&tmp_path, path) {
                // Clean up temp file on rename failure.
                let _ = fs::remove_file(&tmp_path);
                return Err(e);
            }
            self.modified = false;
            // changed_lines persist across saves — cleared on revert/reload.
            self.file_mtime = fs::metadata(path).and_then(|m| m.modified()).ok();
            Ok(())
        } else {
            Err(std::io::Error::other("No file path set"))
        }
    }

    pub fn set_file_path(&mut self, path: PathBuf) {
        self.name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.file_path = Some(path);
    }

    pub fn file_path(&self) -> Option<&Path> {
        self.file_path.as_deref()
    }

    /// Replace the entire rope content (used by `:recover` from swap file).
    pub fn replace_rope(&mut self, rope: Rope) {
        self.rope = rope;
        self.generation += 1;
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    /// Check whether the backing file has been modified externally since we
    /// last loaded or saved it.
    pub fn check_disk_changed(&self) -> bool {
        let Some(ref path) = self.file_path else {
            return false;
        };
        let Some(stored) = self.file_mtime else {
            return false;
        };
        let Ok(meta) = fs::metadata(path) else {
            return false;
        };
        let Ok(disk_mtime) = meta.modified() else {
            return false;
        };
        disk_mtime > stored
    }

    /// Reload buffer contents from its backing file. Returns Ok(()) on
    /// success, Err if file_path is None or the read fails. Clears the
    /// modified flag and undo/redo history.
    pub fn reload_from_disk(&mut self) -> std::io::Result<()> {
        let path = self
            .file_path
            .as_ref()
            .ok_or_else(|| std::io::Error::other("No file path set"))?
            .clone();
        let content = fs::read_to_string(&path)?;
        self.rope = Rope::from_str(&content);
        self.modified = false;
        self.changed_lines.clear();
        self.file_mtime = fs::metadata(&path).and_then(|m| m.modified()).ok();
        self.undo_stack.clear();
        self.redo_stack.clear();
        Ok(())
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Replace the entire buffer contents. Used for read-only/generated buffers
    /// like *Messages*. Clears undo history.
    pub fn replace_contents(&mut self, text: &str) {
        self.rope = Rope::from_str(text);
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    // --- Text extraction ---

    /// Get the full text of a line, including trailing newline if present.
    pub fn line_text(&self, line: usize) -> String {
        if line >= self.rope.len_lines() {
            return String::new();
        }
        self.rope.line(line).to_string()
    }

    /// Get text in a character range [char_start, char_end).
    pub fn text_range(&self, char_start: usize, char_end: usize) -> String {
        let start = char_start.min(self.rope.len_chars());
        let end = char_end.min(self.rope.len_chars());
        if start >= end {
            return String::new();
        }
        self.rope.slice(start..end).to_string()
    }

    // --- Metrics ---

    /// Total rope line count, including the phantom empty line that ropey
    /// adds after a trailing `\n`.
    ///
    /// Use for: **clamp_cursor** (insert mode needs the phantom line after
    /// pressing Enter at EOF), rope char/byte index lookups, and search
    /// iteration over all rope lines.
    ///
    /// Do NOT use for: navigation bounds (jump-to, marks, jumplist, go-to-line),
    /// scroll bounds, layout iteration, movement limits, gutter width, or
    /// "go to last line" — use `display_line_count()` instead.
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Line count excluding the phantom empty line that ropey adds after
    /// a trailing `\n`.
    ///
    /// Use for: **cursor clamping and display** — scroll bounds, viewport
    /// limits, layout iteration, gutter width (line numbering), movement
    /// bounds (`move_down`, `G`, `goto-line`), mouse click clamping, cursor
    /// clamping, and any context where the user shouldn't land on or see the
    /// phantom line.
    ///
    /// Do NOT use for: rope char/byte index lookups that need the phantom
    /// line, or search iteration over all rope lines.
    pub fn display_line_count(&self) -> usize {
        let n = self.rope.len_lines();
        if n > 1 && self.rope.len_chars() > 0 && self.rope.char(self.rope.len_chars() - 1) == '\n' {
            n - 1
        } else {
            n
        }
    }

    /// Given a line, find the next visible line going forward (skipping folds).
    /// Returns `line + 1` if the next line is visible, or jumps past fold ends.
    pub fn next_visible_line(&self, line: usize) -> usize {
        let mut next = line + 1;
        // If next lands inside a fold, skip to the fold end.
        for (start, end) in &self.folded_ranges {
            if next > *start && next < *end {
                next = *end;
            }
        }
        next
    }

    /// Given a line, find the previous visible line going backward (skipping folds).
    /// Returns `line - 1` if visible, or jumps before fold starts.
    pub fn prev_visible_line(&self, line: usize) -> usize {
        if line == 0 {
            return 0;
        }
        let mut prev = line - 1;
        // If prev lands inside a fold, skip to the fold start.
        for (start, end) in &self.folded_ranges {
            if prev > *start && prev < *end {
                prev = *start;
            }
        }
        prev
    }

    /// Narrow the buffer view to a range of lines `[start, end)`.
    pub fn narrow_to(&mut self, start: usize, end: usize) {
        let clamped_end = end.min(self.line_count());
        if start < clamped_end {
            self.narrowed_range = Some((start, clamped_end));
        }
    }

    /// Remove narrowing, restoring the full buffer view.
    pub fn widen(&mut self) {
        self.narrowed_range = None;
    }

    /// Check if a line is visible (not hidden by narrowing).
    pub fn is_line_visible(&self, line: usize) -> bool {
        match self.narrowed_range {
            Some((start, end)) => line >= start && line < end,
            None => true,
        }
    }

    /// Length of a line in characters, excluding the trailing newline.
    pub fn line_len(&self, line: usize) -> usize {
        if line >= self.rope.len_lines() {
            return 0;
        }
        let line_slice = self.rope.line(line);
        let len = line_slice.len_chars();
        if len > 0 && line_slice.char(len - 1) == '\n' {
            len - 1
        } else {
            len
        }
    }

    /// Check if a line is inside a folded range (hidden).
    pub fn is_line_folded(&self, line: usize) -> bool {
        self.folded_ranges
            .iter()
            .any(|(start, end)| line > *start && line < *end)
    }

    /// Toggle fold at a given line. If the line is the start of a fold range,
    /// unfold it. If the line is inside a foldable range, fold it.
    pub fn toggle_fold_at(&mut self, line: usize, fold_ranges: &[(usize, usize)]) {
        // If this line starts an existing fold, remove it.
        if let Some(idx) = self.folded_ranges.iter().position(|(s, _)| *s == line) {
            self.folded_ranges.remove(idx);
            return;
        }
        // Find the innermost foldable range containing this line.
        let mut best: Option<(usize, usize)> = None;
        for &(start, end) in fold_ranges {
            if line >= start && line <= end && best.is_none_or(|(bs, be)| (end - start) < (be - bs))
            {
                best = Some((start, end));
            }
        }
        if let Some((start, end)) = best {
            // If already folded at this start, unfold.
            if let Some(idx) = self.folded_ranges.iter().position(|(s, _)| *s == start) {
                self.folded_ranges.remove(idx);
            } else {
                self.folded_ranges.push((start, end));
            }
        }
    }

    /// Fold all given ranges (zM).
    pub fn fold_all(&mut self, fold_ranges: &[(usize, usize)]) {
        self.folded_ranges.clear();
        self.folded_ranges.extend_from_slice(fold_ranges);
    }

    /// Unfold all ranges (zR).
    pub fn unfold_all(&mut self) {
        self.folded_ranges.clear();
    }

    /// Char offset in the rope for a given (row, col) position.
    pub fn char_offset_at(&self, row: usize, col: usize) -> usize {
        if self.rope.len_chars() == 0 {
            return 0;
        }
        let row = row.min(self.line_count().saturating_sub(1));
        let line_start = self.rope.line_to_char(row);
        line_start + col
    }

    /// Maximum number of undo entries to retain.
    const MAX_UNDO_ENTRIES: usize = 1000;

    /// Push an edit action onto the undo stack (or into the active group).
    fn push_undo(&mut self, action: EditAction) {
        if let Some(ref mut group) = self.undo_group_acc {
            group.push(action);
            return;
        }
        self.undo_stack.push(action);
        if self.undo_stack.len() > Self::MAX_UNDO_ENTRIES {
            let excess = self.undo_stack.len() - Self::MAX_UNDO_ENTRIES;
            self.undo_stack.drain(..excess);
        }
    }

    /// Begin accumulating edits into a single undo group.
    /// Call `end_undo_group()` to flush as one `EditAction::Group`.
    pub fn begin_undo_group(&mut self) {
        self.undo_group_acc = Some(Vec::new());
    }

    /// Flush the accumulated edits as a single undo entry.
    pub fn end_undo_group(&mut self) {
        if let Some(actions) = self.undo_group_acc.take() {
            if actions.len() == 1 {
                // Unwrap single-action groups for simplicity.
                self.undo_stack.push(actions.into_iter().next().unwrap());
            } else if !actions.is_empty() {
                self.undo_stack.push(EditAction::Group(actions));
            }
            self.redo_stack.clear();
        }
    }

    /// Increment the generation counter. Called on every rope mutation so
    /// that `SyntaxMap` can detect stale caches without explicit invalidation.
    fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    // --- Editing operations ---
    // Each records an EditAction for undo and clears the redo stack.
    // Cursor state is on Window, passed as parameter.

    pub fn insert_char(&mut self, win: &mut Window, ch: char) {
        if self.read_only {
            return;
        }
        let pos = self.char_offset_at(win.cursor_row, win.cursor_col);
        self.rope.insert_char(pos, ch);
        self.push_undo(EditAction::InsertChar { pos, ch });
        self.redo_stack.clear();
        self.changed_lines.insert(win.cursor_row);
        if ch == '\n' {
            win.cursor_row += 1;
            win.cursor_col = 0;
            self.changed_lines.insert(win.cursor_row);
        } else {
            win.cursor_col += 1;
        }
        self.modified = true;
        self.bump_generation();
    }

    pub fn delete_char_backward(&mut self, win: &mut Window) {
        if self.read_only {
            return;
        }
        if win.cursor_col == 0 && win.cursor_row == 0 {
            return;
        }
        let pos = self.char_offset_at(win.cursor_row, win.cursor_col);
        if pos == 0 {
            return;
        }
        let ch = self.rope.char(pos - 1);
        let prev_line_len = if ch == '\n' {
            self.line_len(win.cursor_row - 1)
        } else {
            0
        };
        self.rope.remove(pos - 1..pos);
        self.push_undo(EditAction::DeleteChar { pos: pos - 1, ch });
        self.redo_stack.clear();
        if ch == '\n' {
            win.cursor_row -= 1;
            win.cursor_col = prev_line_len;
        } else {
            win.cursor_col -= 1;
        }
        self.changed_lines.insert(win.cursor_row);
        self.modified = true;
        self.bump_generation();
    }

    pub fn delete_char_forward(&mut self, win: &mut Window) {
        if self.read_only {
            return;
        }
        let pos = self.char_offset_at(win.cursor_row, win.cursor_col);
        if pos >= self.rope.len_chars() {
            return;
        }
        let ch = self.rope.char(pos);
        self.rope.remove(pos..pos + 1);
        self.push_undo(EditAction::DeleteChar { pos, ch });
        self.redo_stack.clear();
        self.changed_lines.insert(win.cursor_row);
        self.modified = true;
        self.bump_generation();
        win.clamp_cursor(self);
    }

    /// Delete the current line. Returns the deleted text (for yank register).
    pub fn delete_line(&mut self, win: &mut Window) -> String {
        if self.read_only {
            return String::new();
        }
        let line_count = self.line_count();
        if line_count == 0 || self.rope.len_chars() == 0 {
            return String::new();
        }
        let line_start = self.rope.line_to_char(win.cursor_row);
        let line = self.rope.line(win.cursor_row);
        let line_chars = line.len_chars();
        if line_chars == 0 {
            return String::new();
        }
        let text: String = self.rope.slice(line_start..line_start + line_chars).into();
        self.rope.remove(line_start..line_start + line_chars);
        self.push_undo(EditAction::DeleteRange {
            pos: line_start,
            text: text.clone(),
        });
        self.redo_stack.clear();
        self.changed_lines.insert(win.cursor_row);
        self.modified = true;
        self.bump_generation();
        win.clamp_cursor(self);
        text
    }

    /// Delete backward to the start of the previous whitespace-delimited token
    /// (readline/bash C-w behaviour). Does NOT cross line boundaries.
    pub fn delete_word_backward(&mut self, win: &mut Window) {
        if self.read_only {
            return;
        }
        let cursor = self.char_offset_at(win.cursor_row, win.cursor_col);
        let line_start = self.rope.line_to_char(win.cursor_row);
        if cursor <= line_start {
            return;
        }
        // Walk back over trailing whitespace, then over the word.
        let mut pos = cursor;
        while pos > line_start && self.rope.char(pos - 1).is_whitespace() {
            pos -= 1;
        }
        while pos > line_start && !self.rope.char(pos - 1).is_whitespace() {
            pos -= 1;
        }
        if pos == cursor {
            return;
        }
        let deleted: String = self.rope.slice(pos..cursor).into();
        self.rope.remove(pos..cursor);
        self.push_undo(EditAction::DeleteRange { pos, text: deleted });
        self.redo_stack.clear();
        self.modified = true;
        self.bump_generation();
        win.cursor_col = pos - line_start;
    }

    /// Delete from the cursor to the beginning of the current line (C-u).
    pub fn delete_to_line_start(&mut self, win: &mut Window) {
        if self.read_only {
            return;
        }
        let cursor = self.char_offset_at(win.cursor_row, win.cursor_col);
        let line_start = self.rope.line_to_char(win.cursor_row);
        if cursor <= line_start {
            return;
        }
        let deleted: String = self.rope.slice(line_start..cursor).into();
        self.rope.remove(line_start..cursor);
        self.push_undo(EditAction::DeleteRange {
            pos: line_start,
            text: deleted,
        });
        self.redo_stack.clear();
        self.modified = true;
        self.bump_generation();
        win.cursor_col = 0;
    }

    /// Delete from the cursor to the end of the current line (C-k / kill-line).
    /// Deletes the newline itself only if the line is otherwise empty.
    pub fn delete_to_line_end(&mut self, win: &mut Window) {
        if self.read_only {
            return;
        }
        let cursor = self.char_offset_at(win.cursor_row, win.cursor_col);
        let rope = &self.rope;
        let line_end = {
            let line_start = rope.line_to_char(win.cursor_row);
            let line = rope.line(win.cursor_row);
            let raw_end = line_start + line.len_chars();
            // If the line ends with '\n', stop before it (don't kill the newline
            // unless the cursor is already AT the newline).
            if raw_end > line_start && raw_end <= rope.len_chars() && rope.char(raw_end - 1) == '\n'
            {
                if cursor == raw_end - 1 {
                    // Cursor on the newline itself — kill it.
                    raw_end
                } else {
                    raw_end - 1
                }
            } else {
                raw_end
            }
        };
        if cursor >= line_end {
            return;
        }
        let deleted: String = self.rope.slice(cursor..line_end).into();
        self.rope.remove(cursor..line_end);
        self.push_undo(EditAction::DeleteRange {
            pos: cursor,
            text: deleted,
        });
        self.redo_stack.clear();
        self.modified = true;
        self.bump_generation();
        win.clamp_cursor(self);
    }

    /// Insert text at an arbitrary character offset. Used by the AI agent.
    pub fn insert_text_at(&mut self, char_offset: usize, text: &str) {
        if self.read_only {
            return;
        }
        let offset = char_offset.min(self.rope.len_chars());
        let start_line = self.rope.char_to_line(offset);
        self.rope.insert(offset, text);
        let end_line = self
            .rope
            .char_to_line((offset + text.len()).min(self.rope.len_chars()));
        for line in start_line..=end_line {
            self.changed_lines.insert(line);
        }
        self.push_undo(EditAction::InsertRange {
            pos: offset,
            text: text.to_string(),
        });
        self.redo_stack.clear();
        self.modified = true;
        self.bump_generation();
    }

    /// Delete a character range [start, end). Used by the AI agent.
    pub fn delete_range(&mut self, start: usize, end: usize) {
        if self.read_only {
            return;
        }
        let start = start.min(self.rope.len_chars());
        let end = end.min(self.rope.len_chars());
        if start >= end {
            return;
        }
        let del_line = self.rope.char_to_line(start);
        let text: String = self.rope.slice(start..end).into();
        self.rope.remove(start..end);
        self.changed_lines.insert(del_line);
        self.push_undo(EditAction::DeleteRange { pos: start, text });
        self.redo_stack.clear();
        self.modified = true;
        self.bump_generation();
    }

    pub fn open_line_below(&mut self, win: &mut Window) {
        if self.read_only {
            return;
        }
        let line_start = self.rope.line_to_char(win.cursor_row);
        let line = self.rope.line(win.cursor_row);
        let line_chars = line.len_chars();

        let insert_pos = line_start + line_chars;
        self.rope.insert_char(insert_pos, '\n');
        self.push_undo(EditAction::InsertChar {
            pos: insert_pos,
            ch: '\n',
        });
        self.redo_stack.clear();
        win.cursor_row += 1;
        win.cursor_col = 0;
        self.modified = true;
        self.bump_generation();
    }

    pub fn open_line_above(&mut self, win: &mut Window) {
        if self.read_only {
            return;
        }
        let line_start = self.rope.line_to_char(win.cursor_row);
        self.rope.insert_char(line_start, '\n');
        self.push_undo(EditAction::InsertChar {
            pos: line_start,
            ch: '\n',
        });
        self.redo_stack.clear();
        win.cursor_col = 0;
        self.modified = true;
        self.bump_generation();
    }

    // --- Undo / Redo ---

    /// Apply a single undo action (reverse the edit) without touching the stacks.
    fn apply_undo_action(rope: &mut Rope, win: &mut Window, action: &EditAction) {
        match action {
            EditAction::InsertChar { pos, .. } => {
                rope.remove(*pos..*pos + 1);
                Self::set_cursor_from_char_pos(rope, win, *pos);
            }
            EditAction::DeleteChar { pos, ch } => {
                rope.insert_char(*pos, *ch);
                Self::set_cursor_from_char_pos(rope, win, *pos + 1);
            }
            EditAction::InsertRange { pos, text } => {
                rope.remove(*pos..*pos + text.chars().count());
                Self::set_cursor_from_char_pos(rope, win, *pos);
            }
            EditAction::DeleteRange { pos, text } => {
                rope.insert(*pos, text);
                Self::set_cursor_from_char_pos(rope, win, *pos);
            }
            EditAction::Group(actions) => {
                // Undo in reverse order.
                for a in actions.iter().rev() {
                    Self::apply_undo_action(rope, win, a);
                }
            }
        }
    }

    /// Apply a single redo action (re-apply the edit) without touching the stacks.
    fn apply_redo_action(rope: &mut Rope, win: &mut Window, action: &EditAction) {
        match action {
            EditAction::InsertChar { pos, ch } => {
                rope.insert_char(*pos, *ch);
                Self::set_cursor_from_char_pos(rope, win, *pos + 1);
            }
            EditAction::DeleteChar { pos, .. } => {
                rope.remove(*pos..*pos + 1);
                Self::set_cursor_from_char_pos(rope, win, *pos);
            }
            EditAction::InsertRange { pos, text } => {
                rope.insert(*pos, text);
                Self::set_cursor_from_char_pos(rope, win, *pos + text.chars().count());
            }
            EditAction::DeleteRange { pos, text } => {
                let end = *pos + text.chars().count();
                rope.remove(*pos..end);
                Self::set_cursor_from_char_pos(rope, win, *pos);
            }
            EditAction::Group(actions) => {
                // Redo in forward order.
                for a in actions.iter() {
                    Self::apply_redo_action(rope, win, a);
                }
            }
        }
    }

    pub fn undo(&mut self, win: &mut Window) {
        let action = match self.undo_stack.pop() {
            Some(a) => a,
            None => return,
        };
        Self::apply_undo_action(&mut self.rope, win, &action);
        self.redo_stack.push(action);
        self.modified = true;
        self.bump_generation();
        win.clamp_cursor(self);
    }

    pub fn redo(&mut self, win: &mut Window) {
        let action = match self.redo_stack.pop() {
            Some(a) => a,
            None => return,
        };
        Self::apply_redo_action(&mut self.rope, win, &action);
        self.push_undo(action);
        self.modified = true;
        self.bump_generation();
        win.clamp_cursor(self);
    }

    /// Set cursor row/col from a char offset in the rope.
    fn set_cursor_from_char_pos(rope: &Rope, win: &mut Window, pos: usize) {
        let pos = pos.min(rope.len_chars());
        win.cursor_row = rope.char_to_line(pos);
        let line_start = rope.line_to_char(win.cursor_row);
        win.cursor_col = pos - line_start;
    }

    /// Rebuild the buffer's rope from the flattened conversation text.
    /// This allows standard motions and visual mode to work on the AI history.
    pub fn sync_conversation_rope(&mut self) {
        if let Some(conv) = self.view.conversation() {
            let flat = conv.flat_text();
            self.rope = Rope::from_str(&flat);
        }
    }

    // --- BufferView accessor convenience methods ---

    pub fn conversation(&self) -> Option<&Conversation> {
        self.view.conversation()
    }

    pub fn conversation_mut(&mut self) -> Option<&mut Conversation> {
        self.view.conversation_mut()
    }

    pub fn help_view(&self) -> Option<&HelpView> {
        self.view.help_view()
    }

    pub fn help_view_mut(&mut self) -> Option<&mut HelpView> {
        self.view.help_view_mut()
    }

    pub fn debug_view(&self) -> Option<&DebugView> {
        self.view.debug_view()
    }

    pub fn debug_view_mut(&mut self) -> Option<&mut DebugView> {
        self.view.debug_view_mut()
    }

    pub fn git_status_view(&self) -> Option<&GitStatusView> {
        self.view.git_status()
    }

    pub fn git_status_view_mut(&mut self) -> Option<&mut GitStatusView> {
        self.view.git_status_mut()
    }

    pub fn visual(&self) -> Option<&VisualBuffer> {
        self.view.visual()
    }

    pub fn visual_mut(&mut self) -> Option<&mut VisualBuffer> {
        self.view.visual_mut()
    }

    pub fn file_tree(&self) -> Option<&FileTree> {
        self.view.file_tree()
    }

    pub fn file_tree_mut(&mut self) -> Option<&mut FileTree> {
        self.view.file_tree_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a buffer + window pair for testing.
    fn new_buf_win() -> (Buffer, Window) {
        (Buffer::new(), Window::new(0, 0))
    }

    /// Helper: insert a string into buffer char by char.
    fn insert_str(buf: &mut Buffer, win: &mut Window, s: &str) {
        for ch in s.chars() {
            buf.insert_char(win, ch);
        }
    }

    // --- Construction ---

    #[test]
    fn new_buffer_is_empty() {
        let (buf, _win) = new_buf_win();
        assert_eq!(buf.text(), "");
        assert!(!buf.modified);
        assert_eq!(buf.name, "[scratch]");
    }

    #[test]
    fn from_file_and_save_round_trip() {
        let dir = std::env::temp_dir().join("mae_test_round_trip");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        fs::write(&path, "hello\nworld\n").unwrap();

        let mut buf = Buffer::from_file(&path).unwrap();
        let mut win = Window::new(0, 0);
        assert_eq!(buf.text(), "hello\nworld\n");
        assert_eq!(buf.name, "test.txt");
        assert!(!buf.modified);

        buf.insert_char(&mut win, '!');
        assert!(buf.modified);
        buf.save().unwrap();
        assert!(!buf.modified);

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "!hello\nworld\n");

        let _ = fs::remove_dir_all(&dir);
    }

    // --- Insert ---

    #[test]
    fn insert_char_at_start() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        assert_eq!(buf.text(), "a");
        assert_eq!(win.cursor_col, 1);
        assert!(buf.modified);
    }

    #[test]
    fn insert_multiple_chars() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'h');
        buf.insert_char(&mut win, 'i');
        assert_eq!(buf.text(), "hi");
        assert_eq!(win.cursor_col, 2);
    }

    #[test]
    fn insert_newline_splits_line() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        buf.insert_char(&mut win, '\n');
        buf.insert_char(&mut win, 'b');
        assert_eq!(buf.text(), "a\nb");
        assert_eq!(win.cursor_row, 1);
        assert_eq!(win.cursor_col, 1);
    }

    // --- Delete backward ---

    #[test]
    fn delete_backward_at_start_is_noop() {
        let (mut buf, mut win) = new_buf_win();
        buf.delete_char_backward(&mut win);
        assert_eq!(buf.text(), "");
        assert!(!buf.modified);
    }

    #[test]
    fn delete_backward_mid_line() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        buf.insert_char(&mut win, 'b');
        buf.delete_char_backward(&mut win);
        assert_eq!(buf.text(), "a");
        assert_eq!(win.cursor_col, 1);
    }

    #[test]
    fn delete_backward_at_line_start_joins_lines() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "a\nb");
        // move to start of line 1
        win.cursor_col = 0;
        buf.delete_char_backward(&mut win);
        assert_eq!(buf.text(), "ab");
        assert_eq!(win.cursor_row, 0);
        assert_eq!(win.cursor_col, 1);
    }

    // --- Delete forward ---

    #[test]
    fn delete_forward_at_end_is_noop() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        buf.delete_char_forward(&mut win);
        assert_eq!(buf.text(), "a");
    }

    #[test]
    fn delete_forward_mid_line() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "ab");
        win.cursor_col = 0;
        buf.delete_char_forward(&mut win);
        assert_eq!(buf.text(), "b");
        assert_eq!(win.cursor_col, 0);
    }

    // --- Change markers for deletions ---

    #[test]
    fn delete_char_forward_marks_line_changed() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "abc");
        buf.changed_lines.clear();
        win.cursor_col = 1;
        buf.delete_char_forward(&mut win);
        assert!(
            buf.changed_lines.contains(&0),
            "delete_char_forward should mark line as changed"
        );
    }

    #[test]
    fn delete_char_backward_marks_line_changed() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "abc");
        buf.changed_lines.clear();
        win.cursor_col = 2;
        buf.delete_char_backward(&mut win);
        assert!(
            buf.changed_lines.contains(&0),
            "delete_char_backward should mark line as changed"
        );
    }

    // --- Delete line ---

    #[test]
    fn delete_line_single_line() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "ab");
        win.cursor_col = 0;
        buf.delete_line(&mut win);
        assert_eq!(buf.text(), "");
    }

    #[test]
    fn delete_line_middle() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "aaa\nbbb\nccc\n");
        win.cursor_row = 1;
        win.cursor_col = 0;
        buf.delete_line(&mut win);
        assert_eq!(buf.text(), "aaa\nccc\n");
    }

    #[test]
    fn delete_line_last_line() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "aaa\nbbb");
        win.cursor_row = 1;
        win.cursor_col = 0;
        buf.delete_line(&mut win);
        assert_eq!(buf.text(), "aaa\n");
    }

    // --- Movement (now on Window) ---

    #[test]
    fn move_up_at_top_is_noop() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        win.move_up(&buf);
        assert_eq!(win.cursor_row, 0);
    }

    #[test]
    fn move_down_at_bottom_is_noop() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        win.cursor_col = 0;
        win.move_down(&buf);
        assert_eq!(win.cursor_row, 0);
    }

    #[test]
    fn move_up_and_down() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "aaa\nbbb\nccc");
        win.cursor_row = 2;
        win.cursor_col = 0;
        win.move_up(&buf);
        assert_eq!(win.cursor_row, 1);
        win.move_down(&buf);
        assert_eq!(win.cursor_row, 2);
    }

    #[test]
    fn move_down_clamps_col_to_shorter_line() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "long line\nhi");
        win.cursor_row = 0;
        win.cursor_col = 8;
        win.move_down(&buf);
        assert_eq!(win.cursor_row, 1);
        assert_eq!(win.cursor_col, 2);
    }

    #[test]
    fn move_left_at_start_is_noop() {
        let (_buf, mut win) = new_buf_win();
        win.move_left();
        assert_eq!(win.cursor_col, 0);
    }

    #[test]
    fn move_right_at_end_is_noop() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        win.move_right(&buf);
        assert_eq!(win.cursor_col, 1);
    }

    #[test]
    fn move_left_and_right() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "ab");
        win.move_left();
        assert_eq!(win.cursor_col, 1);
        win.move_right(&buf);
        assert_eq!(win.cursor_col, 2);
    }

    #[test]
    fn move_to_line_start_and_end() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "hello");
        win.move_to_line_start();
        assert_eq!(win.cursor_col, 0);
        win.move_to_line_end(&buf);
        assert_eq!(win.cursor_col, 5);
    }

    #[test]
    fn move_to_first_and_last_line() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "aaa\nbbb\nccc");
        win.move_to_first_line(&buf);
        assert_eq!(win.cursor_row, 0);
        win.move_to_last_line(&buf);
        assert_eq!(win.cursor_row, 2);
    }

    // --- Clamp cursor ---

    #[test]
    fn clamp_cursor_after_line_shortening() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "hello\nhi");
        win.cursor_row = 1;
        win.cursor_col = 10;
        win.clamp_cursor(&buf);
        assert_eq!(win.cursor_col, 2);
    }

    #[test]
    fn clamp_cursor_empty_buffer() {
        let (buf, mut win) = new_buf_win();
        win.cursor_row = 5;
        win.cursor_col = 10;
        win.clamp_cursor(&buf);
        assert_eq!(win.cursor_row, 0);
        assert_eq!(win.cursor_col, 0);
    }

    #[test]
    fn clamp_cursor_allows_trailing_newline_position() {
        // Inserting '\n' at end of text should leave cursor on the new empty line.
        // Regression: clamp_cursor used display_line_count() which excluded the
        // trailing phantom line, clamping cursor back to row 0.
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "hello");
        assert_eq!(win.cursor_row, 0);
        buf.insert_char(&mut win, '\n');
        assert_eq!(win.cursor_row, 1);
        assert_eq!(win.cursor_col, 0);
        win.clamp_cursor(&buf);
        assert_eq!(win.cursor_row, 1); // must NOT clamp back to 0
        assert_eq!(win.cursor_col, 0);
    }

    #[test]
    fn clamp_cursor_still_clamps_past_end() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "hello");
        win.cursor_row = 5;
        win.clamp_cursor(&buf);
        assert_eq!(win.cursor_row, 0); // only 1 line
    }

    // --- Scrolling ---

    #[test]
    fn ensure_scroll_cursor_above_viewport() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
        win.scroll_offset = 5;
        win.cursor_row = 2;
        win.ensure_scroll(5);
        assert_eq!(win.scroll_offset, 2);
    }

    #[test]
    fn ensure_scroll_cursor_below_viewport() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
        win.scroll_offset = 0;
        win.cursor_row = 7;
        win.ensure_scroll(5);
        assert_eq!(win.scroll_offset, 3);
    }

    #[test]
    fn ensure_scroll_cursor_within_viewport() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "a\nb\nc\nd\ne");
        win.scroll_offset = 0;
        win.cursor_row = 2;
        win.ensure_scroll(5);
        assert_eq!(win.scroll_offset, 0);
    }

    // --- Open line ---

    #[test]
    fn open_line_below() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "aaa\nbbb");
        win.cursor_row = 0;
        win.cursor_col = 0;
        buf.open_line_below(&mut win);
        assert_eq!(buf.text(), "aaa\n\nbbb");
        assert_eq!(win.cursor_row, 1);
        assert_eq!(win.cursor_col, 0);
    }

    #[test]
    fn open_line_above() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "aaa\nbbb");
        win.cursor_row = 1;
        win.cursor_col = 0;
        buf.open_line_above(&mut win);
        assert_eq!(win.cursor_row, 1);
        assert_eq!(win.cursor_col, 0);
        assert!(buf.text().contains("aaa\n\nbbb"));
    }

    // --- Undo / Redo ---

    #[test]
    fn undo_insert_char() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        buf.insert_char(&mut win, 'b');
        assert_eq!(buf.text(), "ab");
        buf.undo(&mut win);
        assert_eq!(buf.text(), "a");
        buf.undo(&mut win);
        assert_eq!(buf.text(), "");
    }

    #[test]
    fn undo_delete_char() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "ab");
        buf.delete_char_backward(&mut win);
        assert_eq!(buf.text(), "a");
        buf.undo(&mut win);
        assert_eq!(buf.text(), "ab");
    }

    #[test]
    fn undo_delete_line() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "aaa\nbbb\n");
        win.cursor_row = 1;
        win.cursor_col = 0;
        buf.delete_line(&mut win);
        assert_eq!(buf.text(), "aaa\n");
        buf.undo(&mut win);
        assert_eq!(buf.text(), "aaa\nbbb\n");
    }

    #[test]
    fn redo_after_undo() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        buf.undo(&mut win);
        assert_eq!(buf.text(), "");
        buf.redo(&mut win);
        assert_eq!(buf.text(), "a");
    }

    #[test]
    fn redo_cleared_on_new_edit() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        buf.undo(&mut win);
        buf.insert_char(&mut win, 'b');
        buf.redo(&mut win);
        assert_eq!(buf.text(), "b");
    }

    #[test]
    fn undo_empty_stack_is_noop() {
        let (mut buf, mut win) = new_buf_win();
        buf.undo(&mut win);
        assert_eq!(buf.text(), "");
    }

    #[test]
    fn redo_empty_stack_is_noop() {
        let (mut buf, mut win) = new_buf_win();
        buf.redo(&mut win);
        assert_eq!(buf.text(), "");
    }

    // --- Range operations (AI agent) ---

    #[test]
    fn insert_text_at_beginning() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "world");
        buf.insert_text_at(0, "hello ");
        assert_eq!(buf.text(), "hello world");
    }

    #[test]
    fn insert_text_at_end() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "hello");
        buf.insert_text_at(5, " world");
        assert_eq!(buf.text(), "hello world");
    }

    #[test]
    fn insert_text_at_undo() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "ab");
        buf.insert_text_at(1, "XY");
        assert_eq!(buf.text(), "aXYb");
        buf.undo(&mut win);
        assert_eq!(buf.text(), "ab");
        buf.redo(&mut win);
        assert_eq!(buf.text(), "aXYb");
    }

    #[test]
    fn delete_range_middle() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "hello world");
        buf.delete_range(5, 11);
        assert_eq!(buf.text(), "hello");
    }

    #[test]
    fn delete_range_undo() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "abcdef");
        buf.delete_range(2, 4);
        assert_eq!(buf.text(), "abef");
        buf.undo(&mut win);
        assert_eq!(buf.text(), "abcdef");
        buf.redo(&mut win);
        assert_eq!(buf.text(), "abef");
    }

    #[test]
    fn delete_range_empty_is_noop() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "abc");
        buf.delete_range(2, 2);
        assert_eq!(buf.text(), "abc");
    }

    // --- Line metrics ---

    #[test]
    fn line_len_excludes_newline() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "hello\nworld");
        assert_eq!(buf.line_len(0), 5);
        assert_eq!(buf.line_len(1), 5);
    }

    #[test]
    fn line_count_with_trailing_newline() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "a\nb\n");
        assert_eq!(buf.line_count(), 3);
    }

    // --- BufferKind ---

    #[test]
    fn default_kind_is_text() {
        let buf = Buffer::new();
        assert_eq!(buf.kind, BufferKind::Text);
        assert!(buf.conversation().is_none());
    }

    #[test]
    fn conversation_buffer_creation() {
        let buf = Buffer::new_conversation("[conversation]");
        assert_eq!(buf.kind, BufferKind::Conversation);
        assert!(buf.conversation().is_some());
        assert_eq!(buf.name, "[conversation]");
    }

    #[test]
    fn buffer_local_word_wrap_defaults() {
        // Conversation, Help, Messages buffers default to word_wrap=true.
        let conv = Buffer::new_conversation("conv");
        assert_eq!(conv.local_options.word_wrap, Some(true));

        let help = Buffer::new_help("test");
        assert_eq!(help.local_options.word_wrap, Some(true));

        let msgs = Buffer::new_messages();
        assert_eq!(msgs.local_options.word_wrap, Some(true));

        // Normal text buffers have no override (use global default).
        let text = Buffer::new();
        assert_eq!(text.local_options.word_wrap, None);
    }

    // --- delete_word_backward (C-w) ---

    #[test]
    fn delete_word_backward_basic() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "hello world");
        let mut win = Window::new(0, 0);
        win.cursor_col = 11; // end of "world"
        buf.delete_word_backward(&mut win);
        assert_eq!(buf.text(), "hello ");
        assert_eq!(win.cursor_col, 6);
    }

    #[test]
    fn delete_word_backward_strips_trailing_whitespace_first() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "foo   ");
        let mut win = Window::new(0, 0);
        win.cursor_col = 6;
        buf.delete_word_backward(&mut win); // removes "foo   "
        assert_eq!(buf.text(), "");
        assert_eq!(win.cursor_col, 0);
    }

    #[test]
    fn delete_word_backward_at_line_start_is_noop() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "hello\n");
        let mut win = Window::new(0, 0);
        win.cursor_row = 1;
        win.cursor_col = 0;
        buf.delete_word_backward(&mut win);
        assert_eq!(buf.text(), "hello\n"); // newline not crossed
    }

    // --- delete_to_line_start (C-u) ---

    #[test]
    fn delete_to_line_start_basic() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "hello world");
        let mut win = Window::new(0, 0);
        win.cursor_col = 5;
        buf.delete_to_line_start(&mut win);
        assert_eq!(buf.text(), " world");
        assert_eq!(win.cursor_col, 0);
    }

    #[test]
    fn delete_to_line_start_at_col0_is_noop() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "hello");
        let mut win = Window::new(0, 0);
        win.cursor_col = 0;
        buf.delete_to_line_start(&mut win);
        assert_eq!(buf.text(), "hello");
    }

    // --- delete_to_line_end (C-k) ---

    #[test]
    fn delete_to_line_end_basic() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "hello world\n");
        let mut win = Window::new(0, 0);
        win.cursor_col = 5;
        buf.delete_to_line_end(&mut win);
        assert_eq!(buf.text(), "hello\n");
    }

    #[test]
    fn delete_to_line_end_on_newline_kills_it() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "hello\nworld\n");
        let mut win = Window::new(0, 0);
        win.cursor_col = 5; // cursor on the '\n' of "hello\n"
        buf.delete_to_line_end(&mut win);
        // kills the newline, joining with next line
        assert_eq!(buf.text(), "helloworld\n");
    }

    #[test]
    fn delete_to_line_end_at_end_is_noop() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "hello\n");
        let mut win = Window::new(0, 0);
        win.cursor_col = 5; // already at '\n'
        buf.delete_to_line_end(&mut win);
        // '\n' is killed when cursor is on it
        assert_eq!(buf.text(), "hello");
    }

    // --- File mtime / auto-reload ---

    #[test]
    fn test_buffer_mtime_set_on_load() {
        let dir = std::env::temp_dir().join("mae_test_mtime_load");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("mtime.txt");
        fs::write(&path, "hello").unwrap();

        let buf = Buffer::from_file(&path).unwrap();
        assert!(buf.file_mtime.is_some());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_buffer_mtime_updated_on_save() {
        let dir = std::env::temp_dir().join("mae_test_mtime_save");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("mtime_save.txt");
        fs::write(&path, "hello").unwrap();

        let mut buf = Buffer::from_file(&path).unwrap();
        let mtime1 = buf.file_mtime;

        // Small delay to ensure mtime changes
        std::thread::sleep(std::time::Duration::from_millis(50));
        buf.insert_text_at(0, "new ");
        buf.save().unwrap();
        let mtime2 = buf.file_mtime;
        assert!(mtime2 >= mtime1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_check_disk_changed_detects_external_edit() {
        let dir = std::env::temp_dir().join("mae_test_disk_changed");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("detect.txt");
        fs::write(&path, "original").unwrap();

        let buf = Buffer::from_file(&path).unwrap();
        assert!(!buf.check_disk_changed());

        // Simulate external edit
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&path, "modified externally").unwrap();
        assert!(buf.check_disk_changed());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_auto_reload_clean_buffer() {
        let dir = std::env::temp_dir().join("mae_test_auto_reload");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("reload.txt");
        fs::write(&path, "original").unwrap();

        let mut buf = Buffer::from_file(&path).unwrap();
        assert!(!buf.modified);

        // External edit
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&path, "updated content").unwrap();

        // Reload should succeed on clean buffer
        assert!(buf.check_disk_changed());
        buf.reload_from_disk().unwrap();
        assert_eq!(buf.text(), "updated content");
        assert!(!buf.modified);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_no_reload_dirty_buffer() {
        let dir = std::env::temp_dir().join("mae_test_no_reload_dirty");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("dirty.txt");
        fs::write(&path, "original").unwrap();

        let mut buf = Buffer::from_file(&path).unwrap();
        buf.insert_text_at(0, "local edit ");
        assert!(buf.modified);

        // External edit
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&path, "external edit").unwrap();

        // check_disk_changed should detect the change
        assert!(buf.check_disk_changed());
        // But we should NOT reload — buffer has local changes.
        // (The caller decides this, not the buffer itself.)
        assert!(buf.modified);
        assert!(buf.text().contains("local edit"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_buffer_is_text_kind() {
        let dir = std::env::temp_dir().join("mae_test_kind");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        fs::write(&path, "hello").unwrap();

        let buf = Buffer::from_file(&path).unwrap();
        assert_eq!(buf.kind, BufferKind::Text);
        assert!(buf.conversation().is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_is_atomic_no_temp_file_left() {
        let dir = std::env::temp_dir().join("mae_test_atomic_save");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("atomic.txt");
        fs::write(&path, "original").unwrap();

        let mut buf = Buffer::from_file(&path).unwrap();
        buf.insert_text_at(0, "new ");
        buf.save().unwrap();

        // File should contain the new content.
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "new original");

        // No temp file should remain.
        let temps: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".mae-save-"))
            .collect();
        assert!(temps.is_empty(), "temp file left behind: {:?}", temps);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn buffer_from_file_detects_project_root() {
        // Create a temp dir with a Cargo.toml marker
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        let sub = dir.path().join("src");
        std::fs::create_dir_all(&sub).unwrap();
        let file = sub.join("main.rs");
        std::fs::write(&file, "fn main() {}").unwrap();

        let buf = Buffer::from_file(&file).unwrap();
        assert_eq!(buf.project_root, Some(dir.path().to_path_buf()));
    }

    // --- Change markers ---

    #[test]
    fn buffer_tracks_changed_lines_on_insert() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "hello\n");
        assert!(buf.changed_lines.contains(&0));
    }

    #[test]
    fn buffer_tracks_changed_lines_on_delete() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "hello world");
        buf.changed_lines.clear();
        buf.delete_range(0, 5);
        assert!(buf.changed_lines.contains(&0));
    }

    #[test]
    fn buffer_changed_lines_persist_across_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("change_test.txt");
        std::fs::write(&path, "original").unwrap();
        let mut buf = Buffer::from_file(&path).unwrap();
        let mut win = Window::new(0, 0);
        buf.insert_char(&mut win, '!');
        assert!(!buf.changed_lines.is_empty());
        buf.save().unwrap();
        // changed_lines persist across saves — cleared on revert/reload
        assert!(!buf.changed_lines.is_empty());
    }

    #[test]
    fn buffer_changed_lines_clear_on_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reload_test.txt");
        std::fs::write(&path, "original").unwrap();
        let mut buf = Buffer::from_file(&path).unwrap();
        let mut win = Window::new(0, 0);
        buf.insert_char(&mut win, '!');
        assert!(!buf.changed_lines.is_empty());
        buf.reload_from_disk().unwrap();
        assert!(buf.changed_lines.is_empty());
    }

    #[test]
    fn is_line_folded_basic() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "line0\nline1\nline2\nline3\nline4\n");
        buf.folded_ranges.push((1, 4));
        assert!(!buf.is_line_folded(0));
        assert!(!buf.is_line_folded(1)); // fold start is visible
        assert!(buf.is_line_folded(2));
        assert!(buf.is_line_folded(3));
        assert!(!buf.is_line_folded(4)); // fold end is visible
    }

    #[test]
    fn toggle_fold_at_creates_and_removes() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "fn main() {\n    x\n    y\n}\n");
        let ranges = vec![(0, 3)];
        buf.toggle_fold_at(0, &ranges);
        assert_eq!(buf.folded_ranges, vec![(0, 3)]);
        buf.toggle_fold_at(0, &ranges);
        assert!(buf.folded_ranges.is_empty());
    }

    #[test]
    fn fold_all_and_unfold_all() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "fn a() {\n}\nfn b() {\n}\n");
        let ranges = vec![(0, 1), (2, 3)];
        buf.fold_all(&ranges);
        assert_eq!(buf.folded_ranges.len(), 2);
        buf.unfold_all();
        assert!(buf.folded_ranges.is_empty());
    }

    #[test]
    fn toggle_fold_innermost_range() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "fn a() {\n  if x {\n    y\n  }\n}\n");
        // Outer: (0, 4), inner: (1, 3)
        let ranges = vec![(0, 4), (1, 3)];
        buf.toggle_fold_at(2, &ranges);
        // Should fold innermost range (1, 3) since cursor line 2 is in both
        assert_eq!(buf.folded_ranges, vec![(1, 3)]);
    }

    #[test]
    fn apply_defaults_fills_none_preserves_some() {
        let mut opts = BufferLocalOptions {
            heading_scale: Some(false),
            ..Default::default()
        };
        let defaults = BufferLocalOptions {
            heading_scale: Some(true),
            render_markup: Some(true),
            link_descriptive: Some(true),
            ..Default::default()
        };
        opts.apply_defaults(&defaults);
        // Existing Some(false) is preserved, not overwritten
        assert_eq!(opts.heading_scale, Some(false));
        // None fields filled from defaults
        assert_eq!(opts.render_markup, Some(true));
        assert_eq!(opts.link_descriptive, Some(true));
        // Fields None in both stay None
        assert_eq!(opts.word_wrap, None);
    }
}
