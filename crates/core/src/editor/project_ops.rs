//! Project management commands (SPC p group).

use crate::command_palette::CommandPalette;
use crate::file_picker::FilePicker;
use crate::Mode;

use super::Editor;

impl Editor {
    /// `project-find-file` — open file picker rooted at the project root.
    pub(crate) fn project_find_file(&mut self) {
        let root = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        self.file_picker = Some(FilePicker::scan(&root));
        self.set_mode(Mode::FilePicker);
    }

    /// `project-browse` — open directory browser at project root.
    pub(crate) fn project_browse(&mut self) {
        let root = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        self.file_browser = Some(crate::FileBrowser::open(&root));
        self.set_mode(Mode::FileBrowser);
    }

    /// `project-search` — interactive grep in project root, results in scratch buffer.
    pub(crate) fn project_search(&mut self) {
        self.set_mode(Mode::Command);
        let root = self
            .project
            .as_ref()
            .map(|p| p.root.display().to_string())
            .unwrap_or_else(|| ".".to_string());
        self.command_line = format!("grep {} ", root);
        self.command_cursor = self.command_line.len();
        self.set_status("Project search: enter pattern");
    }

    /// `project-recent-files` — palette with recent files filtered to current project.
    pub(crate) fn project_recent_files(&mut self) {
        let files: Vec<String> = if let Some(ref proj) = self.project {
            self.recent_files
                .filter_by_dir(&proj.root)
                .iter()
                .map(|p| p.display().to_string())
                .collect()
        } else {
            self.recent_files
                .list()
                .iter()
                .map(|p| p.display().to_string())
                .collect()
        };
        if files.is_empty() {
            self.set_status("No recent files");
            return;
        }
        let name_refs: Vec<&str> = files.iter().map(|s| s.as_str()).collect();
        self.command_palette = Some(CommandPalette::for_recent_files(&name_refs));
        self.set_mode(Mode::CommandPalette);
    }

    /// `project-switch` — palette with recently used project roots.
    /// Opens even when empty so the user can type a new project path.
    pub(crate) fn project_switch_palette(&mut self) {
        let roots: Vec<String> = self
            .recent_projects
            .list()
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        let name_refs: Vec<&str> = roots.iter().map(|s| s.as_str()).collect();
        self.command_palette = Some(CommandPalette::for_project_switch(&name_refs));
        self.set_mode(Mode::CommandPalette);
    }

    /// `add-project` — add a directory to recent projects and switch to it.
    pub fn add_project(&mut self, path_str: &str) {
        let path = std::path::PathBuf::from(crate::file_picker::expand_tilde(path_str));
        if path.is_dir() {
            self.recent_projects.push(path.clone());
            self.project = Some(crate::project::Project::from_root(path.clone()));
            self.refresh_git_branch();
            self.set_status(format!("Added & switched to project: {}", path.display()));
        } else {
            self.set_status(format!("Not a directory: {}", path_str));
        }
    }

    /// `remove-project` — remove a directory from recent projects.
    pub(crate) fn remove_project(&mut self, path_str: &str) {
        let path = std::path::PathBuf::from(path_str);
        let before = self.recent_projects.len();
        self.recent_projects.remove(&path);
        if self.recent_projects.len() < before {
            self.set_status(format!("Removed project: {}", path_str));
        } else {
            self.set_status(format!("Project not found: {}", path_str));
        }
    }

    /// `recent-files` — palette with all recent files.
    pub(crate) fn recent_files_palette(&mut self) {
        let files: Vec<String> = self
            .recent_files
            .list()
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        if files.is_empty() {
            self.set_status("No recent files");
            return;
        }
        let name_refs: Vec<&str> = files.iter().map(|s| s.as_str()).collect();
        self.command_palette = Some(CommandPalette::for_recent_files(&name_refs));
        self.set_mode(Mode::CommandPalette);
    }
}
