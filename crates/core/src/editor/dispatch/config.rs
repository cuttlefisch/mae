//! Configuration, theme, toggle, debug, and font zoom dispatch commands.

use crate::theme::bundled_theme_names;
use crate::Mode;

use super::super::Editor;

impl Editor {
    /// Dispatch configuration, theme, toggle, debug, and font zoom commands.
    /// Returns `Some(true)` if handled.
    pub(super) fn dispatch_config(&mut self, name: &str) -> Option<bool> {
        match name {
            "leader-dispatch" => {
                // Enter the transient keypad/leader layer. Subsequent keys
                // resolve against the shared `leader` keymap (which-key tree);
                // the layer clears after one command or on Esc/C-g (handled in
                // key routing). Base mode is untouched, so it returns to Insert
                // (non-modal) or Normal (doom) automatically.
                self.set_leader_active(true);
                self.clear_which_key_prefix();
                self.set_status("-- leader -- (Esc cancels)".to_string());
                // Lifecycle hook: keypad opened (paired with leader-execute /
                // leader-cancel). Lets users extend keypad behavior (hints,
                // logging, transient UI) without patching the kernel.
                self.fire_hook("leader-open");
            }
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
;; MAE init.scm — Primary configuration surface
;; Run :setup-wizard for interactive configuration.
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
                self.open_setup_hub();
            }
            "setup-ai" => {
                self.command_palette =
                    Some(crate::command_palette::CommandPalette::for_setup_ai_provider());
                self.set_mode(Mode::CommandPalette);
            }
            "setup-collab" => {
                self.command_palette =
                    Some(crate::command_palette::CommandPalette::for_setup_collab_mode());
                self.set_mode(Mode::CommandPalette);
            }
            "setup-kb" => {
                let default_dir = platform_default_notes_dir();
                let current = self
                    .kb
                    .notes_dir
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| default_dir.display().to_string());
                self.mini_dialog = Some(crate::command_palette::MiniDialogState::single_input(
                    "Notes directory",
                    &current,
                    default_dir.display().to_string(),
                    crate::command_palette::MiniDialogContext::SetupKbNotesDir,
                ));
                self.command_palette =
                    Some(crate::command_palette::CommandPalette::with_name_list(
                        &[],
                        crate::command_palette::PalettePurpose::MiniDialog,
                    ));
                self.set_mode(Mode::CommandPalette);
            }
            "setup-daemon" => {
                let new_val = !self.kb.daemon_enabled;
                let _ = self.set_option("daemon_enabled", &new_val.to_string());
                let _ = self.save_option_to_init("daemon_enabled");
                let msg = if new_val {
                    // #347: this command only flips config — it does not itself
                    // start a process. Point at both the in-editor path
                    // (:collab-start, no shell needed) and the systemd/launchd
                    // path (persists across editor restarts) so a first-time user
                    // doesn't read "enabled" and assume the daemon is now running.
                    let hint = if cfg!(target_os = "macos") {
                        "Run :collab-start now, or persist it: brew services start mae (or launchctl)"
                    } else {
                        "Run :collab-start now, or persist it: systemctl --user enable --now mae-daemon"
                    };
                    format!("Daemon enabled (config only). {}", hint)
                } else {
                    "Daemon disabled.".to_string()
                };
                self.set_status(msg);
                self.refresh_setup_hub();
            }
            "setup-all" => {
                self.setup_all_pending = true;
                self.dispatch_next_setup_section();
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
                let cur = self.inline_images_for(idx);
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
            // Live reload of ALL modules (the "__all__" sentinel is handled in
            // the event loop). `mae-reload` is an alias.
            "reload-modules" | "mae-reload" => {
                self.pending_module_reloads.push("__all__".to_string());
                self.set_status("Reloading all modules...".to_string());
            }
            // Live keymap-flavor switch. With an arg (`:keymap-set-flavor
            // nonmodal`) switch to it; with none (leader binding) toggle
            // doom↔nonmodal. The "__flavor:<name>" sentinel is handled in the
            // event loop (needs the SchemeRuntime to reload modules).
            "choose-keymap-flavor" => {
                // Guided picker (dashboard quick-action): explains each flavor.
                self.command_palette =
                    Some(crate::command_palette::CommandPalette::for_keymap_flavor());
                self.set_mode(Mode::CommandPalette);
            }
            // "kb-set-search-scope" kept as a deprecated alias — see its
            // registration in commands.rs.
            "kb-set-scope" | "kb-set-search-scope" => {
                // Guided picker for the default KB search scope; lists keyword
                // scopes plus each registered instance.
                let names: Vec<&str> = self
                    .kb
                    .registry
                    .instances
                    .iter()
                    .filter(|i| !i.primary)
                    .map(|i| i.name.as_str())
                    .collect();
                self.command_palette = Some(
                    crate::command_palette::CommandPalette::for_kb_search_scope(&names),
                );
                self.set_mode(Mode::CommandPalette);
            }
            "keymap-set-flavor" => {
                let arg = self.vi.command_line.trim().to_string();
                let target = if !arg.is_empty() {
                    arg
                } else if self.keymap_flavor == "doom" {
                    "nonmodal".to_string()
                } else {
                    "doom".to_string()
                };
                self.pending_module_reloads
                    .push(format!("__flavor:{target}"));
                self.set_status(format!("Switching to keymap flavor '{target}'..."));
            }

            _ => return None,
        }
        self.mark_full_redraw();
        Some(true)
    }

    /// Generate and display the `*Setup*` hub buffer.
    fn open_setup_hub(&mut self) {
        let content = self.generate_setup_content();
        // Reuse existing *Setup* buffer or create new
        let existing = self.buffers.iter().position(|b| b.name == "*Setup*");
        let buf_idx = if let Some(idx) = existing {
            self.buffers[idx].replace_contents(&content);
            self.buffers[idx].modified = false;
            idx
        } else {
            let mut buf = crate::buffer::Buffer::new();
            buf.name = "*Setup*".to_string();
            buf.replace_contents(&content);
            buf.modified = false;
            buf.read_only = true;
            let idx = self.buffers.len();
            self.buffers.push(buf);
            idx
        };
        self.display_buffer(buf_idx);
    }

    /// Refresh the `*Setup*` buffer if it exists.
    pub fn refresh_setup_hub(&mut self) {
        let content = self.generate_setup_content();
        if let Some(idx) = self.buffers.iter().position(|b| b.name == "*Setup*") {
            self.buffers[idx].replace_contents(&content);
            self.buffers[idx].modified = false;
        }
    }

    /// Generate the setup hub buffer content.
    fn generate_setup_content(&self) -> String {
        let ai_status = if self.ai.provider.is_empty() {
            "not configured".to_string()
        } else {
            let model_part = if self.ai.model.is_empty() {
                String::new()
            } else {
                format!(" ({})", self.ai.model)
            };
            format!("{}{}", self.ai.provider, model_part)
        };

        let theme_status = self.theme.name.clone();

        let collab_status = if self.collab.auto_connect {
            format!(
                "{} ({})",
                self.collab.status.as_str(),
                self.collab.server_address
            )
        } else {
            "solo (not configured)".to_string()
        };

        let kb_status = self
            .kb
            .notes_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not set".to_string());

        // Surface the configured mode (ADR-035), not just enabled/disabled.
        let daemon_status = match self.kb.daemon_mode {
            crate::editor::DaemonMode::Off => "off (in-process KB)",
            crate::editor::DaemonMode::OnDemand => "on-demand",
            crate::editor::DaemonMode::Shared => "shared",
        };

        format!(
            "\
MAE Setup
=========

Section              Status                              Command
-------              ------                              -------
AI Provider          {:<35} :setup-ai
Theme                {:<35} :set-theme
Collaboration        {:<35} :setup-collab
KB Notes             {:<35} :setup-kb
Daemon               {:<35} :setup-daemon

  :setup-all    — configure all unconfigured sections
  q             — close this buffer
",
            ai_status, theme_status, collab_status, kb_status, daemon_status
        )
    }

    /// Dispatch the next unconfigured setup section for `:setup-all`.
    pub fn dispatch_next_setup_section(&mut self) {
        if !self.setup_all_pending {
            return;
        }
        if self.ai.provider.is_empty() {
            self.dispatch_builtin("setup-ai");
        } else if !self.collab.auto_connect
            && self.collab.server_address.as_str() == crate::DEFAULT_COLLAB_ADDRESS
        {
            self.dispatch_builtin("setup-collab");
        } else if self.kb.notes_dir.is_none() {
            self.dispatch_builtin("setup-kb");
        } else if !self.kb.daemon_enabled {
            self.dispatch_builtin("setup-daemon");
        } else {
            self.setup_all_pending = false;
            self.set_status("All sections configured!");
            self.refresh_setup_hub();
        }
    }
}

/// Platform-appropriate default notes directory.
fn platform_default_notes_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "~".to_string());
    if cfg!(target_os = "macos") {
        std::path::PathBuf::from(home)
            .join("Documents")
            .join("mae-notes")
    } else {
        std::path::PathBuf::from(home).join("mae-notes")
    }
}
