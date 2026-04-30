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
                    .map(|v| v.collapsed_paths.clone())
            })
            .unwrap_or_default();

        let mut view = GitStatusView::new(root.clone());
        view.collapsed_paths = prev_collapsed;
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

        // Header
        let header_line = format!("Head:     {}", branch);
        text.push_str(&header_line);
        text.push('\n');
        view.lines.push(GitStatusLine {
            text: header_line,
            section: None,
            file_path: None,
            hunk: None,
            is_header: true,
            is_collapsed: false,
            kind: GitLineKind::Header,
        });
        view.line_kinds.push(GitLineKind::Header);

        // Blank separator
        text.push('\n');
        view.lines.push(GitStatusLine {
            text: String::new(),
            section: None,
            file_path: None,
            hunk: None,
            is_header: false,
            is_collapsed: false,
            kind: GitLineKind::Blank,
        });
        view.line_kinds.push(GitLineKind::Blank);

        // Helper closure to push a section
        let push_section = |view: &mut GitStatusView,
                            text: &mut String,
                            section: GitSection,
                            heading: &str,
                            files: &[(String, char)]| {
            if files.is_empty() {
                return;
            }
            // Section header
            text.push_str(heading);
            text.push('\n');
            let kind = GitLineKind::SectionHeader(section);
            view.lines.push(GitStatusLine {
                text: heading.to_string(),
                section: Some(section),
                file_path: None,
                hunk: None,
                is_header: true,
                is_collapsed: false,
                kind: kind.clone(),
            });
            view.line_kinds.push(kind);

            // File entries
            for (p, status_char) in files {
                let file_text = format!("  {} {}", status_char, p);
                text.push_str(&file_text);
                text.push('\n');
                let file_kind = GitLineKind::File {
                    section,
                    status_char: *status_char,
                };
                view.lines.push(GitStatusLine {
                    text: file_text,
                    section: Some(section),
                    file_path: Some(p.clone()),
                    hunk: None,
                    is_header: false,
                    is_collapsed: !view.is_expanded(p),
                    kind: file_kind.clone(),
                });
                view.line_kinds.push(file_kind);

                // Inline diff for expanded files
                if view.is_expanded(p) {
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
                    for diff_line in diff_output.lines() {
                        if diff_line.starts_with("diff ")
                            || diff_line.starts_with("index ")
                            || diff_line.starts_with("--- ")
                            || diff_line.starts_with("+++ ")
                        {
                            continue; // Skip diff metadata
                        }
                        let diff_kind = if diff_line.starts_with("@@") {
                            GitLineKind::DiffHunk
                        } else if diff_line.starts_with('+') {
                            GitLineKind::DiffLine(DiffLineType::Added)
                        } else if diff_line.starts_with('-') {
                            GitLineKind::DiffLine(DiffLineType::Removed)
                        } else {
                            GitLineKind::DiffLine(DiffLineType::Context)
                        };
                        let display_line = format!("    {}", diff_line);
                        text.push_str(&display_line);
                        text.push('\n');
                        view.lines.push(GitStatusLine {
                            text: display_line,
                            section: Some(section),
                            file_path: Some(p.clone()),
                            hunk: None,
                            is_header: false,
                            is_collapsed: false,
                            kind: diff_kind.clone(),
                        });
                        view.line_kinds.push(diff_kind);
                    }
                }
            }

            // Blank separator
            text.push('\n');
            view.lines.push(GitStatusLine {
                text: String::new(),
                section: None,
                file_path: None,
                hunk: None,
                is_header: false,
                is_collapsed: false,
                kind: GitLineKind::Blank,
            });
            view.line_kinds.push(GitLineKind::Blank);
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
            text.push_str("Stashes:\n");
            let kind = GitLineKind::SectionHeader(GitSection::Stashes);
            view.lines.push(GitStatusLine {
                text: "Stashes:".to_string(),
                section: Some(GitSection::Stashes),
                file_path: None,
                hunk: None,
                is_header: true,
                is_collapsed: false,
                kind: kind.clone(),
            });
            view.line_kinds.push(kind);

            for stash_line in stash_stdout.lines() {
                let display_line = format!("  {}", stash_line);
                text.push_str(&display_line);
                text.push('\n');
                view.lines.push(GitStatusLine {
                    text: display_line,
                    section: Some(GitSection::Stashes),
                    file_path: None,
                    hunk: None,
                    is_header: false,
                    is_collapsed: false,
                    kind: GitLineKind::File {
                        section: GitSection::Stashes,
                        status_char: 'S',
                    },
                });
                view.line_kinds.push(GitLineKind::File {
                    section: GitSection::Stashes,
                    status_char: 'S',
                });
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
        self.set_mode(crate::Mode::GitStatus);
    }

    /// Toggle inline diff expansion for the file at cursor.
    pub fn git_toggle_section(&mut self) {
        let win = self.window_mgr.focused_window();
        let cursor_row = win.cursor_row;
        let idx = self.active_buffer_idx();

        let path = self.buffers[idx]
            .git_status_view()
            .and_then(|v| v.lines.get(cursor_row))
            .and_then(|line| line.file_path.clone());

        if let Some(p) = path {
            if let Some(view) = self.buffers[idx].git_status_view_mut() {
                view.toggle_file_expansion(&p);
            }
            // Refresh to rebuild with diff lines
            self.git_status();
        }
    }

    /// Discard unstaged changes for the file at cursor.
    pub fn git_discard_file(&mut self) {
        let win = self.window_mgr.focused_window();
        let cursor_row = win.cursor_row;
        let idx = self.active_buffer_idx();

        let (path, section) = self.buffers[idx]
            .git_status_view()
            .and_then(|v| {
                v.lines
                    .get(cursor_row)
                    .and_then(|line| line.file_path.as_ref().map(|p| (p.clone(), line.section)))
            })
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
