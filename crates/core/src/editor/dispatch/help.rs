//! Help / KB view / tutor dispatch commands.

use crate::Mode;

use super::super::Editor;

impl Editor {
    /// Dispatch help and tutorial commands.
    /// Returns `Some(true)` if handled.
    pub(super) fn dispatch_help(&mut self, name: &str) -> Option<bool> {
        match name {
            "help" => self.open_help_at("index"),
            "help-follow-link" => self.help_follow_link(),
            "help-back" => self.help_back(),
            "help-forward" => self.help_forward(),
            "help-next-link" => self.help_next_link(),
            "help-prev-link" => self.help_prev_link(),
            "help-close" => self.help_close(),
            "help-search" => {
                let mut nodes: Vec<(String, String)> = self
                    .kb
                    .list_ids(None)
                    .iter()
                    .filter(|id| crate::editor::help_ops::is_builtin_node(id))
                    .filter_map(|id| self.kb.get(id).map(|n| (id.clone(), n.title.clone())))
                    .collect();
                if self.kb_search_sort == "activity" {
                    let weights = mae_kb::activity::ActivityWeights {
                        decay: self.kb_activity_decay,
                        ..Default::default()
                    };
                    let today = crate::editor::kb_ops::today_ymd();
                    nodes.sort_by(|a, b| {
                        let sa = self.kb_activity_score_for_id(&a.0, &weights, today);
                        let sb = self.kb_activity_score_for_id(&b.0, &weights, today);
                        sb.partial_cmp(&sa)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| a.0.cmp(&b.0))
                    });
                }
                self.command_palette = Some(
                    crate::command_palette::CommandPalette::for_help_search(&nodes),
                );
                self.set_mode(Mode::CommandPalette);
            }
            "help-reopen" => {
                self.help_reopen();
            }
            "kb-view" => {
                self.help_return_to_view();
            }
            "tutor" => {
                self.open_help_at("tutorial:getting-started");
            }
            _ => return None,
        }
        self.mark_full_redraw();
        Some(true)
    }
}
