//! Git status buffer data model (Phase 6 M5).

use std::collections::HashMap;
use std::path::PathBuf;

/// A section in the git status buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitSection {
    Untracked,
    Unstaged,
    Staged,
    Stashes,
}

/// A line in the git status buffer mapped to an action.
#[derive(Debug, Clone)]
pub struct GitStatusLine {
    pub text: String,
    pub section: Option<GitSection>,
    pub file_path: Option<String>,
    /// If Some, this line represents a specific hunk (start, count)
    pub hunk: Option<(usize, usize)>,
    pub is_header: bool,
    pub is_collapsed: bool,
}

/// Structured state for the `*Git Status*` buffer.
#[derive(Debug, Clone)]
pub struct GitStatusView {
    pub lines: Vec<GitStatusLine>,
    /// Which sections/files are currently collapsed.
    pub collapsed_paths: HashMap<String, bool>,
    /// Root directory of the repository.
    pub repo_root: PathBuf,
}

impl GitStatusView {
    pub fn new(repo_root: PathBuf) -> Self {
        GitStatusView {
            lines: Vec::new(),
            collapsed_paths: HashMap::new(),
            repo_root,
        }
    }
}
