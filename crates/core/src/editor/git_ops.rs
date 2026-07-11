//! Git integration commands (SPC g group).
//!
//! Shell-out stubs that capture git output into read-only scratch buffers.
//! Full integration deferred to Phase 6 (Embedded Shell + Magit Parity).

use crate::buffer::Buffer;
use tracing::{error, info};

use super::Editor;

/// Per-line blame annotation from `git blame`.
#[derive(Debug, Clone)]
pub struct BlameEntry {
    /// Short commit hash (8 chars).
    pub commit_hash: String,
    /// Author name.
    pub author: String,
    /// Unix timestamp.
    pub timestamp: i64,
    /// First line of commit message.
    pub summary: String,
    /// 0-indexed line in buffer.
    pub final_line: usize,
}

/// Blame overlay for the active buffer.
#[derive(Debug, Clone)]
pub struct BlameOverlay {
    /// Which buffer this blame is for.
    pub buffer_idx: usize,
    /// Blame entries, one per line.
    pub entries: Vec<BlameEntry>,
}

/// Pending async git diff: spawned on a background thread, polled on idle ticks.
pub struct PendingGitDiff {
    pub file_path: std::path::PathBuf,
    pub receiver: std::sync::mpsc::Receiver<
        std::collections::HashMap<usize, crate::render_common::gutter::GitLineStatus>,
    >,
}

impl Editor {
    /// Run a git command and put output in a read-only scratch buffer.
    fn git_command_to_buffer(&mut self, args: &[&str], buf_name: &str) {
        let root = self.git_root();

        match std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
        {
            Ok(output) => {
                let text = if output.status.success() {
                    String::from_utf8_lossy(&output.stdout).to_string()
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    format!("git error: {}", stderr)
                };
                // Find or create the buffer
                let idx = if let Some(i) = self.find_buffer_by_name(buf_name) {
                    self.buffers[i] = Buffer::new();
                    self.buffers[i].name = buf_name.to_string();
                    i
                } else {
                    let mut buf = Buffer::new();
                    buf.name = buf_name.to_string();
                    self.buffers.push(buf);
                    self.buffers.len() - 1
                };
                // Insert content
                let win_temp = &mut crate::window::Window::new(0, idx);
                for ch in text.chars() {
                    self.buffers[idx].insert_char(win_temp, ch);
                }
                self.buffers[idx].modified = false;
                // Switch to it
                let prev = self.active_buffer_idx();
                self.vi.alternate_buffer_idx = Some(prev);
                self.display_buffer(idx);
            }
            Err(e) => {
                self.set_status(format!("git: {}", e));
            }
        }
    }

    /// Refresh the cached git branch by running `git rev-parse --abbrev-ref HEAD`.
    pub fn refresh_git_branch(&mut self) {
        let dir = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(|| std::env::current_dir().ok());
        self.git_branch = dir.and_then(|d| {
            std::process::Command::new("git")
                .args(["rev-parse", "--abbrev-ref", "HEAD"])
                .current_dir(&d)
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                    } else {
                        None
                    }
                })
        });
    }

    /// Populate `buffer.git_branch` if missing and the buffer has a project root.
    /// Cheap no-op if already populated. Called on buffer focus change.
    pub fn ensure_buffer_git_branch(&mut self, buf_idx: usize) {
        if buf_idx >= self.buffers.len() || self.buffers[buf_idx].git_branch.is_some() {
            return;
        }
        let dir = self.buffers[buf_idx]
            .project_root
            .clone()
            .or_else(|| self.project.as_ref().map(|p| p.root.clone()));
        self.buffers[buf_idx].git_branch = dir.and_then(|d| {
            std::process::Command::new("git")
                .args(["rev-parse", "--abbrev-ref", "HEAD"])
                .current_dir(&d)
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                    } else {
                        None
                    }
                })
        });
    }

    pub fn git_status(&mut self) {
        use crate::git_status::*;

        let root = self
            .active_project_root()
            .map(|p| p.to_path_buf())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();

        info!(path = %root.display(), "refreshing git status");
        let (ok, stdout, stderr) =
            self.run_git_porcelain(&["status", "--porcelain=v2", "--branch"]);
        if !ok {
            error!(error = %stderr, "git status failed");
            self.set_status(format!("git status failed: {}", stderr));
            return;
        }

        // Preserve collapsed state from previous view
        let prev_collapsed = self
            .find_buffer_by_name("*git-status*")
            .and_then(|i| {
                self.buffers[i]
                    .git_status_view()
                    .map(|v| v.collapsed.clone())
            })
            .unwrap_or_default();

        let mut view = GitStatusView::new(root.clone());
        view.collapsed = prev_collapsed;
        let mut text = String::new();

        let mut branch = "unknown".to_string();
        let mut staged: Vec<(String, char)> = Vec::new();
        let mut unstaged: Vec<(String, char)> = Vec::new();
        let mut untracked = Vec::new();

        for line in stdout.lines() {
            if let Some(stripped) = line.strip_prefix("# branch.head ") {
                branch = stripped.to_string();
            } else if line.starts_with("1 ") || line.starts_with("2 ") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 9 {
                    let staging = parts[1];
                    let path = parts[parts.len() - 1].to_string();
                    let s_char = staging.chars().next().unwrap_or('.');
                    let u_char = staging.chars().nth(1).unwrap_or('.');
                    if s_char != '.' {
                        staged.push((path.clone(), s_char));
                    }
                    if u_char != '.' {
                        unstaged.push((path, u_char));
                    }
                }
            } else if let Some(stripped) = line.strip_prefix("? ") {
                untracked.push(stripped.to_string());
            }
        }

        // Helper: push a line into view + text
        fn push_line(view: &mut GitStatusView, text: &mut String, line: GitStatusLine) {
            let line_text = line.text.clone();
            crate::foldable_view::push_line(text, &line_text, &mut view.lines, line);
        }

        // Header
        push_line(
            &mut view,
            &mut text,
            GitStatusLine {
                text: format!("Head:     {}", branch),
                section: None,
                file_path: None,
                hunk_index: None,
                hunk_header: None,
                kind: GitLineKind::Header,
            },
        );

        // Blank separator
        push_line(&mut view, &mut text, GitStatusLine::blank());

        // Helper closure to push a section with multi-level collapse
        let push_section = |view: &mut GitStatusView,
                            text: &mut String,
                            section: GitSection,
                            heading: &str,
                            files: &[(String, char)]| {
            if files.is_empty() {
                return;
            }
            let section_key = CollapseKey::Section(section);
            let section_collapsed = view.is_collapsed(&section_key);

            // Section header (with collapse indicator)
            let indicator = if section_collapsed { "▸" } else { "▾" };
            let header_text = format!("{} {}", indicator, heading);
            push_line(
                view,
                text,
                GitStatusLine {
                    text: header_text,
                    section: Some(section),
                    file_path: None,
                    hunk_index: None,
                    hunk_header: None,
                    kind: GitLineKind::SectionHeader(section),
                },
            );

            if section_collapsed {
                push_line(view, text, GitStatusLine::blank());
                return;
            }

            // File entries
            for (p, status_char) in files {
                let file_expanded = view.is_file_expanded(p, &section);
                let file_indicator = if file_expanded { "▾" } else { "▸" };
                let file_text = format!("  {} {} {}", file_indicator, status_char, p);
                push_line(
                    view,
                    text,
                    GitStatusLine {
                        text: file_text,
                        section: Some(section),
                        file_path: Some(p.clone()),
                        hunk_index: None,
                        hunk_header: None,
                        kind: GitLineKind::File {
                            section,
                            status_char: *status_char,
                        },
                    },
                );

                // Inline diff for expanded files
                if file_expanded {
                    let diff_args = if section == GitSection::Staged {
                        vec!["diff", "--cached", "--", p.as_str()]
                    } else {
                        vec!["diff", "--", p.as_str()]
                    };
                    let diff_output = match std::process::Command::new("git")
                        .args(&diff_args)
                        .current_dir(&view.repo_root)
                        .output()
                    {
                        Ok(o) if o.status.success() => {
                            String::from_utf8_lossy(&o.stdout).to_string()
                        }
                        _ => String::new(),
                    };
                    let mut hunk_idx: usize = 0;
                    let mut current_hunk_header: Option<String> = None;
                    for diff_line in diff_output.lines() {
                        if diff_line.starts_with("diff ")
                            || diff_line.starts_with("index ")
                            || diff_line.starts_with("--- ")
                            || diff_line.starts_with("+++ ")
                        {
                            continue; // Skip diff metadata
                        }
                        let (diff_kind, is_hunk_header) = if diff_line.starts_with("@@") {
                            current_hunk_header = Some(diff_line.to_string());
                            (GitLineKind::DiffHunk, true)
                        } else if diff_line.starts_with('+') {
                            (GitLineKind::DiffLine(DiffLineType::Added), false)
                        } else if diff_line.starts_with('-') {
                            (GitLineKind::DiffLine(DiffLineType::Removed), false)
                        } else {
                            (GitLineKind::DiffLine(DiffLineType::Context), false)
                        };

                        // Check hunk-level collapse
                        let hunk_key = CollapseKey::Hunk {
                            path: p.clone(),
                            section,
                            index: hunk_idx,
                        };
                        let hunk_collapsed = view.is_collapsed(&hunk_key);

                        // Hunk headers always visible; diff lines only when not collapsed
                        if is_hunk_header || !hunk_collapsed {
                            let display_line = format!("    {}", diff_line);
                            push_line(
                                view,
                                text,
                                GitStatusLine {
                                    text: display_line,
                                    section: Some(section),
                                    file_path: Some(p.clone()),
                                    hunk_index: Some(hunk_idx),
                                    hunk_header: current_hunk_header.clone(),
                                    kind: diff_kind,
                                },
                            );
                        }

                        if is_hunk_header {
                            hunk_idx += 1;
                        }
                    }
                }
            }

            push_line(view, text, GitStatusLine::blank());
        };

        // Untracked files (use '?' as status char)
        let untracked_files: Vec<(String, char)> =
            untracked.iter().map(|p| (p.clone(), '?')).collect();
        push_section(
            &mut view,
            &mut text,
            GitSection::Untracked,
            "Untracked files:",
            &untracked_files,
        );

        // Unstaged changes
        push_section(
            &mut view,
            &mut text,
            GitSection::Unstaged,
            "Unstaged changes:",
            &unstaged,
        );

        // Staged changes
        push_section(
            &mut view,
            &mut text,
            GitSection::Staged,
            "Staged changes:",
            &staged,
        );

        // Stash list
        let (stash_ok, stash_stdout, _) = self.run_git_porcelain(&["stash", "list"]);
        if stash_ok && !stash_stdout.trim().is_empty() {
            let stash_section_key = CollapseKey::Section(GitSection::Stashes);
            let stash_collapsed = view.is_collapsed(&stash_section_key);
            let indicator = if stash_collapsed { "▸" } else { "▾" };
            push_line(
                &mut view,
                &mut text,
                GitStatusLine {
                    text: format!("{} Stashes:", indicator),
                    section: Some(GitSection::Stashes),
                    file_path: None,
                    hunk_index: None,
                    hunk_header: None,
                    kind: GitLineKind::SectionHeader(GitSection::Stashes),
                },
            );

            if !stash_collapsed {
                for stash_line in stash_stdout.lines() {
                    let display_line = format!("  {}", stash_line);
                    push_line(
                        &mut view,
                        &mut text,
                        GitStatusLine {
                            text: display_line,
                            section: Some(GitSection::Stashes),
                            file_path: None,
                            hunk_index: None,
                            hunk_header: None,
                            kind: GitLineKind::File {
                                section: GitSection::Stashes,
                                status_char: 'S',
                            },
                        },
                    );
                }
            }
        }

        // Find or create the buffer
        let buf_name = "*git-status*";
        let idx = if let Some(i) = self.find_buffer_by_name(buf_name) {
            self.buffers[i] = Buffer::new();
            self.buffers[i].name = buf_name.to_string();
            self.buffers[i].kind = crate::buffer::BufferKind::GitStatus;
            i
        } else {
            let mut buf = Buffer::new();
            buf.name = buf_name.to_string();
            buf.kind = crate::buffer::BufferKind::GitStatus;
            self.buffers.push(buf);
            self.buffers.len() - 1
        };

        self.buffers[idx].view = crate::buffer_view::BufferView::GitStatus(Box::new(view));

        // Populate rope BEFORE setting read_only (insert_text_at is a no-op on read-only buffers)
        self.buffers[idx].insert_text_at(0, &text);
        self.buffers[idx].read_only = true;
        self.buffers[idx].modified = false;

        // Switch to it
        let prev = self.active_buffer_idx();
        self.vi.alternate_buffer_idx = Some(prev);
        self.display_buffer(idx);
        self.set_mode(crate::Mode::Normal);

        use crate::buffer_mode::BufferMode;
        if let Some(hint) = self.active_buffer().kind.status_hint() {
            self.set_status(hint.to_string());
        }
    }

    // ── Helpers ─────────────────────────────────────────────────────

    /// Get the GitStatusLine at the cursor, if in a git-status buffer.
    pub fn current_git_line(&self) -> Option<&crate::git_status::GitStatusLine> {
        let win = self.window_mgr.focused_window();
        let idx = self.active_buffer_idx();
        self.buffers[idx]
            .git_status_view()
            .and_then(|v| v.lines.get(win.cursor_row))
    }

    /// Get the file path from the git line at cursor, if any.
    pub fn current_git_file_path(&self) -> Option<String> {
        self.current_git_line()
            .and_then(|line| line.file_path.clone())
    }

    // ── Multi-Level Fold ────────────────────────────────────────────

    /// Toggle fold at cursor: section header → section, file → file diff,
    /// hunk header → hunk body.
    pub fn git_toggle_fold(&mut self) {
        let idx = self.active_buffer_idx();
        let cursor_row = self.window_mgr.focused_window().cursor_row;

        let key = self.buffers[idx].git_status_view().and_then(|v| {
            v.line_at(cursor_row)
                .and_then(crate::git_status::GitStatusView::collapse_key_for_line)
        });

        if let Some(k) = key {
            if let Some(view) = self.buffers[idx].git_status_view_mut() {
                // File keys default to collapsed (true); sections/hunks default to expanded (false).
                if matches!(k, crate::git_status::CollapseKey::File { .. }) {
                    let collapsed = view.collapsed.entry(k).or_insert(true);
                    *collapsed = !*collapsed;
                } else {
                    view.toggle(k);
                }
            }
            // Rebuild and restore cursor position (clamped to new line count).
            self.git_status();
            let line_count = self.buffers[idx]
                .git_status_view()
                .map(|v| v.lines.len())
                .unwrap_or(1);
            self.window_mgr.focused_window_mut().cursor_row =
                cursor_row.min(line_count.saturating_sub(1));
        }
    }

    /// Toggle inline diff expansion for the file at cursor (legacy, now wraps toggle_fold).
    pub fn git_toggle_section(&mut self) {
        self.git_toggle_fold();
    }

    // ── Hunk Navigation ─────────────────────────────────────────────

    /// Move cursor to the next DiffHunk line.
    pub fn git_next_hunk(&mut self) {
        let idx = self.active_buffer_idx();
        let cursor_row = self.window_mgr.focused_window().cursor_row;
        if let Some(view) = self.buffers[idx].git_status_view() {
            for i in (cursor_row + 1)..view.lines.len() {
                if matches!(view.lines[i].kind, crate::git_status::GitLineKind::DiffHunk) {
                    self.window_mgr.focused_window_mut().cursor_row = i;
                    self.window_mgr.focused_window_mut().cursor_col = 0;
                    return;
                }
            }
            self.set_status("No more hunks");
        }
    }

    /// Move cursor to the previous DiffHunk line.
    pub fn git_prev_hunk(&mut self) {
        let idx = self.active_buffer_idx();
        let cursor_row = self.window_mgr.focused_window().cursor_row;
        if let Some(view) = self.buffers[idx].git_status_view() {
            for i in (0..cursor_row).rev() {
                if matches!(view.lines[i].kind, crate::git_status::GitLineKind::DiffHunk) {
                    self.window_mgr.focused_window_mut().cursor_row = i;
                    self.window_mgr.focused_window_mut().cursor_col = 0;
                    return;
                }
            }
            self.set_status("No previous hunks");
        }
    }

    // ── Hunk-Level Stage/Unstage/Discard ────────────────────────────

    /// Find the hunk at cursor: returns (file_path, section, hunk_header, hunk_lines).
    fn find_cursor_hunk(
        &self,
    ) -> Option<(String, crate::git_status::GitSection, String, Vec<String>)> {
        let idx = self.active_buffer_idx();
        let cursor_row = self.window_mgr.focused_window().cursor_row;
        let view = self.buffers[idx].git_status_view()?;
        let cursor_line = view.lines.get(cursor_row)?;

        let file_path = cursor_line.file_path.clone()?;
        let section = cursor_line.section?;
        let hunk_idx = cursor_line.hunk_index?;

        // Collect all lines belonging to this hunk
        let mut hunk_header = String::new();
        let mut hunk_lines = Vec::new();
        for line in &view.lines {
            if line.file_path.as_deref() != Some(&file_path) || line.section != Some(section) {
                continue;
            }
            if line.hunk_index != Some(hunk_idx) {
                continue;
            }
            // Strip the 4-space indent we added during display
            let raw = line.text.strip_prefix("    ").unwrap_or(&line.text);
            if matches!(line.kind, crate::git_status::GitLineKind::DiffHunk) {
                hunk_header = raw.to_string();
            } else {
                hunk_lines.push(raw.to_string());
            }
        }

        if hunk_header.is_empty() {
            return None;
        }
        Some((file_path, section, hunk_header, hunk_lines))
    }

    /// Build a minimal patch for `git apply` from a single hunk.
    fn build_hunk_patch(file_path: &str, hunk_header: &str, hunk_lines: &[String]) -> String {
        let mut patch = String::new();
        patch.push_str(&format!("diff --git a/{} b/{}\n", file_path, file_path));
        patch.push_str(&format!("--- a/{}\n", file_path));
        patch.push_str(&format!("+++ b/{}\n", file_path));
        patch.push_str(hunk_header);
        patch.push('\n');
        for line in hunk_lines {
            patch.push_str(line);
            patch.push('\n');
        }
        patch
    }

    /// Apply the hunk patch at cursor with the given git-apply args.
    fn apply_hunk_patch(&mut self, args: &[&str], success_msg: &str, fail_prefix: &str) {
        if let Some((file_path, _section, hunk_header, hunk_lines)) = self.find_cursor_hunk() {
            let patch = Self::build_hunk_patch(&file_path, &hunk_header, &hunk_lines);
            let root = self.git_root();
            let result = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    use std::io::Write;
                    if let Some(stdin) = child.stdin.as_mut() {
                        stdin.write_all(patch.as_bytes())?;
                    }
                    child.wait_with_output()
                });
            match result {
                Ok(output) if output.status.success() => {
                    self.set_status(success_msg.to_string());
                    self.git_status();
                }
                Ok(output) => {
                    let err = String::from_utf8_lossy(&output.stderr);
                    self.set_status(format!("{}: {}", fail_prefix, err));
                }
                Err(e) => self.set_status(format!("{}: {}", fail_prefix, e)),
            }
        } else {
            self.set_status("No hunk at cursor");
        }
    }

    pub fn git_stage_hunk(&mut self) {
        self.apply_hunk_patch(
            &["apply", "--cached", "--recount"],
            "Hunk staged",
            "Stage hunk failed",
        );
    }

    pub fn git_unstage_hunk(&mut self) {
        self.apply_hunk_patch(
            &["apply", "--cached", "--recount", "-R"],
            "Hunk unstaged",
            "Unstage hunk failed",
        );
    }

    pub fn git_discard_hunk(&mut self) {
        self.apply_hunk_patch(
            &["apply", "--recount", "-R"],
            "Hunk discarded",
            "Discard hunk failed",
        );
    }

    /// Discard unstaged changes for the file at cursor.
    pub fn git_discard_file(&mut self) {
        let (path, section) = self
            .current_git_line()
            .and_then(|line| line.file_path.as_ref().map(|p| (p.clone(), line.section)))
            .unwrap_or_default();

        if path.is_empty() {
            return;
        }

        // Only discard unstaged changes
        if section != Some(crate::git_status::GitSection::Unstaged) {
            self.set_status("Can only discard unstaged changes");
            return;
        }

        let (ok, _, stderr) = self.run_git_porcelain(&["checkout", "--", &path]);
        if ok {
            self.set_status(format!("Discarded changes to {}", path));
            self.git_status();
        } else {
            self.set_status(format!("git checkout failed: {}", stderr));
        }
    }

    // ── Push/Pull/Fetch ─────────────────────────────────────────────

    pub fn git_push(&mut self) {
        self.set_status("Pushing...");
        let (ok, _, stderr) = self.run_git_porcelain(&["push"]);
        if ok {
            self.set_status("Push complete");
        } else {
            self.set_status(format!("Push failed: {}", stderr));
        }
    }

    pub fn git_pull(&mut self) {
        self.set_status("Pulling...");
        let (ok, _, stderr) = self.run_git_porcelain(&["pull"]);
        if ok {
            self.set_status("Pull complete");
            self.git_status();
        } else {
            self.set_status(format!("Pull failed: {}", stderr));
        }
    }

    pub fn git_fetch(&mut self) {
        self.set_status("Fetching...");
        let (ok, _, stderr) = self.run_git_porcelain(&["fetch", "--all"]);
        if ok {
            self.set_status("Fetch complete");
            self.refresh_git_branch();
        } else {
            self.set_status(format!("Fetch failed: {}", stderr));
        }
    }

    // ── Branch Operations ───────────────────────────────────────────

    /// Open command palette with branch list for switching.
    pub fn git_branch_switch_palette(&mut self) {
        let (ok, stdout, _) = self.run_git_porcelain(&["branch", "--format=%(refname:short)"]);
        if !ok {
            self.set_status("Failed to list branches");
            return;
        }
        let branches: Vec<&str> = stdout.lines().collect();
        if branches.is_empty() {
            self.set_status("No branches found");
            return;
        }
        self.command_palette = Some(crate::command_palette::CommandPalette::for_git_branch(
            &branches,
        ));
        self.set_mode(crate::Mode::CommandPalette);
    }

    /// Switch to a named branch.
    pub fn git_branch_switch(&mut self, name: &str) {
        let (ok, _, stderr) = self.run_git_porcelain(&["switch", name]);
        if ok {
            self.set_status(format!("Switched to {}", name));
            self.refresh_git_branch();
            // Refresh status if in git-status buffer
            if self.buffers[self.active_buffer_idx()].kind == crate::buffer::BufferKind::GitStatus {
                self.git_status();
            }
        } else {
            self.set_status(format!("Switch failed: {}", stderr));
        }
    }

    /// Create a new branch (name from command line input).
    pub fn git_branch_create(&mut self, name: &str) {
        if name.is_empty() {
            self.set_status("Branch name required");
            return;
        }
        let (ok, _, stderr) = self.run_git_porcelain(&["checkout", "-b", name]);
        if ok {
            self.set_status(format!("Created and switched to {}", name));
            self.refresh_git_branch();
        } else {
            self.set_status(format!("Create branch failed: {}", stderr));
        }
    }

    /// Delete a branch by name.
    pub fn git_branch_delete(&mut self, name: &str) {
        if name.is_empty() {
            self.set_status("Branch name required");
            return;
        }
        let (ok, _, stderr) = self.run_git_porcelain(&["branch", "-d", name]);
        if ok {
            self.set_status(format!("Deleted branch {}", name));
        } else {
            self.set_status(format!("Delete branch failed: {}", stderr));
        }
    }

    // ── Stash Operations ────────────────────────────────────────────

    pub fn git_stash_push(&mut self) {
        let (ok, _, stderr) = self.run_git_porcelain(&["stash", "push"]);
        if ok {
            self.set_status("Changes stashed");
            self.git_status();
        } else {
            self.set_status(format!("Stash failed: {}", stderr));
        }
    }

    /// Extract stash index from cursor line text (e.g. "stash@{0}: ...").
    fn stash_index_at_cursor(&self) -> Option<usize> {
        let line = self.current_git_line()?;
        // Look for stash@{N} pattern in the line text
        let text = &line.text;
        let start = text.find("stash@{")?;
        let rest = &text[start + 7..];
        let end = rest.find('}')?;
        rest[..end].parse().ok()
    }

    /// Resolve stash ref at cursor, defaulting to stash@{0}.
    fn stash_ref_at_cursor(&self) -> String {
        self.stash_index_at_cursor()
            .map(|n| format!("stash@{{{}}}", n))
            .unwrap_or_else(|| "stash@{0}".to_string())
    }

    /// Run a stash subcommand with the ref at cursor.
    fn stash_op(&mut self, verb: &str, past_tense: &str) {
        let idx_str = self.stash_ref_at_cursor();
        let (ok, _, stderr) = self.run_git_porcelain(&["stash", verb, &idx_str]);
        if ok {
            self.set_status(format!("{} {}", past_tense, idx_str));
            self.git_status();
        } else {
            self.set_status(format!("Stash {} failed: {}", verb, stderr));
        }
    }

    pub fn git_stash_pop(&mut self) {
        self.stash_op("pop", "Popped");
    }

    pub fn git_stash_apply(&mut self) {
        self.stash_op("apply", "Applied");
    }

    pub fn git_stash_drop(&mut self) {
        self.stash_op("drop", "Dropped");
    }

    // ── Discard (context-aware) ─────────────────────────────────────

    /// Context-aware discard: hunk if on diff line, file if on file line.
    pub fn git_discard_at_cursor(&mut self) {
        let idx = self.active_buffer_idx();
        let cursor_row = self.window_mgr.focused_window().cursor_row;
        let kind = self.buffers[idx]
            .git_status_view()
            .and_then(|v| v.kind_at(cursor_row).cloned());

        match kind {
            Some(crate::git_status::GitLineKind::DiffHunk)
            | Some(crate::git_status::GitLineKind::DiffLine(_)) => {
                self.git_discard_hunk();
            }
            _ => {
                self.git_discard_file();
            }
        }
    }

    // ── Context-Aware Stage/Unstage ─────────────────────────────────

    /// Collect file paths belonging to a section in the git status view.
    fn section_file_paths(&self, section: crate::git_status::GitSection) -> Vec<String> {
        let idx = self.active_buffer_idx();
        self.buffers[idx]
            .git_status_view()
            .map(|v| {
                v.lines
                    .iter()
                    .filter(|l| l.section == Some(section))
                    .filter_map(|l| l.file_path.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Context-aware stage: hunk if on diff line, file if on file line,
    /// all files if on section header. Batch: only refresh once at the end.
    pub fn git_stage_at_cursor(&mut self) {
        let cursor_row = self.window_mgr.focused_window().cursor_row;
        let kind = self.buffers[self.active_buffer_idx()]
            .git_status_view()
            .and_then(|v| v.kind_at(cursor_row).cloned());

        match kind {
            Some(crate::git_status::GitLineKind::DiffHunk)
            | Some(crate::git_status::GitLineKind::DiffLine(_)) => {
                self.git_stage_hunk();
            }
            Some(crate::git_status::GitLineKind::SectionHeader(section)) => {
                let paths = self.section_file_paths(section);
                for p in &paths {
                    let _ = self.run_git_porcelain(&["add", "--", p]);
                }
                self.git_status();
            }
            _ => {
                if let Some(p) = self.current_git_file_path() {
                    self.git_stage_file(&p);
                }
            }
        }
    }

    /// Context-aware unstage: hunk if on diff line, file if on file line,
    /// all files if on section header. Batch: only refresh once at the end.
    pub fn git_unstage_at_cursor(&mut self) {
        let cursor_row = self.window_mgr.focused_window().cursor_row;
        let kind = self.buffers[self.active_buffer_idx()]
            .git_status_view()
            .and_then(|v| v.kind_at(cursor_row).cloned());

        match kind {
            Some(crate::git_status::GitLineKind::DiffHunk)
            | Some(crate::git_status::GitLineKind::DiffLine(_)) => {
                self.git_unstage_hunk();
            }
            Some(crate::git_status::GitLineKind::SectionHeader(section)) => {
                let paths = self.section_file_paths(section);
                for p in &paths {
                    let _ = self.run_git_porcelain(&["reset", "HEAD", "--", p]);
                }
                self.git_status();
            }
            _ => {
                if let Some(p) = self.current_git_file_path() {
                    self.git_unstage_file(&p);
                }
            }
        }
    }

    fn git_root(&self) -> std::path::PathBuf {
        self.active_project_root()
            .map(|p| p.to_path_buf())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default()
    }

    /// Amend the previous commit.
    pub fn git_amend(&mut self) {
        let root = self.git_root();

        info!("opening amend commit message buffer");
        let commit_file = root.join(".git/COMMIT_EDITMSG");
        // Pre-populate with previous commit message
        let (ok, msg, _) = self.run_git_porcelain(&["log", "-1", "--format=%B"]);
        if ok {
            let _ = std::fs::write(&commit_file, msg.trim());
        }
        self.open_file(&commit_file);
        self.set_status("Edit commit message and save to amend (use :!git commit --amend)");
    }

    fn run_git_porcelain(&self, args: &[&str]) -> (bool, String, String) {
        let root = self.git_root();

        match std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
        {
            Ok(output) => {
                let success = output.status.success();
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                (success, stdout, stderr)
            }
            Err(e) => (false, String::new(), e.to_string()),
        }
    }

    pub fn git_stage_file(&mut self, path: &str) {
        info!(path, "git staging path");
        let (ok, _, stderr) = self.run_git_porcelain(&["add", path]);
        if ok {
            self.set_status(format!("Staged {}", path));
            self.git_status(); // Refresh
        } else {
            error!(path, error = %stderr, "git add failed");
            self.set_status(format!("git add failed: {}", stderr));
        }
    }

    pub fn git_unstage_file(&mut self, path: &str) {
        info!(path, "git unstaging path");
        let (ok, _, stderr) = self.run_git_porcelain(&["reset", "HEAD", "--", path]);
        if ok {
            self.set_status(format!("Unstaged {}", path));
            self.git_status(); // Refresh
        } else {
            error!(path, error = %stderr, "git reset failed");
            self.set_status(format!("git reset failed: {}", stderr));
        }
    }

    pub(crate) fn git_blame(&mut self) {
        let buf_idx = self.active_buffer_idx();
        // Toggle: if blame overlay is already showing for this buffer, dismiss it.
        if let Some(ref overlay) = self.blame_overlay {
            if overlay.buffer_idx == buf_idx {
                self.blame_overlay = None;
                self.set_status("Blame overlay dismissed");
                return;
            }
        }

        let file = self
            .active_buffer()
            .file_path()
            .map(|p| p.display().to_string());
        if let Some(path) = file {
            info!(path, "showing git blame overlay");
            let (ok, stdout, stderr) = self.run_git_porcelain(&["blame", "--porcelain", &path]);
            if !ok {
                self.set_status(format!("git blame failed: {}", stderr));
                return;
            }
            let entries = Self::parse_blame_porcelain(&stdout);
            self.blame_overlay = Some(BlameOverlay {
                buffer_idx: buf_idx,
                entries,
            });
            self.set_status("Blame overlay active (SPC g b to toggle)");
        } else {
            self.set_status("git blame: buffer has no file path");
        }
    }

    /// Parse `git blame --porcelain` output into BlameEntry list.
    fn parse_blame_porcelain(output: &str) -> Vec<BlameEntry> {
        let mut entries = Vec::new();
        let mut current_hash = String::new();
        let mut current_author = String::new();
        let mut current_timestamp: i64 = 0;
        let mut current_summary = String::new();
        let mut current_line: usize = 0;

        for line in output.lines() {
            if let Some(rest) = line.strip_prefix("author ") {
                current_author = rest.to_string();
            } else if let Some(rest) = line.strip_prefix("author-time ") {
                current_timestamp = rest.parse().unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("summary ") {
                current_summary = rest.to_string();
            } else if line.starts_with('\t') {
                // Content line — this ends the current entry
                entries.push(BlameEntry {
                    commit_hash: if current_hash.len() >= 8 {
                        current_hash[..8].to_string()
                    } else {
                        current_hash.clone()
                    },
                    author: current_author.clone(),
                    timestamp: current_timestamp,
                    summary: current_summary.clone(),
                    final_line: current_line.saturating_sub(1), // 0-indexed
                });
            } else if let Some(first_space) = line.find(' ') {
                let hash_candidate = &line[..first_space];
                // Porcelain format: <40-hex-hash> <orig-line> <final-line> [<group-count>]
                if hash_candidate.len() == 40
                    && hash_candidate.chars().all(|c| c.is_ascii_hexdigit())
                {
                    current_hash = hash_candidate.to_string();
                    // Parse final line number (second number after orig-line)
                    let rest = &line[first_space + 1..];
                    let parts: Vec<&str> = rest.split_whitespace().collect();
                    if parts.len() >= 2 {
                        current_line = parts[1].parse().unwrap_or(0);
                    }
                }
            }
        }
        entries
    }

    pub(crate) fn git_diff(&mut self) {
        info!("showing git diff");
        self.git_command_to_buffer(&["diff"], "*git-diff*");
    }
    pub(crate) fn git_commit(&mut self) {
        let root = self.git_root();

        info!("opening commit message buffer");
        let commit_file = root.join(".git/COMMIT_EDITMSG");
        self.open_file(&commit_file);
        self.set_status("Edit commit message and save to commit");
    }

    pub(crate) fn git_log(&mut self) {
        info!("showing git log");
        self.git_command_to_buffer(
            &[
                "log",
                "--oneline",
                "--graph",
                "--decorate",
                "--date=relative",
                "--format=%h %ad %an  %s%d",
                "-100",
            ],
            "*git-log*",
        );
    }

    /// Spawn a background thread to run `git diff HEAD --unified=0` for a buffer.
    /// Results are polled via `poll_pending_git_diff()` on idle ticks.
    pub(crate) fn request_git_diff(&mut self, buffer_idx: usize) {
        let file_path = match self.buffers[buffer_idx].file_path() {
            Some(p) => p.to_path_buf(),
            None => return,
        };
        // Use buffer's project root if available, then editor-wide, then CWD.
        let root = self.buffers[buffer_idx]
            .project_root
            .clone()
            .or_else(|| self.project.as_ref().map(|p| p.root.clone()))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();

        let (tx, rx) = std::sync::mpsc::channel();
        let fp = file_path.clone();
        std::thread::spawn(move || {
            let result = match std::process::Command::new("git")
                .args(["diff", "HEAD", "--unified=0", "--"])
                .arg(&fp)
                .current_dir(&root)
                .output()
            {
                Ok(o) if o.status.success() => {
                    let stdout = String::from_utf8_lossy(&o.stdout);
                    parse_diff_hunks(&stdout)
                }
                _ => std::collections::HashMap::new(),
            };
            let _ = tx.send(result);
        });

        // Latest request wins — any prior pending result is dropped.
        self.pending_git_diff = Some(PendingGitDiff {
            file_path,
            receiver: rx,
        });
    }

    /// Poll for a completed async git diff result. Called from `idle_work()`.
    pub(crate) fn poll_pending_git_diff(&mut self) {
        let pending = match self.pending_git_diff.as_ref() {
            Some(p) => p,
            None => return,
        };
        match pending.receiver.try_recv() {
            Ok(diff_lines) => {
                let path = pending.file_path.clone();
                self.pending_git_diff = None;
                // Find the buffer by file path (not index) to avoid stale-index bugs.
                if let Some(idx) = self
                    .buffers
                    .iter()
                    .position(|b| b.file_path() == Some(&path))
                {
                    self.buffers[idx].git_diff_lines = diff_lines;
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // Still running — leave pending.
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                // Thread panicked or channel closed — drop silently.
                self.pending_git_diff = None;
            }
        }
    }
}

/// Parse `@@ -old_start,old_count +new_start,new_count @@` hunk headers from unified diff output.
fn parse_diff_hunks(
    diff_output: &str,
) -> std::collections::HashMap<usize, crate::render_common::gutter::GitLineStatus> {
    use crate::render_common::gutter::GitLineStatus;
    let mut map = std::collections::HashMap::new();
    for line in diff_output.lines() {
        if !line.starts_with("@@ ") {
            continue;
        }
        // Parse @@ -old_start[,old_count] +new_start[,new_count] @@
        let Some(plus_idx) = line.find('+') else {
            continue;
        };
        let rest = &line[plus_idx + 1..];
        let end = rest.find(' ').unwrap_or(rest.len());
        let range_str = &rest[..end];
        let (new_start, new_count) = if let Some(comma) = range_str.find(',') {
            let start: usize = range_str[..comma].parse().unwrap_or(0);
            let count: usize = range_str[comma + 1..].parse().unwrap_or(0);
            (start, count)
        } else {
            let start: usize = range_str.parse().unwrap_or(0);
            (start, 1)
        };

        // Determine old_count from the minus portion
        let minus_start = line.find('-').unwrap_or(0) + 1;
        let minus_rest = &line[minus_start..plus_idx].trim();
        let old_count = if let Some(comma) = minus_rest.find(',') {
            minus_rest[comma + 1..].parse::<usize>().unwrap_or(1)
        } else {
            1
        };

        if new_count == 0 {
            // Deletion: mark the line at new_start (0-indexed) as deleted
            if new_start > 0 {
                map.insert(new_start - 1, GitLineStatus::Deleted);
            }
        } else if old_count == 0 {
            // Pure addition
            for i in 0..new_count {
                map.insert(new_start - 1 + i, GitLineStatus::Added);
            }
        } else {
            // Modification
            for i in 0..new_count {
                map.insert(new_start - 1 + i, GitLineStatus::Modified);
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_diff_hunks_added() {
        let diff = "@@ -0,0 +1,3 @@\n+line1\n+line2\n+line3\n";
        let map = parse_diff_hunks(diff);
        assert_eq!(map.len(), 3);
        assert_eq!(map[&0], crate::render_common::gutter::GitLineStatus::Added);
        assert_eq!(map[&1], crate::render_common::gutter::GitLineStatus::Added);
        assert_eq!(map[&2], crate::render_common::gutter::GitLineStatus::Added);
    }

    #[test]
    fn parse_diff_hunks_modified() {
        let diff = "@@ -5,2 +5,2 @@\n-old1\n-old2\n+new1\n+new2\n";
        let map = parse_diff_hunks(diff);
        assert_eq!(map.len(), 2);
        assert_eq!(
            map[&4],
            crate::render_common::gutter::GitLineStatus::Modified
        );
        assert_eq!(
            map[&5],
            crate::render_common::gutter::GitLineStatus::Modified
        );
    }

    #[test]
    fn parse_diff_hunks_deleted() {
        let diff = "@@ -3,2 +2,0 @@\n-removed1\n-removed2\n";
        let map = parse_diff_hunks(diff);
        assert_eq!(map.len(), 1);
        assert_eq!(
            map[&1],
            crate::render_common::gutter::GitLineStatus::Deleted
        );
    }

    #[test]
    fn parse_diff_hunks_empty() {
        let map = parse_diff_hunks("");
        assert!(map.is_empty());
    }

    #[test]
    fn parse_diff_hunks_multiple() {
        let diff = "@@ -1,1 +1,1 @@\n-old\n+new\n@@ -10,0 +10,2 @@\n+added1\n+added2\n";
        let map = parse_diff_hunks(diff);
        assert_eq!(
            map[&0],
            crate::render_common::gutter::GitLineStatus::Modified
        );
        assert_eq!(map[&9], crate::render_common::gutter::GitLineStatus::Added);
        assert_eq!(map[&10], crate::render_common::gutter::GitLineStatus::Added);
    }

    #[test]
    fn parse_blame_porcelain_basic() {
        let output = "\
abcdef1234567890abcdef1234567890abcdef12 1 1 1\n\
author John Doe\n\
author-mail <john@example.com>\n\
author-time 1700000000\n\
author-tz +0000\n\
committer John Doe\n\
committer-mail <john@example.com>\n\
committer-time 1700000000\n\
committer-tz +0000\n\
summary Initial commit\n\
filename src/main.rs\n\
\tuse std::io;\n\
abcdef1234567890abcdef1234567890abcdef12 2 2\n\
author Jane Smith\n\
author-mail <jane@example.com>\n\
author-time 1700100000\n\
author-tz +0000\n\
committer Jane Smith\n\
committer-mail <jane@example.com>\n\
committer-time 1700100000\n\
committer-tz +0000\n\
summary Second commit\n\
filename src/main.rs\n\
\tfn main() {}\n";

        let entries = super::Editor::parse_blame_porcelain(output);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].commit_hash, "abcdef12");
        assert_eq!(entries[0].author, "John Doe");
        assert_eq!(entries[0].timestamp, 1700000000);
        assert_eq!(entries[0].summary, "Initial commit");
        assert_eq!(entries[0].final_line, 0); // 0-indexed

        assert_eq!(entries[1].author, "Jane Smith");
        assert_eq!(entries[1].timestamp, 1700100000);
        assert_eq!(entries[1].summary, "Second commit");
        assert_eq!(entries[1].final_line, 1);
    }
}
