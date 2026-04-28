use crate::buffer::Buffer;
use crate::command_palette::CommandPalette;
use crate::theme::bundled_theme_names;
use crate::Mode;

use super::super::Editor;

impl Editor {
    /// Dispatch UI, config, diagnostics, terminal, help, AI, project, and toggle commands.
    /// Returns `Some(true)` if handled.
    pub(super) fn dispatch_ui(&mut self, name: &str) -> Option<bool> {
        match name {
            "view-messages" => {
                self.open_messages_buffer();
            }
            "dashboard" => {
                if let Some(idx) = self
                    .buffers
                    .iter()
                    .position(|b| b.kind == crate::BufferKind::Dashboard)
                {
                    let prev = self.active_buffer_idx();
                    self.alternate_buffer_idx = Some(prev);
                    self.window_mgr.focused_window_mut().buffer_idx = idx;
                } else {
                    let prev = self.active_buffer_idx();
                    self.buffers.push(Buffer::new_dashboard());
                    let idx = self.buffers.len() - 1;
                    self.alternate_buffer_idx = Some(prev);
                    self.window_mgr.focused_window_mut().buffer_idx = idx;
                }
                self.set_mode(Mode::Normal);
            }
            "toggle-scratch-buffer" => {
                let current = self.active_buffer_idx();
                let is_scratch = self.buffers[current].kind == crate::BufferKind::Text
                    && self.buffers[current].name == "[scratch]";
                if is_scratch {
                    let alt = self.alternate_buffer_idx.unwrap_or(0);
                    if alt < self.buffers.len() && alt != current {
                        self.alternate_buffer_idx = Some(current);
                        self.window_mgr.focused_window_mut().buffer_idx = alt;
                        self.sync_mode_to_buffer();
                    }
                } else {
                    if let Some(idx) = self
                        .buffers
                        .iter()
                        .position(|b| b.kind == crate::BufferKind::Text && b.name == "[scratch]")
                    {
                        self.alternate_buffer_idx = Some(current);
                        self.window_mgr.focused_window_mut().buffer_idx = idx;
                    } else {
                        self.buffers.push(Buffer::new());
                        let idx = self.buffers.len() - 1;
                        self.alternate_buffer_idx = Some(current);
                        self.window_mgr.focused_window_mut().buffer_idx = idx;
                    }
                    self.set_mode(Mode::Normal);
                }
            }

            // Help / KB
            "help" => self.open_help_at("index"),
            "help-follow-link" => self.help_follow_link(),
            "help-back" => self.help_back(),
            "help-forward" => self.help_forward(),
            "help-next-link" => self.help_next_link(),
            "help-prev-link" => self.help_prev_link(),
            "help-close" => self.help_close(),
            "help-search" => {
                let nodes: Vec<(String, String)> = self
                    .kb
                    .list_ids(None)
                    .iter()
                    .filter_map(|id| self.kb.get(id).map(|n| (id.clone(), n.title.clone())))
                    .collect();
                self.command_palette = Some(
                    crate::command_palette::CommandPalette::for_help_search(&nodes),
                );
                self.set_mode(Mode::CommandPalette);
            }
            "help-reopen" => {
                self.help_reopen();
            }
            "tutor" => {
                self.open_help_at("tutor:index");
            }

            // Shell / terminal
            "terminal" => {
                let shell_name = format!("*Terminal {}*", self.buffers.len());
                let buf = Buffer::new_shell(shell_name);
                self.buffers.push(buf);
                let idx = self.buffers.len() - 1;
                self.pending_shell_spawns.push(idx);
                self.switch_to_buffer(idx);
                self.set_mode(Mode::ShellInsert);
            }
            "terminal-reset" => {
                let idx = self.active_buffer_idx();
                if self.buffers[idx].kind == crate::BufferKind::Shell {
                    self.pending_shell_resets.push(idx);
                    self.set_status("Terminal reset");
                } else {
                    self.set_status("Not a terminal buffer");
                }
            }
            "shell-normal-mode" => {
                self.set_mode(Mode::Normal);
                self.set_status("Terminal: normal mode");
            }
            "terminal-close" => {
                let idx = self.active_buffer_idx();
                if self.buffers[idx].kind == crate::BufferKind::Shell {
                    self.pending_shell_closes.push(idx);
                    self.set_mode(Mode::Normal);
                } else {
                    self.set_status("Not a terminal buffer");
                }
            }
            "shell-scroll-page-up" => {
                self.pending_shell_scroll = Some(self.viewport_height as i32);
            }
            "shell-scroll-page-down" => {
                self.pending_shell_scroll = Some(-(self.viewport_height as i32));
            }
            "shell-scroll-to-bottom" => {
                self.pending_shell_scroll = Some(0);
            }
            "send-to-shell" => {
                self.send_line_to_shell();
            }
            "send-region-to-shell" => {
                self.send_region_to_shell();
            }

            "command-palette" => {
                self.command_palette = Some(CommandPalette::from_registry(&self.commands));
                self.set_mode(Mode::CommandPalette);
            }

            // AI
            "ai-prompt" => {
                self.open_conversation_buffer();
            }
            "ai-set-mode" => {
                let modes = vec!["standard", "plan", "auto-accept"];
                self.command_palette = Some(CommandPalette::for_ai_mode(&modes));
                self.set_mode(Mode::CommandPalette);
            }
            "ai-set-profile" => {
                let profiles = vec!["pair-programmer", "explorer", "planner", "reviewer"];
                self.command_palette = Some(CommandPalette::for_ai_profile(&profiles));
                self.set_mode(Mode::CommandPalette);
            }
            "ai-cancel" => {
                let status = match self.conversation_mut() {
                    Some(conv) if conv.streaming => {
                        conv.end_streaming();
                        conv.push_system("[cancelled]");
                        "[AI] Cancelled"
                    }
                    Some(_) => "No active AI request to cancel",
                    None => "No AI conversation active",
                };
                self.set_status(status);
                self.ai_cancel_requested = true;
            }

            // Describe
            "describe-key" => {
                self.awaiting_key_description = true;
                self.set_status("Describe key: press a key sequence (Esc to cancel)");
            }
            "describe-command" => {
                self.command_palette = Some(CommandPalette::for_describe(&self.commands));
                self.set_mode(Mode::CommandPalette);
            }
            "describe-option" => {
                self.show_all_options();
            }

            // Theme
            "set-theme" => {
                let names = bundled_theme_names();
                let name_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
                self.command_palette = Some(crate::command_palette::CommandPalette::for_themes(
                    &name_refs,
                ));
                self.set_mode(Mode::CommandPalette);
            }
            "cycle-theme" => {
                self.cycle_theme();
            }
            "set-splash-art" => {
                self.command_palette =
                    Some(crate::command_palette::CommandPalette::for_splash_art());
                self.set_mode(Mode::CommandPalette);
            }

            // +project
            "open-scheme-repl" => self.open_scheme_repl(),
            "project-find-file" => self.project_find_file(),
            "project-search" => self.project_search(),
            "project-browse" => self.project_browse(),
            "project-recent-files" => self.project_recent_files(),
            "project-switch" => self.project_switch_palette(),

            // +notes (KB)
            "kb-find" => {
                let nodes: Vec<(String, String)> = self
                    .kb
                    .list_ids(None)
                    .iter()
                    .filter_map(|id| self.kb.get(id).map(|n| (id.clone(), n.title.clone())))
                    .collect();
                self.command_palette = Some(
                    crate::command_palette::CommandPalette::for_help_search(&nodes),
                );
                self.set_mode(Mode::CommandPalette);
            }
            "kb-save" => {
                self.set_status("Usage: :kb-save <path>");
            }
            "kb-load" => {
                self.set_status("Usage: :kb-load <path>");
            }
            "kb-ingest" => {
                self.set_status("Usage: :kb-ingest <directory>");
            }
            "ai-save" => {
                self.set_status("Usage: :ai-save <path>");
            }
            "ai-load" => {
                self.set_status("Usage: :ai-load <path>");
            }

            // Config
            "edit-config" => {
                let config_dir = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
                    std::path::PathBuf::from(xdg)
                } else if let Ok(home) = std::env::var("HOME") {
                    std::path::PathBuf::from(home).join(".config")
                } else {
                    std::path::PathBuf::from(".config")
                }
                .join("mae");
                let init_path = config_dir.join("init.scm");
                if !init_path.exists() {
                    let _ = std::fs::create_dir_all(&config_dir);
                    let template = "\
;; MAE init.scm — Scheme configuration (loaded after config.toml)
;; This file is the primary config surface. TOML is bootstrap-only.
;;
;; Examples:
;;   (set-option! \"theme\" \"catppuccin-mocha\")
;;   (set-option! \"font_size\" \"16\")
;;   (set-option! \"word_wrap\" \"true\")
;;   (set-option! \"relative_line_numbers\" \"true\")
;;
;; Keybindings:
;;   (define-key \"normal\" \"g c\" \"toggle-comment\")
;;
;; Hooks:
;;   (add-hook! \"buffer-open\" (lambda () (display \"opened!\")))
;;
";
                    let _ = std::fs::write(&init_path, template);
                }
                self.open_file(init_path.display().to_string());
            }
            "setup-wizard" => {
                self.set_status(
                    "Run `mae --init-config --force` from a terminal to re-run the setup wizard. Or use :edit-settings to edit config.toml directly."
                );
            }
            "edit-settings" => {
                let config_path = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
                    std::path::PathBuf::from(xdg)
                } else if let Ok(home) = std::env::var("HOME") {
                    std::path::PathBuf::from(home).join(".config")
                } else {
                    std::path::PathBuf::from(".config")
                }
                .join("mae")
                .join("config.toml");
                self.open_file(config_path.display().to_string());
            }

            // Toggles
            "toggle-line-numbers" => {
                self.show_line_numbers = !self.show_line_numbers;
                self.set_status(format!(
                    "Line numbers: {}",
                    if self.show_line_numbers { "on" } else { "off" }
                ));
            }
            "toggle-relative-line-numbers" => {
                self.relative_line_numbers = !self.relative_line_numbers;
                self.set_status(format!(
                    "Relative line numbers: {}",
                    if self.relative_line_numbers {
                        "on"
                    } else {
                        "off"
                    }
                ));
            }
            "toggle-word-wrap" => {
                self.word_wrap = !self.word_wrap;
                self.set_status(format!(
                    "Word wrap: {}",
                    if self.word_wrap { "on" } else { "off" }
                ));
            }
            "toggle-fps" => {
                self.show_fps = !self.show_fps;
                self.set_status(format!(
                    "FPS overlay: {}",
                    if self.show_fps { "on" } else { "off" }
                ));
            }
            "debug-mode" => {
                self.debug_mode = !self.debug_mode;
                if self.debug_mode {
                    self.show_fps = true;
                }
                self.set_status(format!(
                    "Debug mode: {}",
                    if self.debug_mode { "on" } else { "off" }
                ));
            }

            // Event recording
            "record-start" => {
                self.event_recorder.start_recording();
                self.set_status("Recording started");
            }
            "record-stop" => {
                self.event_recorder.stop_recording();
                self.set_status(format!(
                    "Recording stopped ({} events)",
                    self.event_recorder.event_count()
                ));
            }

            // Font zoom
            "increase-font-size" => {
                let new_size = (self.gui_font_size + 1.0).min(72.0);
                self.gui_font_size = new_size;
                self.set_status(format!("Font size: {}", new_size));
            }
            "decrease-font-size" => {
                let new_size = (self.gui_font_size - 1.0).max(6.0);
                self.gui_font_size = new_size;
                self.set_status(format!("Font size: {}", new_size));
            }
            "reset-font-size" => {
                self.gui_font_size = self.gui_font_size_default;
                self.set_status(format!(
                    "Font size: {} (default)",
                    self.gui_font_size_default
                ));
            }
            "debug-path" => {
                let path = std::env::var("PATH").unwrap_or_else(|_| "not set".to_string());
                self.set_status(format!("PATH={}", path));
            }

            // AI agent launcher
            "open-ai-agent" => {
                let shell_name = format!("*AI:{}*", self.ai_editor);
                let mut buf = Buffer::new_shell(shell_name);
                buf.agent_shell = true;
                let prev_idx = self.active_buffer_idx();
                self.buffers.push(buf);
                let new_idx = self.buffers.len() - 1;
                self.alternate_buffer_idx = Some(prev_idx);
                self.window_mgr.focused_window_mut().buffer_idx = new_idx;
                let cmd = self.ai_editor.clone();
                self.pending_agent_spawns.push((new_idx, cmd));
                self.set_mode(Mode::ShellInsert);
            }

            _ => return None,
        }
        Some(true)
    }
}
