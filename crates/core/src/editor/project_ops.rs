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
        self.file_picker = Some(FilePicker::scan(
            &root,
            self.file_picker_max_depth,
            self.file_picker_max_candidates,
        ));
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
        self.vi.command_line = format!("grep {} ", root);
        self.vi.command_cursor = self.vi.command_line.len();
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
            let proj = crate::project::Project::from_root(path.clone());
            self.project_list.touch(path.clone(), proj.name.clone());
            self.project = Some(proj);
            self.refresh_git_branch();
            self.lsp.pending_root_change = Some(format!("file://{}", path.display()));
            self.save_project_list();
            self.set_status(format!("Added & switched to project: {}", path.display()));
        } else {
            self.set_status(format!("Not a directory: {}", path_str));
        }
    }

    /// `remove-project` — remove a directory from recent projects.
    pub fn remove_project(&mut self, path_str: &str) {
        let path = std::path::PathBuf::from(crate::file_picker::expand_tilde(path_str));
        let before = self.recent_projects.len();
        self.recent_projects.remove(&path);
        self.project_list.remove(&path);
        if self.recent_projects.len() < before {
            self.save_project_list();
            self.set_status(format!("Removed project: {}", path.display()));
        } else {
            self.set_status(format!("Project not found: {}", path_str));
        }
    }

    /// `project-clean` — prune subprojects and missing entries from the project list.
    pub(crate) fn project_clean(&mut self) {
        let before: Vec<String> = self
            .project_list
            .projects
            .iter()
            .map(|e| e.root.display().to_string())
            .collect();
        self.project_list.prune_subprojects();
        self.project_list.prune_missing();
        let after: std::collections::HashSet<String> = self
            .project_list
            .projects
            .iter()
            .map(|e| e.root.display().to_string())
            .collect();
        let removed: Vec<&str> = before
            .iter()
            .filter(|p| !after.contains(p.as_str()))
            .map(|s| s.as_str())
            .collect();
        // Sync back to in-memory recent_projects
        self.recent_projects = crate::project::RecentProjects::default();
        self.project_list.sync_to_recent(&mut self.recent_projects);
        self.save_project_list();
        if removed.is_empty() {
            self.set_status(format!(
                "Project list clean: {} projects, nothing removed",
                after.len()
            ));
        } else {
            self.set_status(format!(
                "Removed {} project(s): {}",
                removed.len(),
                removed.join(", ")
            ));
        }
    }

    /// `project-forget` — open palette to select a project to remove.
    pub(crate) fn project_forget_palette(&mut self) {
        let roots: Vec<String> = self
            .project_list
            .sorted()
            .iter()
            .map(|e| e.root.display().to_string())
            .collect();
        if roots.is_empty() {
            self.set_status("No projects to forget");
            return;
        }
        let name_refs: Vec<&str> = roots.iter().map(|s| s.as_str()).collect();
        self.command_palette = Some(CommandPalette::for_forget_project(&name_refs));
        self.set_mode(Mode::CommandPalette);
    }

    /// Best-effort save of `projects.toml` to XDG data dir.
    fn save_project_list(&self) {
        if let Some(data_dir) = self.mae_data_dir() {
            let _ = self.project_list.save(&data_dir);
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
