//! Project navigation dispatch commands.

use super::super::Editor;

impl Editor {
    /// Dispatch project commands.
    /// Returns `Some(true)` if handled.
    pub(super) fn dispatch_project(&mut self, name: &str) -> Option<bool> {
        match name {
            "open-scheme-repl" => self.open_scheme_repl(),
            "project-find-file" => self.project_find_file(),
            "project-search" => self.project_search(),
            "project-browse" => self.project_browse(),
            "project-recent-files" => self.project_recent_files(),
            "project-switch" => self.project_switch_palette(),
            "project-forget" => self.project_forget_palette(),
            "project-clean" => self.project_clean(),
            _ => return None,
        }
        self.mark_full_redraw();
        Some(true)
    }
}
