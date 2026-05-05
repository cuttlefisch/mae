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
                let idx = if let Some(idx) = self
                    .buffers
                    .iter()
                    .position(|b| b.kind == crate::BufferKind::Dashboard)
                {
                    idx
                } else {
                    self.buffers.push(Buffer::new_dashboard());
                    self.buffers.len() - 1
                };
                let prev = self.active_buffer_idx();
                self.alternate_buffer_idx = Some(prev);
                self.display_buffer(idx);
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
                        self.display_buffer(alt);
                        self.sync_mode_to_buffer();
                    }
                } else {
                    let idx =
                        if let Some(idx) = self.buffers.iter().position(|b| {
                            b.kind == crate::BufferKind::Text && b.name == "[scratch]"
                        }) {
                            idx
                        } else {
                            self.buffers.push(Buffer::new());
                            self.buffers.len() - 1
                        };
                    self.alternate_buffer_idx = Some(current);
                    self.display_buffer(idx);
                    self.set_mode(Mode::Normal);
                }
            }

            "show-buffer-keys" => {
                self.buffer_keys_popup = true;
            }

            "file-info" => {
                let idx = self.active_buffer_idx();
                let buf = &self.buffers[idx];
                let total = buf.line_count();
                let row = self.window_mgr.focused_window().cursor_row + 1;
                let pct = (row * 100).checked_div(total).unwrap_or(0);
                let name = buf
                    .file_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| buf.name.clone());
                let modified = if buf.modified { " [+]" } else { "" };
                self.set_status(format!(
                    "\"{}\"{}  line {} of {} --{}%--",
                    name, modified, row, total, pct
                ));
            }

            // Link following (gx / Enter on links in any buffer)
            "open-link-at-cursor" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let row = win.cursor_row;
                let col = win.cursor_col;
                let buf = &self.buffers[idx];

                // Check display regions first (link concealment in text buffers).
                if !buf.display_regions.is_empty() {
                    let line_chars: Vec<char> = buf
                        .rope()
                        .line(row)
                        .chars()
                        .filter(|c| *c != '\n' && *c != '\r')
                        .collect();
                    let line_byte_start = buf.rope().char_to_byte(buf.rope().line_to_char(row));
                    // The cursor col is a rope col — find the matching display region.
                    let cursor_byte = line_byte_start + {
                        let line_str: String = line_chars.iter().collect();
                        line_str
                            .char_indices()
                            .nth(col)
                            .map(|(b, _)| b)
                            .unwrap_or(line_str.len())
                    };
                    if let Some(region) = buf
                        .display_regions
                        .iter()
                        .find(|r| cursor_byte >= r.byte_start && cursor_byte < r.byte_end)
                    {
                        if let Some(ref target) = region.link_target {
                            let target = target.clone();
                            self.handle_link_click(&target);
                            return Some(true);
                        }
                    }
                }

                // Check conversation rendered links first (from markdown stripping)
                if let Some(conv) = buf.conversation() {
                    if let Some(link) = conv.link_at_position(row, col) {
                        let target = link.target.clone();
                        self.handle_link_click(&target);
                        return Some(true);
                    }
                }

                // Then check buffer link_spans (populated by renderer for conversation/shell)
                if !buf.link_spans.is_empty() {
                    let line_start_byte = buf.rope().char_to_byte(buf.rope().line_to_char(row));
                    let click_byte = line_start_byte + col;
                    if let Some(link) = buf
                        .link_spans
                        .iter()
                        .find(|s| click_byte >= s.byte_start && click_byte < s.byte_end)
                    {
                        let target = link.target.clone();
                        self.handle_link_click(&target);
                        return Some(true);
                    }
                }

                // Fall back: detect links in current line, find one containing cursor col
                let line_text: String = buf.rope().line(row).chars().collect();
                let links = crate::link_detect::detect_links(&line_text);
                for link in &links {
                    let link_char_start = line_text[..link.byte_start].chars().count();
                    let link_char_end = line_text[..link.byte_end].chars().count();
                    if col >= link_char_start && col < link_char_end {
                        let target = link.target.clone();
                        self.handle_link_click(&target);
                        return Some(true);
                    }
                }
                self.set_status("No link under cursor");
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
                self.open_help_at("tutorial:getting-started");
            }

            // Shell / terminal
            "terminal" => {
                let shell_name = format!("*Terminal {}*", self.buffers.len());
                let buf = Buffer::new_shell(shell_name);
                self.buffers.push(buf);
                let idx = self.buffers.len() - 1;
                self.pending_shell_spawns.push(idx);
                self.display_buffer_and_focus(idx);
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
            "ai-prompt" | "ai-chat" => {
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
            "describe-configuration" => {
                self.show_configuration_report();
            }
            "describe-display-policy" => {
                let report = self.display_policy.format_report();
                let mut buf = crate::buffer::Buffer::new();
                buf.name = "*Display Policy*".to_string();
                buf.replace_contents(&report);
                buf.modified = false;
                buf.read_only = true;
                let buf_idx = self.buffers.len();
                self.buffers.push(buf);
                self.display_buffer(buf_idx);
            }
            "reload-config" => {
                // Reload config.toml — parse as TOML table and apply known editor options.
                // This lives in mae-core so we can't import the mae crate's Config struct.
                // Instead we read the raw TOML and extract [editor] keys.
                let config_path = std::env::var("XDG_CONFIG_HOME")
                    .ok()
                    .map(std::path::PathBuf::from)
                    .or_else(|| {
                        std::env::var("HOME")
                            .ok()
                            .map(|h| std::path::PathBuf::from(h).join(".config"))
                    })
                    .unwrap_or_else(|| std::path::PathBuf::from(".config"))
                    .join("mae")
                    .join("config.toml");
                if !config_path.exists() {
                    self.set_status("No config.toml found");
                } else {
                    match std::fs::read_to_string(&config_path) {
                        Ok(contents) => {
                            match contents.parse::<toml::Table>() {
                                Ok(table) => {
                                    let mut applied = 0;
                                    // Apply [editor] section options
                                    if let Some(editor_table) =
                                        table.get("editor").and_then(|v| v.as_table())
                                    {
                                        for (key, val) in editor_table {
                                            let val_str = match val {
                                                toml::Value::String(s) => s.clone(),
                                                toml::Value::Boolean(b) => b.to_string(),
                                                toml::Value::Integer(i) => i.to_string(),
                                                toml::Value::Float(f) => f.to_string(),
                                                _ => continue,
                                            };
                                            let _ = self.set_option(key, &val_str);
                                            applied += 1;
                                        }
                                    }
                                    // Also re-evaluate init.scm
                                    let init_path = config_path
                                        .parent()
                                        .unwrap_or(std::path::Path::new("."))
                                        .join("init.scm");
                                    if init_path.exists() {
                                        self.pending_scheme_eval
                                            .push(format!("(load \"{}\")", init_path.display()));
                                    }
                                    self.set_status(format!(
                                        "Configuration reloaded ({} options + init.scm)",
                                        applied
                                    ));
                                }
                                Err(e) => {
                                    self.set_status(format!("Config parse error: {}", e));
                                }
                            }
                        }
                        Err(e) => {
                            self.set_status(format!("Failed to read config: {}", e));
                        }
                    }
                }
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
            "kb-rebuild" => {
                self.kb = crate::kb_seed::seed_kb(&self.commands, &self.keymaps, &self.hooks);
                let count = self.kb.list_ids(None).len();
                self.set_status(format!("KB rebuilt: {} nodes", count));
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
                // Toggle per-buffer (setlocal). Flips the effective value.
                let new_val = !self.effective_word_wrap();
                let idx = self.active_buffer_idx();
                self.buffers[idx].local_options.word_wrap = Some(new_val);
                self.buffers[idx].visual_rows_cache = None;
                self.set_status(format!(
                    "Word wrap: {} (buffer-local)",
                    if new_val { "on" } else { "off" }
                ));
            }
            "toggle-inline-images" => {
                let idx = self.active_buffer_idx();
                let cur = self.buffers[idx]
                    .local_options
                    .inline_images
                    .unwrap_or(false);
                let new_val = !cur;
                self.buffers[idx].local_options.inline_images = Some(new_val);
                self.buffers[idx].collapsed_images.clear();
                // Force display region recompute (bypass debounce).
                self.buffers[idx].display_regions_gen = u64::MAX;
                self.buffers[idx].display_regions_dirty_since = None;
                self.set_status(format!(
                    "Inline images: {}",
                    if new_val { "on" } else { "off" }
                ));
            }
            "toggle-image-at-point" => {
                let idx = self.active_buffer_idx();
                let row = self.window_mgr.focused_window().cursor_row;
                // Check if this line has an image region.
                let has_image = self.buffers[idx].display_regions.iter().any(|r| {
                    r.image.is_some() && {
                        let line_num = self.buffers[idx].rope().byte_to_line(r.byte_start);
                        line_num == row
                    }
                });
                if has_image {
                    if self.buffers[idx].collapsed_images.contains(&row) {
                        self.buffers[idx].collapsed_images.remove(&row);
                        self.set_status("Image expanded");
                    } else {
                        self.buffers[idx].collapsed_images.insert(row);
                        self.set_status("Image collapsed");
                    }
                    self.buffers[idx].display_regions_gen = u64::MAX;
                    self.buffers[idx].display_regions_dirty_since = None;
                } else {
                    self.set_status("No image at cursor line");
                }
            }
            "image-info-at-point" => {
                let idx = self.active_buffer_idx();
                let row = self.window_mgr.focused_window().cursor_row;
                let image_path = self.buffers[idx]
                    .display_regions
                    .iter()
                    .find_map(|r| {
                        r.image.as_ref().map(|img| {
                            let text: String = self.buffers[idx].rope().chars().collect();
                            let line_num =
                                text[..r.byte_start].chars().filter(|&c| c == '\n').count();
                            (line_num, img.path.clone())
                        })
                    })
                    .and_then(|(line_num, path)| if line_num == row { Some(path) } else { None });
                match image_path {
                    Some(path) => {
                        let meta = std::fs::metadata(&path);
                        match meta {
                            Ok(m) => {
                                let size_kb = m.len() / 1024;
                                self.set_status(format!(
                                    "Image: {} ({}KB)",
                                    path.display(),
                                    size_kb
                                ));
                            }
                            Err(e) => {
                                self.set_status(format!("Image error: {}", e));
                            }
                        }
                    }
                    None => {
                        self.set_status("No image at cursor line");
                    }
                }
            }
            "terminal-here" => {
                // Open terminal in current buffer's file directory.
                let idx = self.active_buffer_idx();
                let cwd = self.buffers[idx]
                    .file_path()
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                    .or_else(|| self.project.as_ref().map(|p| p.root.clone()));
                if let Some(dir) = cwd {
                    let shell_name = format!("*Terminal {}*", self.buffers.len());
                    let buf = Buffer::new_shell(shell_name);
                    self.buffers.push(buf);
                    let shell_idx = self.buffers.len() - 1;
                    self.pending_shell_spawns.push(shell_idx);
                    self.pending_shell_cwds.insert(shell_idx, dir.clone());
                    self.display_buffer_and_focus(shell_idx);
                    self.set_mode(Mode::ShellInsert);
                    self.set_status(format!("Terminal: {}", dir.display()));
                } else {
                    // Fall back to regular terminal.
                    self.dispatch_builtin("terminal");
                }
            }
            "toggle-scrollbar" => {
                self.scrollbar = !self.scrollbar;
                self.set_status(format!(
                    "Scrollbar: {}",
                    if self.scrollbar { "on" } else { "off" }
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
                self.buffers.push(buf);
                let new_idx = self.buffers.len() - 1;
                self.display_buffer_and_focus(new_idx);
                let cmd = self.ai_editor.clone();
                self.pending_agent_spawns.push((new_idx, cmd));
                self.set_mode(Mode::ShellInsert);
            }

            // Agenda
            "open-agenda" => {
                self.open_agenda(crate::agenda_view::AgendaFilter::default());
            }
            "agenda-goto" => {
                self.agenda_goto();
            }
            "agenda-refresh" => {
                self.agenda_refresh();
            }
            "agenda-filter-todo" => {
                self.agenda_filter_todo();
            }
            "agenda-filter-priority" => {
                self.agenda_filter_priority();
            }

            // Demo buffers
            "open-demo-tables" => {
                self.open_demo("Tables", DEMO_TABLES);
            }
            "open-demo-markup" => {
                self.open_demo("Markup", DEMO_MARKUP);
            }
            "open-demo-agenda" => {
                self.open_demo("Agenda", DEMO_AGENDA);
            }

            // Edit a link under cursor: open a mini-dialog with URL + Label fields.
            "edit-link" => {
                use crate::command_palette::{
                    MiniDialogContext, MiniDialogField, MiniDialogKind, MiniDialogState,
                    PalettePurpose,
                };
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let row = win.cursor_row;
                let col = win.cursor_col;
                let buf = &self.buffers[idx];

                // Compute cursor byte offset
                let line_byte_start = buf.rope().char_to_byte(buf.rope().line_to_char(row));
                let line_chars: Vec<char> = buf
                    .rope()
                    .line(row)
                    .chars()
                    .filter(|c| *c != '\n' && *c != '\r')
                    .collect();
                let line_str: String = line_chars.iter().collect();
                let cursor_byte = line_byte_start
                    + line_str
                        .char_indices()
                        .nth(col)
                        .map(|(b, _)| b)
                        .unwrap_or(line_str.len());

                // Find link region at cursor, or the next link region
                let region = buf
                    .display_regions
                    .iter()
                    .find(|r| {
                        r.link_target.is_some()
                            && cursor_byte >= r.byte_start
                            && cursor_byte < r.byte_end
                    })
                    .or_else(|| {
                        crate::display_region::next_link_region(&buf.display_regions, cursor_byte)
                            .and_then(|(s, _)| {
                                buf.display_regions
                                    .iter()
                                    .find(|r| r.link_target.is_some() && r.byte_start == s)
                            })
                    });

                if let Some(region) = region {
                    // Extract raw link text from the buffer
                    let raw_text: String = buf
                        .rope()
                        .byte_slice(region.byte_start..region.byte_end)
                        .chars()
                        .collect();
                    let is_org = buf
                        .file_path()
                        .and_then(|p| p.extension())
                        .and_then(|e| e.to_str())
                        .map(|e| e.eq_ignore_ascii_case("org"))
                        .unwrap_or(false);

                    let (url, label) = if is_org {
                        crate::display_region::parse_org_link(&raw_text)
                            .map(|(u, l)| (u, l.unwrap_or_default()))
                            .unwrap_or_else(|| (raw_text.clone(), String::new()))
                    } else {
                        crate::display_region::parse_md_link(&raw_text)
                            .unwrap_or_else(|| (raw_text.clone(), String::new()))
                    };

                    let state = MiniDialogState {
                        kind: MiniDialogKind::EditLink,
                        fields: vec![
                            MiniDialogField {
                                label: "URL".to_string(),
                                value: url,
                                placeholder: "https://...".to_string(),
                            },
                            MiniDialogField {
                                label: "Label".to_string(),
                                value: label,
                                placeholder: "Link text".to_string(),
                            },
                        ],
                        active_field: 0,
                        context: MiniDialogContext::LinkEdit {
                            buf_idx: idx,
                            byte_start: region.byte_start,
                            byte_end: region.byte_end,
                            is_org,
                        },
                    };
                    self.mini_dialog = Some(state);
                    // Open an empty palette in MiniDialog mode — renderers check mini_dialog
                    self.command_palette = Some(crate::command_palette::CommandPalette {
                        query: String::new(),
                        entries: Vec::new(),
                        filtered: Vec::new(),
                        selected: 0,
                        purpose: PalettePurpose::MiniDialog,
                    });
                    self.set_mode(Mode::CommandPalette);
                    self.set_status("Edit link — Tab: next field, Enter: apply, Esc: cancel");
                } else {
                    self.set_status("No link at cursor");
                }
            }

            _ => return None,
        }
        self.mark_full_redraw();
        Some(true)
    }

    fn open_demo(&mut self, label: &str, content: &str) {
        let name = format!("*Demo: {}*", label);
        let buf_idx = if let Some(idx) = self.find_buffer_by_name(&name) {
            idx
        } else {
            let mut buf = Buffer::new();
            buf.name = name;
            buf.kind = crate::BufferKind::Demo;
            buf.read_only = false;
            self.buffers.push(buf);
            let idx = self.buffers.len() - 1;
            self.buffers[idx].insert_text_at(0, content);
            self.buffers[idx].modified = false;
            idx
        };
        self.display_buffer_and_focus(buf_idx);
    }
}

const DEMO_TABLES: &str = "\
* Demo: Tables
  This is an interactive demo. Edit freely — changes won't be saved.
  Press q to close.

** Org Table
| Name    | Age | City       |
|---------+-----+------------|
| Alice   |  30 | New York   |
| Bob     |  25 | London     |
| Charlie |  35 | Tokyo      |

  Try: Tab to move between cells, S-Tab to go back.
  Try: SPC m b a to align columns after editing.

** Markdown Table
| Language | Typing     | GC   |
|----------|------------|------|
| Rust     | Static     | None |
| Go       | Static     | Yes  |
| Python   | Dynamic    | Yes  |
";

const DEMO_MARKUP: &str = "\
* Demo: Markup
  This is an interactive demo. Edit freely — changes won't be saved.

** Text Formatting
  *bold text* and /italic text/ and =verbatim= and ~code~
  +strikethrough text+

** Blockquotes
> This is a blockquote.
> It can span multiple lines.
>> Nested blockquotes work too.

** Horizontal Rules
-----

** Headings with TODO and Priority
*** TODO [#A] Urgent task                                      :work:urgent:
*** DONE [#C] Completed task                                   :personal:

** Lists
- Unordered item 1
- Unordered item 2
  - Nested item
- [ ] Checkbox unchecked
- [x] Checkbox checked

1. Ordered item 1
2. Ordered item 2

** Links
  See [[concept:buffer]] for buffer docs.
  External: https://example.com
";

const DEMO_AGENDA: &str = "\
* Demo: Agenda & TODO
  This is an interactive demo. Edit freely — changes won't be saved.
  Run :agenda to see these items in the agenda view.

** TODO [#A] Fix critical bug in parser                        :bug:urgent:
** TODO [#B] Write unit tests for table module                 :testing:
** NEXT [#B] Review pull request from contributor              :review:
** WAIT Waiting on upstream API change                         :blocked:
** DONE [#C] Update documentation for v0.7                     :docs:
** TODO Implement smart list continuation                      :feature:
";
