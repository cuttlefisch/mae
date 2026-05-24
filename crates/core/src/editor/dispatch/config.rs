//! Configuration, theme, toggle, debug, and font zoom dispatch commands.

use crate::theme::bundled_theme_names;
use crate::Mode;

use super::super::Editor;

impl Editor {
    /// Dispatch configuration, theme, toggle, debug, and font zoom commands.
    /// Returns `Some(true)` if handled.
    pub(super) fn dispatch_config(&mut self, name: &str) -> Option<bool> {
        match name {
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
            "reload-config" => {
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
                        Ok(contents) => match contents.parse::<toml::Table>() {
                            Ok(table) => {
                                let mut applied = 0;
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
                        },
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
                let palette = crate::command_palette::CommandPalette::for_splash_art(self);
                self.command_palette = Some(palette);
                self.set_mode(Mode::CommandPalette);
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
            "debug-path" => {
                let path = std::env::var("PATH").unwrap_or_else(|_| "not set".to_string());
                self.set_status(format!("PATH={}", path));
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

            // Describe
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
            "module-reload" => {
                let arg = self.vi.command_line.trim().to_string();
                if arg.is_empty() {
                    self.set_status("Usage: :module-reload <name>".to_string());
                } else {
                    self.pending_module_reloads.push(arg.clone());
                    self.set_status(format!("Reloading module '{}'...", arg));
                }
            }

            _ => return None,
        }
        self.mark_full_redraw();
        Some(true)
    }
}
