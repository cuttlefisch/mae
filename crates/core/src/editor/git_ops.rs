//! Git integration commands (SPC g group).
//!
//! Shell-out stubs that capture git output into read-only scratch buffers.
//! Full integration deferred to Phase 6 (Embedded Shell + Magit Parity).

use crate::buffer::Buffer;
use tracing::{error, info};

use super::Editor;

impl Editor {
    /// Run a git command and put output in a read-only scratch buffer.
    fn git_command_to_buffer(&mut self, args: &[&str], buf_name: &str) {
        let root = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();

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
                self.alternate_buffer_idx = Some(prev);
                self.window_mgr.focused_window_mut().buffer_idx = idx;
                self.window_mgr.focused_window_mut().cursor_row = 0;
                self.window_mgr.focused_window_mut().cursor_col = 0;
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

    pub fn git_status(&mut self) {
        use crate::git_status::*;

        let root = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
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
            text.push_str(&line.text);
            text.push('\n');
            view.line_kinds.push(line.kind.clone());
            view.lines.push(line);
        }

        // Header
        push_line(
            &mut view,
            &mut text,
            GitStatusLine {
                text: format!("Head:     {}", branch),
                section: None,
                file_path: None,
                hunk: None,
                hunk_index: None,
                hunk_header: None,
                is_header: true,
                is_collapsed: false,
                kind: GitLineKind::Header,
            },
        );

        // Blank separator
        push_line(
            &mut view,
            &mut text,
            GitStatusLine {
                text: String::new(),
                section: None,
                file_path: None,
                hunk: None,
                hunk_index: None,
                hunk_header: None,
                is_header: false,
                is_collapsed: false,
                kind: GitLineKind::Blank,
            },
        );

        // Helper closure to push a section with multi-level collapse
        let push_section = |view: &mut GitStatusView,
                            text: &mut String,
                            section: GitSection,
                            heading: &str,
                            files: &[(String, char)]| {
            if files.is_empty() {
                return;
            }
            let section_key = format!("section:{}", section_name(&section));
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
                    hunk: None,
                    hunk_index: None,
                    hunk_header: None,
                    is_header: true,
                    is_collapsed: section_collapsed,
                    kind: GitLineKind::SectionHeader(section),
                },
            );

            if section_collapsed {
                // Blank separator even when collapsed
                push_line(
                    view,
                    text,
                    GitStatusLine {
                        text: String::new(),
                        section: None,
                        file_path: None,
                        hunk: None,
                        hunk_index: None,
                        hunk_header: None,
                        is_header: false,
                        is_collapsed: false,
                        kind: GitLineKind::Blank,
                    },
                );
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
                        hunk: None,
                        hunk_index: None,
                        hunk_header: None,
                        is_header: false,
                        is_collapsed: !file_expanded,
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
                        let hunk_key =
                            format!("hunk:{}:{}:{}", p, section_name(&section), hunk_idx);
                        let hunk_collapsed = view.is_collapsed(&hunk_key);

                        if is_hunk_header {
                            let display_line = format!("    {}", diff_line);
                            push_line(
                                view,
                                text,
                                GitStatusLine {
                                    text: display_line,
                                    section: Some(section),
                                    file_path: Some(p.clone()),
                                    hunk: None,
                                    hunk_index: Some(hunk_idx),
                                    hunk_header: current_hunk_header.clone(),
                                    is_header: false,
                                    is_collapsed: hunk_collapsed,
                                    kind: diff_kind,
                                },
                            );
                            // Increment hunk index AFTER emitting the header
                            if hunk_idx > 0 || !is_hunk_header {
                                // already incremented below
                            }
                        } else if !hunk_collapsed {
                            let display_line = format!("    {}", diff_line);
                            push_line(
                                view,
                                text,
                                GitStatusLine {
                                    text: display_line,
                                    section: Some(section),
                                    file_path: Some(p.clone()),
                                    hunk: None,
                                    hunk_index: Some(hunk_idx),
                                    hunk_header: current_hunk_header.clone(),
                                    is_header: false,
                                    is_collapsed: false,
                                    kind: diff_kind,
                                },
                            );
                        }

                        // Advance hunk counter on next @@ line
                        if is_hunk_header && diff_line.starts_with("@@") {
                            // hunk_idx is already set for this hunk's lines;
                            // we'll increment when we see the NEXT hunk header
                        }
                        // Actually: increment after processing a complete hunk header
                        if is_hunk_header {
                            hunk_idx += 1;
                            // Correct: hunk N's header was emitted with hunk_index=N,
                            // now hunk_idx=N+1 for the next hunk
                        }
                    }
                }
            }

            // Blank separator
            push_line(
                view,
                text,
                GitStatusLine {
                    text: String::new(),
                    section: None,
                    file_path: None,
                    hunk: None,
                    hunk_index: None,
                    hunk_header: None,
                    is_header: false,
                    is_collapsed: false,
                    kind: GitLineKind::Blank,
                },
            );
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
            let stash_section_key = "section:Stashes";
            let stash_collapsed = view.is_collapsed(stash_section_key);
            let indicator = if stash_collapsed { "▸" } else { "▾" };
            push_line(
                &mut view,
                &mut text,
                GitStatusLine {
                    text: format!("{} Stashes:", indicator),
                    section: Some(GitSection::Stashes),
                    file_path: None,
                    hunk: None,
                    hunk_index: None,
                    hunk_header: None,
                    is_header: true,
                    is_collapsed: stash_collapsed,
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
                            hunk: None,
                            hunk_index: None,
                            hunk_header: None,
                            is_header: false,
                            is_collapsed: false,
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
        self.alternate_buffer_idx = Some(prev);
        self.window_mgr.focused_window_mut().buffer_idx = idx;
        self.window_mgr.focused_window_mut().cursor_row = 0;
        self.window_mgr.focused_window_mut().cursor_col = 0;
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
            v.lines
                .get(cursor_row)
                .and_then(crate::git_status::GitStatusView::collapse_key_for_line)
        });

        if let Some(k) = key {
            if let Some(view) = self.buffers[idx].git_status_view_mut() {
                // File keys default to collapsed (true); sections/hunks default to expanded (false).
                if k.starts_with("file:") {
                    let collapsed = view.collapsed.entry(k).or_insert(true);
                    *collapsed = !*collapsed;
                } else {
                    view.toggle(&k);
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
            for i in (cursor_row + 1)..view.line_kinds.len() {
                if matches!(view.line_kinds[i], crate::git_status::GitLineKind::DiffHunk) {
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
                if matches!(view.line_kinds[i], crate::git_status::GitLineKind::DiffHunk) {
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
    fn build_hunk_patch(
        file_path: &str,
        hunk_header: &str,
        hunk_lines: &[String],
        staged: bool,
    ) -> String {
        // For staged hunks, the a/ and b/ are both the file path.
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
        let _ = staged; // both paths use same patch format
        patch
    }

    /// Stage the hunk at cursor.
    pub fn git_stage_hunk(&mut self) {
        if let Some((file_path, _section, hunk_header, hunk_lines)) = self.find_cursor_hunk() {
            let patch = Self::build_hunk_patch(&file_path, &hunk_header, &hunk_lines, false);
            let root = self.git_root();
            let result = std::process::Command::new("git")
                .args(["apply", "--cached", "--recount"])
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
                    self.set_status("Hunk staged");
                    self.git_status();
                }
                Ok(output) => {
                    let err = String::from_utf8_lossy(&output.stderr);
                    self.set_status(format!("Stage hunk failed: {}", err));
                }
                Err(e) => self.set_status(format!("Stage hunk failed: {}", e)),
            }
        } else {
            self.set_status("No hunk at cursor");
        }
    }

    /// Unstage the hunk at cursor.
    pub fn git_unstage_hunk(&mut self) {
        if let Some((file_path, _section, hunk_header, hunk_lines)) = self.find_cursor_hunk() {
            let patch = Self::build_hunk_patch(&file_path, &hunk_header, &hunk_lines, true);
            let root = self.git_root();
            let result = std::process::Command::new("git")
                .args(["apply", "--cached", "--recount", "-R"])
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
                    self.set_status("Hunk unstaged");
                    self.git_status();
                }
                Ok(output) => {
                    let err = String::from_utf8_lossy(&output.stderr);
                    self.set_status(format!("Unstage hunk failed: {}", err));
                }
                Err(e) => self.set_status(format!("Unstage hunk failed: {}", e)),
            }
        } else {
            self.set_status("No hunk at cursor");
        }
    }

    /// Discard the hunk at cursor (apply reverse patch to working tree).
    pub fn git_discard_hunk(&mut self) {
        if let Some((file_path, _section, hunk_header, hunk_lines)) = self.find_cursor_hunk() {
            let patch = Self::build_hunk_patch(&file_path, &hunk_header, &hunk_lines, false);
            let root = self.git_root();
            let result = std::process::Command::new("git")
                .args(["apply", "--recount", "-R"])
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
                    self.set_status("Hunk discarded");
                    self.git_status();
                }
                Ok(output) => {
                    let err = String::from_utf8_lossy(&output.stderr);
                    self.set_status(format!("Discard hunk failed: {}", err));
                }
                Err(e) => self.set_status(format!("Discard hunk failed: {}", e)),
            }
        } else {
            self.set_status("No hunk at cursor");
        }
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

    pub fn git_stash_pop(&mut self) {
        let idx_str = self
            .stash_index_at_cursor()
            .map(|n| format!("stash@{{{}}}", n))
            .unwrap_or_else(|| "stash@{0}".to_string());
        let (ok, _, stderr) = self.run_git_porcelain(&["stash", "pop", &idx_str]);
        if ok {
            self.set_status(format!("Popped {}", idx_str));
            self.git_status();
        } else {
            self.set_status(format!("Stash pop failed: {}", stderr));
        }
    }

    pub fn git_stash_apply(&mut self) {
        let idx_str = self
            .stash_index_at_cursor()
            .map(|n| format!("stash@{{{}}}", n))
            .unwrap_or_else(|| "stash@{0}".to_string());
        let (ok, _, stderr) = self.run_git_porcelain(&["stash", "apply", &idx_str]);
        if ok {
            self.set_status(format!("Applied {}", idx_str));
            self.git_status();
        } else {
            self.set_status(format!("Stash apply failed: {}", stderr));
        }
    }

    pub fn git_stash_drop(&mut self) {
        let idx_str = self
            .stash_index_at_cursor()
            .map(|n| format!("stash@{{{}}}", n))
            .unwrap_or_else(|| "stash@{0}".to_string());
        let (ok, _, stderr) = self.run_git_porcelain(&["stash", "drop", &idx_str]);
        if ok {
            self.set_status(format!("Dropped {}", idx_str));
            self.git_status();
        } else {
            self.set_status(format!("Stash drop failed: {}", stderr));
        }
    }

    // ── Discard (context-aware) ─────────────────────────────────────

    /// Context-aware discard: hunk if on diff line, file if on file line.
    pub fn git_discard_at_cursor(&mut self) {
        let idx = self.active_buffer_idx();
        let cursor_row = self.window_mgr.focused_window().cursor_row;
        let kind = self.buffers[idx]
            .git_status_view()
            .and_then(|v| v.line_kinds.get(cursor_row).cloned());

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

    /// Context-aware stage: hunk if on diff line, file if on file line.
    pub fn git_stage_at_cursor(&mut self) {
        let idx = self.active_buffer_idx();
        let cursor_row = self.window_mgr.focused_window().cursor_row;
        let kind = self.buffers[idx]
            .git_status_view()
            .and_then(|v| v.line_kinds.get(cursor_row).cloned());

        match kind {
            Some(crate::git_status::GitLineKind::DiffHunk)
            | Some(crate::git_status::GitLineKind::DiffLine(_)) => {
                self.git_stage_hunk();
            }
            Some(crate::git_status::GitLineKind::SectionHeader(section)) => {
                // Stage all files in section
                let paths: Vec<String> = self.buffers[idx]
                    .git_status_view()
                    .map(|v| {
                        v.lines
                            .iter()
                            .filter(|l| l.section == Some(section))
                            .filter_map(|l| l.file_path.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                for p in paths {
                    self.git_stage_file(&p);
                }
            }
            _ => {
                if let Some(p) = self.current_git_file_path() {
                    self.git_stage_file(&p);
                }
            }
        }
    }

    /// Context-aware unstage: hunk if on diff line, file if on file line.
    pub fn git_unstage_at_cursor(&mut self) {
        let idx = self.active_buffer_idx();
        let cursor_row = self.window_mgr.focused_window().cursor_row;
        let kind = self.buffers[idx]
            .git_status_view()
            .and_then(|v| v.line_kinds.get(cursor_row).cloned());

        match kind {
            Some(crate::git_status::GitLineKind::DiffHunk)
            | Some(crate::git_status::GitLineKind::DiffLine(_)) => {
                self.git_unstage_hunk();
            }
            Some(crate::git_status::GitLineKind::SectionHeader(section)) => {
                let paths: Vec<String> = self.buffers[idx]
                    .git_status_view()
                    .map(|v| {
                        v.lines
                            .iter()
                            .filter(|l| l.section == Some(section))
                            .filter_map(|l| l.file_path.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                for p in paths {
                    self.git_unstage_file(&p);
                }
            }
            _ => {
                if let Some(p) = self.current_git_file_path() {
                    self.git_unstage_file(&p);
                }
            }
        }
    }

    fn git_root(&self) -> std::path::PathBuf {
        self.project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default()
    }

    /// Amend the previous commit.
    pub fn git_amend(&mut self) {
        let root = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();

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
        let root = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();

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
        let file = self
            .active_buffer()
            .file_path()
            .map(|p| p.display().to_string());
        if let Some(path) = file {
            info!(path, "showing git blame");
            self.git_command_to_buffer(&["blame", &path], "*git-blame*");
        } else {
            self.set_status("git blame: buffer has no file path");
        }
    }

    pub(crate) fn git_diff(&mut self) {
        info!("showing git diff");
        self.git_command_to_buffer(&["diff"], "*git-diff*");
    }
    pub(crate) fn git_commit(&mut self) {
        let root = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();

        info!("opening commit message buffer");
        let commit_file = root.join(".git/COMMIT_EDITMSG");
        self.open_file(&commit_file);
        self.set_status("Edit commit message and save to commit");
    }

    pub(crate) fn git_log(&mut self) {
        info!("showing git log");
        self.git_command_to_buffer(&["log", "--oneline", "-50"], "*git-log*");
    }

    /// Parse `git diff HEAD --unified=0` output for a buffer and populate
    /// its `git_diff_lines` map.
    pub(crate) fn refresh_git_diff(&mut self, buffer_idx: usize) {
        let file_path = match self.buffers[buffer_idx].file_path() {
            Some(p) => p.to_path_buf(),
            None => return,
        };
        let root = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let output = match std::process::Command::new("git")
            .args(["diff", "HEAD", "--unified=0", "--"])
            .arg(&file_path)
            .current_dir(&root)
            .output()
        {
            Ok(o) if o.status.success() => o,
            _ => {
                self.buffers[buffer_idx].git_diff_lines.clear();
                return;
            }
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        self.buffers[buffer_idx].git_diff_lines = parse_diff_hunks(&stdout);
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
}
