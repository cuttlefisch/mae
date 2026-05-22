use crate::options::{parse_option_bool, parse_option_int, OptionKind};

impl super::Editor {
    pub fn set_local_option(&mut self, name: &str, value: &str) -> Result<String, String> {
        let def_name = self
            .option_registry
            .find(name)
            .map(|d| d.name.clone())
            .ok_or_else(|| format!("Unknown option: {}", name))?;
        let idx = self.active_buffer_idx();
        let opts = &mut self.buffers[idx].local_options;
        match def_name.as_ref() {
            "word_wrap" => {
                opts.word_wrap = Some(crate::options::parse_option_bool(value)?);
                self.buffers[idx].visual_rows_cache = None;
            }
            "line_numbers" => {
                opts.line_numbers = Some(crate::options::parse_option_bool(value)?);
            }
            "relative_line_numbers" => {
                opts.relative_line_numbers = Some(crate::options::parse_option_bool(value)?);
            }
            "break_indent" => {
                opts.break_indent = Some(crate::options::parse_option_bool(value)?);
                self.buffers[idx].visual_rows_cache = None;
            }
            "show_break" => {
                opts.show_break = Some(value.to_string());
                self.buffers[idx].visual_rows_cache = None;
            }
            "heading_scale" => {
                opts.heading_scale = Some(crate::options::parse_option_bool(value)?);
                self.buffers[idx].visual_rows_cache = None;
            }
            "link_descriptive" => {
                opts.link_descriptive = Some(crate::options::parse_option_bool(value)?);
            }
            "render_markup" => {
                opts.render_markup = Some(crate::options::parse_option_bool(value)?);
            }
            _ => {
                return Err(format!(
                    "Option '{}' does not support buffer-local override",
                    def_name
                ))
            }
        }
        Ok(format!("{} = {} (buffer-local)", def_name, value))
    }

    /// Get the current value and definition of an option by name or alias.
    pub fn get_option(&self, name: &str) -> Option<(String, &crate::options::OptionDef)> {
        let def = self.option_registry.find(name)?;
        let value = match def.name.as_ref() {
            "line_numbers" => self.show_line_numbers.to_string(),
            "relative_line_numbers" => self.relative_line_numbers.to_string(),
            "word_wrap" => self.word_wrap.to_string(),
            "break_indent" => self.break_indent.to_string(),
            "show_break" => self.show_break.clone(),
            "org_hide_emphasis_markers" => self.org_hide_emphasis_markers.to_string(),
            "show_fps" => self.show_fps.to_string(),
            "font_size" => self.gui_font_size.to_string(),
            "font_family" => self.gui_font_family.clone(),
            "icon_font_family" => self.gui_icon_font_family.clone(),
            "theme" => self.theme.name.clone(),
            "splash_art" => self.splash_art.clone().unwrap_or_default(),
            "splash_image_width" => self.splash_image_width.to_string(),
            "splash_image_height" => self.splash_image_height.to_string(),
            "splash_show_logo" => self.splash_show_logo.to_string(),
            "debug_mode" => self.debug_mode.to_string(),
            "clipboard" => self.clipboard.clone(),
            "ai_tier" => self.ai.permission_tier.clone(),
            "ai_editor" => self.ai.editor_name.clone(),
            "ai_provider" => self.ai.provider.clone(),
            "ai_model" => self.ai.model.clone(),
            "ai_api_key_command" => self.ai.api_key_command.clone(),
            "ai_base_url" => self.ai.base_url.clone(),
            "ai_mode" => self.ai.mode.clone(),
            "ai_profile" => self.ai.profile.clone(),
            "restore_session" => self.restore_session.to_string(),
            "insert_ctrl_d" => self.insert_ctrl_d.clone(),
            "heading_scale" => self.heading_scale.to_string(),
            "ignorecase" => self.ignorecase.to_string(),
            "smartcase" => self.smartcase.to_string(),
            "autosave_interval" => self.autosave_interval.to_string(),
            "swap_file" => self.swap_file.to_string(),
            "swap_directory" => self.swap_directory.clone(),
            "scrolloff" => self.scrolloff.to_string(),
            "scrollbar" => self.scrollbar.to_string(),
            "nyan_mode" => self.nyan_mode.to_string(),
            "link_descriptive" => self.link_descriptive.to_string(),
            "render_markup" => self.render_markup.to_string(),
            "lsp_hover_popup" => self.lsp_hover_popup.to_string(),
            "lsp_diagnostics_inline" => self.lsp_diagnostics_inline.to_string(),
            "lsp_diagnostics_virtual_text" => self.lsp_diagnostics_virtual_text.to_string(),
            "lsp_completion" => self.lsp_completion.to_string(),
            "auto_complete" => self.auto_complete.to_string(),
            "show_breadcrumbs" => self.show_breadcrumbs.to_string(),
            "mouse_autoselect_window" => self.mouse_autoselect_window.to_string(),
            "mouse_wheel_follow_mouse" => self.mouse_wheel_follow_mouse.to_string(),
            "scroll_speed" => self.scroll_speed.to_string(),
            "completion_max_items" => self.completion_max_items.to_string(),
            "hover_max_lines" => self.hover_max_lines.to_string(),
            "popup_width_pct" => self.popup_width_pct.to_string(),
            "popup_height_pct" => self.popup_height_pct.to_string(),
            "scrollbar_width" => self.scrollbar_width.to_string(),
            "file_picker_max_depth" => self.file_picker_max_depth.to_string(),
            "file_picker_max_candidates" => self.file_picker_max_candidates.to_string(),
            "window_title" => self.window_title.clone(),
            "heading_scale_h1" => self.heading_scale_h1.to_string(),
            "heading_scale_h2" => self.heading_scale_h2.to_string(),
            "heading_scale_h3" => self.heading_scale_h3.to_string(),
            "dashboard_dismiss_on_split" => self
                .replaceable_kinds
                .contains(&crate::BufferKind::Dashboard)
                .to_string(),
            "large_file_lines" => self.large_file_lines.to_string(),
            "degrade_threshold_chars" => self.degrade_threshold_chars.to_string(),
            "degrade_threshold_line_length" => self.degrade_threshold_line_length.to_string(),
            "display_region_debounce_ms" => self.display_region_debounce_ms.to_string(),
            "syntax_reparse_debounce_ms" => self.syntax_reparse_debounce_ms.to_string(),
            "org_agenda_files" => self.org_agenda_files.join(", "),
            "kb_watcher_enabled" => self.kb.watcher_enabled.to_string(),
            "kb_watcher_debounce_ms" => self.kb.watcher_debounce_ms.to_string(),
            "kb_max_drain_events" => self.kb.max_drain_events.to_string(),
            "kb_search_excerpt_length" => self.kb.search_excerpt_length.to_string(),
            "kb_search_max_results" => self.kb.search_max_results.to_string(),
            "kb_auto_register" => self.kb.auto_register.to_string(),
            "kb_notes_dir" => self
                .kb
                .notes_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            "kb_activity_tracking" => self.kb.activity_tracking.to_string(),
            "kb_activity_decay" => self.kb.activity_decay.to_string(),
            "kb_search_sort" => self.kb.search_sort.clone(),
            "kb_dailies_dir" => self
                .kb
                .dailies_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            "kb_daily_chain_gap_max" => self.kb.daily_chain_gap_max.to_string(),
            "format_on_save" => self.format_on_save.to_string(),
            "spell_enabled" => self.spell_enabled.to_string(),
            "file_tree_focus_on_open" => self.file_tree_focus_on_open.to_string(),
            "collab_server_address" => self.collab.server_address.clone(),
            "collab_auto_connect" => self.collab.auto_connect.to_string(),
            "collab_auto_share" => self.collab.auto_share.to_string(),
            "collab_reconnect_interval" => self.collab.reconnect_interval.to_string(),
            "collab_user_name" => self.collab.user_name.clone(),
            "collab_write_timeout_ms" => self.collab.write_timeout_ms.to_string(),
            "collab_max_pending_updates" => self.collab.max_pending_updates.to_string(),
            "collab_reconnect_backoff_factor" => self.collab.reconnect_backoff_factor.to_string(),
            "collab_max_reconnect_attempts" => self.collab.max_reconnect_attempts.to_string(),
            "collab_batch_update_ms" => self.collab.batch_update_ms.to_string(),
            "collab_auto_resolve_paths" => self.collab.auto_resolve_paths.to_string(),
            "collab_default_save_dir" => self.collab.default_save_dir.clone(),
            "collab_save_on_remote_update" => self.collab.save_on_remote_update.to_string(),
            "collab_heartbeat_interval" => self.collab.heartbeat_interval.to_string(),
            "fill_column" => self.fill_column.to_string(),
            _ => return None,
        };
        Some((value, def))
    }

    /// Set an option by name or alias, returning a confirmation message.
    pub fn set_option(&mut self, name: &str, value: &str) -> Result<String, String> {
        let def_name = self
            .option_registry
            .find(name)
            .map(|d| d.name.clone())
            .ok_or_else(|| format!("Unknown option: {}", name))?;
        match def_name.as_ref() {
            "line_numbers" => {
                self.show_line_numbers = parse_option_bool(value)?;
            }
            "relative_line_numbers" => {
                self.relative_line_numbers = parse_option_bool(value)?;
            }
            "word_wrap" => {
                self.word_wrap = parse_option_bool(value)?;
                for buf in &mut self.buffers {
                    buf.visual_rows_cache = None;
                }
            }
            "break_indent" => {
                self.break_indent = parse_option_bool(value)?;
                for buf in &mut self.buffers {
                    buf.visual_rows_cache = None;
                }
            }
            "show_break" => {
                self.show_break = value.to_string();
                for buf in &mut self.buffers {
                    buf.visual_rows_cache = None;
                }
            }
            "org_hide_emphasis_markers" => {
                self.org_hide_emphasis_markers = parse_option_bool(value)?;
            }
            "show_fps" => {
                self.show_fps = parse_option_bool(value)?;
            }
            "font_size" => {
                let size: f32 = value
                    .parse()
                    .map_err(|_| format!("Invalid float: '{}'", value))?;
                if !(6.0..=72.0).contains(&size) {
                    return Err("Font size must be between 6 and 72".into());
                }
                self.gui_font_size = size;
                self.gui_font_size_default = size;
            }
            "font_family" => {
                self.gui_font_family = value.to_string();
            }
            "icon_font_family" => {
                self.gui_icon_font_family = value.to_string();
            }
            "theme" => {
                self.set_theme_by_name(value);
            }
            "splash_art" => {
                self.splash_art = Some(value.to_string());
            }
            "splash_image_width" => {
                let v: u32 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.splash_image_width = v.clamp(10, 80);
            }
            "splash_image_height" => {
                let v: u32 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.splash_image_height = v.clamp(5, 50);
            }
            "splash_show_logo" => {
                self.splash_show_logo = parse_option_bool(value)?;
            }
            "debug_mode" => {
                self.debug_mode = parse_option_bool(value)?;
                if self.debug_mode {
                    self.show_fps = true;
                }
            }
            "clipboard" => match value {
                "unnamedplus" | "unnamed" | "internal" => {
                    self.clipboard = value.to_string();
                }
                _ => {
                    return Err(format!(
                        "Invalid clipboard mode: '{}' (expected unnamedplus, unnamed, or internal)",
                        value
                    ))
                }
            },
            "ai_tier" => match value {
                "ReadOnly" | "Write" | "Shell" | "Privileged" => {
                    self.ai.permission_tier = value.to_string();
                }
                _ => {
                    return Err(format!(
                        "Invalid AI tier: '{}' (expected ReadOnly, Write, Shell, or Privileged)",
                        value
                    ))
                }
            },
            "ai_editor" => {
                self.ai.editor_name = value.to_string();
            }
            "ai_provider" => {
                self.ai.provider = value.to_string();
            }
            "ai_model" => {
                self.ai.model = value.to_string();
            }
            "ai_api_key_command" => {
                self.ai.api_key_command = value.to_string();
            }
            "ai_base_url" => {
                self.ai.base_url = value.to_string();
            }
            "ai_mode" => {
                let valid = ["standard", "plan", "auto-accept"];
                if !valid.contains(&value) {
                    return Err(format!(
                        "Invalid AI mode: '{}' (expected: standard, plan, auto-accept)",
                        value
                    ));
                }
                self.ai.mode = value.to_string();
            }
            "ai_profile" => {
                self.ai.profile = value.to_string();
            }
            "restore_session" => {
                self.restore_session = parse_option_bool(value)?;
            }
            "insert_ctrl_d" => {
                let valid = ["dedent", "delete-forward"];
                if !valid.contains(&value) {
                    return Err(format!(
                        "Invalid insert_ctrl_d: '{}' (expected: dedent, delete-forward)",
                        value
                    ));
                }
                self.insert_ctrl_d = value.to_string();
            }
            "heading_scale" => {
                self.heading_scale = parse_option_bool(value)?;
                for buf in &mut self.buffers {
                    buf.visual_rows_cache = None;
                }
            }
            "ignorecase" => {
                self.ignorecase = parse_option_bool(value)?;
            }
            "smartcase" => {
                self.smartcase = parse_option_bool(value)?;
            }
            "autosave_interval" => {
                let secs: u64 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.autosave_interval = secs;
            }
            "swap_file" => {
                self.swap_file = parse_option_bool(value)?;
            }
            "swap_directory" => {
                self.swap_directory = value.to_string();
            }
            "scrolloff" => {
                let n: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.scrolloff = n;
            }
            "scrollbar" => {
                self.scrollbar = parse_option_bool(value)?;
            }
            "nyan_mode" => {
                self.nyan_mode = parse_option_bool(value)?;
            }
            "link_descriptive" => {
                self.link_descriptive = parse_option_bool(value)?;
            }
            "render_markup" => {
                self.render_markup = parse_option_bool(value)?;
            }
            "lsp_hover_popup" => {
                self.lsp_hover_popup = parse_option_bool(value)?;
            }
            "lsp_diagnostics_inline" => {
                self.lsp_diagnostics_inline = parse_option_bool(value)?;
            }
            "lsp_diagnostics_virtual_text" => {
                self.lsp_diagnostics_virtual_text = parse_option_bool(value)?;
            }
            "lsp_completion" => {
                self.lsp_completion = parse_option_bool(value)?;
            }
            "auto_complete" => {
                self.auto_complete = parse_option_bool(value)?;
            }
            "show_breadcrumbs" => {
                self.show_breadcrumbs = parse_option_bool(value)?;
            }
            "mouse_autoselect_window" => {
                self.mouse_autoselect_window = parse_option_bool(value)?;
            }
            "mouse_wheel_follow_mouse" => {
                self.mouse_wheel_follow_mouse = parse_option_bool(value)?;
            }
            "scroll_speed" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.scroll_speed = v.clamp(1, 50);
            }
            "completion_max_items" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.completion_max_items = v.clamp(1, 50);
            }
            "hover_max_lines" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.hover_max_lines = v.clamp(1, 50);
            }
            "popup_width_pct" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.popup_width_pct = v.clamp(10, 100);
            }
            "popup_height_pct" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.popup_height_pct = v.clamp(10, 100);
            }
            "scrollbar_width" => {
                let v: f32 = value
                    .parse()
                    .map_err(|_| format!("Invalid float: '{}'", value))?;
                self.scrollbar_width = v.clamp(1.0, 20.0);
            }
            "file_picker_max_depth" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.file_picker_max_depth = v.clamp(1, 100);
            }
            "file_picker_max_candidates" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.file_picker_max_candidates = v.clamp(100, 500000);
            }
            "window_title" => {
                self.window_title = value.to_string();
            }
            "heading_scale_h1" => {
                let v: f32 = value
                    .parse()
                    .map_err(|_| format!("Invalid float: '{}'", value))?;
                self.heading_scale_h1 = v.clamp(0.5, 3.0);
                for buf in &mut self.buffers {
                    buf.visual_rows_cache = None;
                }
            }
            "heading_scale_h2" => {
                let v: f32 = value
                    .parse()
                    .map_err(|_| format!("Invalid float: '{}'", value))?;
                self.heading_scale_h2 = v.clamp(0.5, 3.0);
                for buf in &mut self.buffers {
                    buf.visual_rows_cache = None;
                }
            }
            "heading_scale_h3" => {
                let v: f32 = value
                    .parse()
                    .map_err(|_| format!("Invalid float: '{}'", value))?;
                self.heading_scale_h3 = v.clamp(0.5, 3.0);
                for buf in &mut self.buffers {
                    buf.visual_rows_cache = None;
                }
            }
            "dashboard_dismiss_on_split" => {
                let val = parse_option_bool(value)?;
                if val {
                    if !self
                        .replaceable_kinds
                        .contains(&crate::BufferKind::Dashboard)
                    {
                        self.replaceable_kinds.push(crate::BufferKind::Dashboard);
                    }
                } else {
                    self.replaceable_kinds
                        .retain(|k| *k != crate::BufferKind::Dashboard);
                }
            }
            "large_file_lines" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.large_file_lines = v.clamp(100, 1_000_000);
            }
            "degrade_threshold_chars" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.degrade_threshold_chars = v.clamp(10_000, 100_000_000);
            }
            "degrade_threshold_line_length" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.degrade_threshold_line_length = v.clamp(100, 1_000_000);
            }
            "display_region_debounce_ms" => {
                let v: u64 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.display_region_debounce_ms = v.clamp(0, 5000);
            }
            "syntax_reparse_debounce_ms" => {
                let v: u64 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.syntax_reparse_debounce_ms = v.clamp(0, 5000);
            }
            "org_agenda_files" => {
                return Err("Use :agenda-add / :agenda-remove to manage agenda files".to_string());
            }
            "kb_watcher_enabled" => {
                self.kb.watcher_enabled = parse_option_bool(value)?;
            }
            "kb_watcher_debounce_ms" => {
                let v: u64 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.kb.watcher_debounce_ms = v.clamp(0, 60_000);
            }
            "kb_max_drain_events" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.kb.max_drain_events = v.clamp(1, 10_000);
            }
            "kb_search_excerpt_length" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.kb.search_excerpt_length = v.clamp(50, 10_000);
            }
            "kb_search_max_results" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.kb.search_max_results = v.clamp(1, 100);
            }
            "kb_auto_register" => {
                self.kb.auto_register = parse_option_bool(value)?;
            }
            "kb_notes_dir" => {
                if value.is_empty() {
                    self.kb.notes_dir = None;
                } else {
                    let expanded = crate::file_picker::expand_tilde(value);
                    self.kb.notes_dir = Some(std::path::PathBuf::from(expanded));
                }
            }
            "kb_activity_tracking" => {
                self.kb.activity_tracking = parse_option_bool(value)?;
            }
            "kb_activity_decay" => {
                let v: f64 = value
                    .parse()
                    .map_err(|_| format!("Invalid float: '{}'", value))?;
                self.kb.activity_decay = v.clamp(0.0001, 1.0);
            }
            "kb_search_sort" => match value {
                "relevance" | "activity" | "alphabetical" => {
                    self.kb.search_sort = value.to_string();
                }
                _ => {
                    return Err(format!(
                    "Invalid kb_search_sort: '{}' (expected: relevance, activity, alphabetical)",
                    value
                ))
                }
            },
            "kb_dailies_dir" => {
                if value.is_empty() {
                    self.kb.dailies_dir = None;
                } else {
                    let expanded = crate::file_picker::expand_tilde(value);
                    self.kb.dailies_dir = Some(std::path::PathBuf::from(expanded));
                }
            }
            "kb_daily_chain_gap_max" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.kb.daily_chain_gap_max = v.clamp(1, 365);
            }
            "format_on_save" => {
                self.format_on_save = parse_option_bool(value)?;
            }
            "spell_enabled" => {
                self.spell_enabled = parse_option_bool(value)?;
            }
            "file_tree_focus_on_open" => {
                self.file_tree_focus_on_open = parse_option_bool(value)?;
            }
            "collab_server_address" => {
                self.collab.server_address = value.to_string();
            }
            "collab_auto_connect" => {
                self.collab.auto_connect = parse_option_bool(value)?;
            }
            "collab_auto_share" => {
                self.collab.auto_share = parse_option_bool(value)?;
            }
            "collab_reconnect_interval" => {
                let v: u64 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.collab.reconnect_interval = v.clamp(1, 300);
            }
            "collab_user_name" => {
                self.collab.user_name = value.to_string();
            }
            "collab_write_timeout_ms" => {
                let v: u64 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.collab.write_timeout_ms = v.clamp(500, 60_000);
            }
            "collab_max_pending_updates" => {
                self.collab.max_pending_updates = parse_option_int(value)? as u64;
            }
            "collab_reconnect_backoff_factor" => {
                let v = parse_option_int(value)? as u64;
                self.collab.reconnect_backoff_factor = v.clamp(1, 10);
            }
            "collab_max_reconnect_attempts" => {
                self.collab.max_reconnect_attempts = parse_option_int(value)? as u64;
            }
            "collab_batch_update_ms" => {
                self.collab.batch_update_ms = parse_option_int(value)? as u64;
            }
            "collab_auto_resolve_paths" => {
                self.collab.auto_resolve_paths = parse_option_bool(value)?;
            }
            "collab_default_save_dir" => {
                self.collab.default_save_dir = value.to_string();
            }
            "collab_save_on_remote_update" => {
                self.collab.save_on_remote_update = parse_option_bool(value)?;
            }
            "collab_heartbeat_interval" => {
                self.collab.heartbeat_interval = parse_option_int(value)? as u64;
            }
            "fill_column" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.fill_column = v.clamp(20, 200);
            }
            _ => return Err(format!("Unknown option: {}", name)),
        }
        let (current, _) = self
            .get_option(&def_name)
            .ok_or_else(|| format!("internal: option '{}' not found after set", def_name))?;
        // Fire parameterized option-change hook (e.g. "option-change:font_size")
        self.fire_hook(&format!("option-change:{}", def_name));
        Ok(format!("{} = {}", def_name, current))
    }

    /// Persist an option's current value to `~/.config/mae/config.toml`.
    pub fn save_option_to_config(&self, name: &str) -> Result<String, String> {
        let (value, def) = self
            .get_option(name)
            .ok_or_else(|| format!("Unknown option: {}", name))?;
        let config_key = def
            .config_key
            .as_deref()
            .ok_or_else(|| format!("Option '{}' cannot be saved to config", def.name))?;

        let config_dir = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            std::path::PathBuf::from(xdg).join("mae")
        } else if let Ok(home) = std::env::var("HOME") {
            std::path::PathBuf::from(home).join(".config").join("mae")
        } else {
            return Err("Cannot determine config directory".into());
        };
        let config_path = config_dir.join("config.toml");

        // Read existing config as a TOML table, or start fresh
        let mut table: toml::Table = if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .map_err(|e| format!("Failed to read config: {}", e))?;
            content
                .parse::<toml::Table>()
                .map_err(|e| format!("Failed to parse config: {}", e))?
        } else {
            std::fs::create_dir_all(&config_dir)
                .map_err(|e| format!("Failed to create config dir: {}", e))?;
            toml::Table::new()
        };

        // Parse config_key like "editor.line_numbers" into section + key
        let parts: Vec<&str> = config_key.splitn(2, '.').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid config key: {}", config_key));
        }
        let (section_name, key_name) = (parts[0], parts[1]);

        // Ensure the section table exists
        if !table.contains_key(section_name) {
            table.insert(
                section_name.to_string(),
                toml::Value::Table(toml::Table::new()),
            );
        }
        let section = table
            .get_mut(section_name)
            .and_then(|v| v.as_table_mut())
            .ok_or_else(|| format!("Config key '{}' is not a table", section_name))?;

        // Set the value with the appropriate TOML type, validating the parse.
        let toml_val = match def.kind {
            OptionKind::Bool => {
                let b: bool = value
                    .parse()
                    .map_err(|_| format!("Invalid bool: '{}'", value))?;
                toml::Value::Boolean(b)
            }
            OptionKind::Int => {
                let i: i64 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                toml::Value::Integer(i)
            }
            OptionKind::Float => {
                let f: f64 = value
                    .parse()
                    .map_err(|_| format!("Invalid float: '{}'", value))?;
                toml::Value::Float(f)
            }
            OptionKind::String | OptionKind::Theme => toml::Value::String(value.clone()),
        };
        section.insert(key_name.to_string(), toml_val);

        let output = toml::to_string_pretty(&table)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        std::fs::write(&config_path, output)
            .map_err(|e| format!("Failed to write config: {}", e))?;

        Ok(format!(
            "Saved {} = {} to {}",
            def.name,
            value,
            config_path.display()
        ))
    }

    /// Open a scratch `*Options*` buffer listing all options with current values.
    pub fn show_all_options(&mut self) {
        let mut lines = Vec::new();
        lines.push("Editor Options".to_string());
        lines.push("==============".to_string());
        lines.push(String::new());
        lines.push(format!(
            "{:<25} {:<10} {:<15} {}",
            "Option", "Type", "Current", "Default"
        ));
        lines.push(format!(
            "{:<25} {:<10} {:<15} {}",
            "------", "----", "-------", "-------"
        ));
        for def in self.option_registry.list() {
            let current = match self.get_option(&def.name) {
                Some((v, _)) => v,
                None => "?".to_string(),
            };
            lines.push(format!(
                "{:<25} {:<10} {:<15} {}",
                def.name, def.kind, current, def.default_value
            ));
        }
        lines.push(String::new());
        lines.push(
            "Use :set <option> <value> to change, :set <option> to toggle booleans.".to_string(),
        );
        lines.push("Use :set-save <option> [value] to persist to config.toml.".to_string());
        lines.push("Use :describe-option <name> or SPC h o for documentation.".to_string());

        let content = lines.join("\n");
        let mut buf = crate::buffer::Buffer::new();
        buf.name = "*Options*".to_string();
        buf.replace_contents(&content);
        buf.modified = false;
        buf.read_only = true;

        let buf_idx = self.buffers.len();
        self.buffers.push(buf);
        self.display_buffer(buf_idx);
    }

    /// Show active modules in a read-only buffer.
    ///
    /// - No argument: summary table with description column + totals.
    /// - With argument (`:describe-module org`): full detail for one module.
    pub fn show_module_report(&mut self, module_name: Option<&str>) {
        if let Some(name) = module_name {
            self.show_module_detail(name);
        } else {
            self.show_module_summary();
        }
    }

    fn show_module_summary(&mut self) {
        let mut lines = Vec::new();
        lines.push("Active Modules".to_string());
        lines.push("==============".to_string());
        lines.push(String::new());

        if self.active_modules.is_empty() {
            lines.push("No modules loaded.".to_string());
        } else {
            lines.push(format!(
                "{:<20} {:<10} {:<10} {:<12} {}",
                "Module", "Version", "Status", "Category", "Description"
            ));
            lines.push(format!(
                "{:<20} {:<10} {:<10} {:<12} {}",
                "------", "-------", "------", "--------", "-----------"
            ));
            let mut loaded = 0usize;
            let mut failed = 0usize;
            for m in &self.active_modules {
                lines.push(format!(
                    "{:<20} {:<10} {:<10} {:<12} {}",
                    m.name, m.version, m.status, m.category, m.description
                ));
                if m.status == "loaded" {
                    loaded += 1;
                } else if m.status.starts_with("failed") {
                    failed += 1;
                }
            }
            lines.push(String::new());
            lines.push(format!(
                "Total: {} modules ({} loaded, {} failed)",
                self.active_modules.len(),
                loaded,
                failed
            ));
            lines.push(String::new());
            lines.push("Press Enter on a module name to see details.".to_string());
        }

        let content = lines.join("\n");
        let mut buf = crate::buffer::Buffer::new();
        buf.name = "*Modules*".to_string();
        buf.replace_contents(&content);
        buf.modified = false;
        buf.read_only = true;
        buf.kind = crate::buffer::BufferKind::Modules;

        let buf_idx = self.buffers.len();
        self.buffers.push(buf);
        self.display_buffer(buf_idx);
    }

    fn show_module_detail(&mut self, name: &str) {
        let m = match self.active_modules.iter().find(|m| m.name == name) {
            Some(m) => m.clone(),
            None => {
                self.set_status(format!("Module '{}' not found", name));
                return;
            }
        };

        let mut lines = Vec::new();
        lines.push(format!(
            "Module: {}  v{}  [{}]",
            m.name, m.version, m.status
        ));
        lines.push("================================".to_string());
        lines.push(m.description.clone());
        lines.push(String::new());

        lines.push(format!("{:<16}{}", "Category:", m.category));
        lines.push(format!("{:<16}{}", "Path:", m.path));
        if m.depends.is_empty() {
            lines.push(format!("{:<16}(none)", "Dependencies:"));
        } else {
            lines.push(format!("{:<16}{}", "Dependencies:", m.depends.join(", ")));
        }
        lines.push(String::new());

        // Flags section
        if !m.flags.is_empty() {
            lines.push("Flags:".to_string());
            for (flag, doc) in &m.flags {
                let enabled = m.enabled_flags.contains(&format!("+{}", flag));
                let tag = if enabled { "[enabled]" } else { "[disabled]" };
                lines.push(format!("  +{:<14} {:<40} {}", flag, doc, tag));
            }
            lines.push(String::new());
        }

        // Commands section
        lines.push(format!("Commands ({}):", m.commands.len()));
        if m.commands.is_empty() {
            lines.push("  (none)".to_string());
        } else {
            // Show in rows of 4
            for chunk in m.commands.chunks(4) {
                lines.push(format!("  {}", chunk.join(", ")));
            }
        }
        lines.push(String::new());

        // Options section
        lines.push(format!("Options ({}):", m.options.len()));
        if m.options.is_empty() {
            lines.push("  (none)".to_string());
        } else {
            for opt in &m.options {
                lines.push(format!("  {}", opt));
            }
        }
        lines.push(String::new());

        // Keybindings section — look up keymap with same name as module
        if let Some(km) = self.keymaps.get(&m.name) {
            let bindings: Vec<_> = km.bindings().collect();
            lines.push(format!(
                "Keybindings ({} keymap, {} bindings):",
                m.name,
                bindings.len()
            ));
            let mut sorted: Vec<_> = bindings
                .iter()
                .map(|(seq, cmd)| (crate::keymap::format_key_seq(seq), (*cmd).clone()))
                .collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            for (key, cmd) in &sorted {
                lines.push(format!("  {:<20} {}", key, cmd));
            }
        } else {
            lines.push("Keybindings: (no dedicated keymap)".to_string());
        }

        let content = lines.join("\n");
        let mut buf = crate::buffer::Buffer::new();
        buf.name = format!("*Module: {}*", m.name);
        buf.replace_contents(&content);
        buf.modified = false;
        buf.read_only = true;

        let buf_idx = self.buffers.len();
        self.buffers.push(buf);
        self.display_buffer(buf_idx);
    }

    pub fn show_kb_health_report(&mut self) {
        let mut report = self.kb.primary.health_report();
        report.stale_nodes = self.kb.primary.detect_stale_nodes();
        let mut lines = Vec::new();
        lines.push("KB Health Report".to_string());
        lines.push("================".to_string());
        lines.push(String::new());
        lines.push(format!("Total nodes: {}", report.total_nodes));
        lines.push(format!("Total links: {}", report.total_links));
        if report.total_nodes > 0 {
            lines.push(format!(
                "Avg links/node: {:.1}",
                report.total_links as f64 / report.total_nodes as f64
            ));
        }
        lines.push(String::new());

        // Namespace counts.
        lines.push("Namespace Counts".to_string());
        lines.push("----------------".to_string());
        let mut ns: Vec<_> = report.namespace_counts.iter().collect();
        ns.sort_by(|a, b| b.1.cmp(a.1));
        for (name, count) in &ns {
            lines.push(format!("  {:<20} {}", name, count));
        }
        lines.push(String::new());

        // Orphan nodes.
        lines.push(format!("Orphan Nodes ({})", report.orphan_ids.len()));
        lines.push("------------".to_string());
        if report.orphan_ids.is_empty() {
            lines.push("  (none)".to_string());
        } else {
            for id in &report.orphan_ids {
                lines.push(format!("  {}", id));
            }
        }
        lines.push(String::new());

        // Broken links — grouped by classification.
        lines.push(format!("Broken Links ({})", report.broken_links.len()));
        lines.push("------------".to_string());
        if report.broken_links.is_empty() {
            lines.push("  (none)".to_string());
        } else {
            use mae_kb::BrokenLinkKind;
            let mut deleted: Vec<_> = report
                .broken_links
                .iter()
                .filter(|b| b.kind == BrokenLinkKind::DeletedNode)
                .collect();
            let mut malformed: Vec<_> = report
                .broken_links
                .iter()
                .filter(|b| b.kind == BrokenLinkKind::MalformedId)
                .collect();
            let mut placeholder: Vec<_> = report
                .broken_links
                .iter()
                .filter(|b| b.kind == BrokenLinkKind::TemplatePlaceholder)
                .collect();
            deleted.sort_by_key(|b| &b.target);
            malformed.sort_by_key(|b| &b.target);
            placeholder.sort_by_key(|b| &b.target);
            if !deleted.is_empty() {
                lines.push(format!("  Deleted nodes ({}):", deleted.len()));
                for b in &deleted {
                    let label = if b.display.is_empty() {
                        &b.target
                    } else {
                        &b.display
                    };
                    lines.push(format!(
                        "    {} → {} ({})",
                        b.source,
                        label,
                        &b.target[..8.min(b.target.len())]
                    ));
                }
            }
            if !malformed.is_empty() {
                lines.push(format!("  Malformed IDs ({}):", malformed.len()));
                for b in &malformed {
                    lines.push(format!("    {} → {:?}", b.source, b.target));
                }
            }
            if !placeholder.is_empty() {
                lines.push(format!("  Template placeholders ({}):", placeholder.len()));
                for b in &placeholder {
                    lines.push(format!("    {} → {:?}", b.source, b.target));
                }
            }
        }
        lines.push(String::new());

        // Stale nodes (source file deleted).
        lines.push(format!("Stale Nodes ({})", report.stale_nodes.len()));
        lines.push("-------------------".to_string());
        if report.stale_nodes.is_empty() {
            lines.push("  (none)".to_string());
        } else {
            for s in &report.stale_nodes {
                lines.push(format!(
                    "  {} — {} (was: {})",
                    s.id,
                    s.title,
                    s.source_file.display()
                ));
            }
        }
        lines.push(String::new());

        // Watcher performance metrics.
        let ws = &self.kb.watcher_stats;
        lines.push("Watcher Metrics".to_string());
        lines.push("---------------".to_string());
        lines.push(format!("  Reimports total:     {}", ws.reimports_total));
        lines.push(format!("  Events upserted:     {}", ws.events_upserted));
        lines.push(format!("  Events removed:      {}", ws.events_removed));
        lines.push(format!("  Suppressed debounce: {}", ws.suppressed_debounce));
        lines.push(format!("  Suppressed timebox:  {}", ws.suppressed_timebox));
        lines.push(format!(
            "  Suppressed write-guard: {}",
            ws.events_suppressed
        ));
        lines.push(format!("  Errors:              {}", ws.errors));
        let avg_ms = if ws.drain_count > 0 {
            format!(
                "{:.1}ms",
                ws.drain_us_sum as f64 / ws.drain_count as f64 / 1000.0
            )
        } else {
            "n/a".to_string()
        };
        lines.push(format!("  Avg reimport time:   {}", avg_ms));
        lines.push(format!("  Total drain cycles:  {}", ws.drain_count));

        let content = lines.join("\n");
        let mut buf = crate::buffer::Buffer::new();
        buf.name = "*KB Health*".to_string();
        buf.replace_contents(&content);
        buf.modified = false;
        buf.read_only = true;

        let buf_idx = self.buffers.len();
        self.buffers.push(buf);
        self.display_buffer(buf_idx);
    }

    /// Show current buffer's mode, keymap, and active options.
    pub fn show_mode_report(&mut self) {
        let mode = self.mode;
        let (primary_map, parent_map) = self.current_keymap_names().unwrap_or(("normal", None));

        let buf_idx = self.active_buffer_idx();
        let buf = &self.buffers[buf_idx];
        let buf_name = buf.name.clone();
        let buf_kind = format!("{:?}", buf.kind);
        let buf_modified = buf.modified;
        let buf_read_only = buf.read_only;
        let line_count = buf.line_count();

        let mut lines = Vec::new();
        lines.push("Mode Report".to_string());
        lines.push("===========".to_string());
        lines.push(String::new());
        lines.push(format!("Mode:      {:?}", mode));
        lines.push(format!("Keymap:    {}", primary_map));
        if let Some(parent) = parent_map {
            lines.push(format!("Parent:    {}", parent));
        }
        if let Some(lang) = self.syntax.language_of(buf_idx) {
            lines.push(format!("Language:  {}", lang.id()));
        }
        lines.push(String::new());
        lines.push("Buffer".to_string());
        lines.push("------".to_string());
        lines.push(format!("Name:      {}", buf_name));
        lines.push(format!("Kind:      {}", buf_kind));
        lines.push(format!("Lines:     {}", line_count));
        lines.push(format!("Modified:  {}", buf_modified));
        lines.push(format!("Read-only: {}", buf_read_only));
        lines.push(String::new());

        // Show active modules
        if !self.active_modules.is_empty() {
            lines.push(format!("Modules: {} loaded", self.active_modules.len()));
        }

        // Show a few key options
        lines.push(String::new());
        lines.push("Options (selected)".to_string());
        lines.push("------------------".to_string());
        for name in &[
            "line_numbers",
            "relative_line_numbers",
            "word_wrap",
            "tab_width",
            "theme",
            "auto_complete",
            "show_breadcrumbs",
        ] {
            if let Some((val, _def)) = self.get_option(name) {
                lines.push(format!("  {:<25} = {}", name, val));
            }
        }

        let content = lines.join("\n");
        let mut buf = crate::buffer::Buffer::new();
        buf.name = "*Mode*".to_string();
        buf.replace_contents(&content);
        buf.modified = false;
        buf.read_only = true;

        let buf_idx = self.buffers.len();
        self.buffers.push(buf);
        self.display_buffer(buf_idx);
    }

    /// Show all keybindings for the current mode in a read-only buffer.
    pub fn show_bindings_report(&mut self) {
        use crate::Key;

        fn format_key(kp: &crate::KeyPress) -> String {
            let mut s = String::new();
            if kp.ctrl {
                s.push_str("C-");
            }
            if kp.alt {
                s.push_str("M-");
            }
            match kp.key {
                Key::Char(' ') => s.push_str("SPC"),
                Key::Char(c) => s.push(c),
                Key::Enter => s.push_str("RET"),
                Key::Escape => s.push_str("ESC"),
                Key::Tab => s.push_str("TAB"),
                Key::Backspace => s.push_str("BS"),
                Key::Delete => s.push_str("DEL"),
                Key::Up => s.push_str("Up"),
                Key::Down => s.push_str("Down"),
                Key::Left => s.push_str("Left"),
                Key::Right => s.push_str("Right"),
                Key::Home => s.push_str("Home"),
                Key::End => s.push_str("End"),
                Key::PageUp => s.push_str("PgUp"),
                Key::PageDown => s.push_str("PgDn"),
                ref k => s.push_str(&format!("{:?}", k)),
            }
            s
        }

        fn format_seq(seq: &[crate::KeyPress]) -> String {
            seq.iter().map(format_key).collect::<Vec<_>>().join(" ")
        }

        let (primary_map, parent_map) = match self.current_keymap_names() {
            Some(names) => names,
            None => {
                self.set_status("No keymap for current mode".to_string());
                return;
            }
        };
        let mode = self.mode;

        let mut lines = Vec::new();
        lines.push(format!(
            "Keybindings — {:?} mode (keymap: {})",
            mode, primary_map
        ));
        lines.push("=".repeat(60));
        lines.push(String::new());

        // Collect bindings from the keymap chain (child → parent)
        let mut all_bindings: Vec<(String, String)> = Vec::new();
        let mut visited_maps = Vec::new();
        let mut current_map = Some(primary_map.to_string());
        if current_map.as_deref() != parent_map {
            // If there's a separate parent, we'll traverse to it via the keymap's parent field
        }

        while let Some(map_name) = current_map.take() {
            if visited_maps.contains(&map_name) {
                break;
            }
            if let Some(km) = self.keymaps.get(&map_name) {
                for (seq, cmd) in km.bindings() {
                    let key_str = format_seq(seq);
                    if !all_bindings.iter().any(|(k, _)| k == &key_str) {
                        all_bindings.push((key_str, cmd.clone()));
                    }
                }
                visited_maps.push(map_name);
                current_map = km.parent.clone();
            } else {
                break;
            }
        }

        // Also include parent_map if it wasn't in the chain
        if let Some(pm) = parent_map {
            if !visited_maps.iter().any(|v| v == pm) {
                if let Some(km) = self.keymaps.get(pm) {
                    for (seq, cmd) in km.bindings() {
                        let key_str = format_seq(seq);
                        if !all_bindings.iter().any(|(k, _)| k == &key_str) {
                            all_bindings.push((key_str, cmd.clone()));
                        }
                    }
                }
            }
        }

        all_bindings.sort_by(|a, b| a.0.cmp(&b.0));

        lines.push(format!("{:<30} {}", "Key", "Command"));
        lines.push(format!("{:<30} {}", "---", "-------"));
        for (key, cmd) in &all_bindings {
            lines.push(format!("{:<30} {}", key, cmd));
        }
        lines.push(String::new());
        lines.push(format!("Total: {} bindings", all_bindings.len()));

        let content = lines.join("\n");
        let mut buf = crate::buffer::Buffer::new();
        buf.name = "*Bindings*".to_string();
        buf.replace_contents(&content);
        buf.modified = false;
        buf.read_only = true;

        let buf_idx = self.buffers.len();
        self.buffers.push(buf);
        self.display_buffer(buf_idx);
    }

    /// Generate a configuration health report and open it in a read-only buffer.
    pub fn show_configuration_report(&mut self) {
        fn find_on_path(cmd: &str) -> bool {
            std::process::Command::new("which")
                .arg(cmd)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        let mut lines = vec![
            "MAE Configuration Report".to_string(),
            "========================".to_string(),
            String::new(),
            "AI Agent (SPC a a):".to_string(),
        ];
        let ai_cmd = if self.ai.editor_name.is_empty() {
            "claude"
        } else {
            &self.ai.editor_name
        };
        let ai_found = find_on_path(ai_cmd);
        lines.push(format!(
            "  Command: {:<20} [{}]",
            ai_cmd,
            if ai_found {
                "found on PATH"
            } else {
                "not found"
            }
        ));
        lines.push(String::new());

        // AI Chat
        lines.push("AI Chat (SPC a p):".to_string());
        let provider = if self.ai.provider.is_empty() {
            "(not configured)"
        } else {
            &self.ai.provider
        };
        lines.push(format!("  Provider: {}", provider));
        if !self.ai.model.is_empty() {
            lines.push(format!("  Model: {}", self.ai.model));
        }
        // Check API key from env
        let key_env = match provider {
            "claude" => std::env::var("ANTHROPIC_API_KEY").ok(),
            "openai" => std::env::var("OPENAI_API_KEY").ok(),
            "gemini" => std::env::var("GEMINI_API_KEY").ok(),
            "deepseek" => std::env::var("DEEPSEEK_API_KEY").ok(),
            _ => None,
        };
        if let Some(key) = &key_env {
            let masked = if key.len() > 4 {
                format!("****...{}", &key[key.len() - 4..])
            } else {
                "****".to_string()
            };
            lines.push(format!("  API Key: {}", masked));
        } else if !self.ai.api_key_command.is_empty() {
            lines.push(format!(
                "  API Key: via command `{}`",
                self.ai.api_key_command
            ));
        } else {
            lines.push("  API Key: [not set]".to_string());
        }
        lines.push(String::new());

        // LSP Servers
        lines.push("LSP Servers:".to_string());
        for (lang, cmd) in &[
            ("rust", "rust-analyzer"),
            ("python", "pyright"),
            ("typescript", "typescript-language-server"),
            ("go", "gopls"),
        ] {
            let found = find_on_path(cmd);
            lines.push(format!(
                "  {:<28} [{}]  {}",
                cmd,
                if found {
                    "found on PATH"
                } else {
                    "not found    "
                },
                if found { "✓" } else { "✗" }
            ));
            let _ = lang; // suppress unused
        }
        lines.push(String::new());

        // DAP Adapters
        lines.push("DAP Adapters:".to_string());
        for cmd in &["lldb-dap", "debugpy"] {
            let found = find_on_path(cmd);
            lines.push(format!(
                "  {:<28} [{}]  {}",
                cmd,
                if found {
                    "found on PATH"
                } else {
                    "not found    "
                },
                if found { "✓" } else { "✗" }
            ));
        }
        lines.push(String::new());

        // Init files
        lines.push("Init Files:".to_string());
        // Check user init
        let user_config_dir = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| std::path::PathBuf::from(h).join(".config"))
            });
        if let Some(ref dir) = user_config_dir {
            let user_init = dir.join("mae").join("init.scm");
            let exists = user_init.exists();
            lines.push(format!(
                "  {:<40} [{}]",
                user_init.display(),
                if exists { "found" } else { "not found" }
            ));
        }
        if let Ok(cwd) = std::env::current_dir() {
            let project_init = cwd.join(".mae").join("init.scm");
            let exists = project_init.exists();
            lines.push(format!(
                "  {:<40} [{}]",
                project_init.display(),
                if exists { "found" } else { "not found" }
            ));
        }
        lines.push(String::new());

        // Modified options
        let mut modified = Vec::new();
        for def in self.option_registry.list() {
            if let Some((val, _)) = self.get_option(&def.name) {
                if val != def.default_value.as_ref() {
                    modified.push(def.name.to_string());
                }
            }
        }
        if modified.is_empty() {
            lines.push("Options Modified: (none)".to_string());
        } else {
            lines.push(format!(
                "Options Modified: {} ({})",
                modified.len(),
                modified.join(", ")
            ));
        }

        let content = lines.join("\n");
        let mut buf = crate::buffer::Buffer::new();
        buf.name = "*Configuration*".to_string();
        buf.replace_contents(&content);
        buf.modified = false;
        buf.read_only = true;

        let buf_idx = self.buffers.len();
        self.buffers.push(buf);
        self.display_buffer(buf_idx);
    }
}
