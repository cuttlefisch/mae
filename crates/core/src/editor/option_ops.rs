use crate::options::{parse_option_bool, parse_option_int};

/// Parse a `notify_route_*` option value into a notification [`Surface`].
fn parse_notify_surface(value: &str) -> Result<crate::notifications::Surface, String> {
    crate::notifications::Surface::parse(value).ok_or_else(|| {
        format!("Invalid notify surface '{value}' (expected status|badge|modal|buffer|silent)")
    })
}

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
        self.fire_hook(&format!("option-change:{}", def_name));
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
            "keymap_flavor" => self.keymap_flavor.clone(),
            "kb_link_follow_mode" => self.kb_link_follow_mode.clone(),
            "default_mode" => self.default_mode.clone(),
            "splash_art" => self.splash_art.clone().unwrap_or_default(),
            "splash_image_width" => self.splash_image_width.to_string(),
            "splash_image_height" => self.splash_image_height.to_string(),
            "splash_show_logo" => self.splash_show_logo.to_string(),
            "debug_mode" => self.debug_mode.to_string(),
            "clipboard" => self.clipboard.clone(),
            "ai_tier" => self.ai.permission_tier.clone(),
            "ai_editor" => self.ai.editor_name.clone(),
            "ai_agent_login_shell" => self.ai.agent_login_shell.to_string(),
            "ai_provider" => self.ai.provider.clone(),
            "ai_model" => self.ai.model.clone(),
            "ai_api_key_command" => self.ai.api_key_command.clone(),
            "ai_base_url" => self.ai.base_url.clone(),
            "ai_mode" => self.ai.mode.clone(),
            "ai_profile" => self.ai.profile.clone(),
            "ai_thinking" => self.ai.thinking.clone(),
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
            "babel_confirm" => self.babel_confirm.to_string(),
            "babel_timeout" => self.babel_timeout.to_string(),
            "babel_inherit_shell_env" => self.babel_inherit_shell_env.to_string(),
            "babel_cxx_compiler" => self.babel_cxx_compiler.clone(),
            "babel_c_compiler" => self.babel_c_compiler.clone(),
            "babel_cxx_std" => self.babel_cxx_std.clone(),
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
            "kb_search_scope" => self.kb.search_scope.clone(),
            "kb_dailies_dir" => self
                .kb
                .dailies_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            "kb_daily_chain_gap_max" => self.kb.daily_chain_gap_max.to_string(),
            "format_on_save" => self.format_on_save.to_string(),
            "spell_enabled" => self.spell_enabled.to_string(),
            "ai_chat_enabled" => self.ai_chat_enabled.to_string(),
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
            "collab_command_queue_size" => self.collab.command_queue_size.to_string(),
            "collab_force_sync_debounce_secs" => self.collab.force_sync_debounce_secs.to_string(),
            "collab_daemon_start_grace_ms" => self.collab.daemon_start_grace_ms.to_string(),
            "collab_host_key_prompt_timeout_secs" => {
                self.collab.host_key_prompt_timeout_secs.to_string()
            }
            "collab_auto_resolve_paths" => self.collab.auto_resolve_paths.to_string(),
            "collab_default_save_dir" => self.collab.default_save_dir.clone(),
            "collab_save_on_remote_update" => self.collab.save_on_remote_update.to_string(),
            "collab_heartbeat_interval" => self.collab.heartbeat_interval.to_string(),
            "collab_kb_sync_mode" => self.collab.kb_sync_mode.clone(),
            "collab_fence_resolution" => self.collab.fence_resolution.clone(),
            "collab_psk" => {
                if self.collab.psk.is_empty() {
                    String::new()
                } else {
                    "********".to_string()
                }
            }
            "collab_psk_command" => self.collab.psk_command.clone(),
            "collab_auth_mode" => self.collab.auth_mode.clone(),
            "collab_host_key_policy" => self.collab.host_key_policy.clone(),
            "collab_tls" => self.collab.tls.to_string(),
            "daemon_mode" => self.kb.daemon_mode.as_str().to_string(),
            "daemon_enabled" => self.kb.daemon_enabled.to_string(),
            "daemon_socket" => self.kb.daemon_socket.display().to_string(),
            "daemon_cache_size" => self.kb.daemon_cache_size.to_string(),
            "daemon_default" => self.kb.daemon_default.to_string(),
            "fill_column" => self.fill_column.to_string(),
            "notify_route_info" => self.notifications.route_info.as_str().to_string(),
            "notify_route_success" => self.notifications.route_success.as_str().to_string(),
            "notify_route_warning" => self.notifications.route_warning.as_str().to_string(),
            "notify_route_error" => self.notifications.route_error.as_str().to_string(),
            "notify_route_action_required" => self
                .notifications
                .route_action_required
                .as_str()
                .to_string(),
            "notify_badge_min_severity" => {
                self.notifications.badge_min_severity.as_str().to_string()
            }
            "which_key_idle_delay" => self.which_key_idle_delay.to_string(),
            "kb_preview_idle_delay" => self.kb_preview_idle_delay.to_string(),
            "kb_preview_on_hover" => self.kb_preview_on_hover.to_string(),
            "kb_preview_max_lines" => self.kb_preview_max_lines.to_string(),
            "kb_graph_default_depth" => self.kb_graph_default_depth.to_string(),
            "kb_graph_include_backlinks" => self.kb_graph_include_backlinks.to_string(),
            "kb_graph_node_radius" => self.kb_graph_node_radius.to_string(),
            "kb_graph_node_size_by_degree" => self.kb_graph_node_size_by_degree.to_string(),
            "kb_graph_node_degree_scale" => self.kb_graph_node_degree_scale.to_string(),
            "kb_graph_node_size_scales_with_zoom" => {
                self.kb_graph_node_size_scales_with_zoom.to_string()
            }
            "kb_graph_node_zoom_scale_exponent" => {
                self.kb_graph_node_zoom_scale_exponent.to_string()
            }
            "kb_graph_node_min_radius" => self.kb_graph_node_min_radius.to_string(),
            "kb_graph_node_max_radius" => self.kb_graph_node_max_radius.to_string(),
            "kb_graph_label_zoom_threshold" => self.kb_graph_label_zoom_threshold.to_string(),
            "kb_graph_edge_curvature" => self.kb_graph_edge_curvature.to_string(),
            "kb_graph_color_tween_enabled" => self.kb_graph_color_tween_enabled.to_string(),
            "kb_graph_color_tween_duration_ms" => self.kb_graph_color_tween_duration_ms.to_string(),
            "kb_graph_node_border_enabled" => self.kb_graph_node_border_enabled.to_string(),
            "kb_graph_font_size" => self.kb_graph_font_size.to_string(),
            "kb_graph_layout_iterations" => self.kb_graph_layout_iterations.to_string(),
            "kb_graph_layout_kind_clustering" => self.kb_graph_layout_kind_clustering.to_string(),
            "kb_graph_follow_current_node" => self.kb_graph_follow_current_node.to_string(),
            "kb_graph_animate" => self.kb_graph_animate.to_string(),
            "kb_graph_hover_enabled" => self.kb_graph_hover_enabled.to_string(),
            "kb_graph_view_overlay_dim_opacity" => {
                self.kb_graph_view_overlay_dim_opacity.to_string()
            }
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
            "keymap_flavor" => {
                // Selects the `keymap-<flavor>` module loaded at startup. Set in
                // init.scm/the mae! block — module loading reads this before
                // autoloads run, so changing it at runtime requires :reload-modules.
                self.keymap_flavor = value.to_string();
            }
            "default_mode" => {
                // Set by the keymap flavor's autoloads; bootstrap applies it
                // (set_mode) after modules + config load.
                self.default_mode = value.to_string();
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
            "kb_link_follow_mode" => match value {
                "kb-view" | "source-file" => {
                    self.kb_link_follow_mode = value.to_string();
                }
                _ => {
                    return Err(format!(
                        "Invalid kb_link_follow_mode: '{}' (expected kb-view or source-file)",
                        value
                    ))
                }
            },
            "ai_editor" => {
                self.ai.editor_name = value.to_string();
            }
            "ai_agent_login_shell" => {
                self.ai.agent_login_shell = crate::options::parse_option_bool(value)?;
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
            "ai_thinking" => {
                let valid = ["", "true", "false", "high", "medium", "low"];
                if !valid.contains(&value) {
                    return Err(format!(
                        "Invalid ai_thinking: '{}' (expected: true, false, high, medium, low, or empty for provider default)",
                        value
                    ));
                }
                self.ai.thinking = value.to_string();
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
            // Babel options were registered + persisted but never applied to
            // their editor fields (dead config); wire them here so `:set` works.
            "babel_confirm" => {
                self.babel_confirm = parse_option_bool(value)?;
            }
            "babel_timeout" => {
                let v: u64 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.babel_timeout = v.clamp(1, 3600);
            }
            "babel_inherit_shell_env" => {
                self.babel_inherit_shell_env = parse_option_bool(value)?;
            }
            "babel_cxx_compiler" => {
                self.babel_cxx_compiler = value.to_string();
            }
            "babel_c_compiler" => {
                self.babel_c_compiler = value.to_string();
            }
            "babel_cxx_std" => {
                self.babel_cxx_std = value.to_string();
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
                "relevance" | "activity" | "alphabetical" | "recency" => {
                    self.kb.search_sort = value.to_string();
                }
                _ => {
                    return Err(format!(
                    "Invalid kb_search_sort: '{}' (expected: relevance, activity, alphabetical, recency)",
                    value
                ))
                }
            },
            "kb_search_scope" => {
                // Freeform: "all" / "local" / "remote" / "<instance-name>".
                // A named instance must exist; the keywords always validate.
                let trimmed = value.trim();
                let keyword = matches!(
                    trimmed.to_ascii_lowercase().as_str(),
                    "" | "all" | "local" | "local-only" | "remote" | "remote-only"
                );
                if !keyword && self.kb.registry.find(trimmed).is_none() {
                    return Err(format!(
                        "Invalid kb_search_scope: no KB instance named '{}' (expected: all, local, remote, or a registered instance name)",
                        trimmed
                    ));
                }
                self.kb.search_scope = if trimmed.is_empty() {
                    "all".to_string()
                } else {
                    trimmed.to_string()
                };
            }
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
            "ai_chat_enabled" => {
                self.ai_chat_enabled = parse_option_bool(value)?;
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
            "collab_command_queue_size" => {
                let v = parse_option_int(value)? as u64;
                // Bounded channel capacity: keep at least 1 slot; cap to avoid an
                // unbounded-in-practice queue that would defeat backpressure.
                self.collab.command_queue_size = v.clamp(1, 65_536);
            }
            "collab_force_sync_debounce_secs" => {
                self.collab.force_sync_debounce_secs = parse_option_int(value)? as u64;
            }
            "collab_daemon_start_grace_ms" => {
                let v = parse_option_int(value)? as u64;
                self.collab.daemon_start_grace_ms = v.clamp(0, 60_000);
            }
            "collab_host_key_prompt_timeout_secs" => {
                let v = parse_option_int(value)? as u64;
                // Fail-closed prompt wait; keep a sane floor so it can't be set to
                // effectively "never prompt / instantly reject".
                self.collab.host_key_prompt_timeout_secs = v.clamp(5, 3600);
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
            "collab_kb_sync_mode" => match value {
                "manual" | "on_save" => self.collab.kb_sync_mode = value.to_string(),
                _ => {
                    return Err(format!(
                        "Invalid kb_sync_mode: '{}' (expected 'manual' or 'on_save')",
                        value
                    ))
                }
            },
            "collab_fence_resolution" => match value {
                "prompt" | "auto" => self.collab.fence_resolution = value.to_string(),
                _ => {
                    return Err(format!(
                        "Invalid collab_fence_resolution: '{}' (expected 'prompt' or 'auto')",
                        value
                    ))
                }
            },
            "collab_psk" => {
                self.collab.psk = value.to_string();
            }
            "collab_psk_command" => {
                self.collab.psk_command = value.to_string();
            }
            "collab_auth_mode" => match value {
                "none" | "psk" | "key" => self.collab.auth_mode = value.to_string(),
                _ => {
                    return Err(format!(
                        "Invalid collab_auth_mode: '{value}' (expected 'none', 'psk', or 'key')"
                    ))
                }
            },
            "collab_host_key_policy" => match value {
                "prompt" | "accept-new" | "strict" => {
                    self.collab.host_key_policy = value.to_string();
                    // B-21: propagate to the live cell the background collab task's
                    // host-key verifier reads, so the change is honored on the next
                    // connect without a relaunch.
                    if let Ok(mut p) = self.collab.host_key_policy_live.lock() {
                        *p = value.to_string();
                    }
                }
                _ => {
                    return Err(format!(
                        "Invalid collab_host_key_policy: '{value}' (expected 'prompt', \
                         'accept-new', or 'strict')"
                    ))
                }
            },
            "collab_tls" => {
                self.collab.tls = parse_option_bool(value)?;
            }
            "daemon_mode" => {
                let mode = crate::editor::kb_state::DaemonMode::parse(value).ok_or_else(|| {
                    format!("Invalid daemon_mode '{value}' (expected off|on-demand|shared)")
                })?;
                self.kb.daemon_mode = mode;
                // Keep the runtime connection gate in sync with the configured mode.
                self.kb.daemon_enabled = mode.connects();
            }
            "daemon_enabled" => {
                // Back-compat alias: the bool maps onto the richer daemon_mode
                // (true ⇒ on-demand, false ⇒ off) so both stay consistent.
                let enabled = parse_option_bool(value)?;
                self.kb.daemon_enabled = enabled;
                self.kb.daemon_mode = if enabled {
                    crate::editor::kb_state::DaemonMode::OnDemand
                } else {
                    crate::editor::kb_state::DaemonMode::Off
                };
            }
            "daemon_default" => {
                self.kb.daemon_default = parse_option_bool(value)?;
                // Recompute whether the daemon hosts the primary right now (Phase D).
                self.refresh_daemon_host_state();
            }
            "daemon_socket" => {
                // Empty ⇒ auto-resolve to the daemon's runtime socket (the option's
                // default), so a config with `daemon.socket = ""` / an unset value
                // still connects without a hardcoded path.
                self.kb.daemon_socket = if value.trim().is_empty() {
                    crate::editor::kb_state::default_daemon_socket()
                } else {
                    std::path::PathBuf::from(value)
                };
            }
            "daemon_cache_size" => {
                let v = parse_option_int(value)?;
                if v < 0 {
                    return Err("daemon_cache_size must be non-negative".into());
                }
                self.kb.daemon_cache_size = v as usize;
            }
            "fill_column" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.fill_column = v.clamp(20, 200);
            }
            "notify_route_info" => self.notifications.route_info = parse_notify_surface(value)?,
            "notify_route_success" => {
                self.notifications.route_success = parse_notify_surface(value)?
            }
            "notify_route_warning" => {
                self.notifications.route_warning = parse_notify_surface(value)?
            }
            "notify_route_error" => self.notifications.route_error = parse_notify_surface(value)?,
            "notify_route_action_required" => {
                self.notifications.route_action_required = parse_notify_surface(value)?
            }
            "notify_badge_min_severity" => {
                self.notifications.badge_min_severity =
                    crate::notifications::Severity::parse(value).ok_or_else(|| {
                        format!(
                            "Invalid severity '{}' (expected info|success|warning|error|action-required)",
                            value
                        )
                    })?
            }
            "which_key_idle_delay" => {
                let v: u64 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.which_key_idle_delay = v.clamp(0, 60_000);
            }
            "kb_preview_idle_delay" => {
                let v: u64 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.kb_preview_idle_delay = v.clamp(0, 60_000);
            }
            "kb_preview_on_hover" => {
                self.kb_preview_on_hover = parse_option_bool(value)?;
            }
            "kb_preview_max_lines" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.kb_preview_max_lines = v.clamp(1, 50);
            }
            "kb_graph_default_depth" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.kb_graph_default_depth = v.clamp(0, 10);
            }
            "kb_graph_include_backlinks" => {
                self.kb_graph_include_backlinks = parse_option_bool(value)?;
            }
            "kb_graph_node_radius" => {
                let v: u32 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.kb_graph_node_radius = v.clamp(1, 500);
            }
            "kb_graph_node_size_by_degree" => {
                self.kb_graph_node_size_by_degree = parse_option_bool(value)?;
            }
            "kb_graph_node_degree_scale" => {
                let v: f32 = value
                    .parse()
                    .map_err(|_| format!("Invalid float: '{}'", value))?;
                self.kb_graph_node_degree_scale = v.clamp(0.0, 200.0);
            }
            "kb_graph_node_size_scales_with_zoom" => {
                self.kb_graph_node_size_scales_with_zoom = parse_option_bool(value)?;
            }
            "kb_graph_node_zoom_scale_exponent" => {
                let v: f32 = value
                    .parse()
                    .map_err(|_| format!("Invalid float: '{}'", value))?;
                self.kb_graph_node_zoom_scale_exponent = v.clamp(0.0, 2.0);
            }
            "kb_graph_node_min_radius" => {
                let v: u32 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.kb_graph_node_min_radius = v.clamp(1, 500);
            }
            "kb_graph_node_max_radius" => {
                let v: u32 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.kb_graph_node_max_radius = v.clamp(1, 500);
            }
            "kb_graph_label_zoom_threshold" => {
                let v: f32 = value
                    .parse()
                    .map_err(|_| format!("Invalid float: '{}'", value))?;
                self.kb_graph_label_zoom_threshold = v.clamp(0.0, 10.0);
            }
            "kb_graph_edge_curvature" => {
                let v: f32 = value
                    .parse()
                    .map_err(|_| format!("Invalid float: '{}'", value))?;
                self.kb_graph_edge_curvature = v.clamp(0.0, 1.0);
            }
            "kb_graph_color_tween_enabled" => {
                self.kb_graph_color_tween_enabled = parse_option_bool(value)?;
            }
            "kb_graph_color_tween_duration_ms" => {
                let v: u32 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.kb_graph_color_tween_duration_ms = v.clamp(0, 10_000);
            }
            "kb_graph_node_border_enabled" => {
                self.kb_graph_node_border_enabled = parse_option_bool(value)?;
            }
            "kb_graph_font_size" => {
                let v: u32 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.kb_graph_font_size = v.clamp(1, 200);
            }
            "kb_graph_layout_iterations" => {
                let v: usize = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.kb_graph_layout_iterations = v.clamp(0, 10_000);
            }
            "kb_graph_layout_kind_clustering" => {
                let v: f32 = value
                    .parse()
                    .map_err(|_| format!("Invalid float: '{}'", value))?;
                self.kb_graph_layout_kind_clustering = v.clamp(0.0, 1.0);
            }
            "kb_graph_follow_current_node" => {
                self.kb_graph_follow_current_node = parse_option_bool(value)?;
            }
            "kb_graph_animate" => {
                self.kb_graph_animate = parse_option_bool(value)?;
            }
            "kb_graph_hover_enabled" => {
                self.kb_graph_hover_enabled = parse_option_bool(value)?;
            }
            "kb_graph_view_overlay_dim_opacity" => {
                let v: f32 = value
                    .parse()
                    .map_err(|_| format!("Invalid float: '{}'", value))?;
                self.kb_graph_view_overlay_dim_opacity = v.clamp(0.0, 1.0);
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

    /// Persist an option's current value to `~/.config/mae/init.scm`.
    ///
    /// Writes a `(set-option! "name" "value")` call between sentinel markers.
    /// User's own `(set-option!)` calls outside the markers take precedence
    /// (evaluated after, since Scheme is sequential).
    pub fn save_option_to_init(&self, name: &str) -> Result<String, String> {
        let (value, def) = self
            .get_option(name)
            .ok_or_else(|| format!("Unknown option: {}", name))?;

        let config_dir = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            std::path::PathBuf::from(xdg).join("mae")
        } else if let Ok(home) = std::env::var("HOME") {
            std::path::PathBuf::from(home).join(".config").join("mae")
        } else {
            return Err("Cannot determine config directory".into());
        };

        std::fs::create_dir_all(&config_dir)
            .map_err(|e| format!("Failed to create config dir: {}", e))?;

        let init_path = config_dir.join("init.scm");
        let content = if init_path.exists() {
            std::fs::read_to_string(&init_path)
                .map_err(|e| format!("Failed to read init.scm: {}", e))?
        } else {
            String::new()
        };

        // Escape backslashes and quotes so a value containing either (e.g. a
        // shell command in ai_api_key_command) still writes a valid Scheme
        // string literal instead of corrupting init.scm on next load.
        let escaped_value = value.replace('\\', "\\\\").replace('"', "\\\"");
        let set_line = format!("(set-option! \"{}\" \"{}\")", def.name, escaped_value);
        let pattern = format!("(set-option! \"{}\"", def.name);

        const MARKER_START: &str = ";; --- MAE managed options ---";
        const MARKER_END: &str = ";; --- end managed options ---";

        let new_content = if content.contains(&pattern) {
            // Replace existing line containing this option
            content
                .lines()
                .map(|line| {
                    if line.trim_start().starts_with(&pattern) {
                        set_line.as_str()
                    } else {
                        line
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else if content.contains(MARKER_START) {
            // Append before end marker
            content.replace(MARKER_END, &format!("{}\n{}", set_line, MARKER_END))
        } else {
            // Create managed section
            format!(
                "{}\n\n{}\n{}\n{}\n",
                content.trim_end(),
                MARKER_START,
                set_line,
                MARKER_END,
            )
        };

        std::fs::write(&init_path, new_content)
            .map_err(|e| format!("Failed to write init.scm: {}", e))?;

        Ok(format!(
            "Saved {} = {} to {}",
            def.name,
            value,
            init_path.display()
        ))
    }

    /// Legacy: persist to config.toml. Kept for backward compatibility
    /// with `persist_editor_preference()` in the binary crate.
    pub fn save_option_to_config(&self, name: &str) -> Result<String, String> {
        // Delegate to init.scm — the sole user config surface going forward
        self.save_option_to_init(name)
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
        lines.push("Use :set-save <option> [value] to persist to init.scm.".to_string());
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
        let mut lines = Vec::new();
        lines.push("KB Health Report".to_string());
        lines.push("================".to_string());
        lines.push(String::new());

        // Prefer the query layer (Phase D: the daemon's cozo under a thin primary,
        // else the local cozo via FederatedQuery), then the local store, then the
        // in-memory mirror. Under a thin primary the local store/mirror are empty, so
        // routing through the query layer is what makes the report accurate.
        let store_report = self
            .kb
            .query_layer()
            .and_then(|q| q.health_report())
            .or_else(|| self.kb.store.as_ref().and_then(|s| s.health_report().ok()));

        if let Some(ref report) = store_report {
            lines.push(format!(
                "Source: CozoDB ({})",
                self.kb
                    .store
                    .as_ref()
                    .map(|s| s.backend_name())
                    .unwrap_or("?")
            ));
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

            // By kind.
            lines.push("By Kind".to_string());
            lines.push("-------".to_string());
            let mut kinds: Vec<_> = report.by_kind.iter().collect();
            kinds.sort_by(|a, b| b.1.cmp(a.1));
            for (kind, count) in &kinds {
                lines.push(format!("  {:<20} {}", kind, count));
            }
            lines.push(String::new());

            // By relationship type.
            if !report.by_rel_type.is_empty() {
                lines.push("By Relationship Type".to_string());
                lines.push("--------------------".to_string());
                let mut rels: Vec<_> = report.by_rel_type.iter().collect();
                rels.sort_by(|a, b| b.1.cmp(a.1));
                for (rt, count) in &rels {
                    lines.push(format!("  {:<20} {}", rt, count));
                }
                lines.push(String::new());
            }

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

            // Broken links.
            lines.push(format!("Broken Links ({})", report.broken_links.len()));
            lines.push("------------".to_string());
            if report.broken_links.is_empty() {
                lines.push("  (none)".to_string());
            } else {
                use mae_kb::BrokenLinkReason;
                let mut deleted: Vec<_> = report
                    .broken_links
                    .iter()
                    .filter(|b| b.reason == BrokenLinkReason::DeletedNode)
                    .collect();
                let mut malformed: Vec<_> = report
                    .broken_links
                    .iter()
                    .filter(|b| b.reason == BrokenLinkReason::MalformedId)
                    .collect();
                deleted.sort_by_key(|b| &b.target);
                malformed.sort_by_key(|b| &b.target);
                if !deleted.is_empty() {
                    lines.push(format!("  Missing targets ({}):", deleted.len()));
                    for b in &deleted {
                        lines.push(format!("    {} —[{}]→ {}", b.source, b.rel_type, b.target));
                    }
                }
                if !malformed.is_empty() {
                    lines.push(format!("  Malformed IDs ({}):", malformed.len()));
                    for b in &malformed {
                        lines.push(format!("    {} → {:?}", b.source, b.target));
                    }
                }
            }
            lines.push(String::new());

            // Hub nodes.
            if !report.hub_nodes.is_empty() {
                lines.push("Hub Nodes (top 10 by in-degree)".to_string());
                lines.push("-------------------------------".to_string());
                for (id, degree) in &report.hub_nodes {
                    lines.push(format!("  {:<40} {}", id, degree));
                }
                lines.push(String::new());
            }
        } else {
            // Fallback: in-memory KnowledgeBase report.
            lines.push("Source: in-memory (CozoDB store not available)".to_string());
            let report = self.kb.primary.health_report();
            lines.push(format!("Total nodes: {}", report.total_nodes));
            lines.push(format!("Total links: {}", report.total_links));
            lines.push(String::new());

            lines.push("Namespace Counts".to_string());
            lines.push("----------------".to_string());
            let mut ns: Vec<_> = report.namespace_counts.iter().collect();
            ns.sort_by(|a, b| b.1.cmp(a.1));
            for (name, count) in &ns {
                lines.push(format!("  {:<20} {}", name, count));
            }
            lines.push(String::new());

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

            lines.push(format!("Broken Links ({})", report.broken_links.len()));
            lines.push("------------".to_string());
            if report.broken_links.is_empty() {
                lines.push("  (none)".to_string());
            } else {
                for b in &report.broken_links {
                    lines.push(format!("    {} → {}", b.source, b.target));
                }
            }
            lines.push(String::new());
        }

        // Stale nodes (source file deleted — detected from the in-memory mirror).
        // Under a thin primary the mirror is empty and there is no daemon RPC for
        // stale detection yet, so report that honestly instead of a misleading
        // "(none)" (no silent caps — #118 tracks the daemon-side capability).
        if self.kb.primary_thin() {
            lines.push("Stale Nodes".to_string());
            lines.push("-------------------".to_string());
            lines.push("  (not available — primary is daemon-hosted; stale".to_string());
            lines.push("   detection has no daemon RPC yet — see #118)".to_string());
        } else {
            let stale_nodes = self.kb.primary.detect_stale_nodes();
            lines.push(format!("Stale Nodes ({})", stale_nodes.len()));
            lines.push("-------------------".to_string());
            if stale_nodes.is_empty() {
                lines.push("  (none)".to_string());
            } else {
                for s in &stale_nodes {
                    lines.push(format!(
                        "  {} — {} (was: {})",
                        s.id,
                        s.title,
                        s.source_file.display()
                    ));
                }
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

        let chain = self.keymap_chain();
        if chain.is_empty() {
            self.set_status("No keymap for current mode".to_string());
            return;
        }
        let primary_map = chain[0].clone();
        let mode = self.mode;

        let mut lines = Vec::new();
        lines.push(format!(
            "Keybindings — {:?} mode (keymap: {})",
            mode, primary_map
        ));
        lines.push("=".repeat(60));
        lines.push(String::new());

        // Collect bindings across the full resolution chain (most-specific first;
        // the same `keymap_chain()` dispatch uses, so the report matches reality).
        let mut all_bindings: Vec<(String, String)> = Vec::new();
        for map_name in &chain {
            if let Some(km) = self.keymaps.get(map_name) {
                for (seq, cmd) in km.bindings() {
                    let key_str = format_seq(seq);
                    if !all_bindings.iter().any(|(k, _)| k == &key_str) {
                        all_bindings.push((key_str, cmd.clone()));
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
            "mae-agent"
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
        if self.ai_chat_enabled {
            lines.push("  Embedded chat: enabled (built-in conversation buffer)".to_string());
        } else {
            lines.push(
                "  Embedded chat: disabled (ADR-049) \u{2014} SPC a p launches the AI Agent \
                 shell above instead. :set ai_chat_enabled true to restore it."
                    .to_string(),
            );
        }
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
        for cmd in &["lldb-dap", "codelldb", "debugpy"] {
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
    /// Effective word-wrap for a specific buffer index.
    pub fn word_wrap_for(&self, buf_idx: usize) -> bool {
        self.buffers[buf_idx]
            .local_options
            .word_wrap
            .unwrap_or(self.word_wrap)
    }

    /// Effective word-wrap for the currently focused buffer.
    pub fn effective_word_wrap(&self) -> bool {
        self.word_wrap_for(self.active_buffer_idx())
    }

    /// Effective show_line_numbers for a specific buffer index.
    pub fn line_numbers_for(&self, buf_idx: usize) -> bool {
        self.buffers[buf_idx]
            .local_options
            .line_numbers
            .unwrap_or(self.show_line_numbers)
    }

    /// Effective relative_line_numbers for a specific buffer index.
    pub fn relative_line_numbers_for(&self, buf_idx: usize) -> bool {
        self.buffers[buf_idx]
            .local_options
            .relative_line_numbers
            .unwrap_or(self.relative_line_numbers)
    }

    /// Effective break_indent for a specific buffer index.
    pub fn break_indent_for(&self, buf_idx: usize) -> bool {
        self.buffers[buf_idx]
            .local_options
            .break_indent
            .unwrap_or(self.break_indent)
    }

    /// Effective show_break for a specific buffer index.
    pub fn show_break_for(&self, buf_idx: usize) -> &str {
        self.buffers[buf_idx]
            .local_options
            .show_break
            .as_deref()
            .unwrap_or(&self.show_break)
    }

    /// Effective heading_scale for a specific buffer index.
    pub fn heading_scale_for(&self, buf_idx: usize) -> bool {
        self.buffers[buf_idx]
            .local_options
            .heading_scale
            .unwrap_or(self.heading_scale)
    }

    /// Effective link_descriptive for a specific buffer index.
    pub fn link_descriptive_for(&self, buf_idx: usize) -> bool {
        self.buffers[buf_idx]
            .local_options
            .link_descriptive
            .unwrap_or(self.link_descriptive)
    }

    /// Effective render_markup for a specific buffer index.
    pub fn render_markup_for(&self, buf_idx: usize) -> bool {
        self.buffers[buf_idx]
            .local_options
            .render_markup
            .unwrap_or(self.render_markup)
    }

    /// Resolve the effective markup flavor for a buffer, respecting the
    /// priority chain: BufferMode → Language → None, gated by render_markup.
    pub fn effective_markup_flavor(&self, buf_idx: usize) -> crate::syntax::MarkupFlavor {
        use crate::buffer_mode::BufferMode;
        if !self.render_markup_for(buf_idx) {
            return crate::syntax::MarkupFlavor::None;
        }
        let buf = &self.buffers[buf_idx];
        if let Some(flavor) = buf.kind.markup_flavor() {
            return flavor;
        }
        if let Some(lang) = self.syntax.language_of(buf_idx) {
            return lang.markup_flavor();
        }
        crate::syntax::MarkupFlavor::None
    }

    /// Detect whether a buffer is too large for full feature rendering.
    /// Returns true for files exceeding `degrade_threshold_chars` or any line
    /// exceeding `degrade_threshold_line_length` (both user-configurable).
    /// Callers should skip markup spans, display regions, code block
    /// detection, and heading scale for such buffers (Emacs `so-long` pattern).
    ///
    /// Result is cached per buffer (`buffer.degraded`). The cache is set on
    /// first access and on file open — degradation status is monotonic during
    /// normal editing so re-scanning every frame is unnecessary.
    pub fn should_degrade_features(&self, buf_idx: usize) -> bool {
        if buf_idx >= self.buffers.len() {
            return false;
        }
        if let Some(cached) = self.buffers[buf_idx].degraded {
            return cached;
        }
        let buf = &self.buffers[buf_idx];
        let rope = buf.rope();
        if rope.len_chars() > self.degrade_threshold_chars {
            return true;
        }
        // Sample first 200 lines + last 50 for long-line detection (avoid O(n) full scan).
        let lc = rope.len_lines();
        let check_lines = (0..200.min(lc)).chain(lc.saturating_sub(50)..lc);
        for li in check_lines {
            let line = rope.line(li);
            if line.len_chars() > self.degrade_threshold_line_length {
                return true;
            }
        }
        false
    }

    /// Compute and cache the degradation status for a buffer.
    pub fn cache_degraded(&mut self, buf_idx: usize) {
        let degraded = self.should_degrade_features(buf_idx);
        self.buffers[buf_idx].degraded = Some(degraded);
    }

    /// Get or compute cached markup spans for a buffer. Returns empty if
    /// flavor is None. The cache is keyed by buffer generation so editing
    /// invalidates it but pure scrolling reuses cached spans.
    pub fn get_or_compute_markup_spans(
        &mut self,
        buf_idx: usize,
        flavor: crate::syntax::MarkupFlavor,
    ) -> Vec<crate::syntax::HighlightSpan> {
        if flavor == crate::syntax::MarkupFlavor::None {
            return Vec::new();
        }
        let gen = self.buffers[buf_idx].generation;
        if let Some(cached) = self.markup_cache.get(&buf_idx) {
            if cached.generation == gen && cached.flavor == flavor {
                return cached.spans.clone();
            }
        }
        let rope = self.buffers[buf_idx].rope();
        let line_count = rope.len_lines();
        let source: String = rope.chars().collect();
        let spans = crate::syntax::compute_markup_spans(&source, flavor);
        self.markup_cache.insert(
            buf_idx,
            crate::syntax::MarkupCache {
                generation: gen,
                flavor,
                line_start: 0,
                line_end: line_count,
                byte_offset: 0,
                spans: spans.clone(),
            },
        );
        spans
    }

    /// Clamp all window cursors to their buffer bounds. Safety net against
    /// stale cursor positions after buffer mutations (MCP tools, AI edits).
    /// Also clamps visual anchors and last_visual so rendering never panics.
    pub fn clamp_all_cursors(&mut self) {
        for win in self.window_mgr.iter_windows_mut() {
            let buf_idx = win.buffer_idx;
            if buf_idx < self.buffers.len() {
                win.clamp_cursor(&self.buffers[buf_idx]);
            }
        }

        // Clamp visual anchor to focused buffer bounds.
        let idx = self.active_buffer_idx();
        let line_count = self.buffers[idx].display_line_count();
        if line_count == 0 {
            self.vi.visual_anchor_row = 0;
            self.vi.visual_anchor_col = 0;
        } else {
            let max_row = line_count.saturating_sub(1);
            if self.vi.visual_anchor_row > max_row {
                self.vi.visual_anchor_row = max_row;
            }
            let max_col = self.buffers[idx].line_len(self.vi.visual_anchor_row);
            if self.vi.visual_anchor_col > max_col {
                self.vi.visual_anchor_col = max_col;
            }
        }

        // Clamp last_visual so `gv` reselect never panics.
        if let Some((ref mut ar, ref mut ac, ref mut cr, ref mut cc, _)) = self.vi.last_visual {
            if line_count == 0 {
                *ar = 0;
                *ac = 0;
                *cr = 0;
                *cc = 0;
            } else {
                let max_row = line_count.saturating_sub(1);
                if *ar > max_row {
                    *ar = max_row;
                }
                *ac = (*ac).min(self.buffers[idx].line_len(*ar));
                if *cr > max_row {
                    *cr = max_row;
                }
                *cc = (*cc).min(self.buffers[idx].line_len(*cr));
            }
        }
    }
}

#[cfg(test)]
mod daemon_mode_tests {
    use crate::editor::{DaemonMode, Editor};

    #[test]
    fn daemon_mode_parse_and_as_str_roundtrip() {
        for (s, m) in [
            ("off", DaemonMode::Off),
            ("on-demand", DaemonMode::OnDemand),
            ("shared", DaemonMode::Shared),
        ] {
            assert_eq!(DaemonMode::parse(s), Some(m));
            assert_eq!(m.as_str(), s);
        }
        // Conveniences + case-insensitivity.
        assert_eq!(DaemonMode::parse("ON_DEMAND"), Some(DaemonMode::OnDemand));
        assert_eq!(DaemonMode::parse("OnDemand"), Some(DaemonMode::OnDemand));
        assert_eq!(DaemonMode::parse("bogus"), None);
        assert!(!DaemonMode::Off.connects());
        assert!(DaemonMode::OnDemand.connects());
        assert!(DaemonMode::Shared.connects());
    }

    #[test]
    fn daemon_mode_option_set_get_and_gate_sync() {
        let mut editor = Editor::new();
        // Default is the in-process floor.
        assert_eq!(editor.get_option("daemon_mode").unwrap().0, "off");
        assert!(!editor.kb.daemon_enabled);

        // Each mode round-trips and keeps the runtime gate in sync.
        editor.set_option("daemon_mode", "shared").unwrap();
        assert_eq!(editor.get_option("daemon_mode").unwrap().0, "shared");
        assert_eq!(editor.kb.daemon_mode, DaemonMode::Shared);
        assert!(editor.kb.daemon_enabled, "shared connects");

        editor.set_option("daemon_mode", "off").unwrap();
        assert_eq!(editor.kb.daemon_mode, DaemonMode::Off);
        assert!(
            !editor.kb.daemon_enabled,
            "off is the floor — no connection"
        );

        editor.set_option("daemon_mode", "on-demand").unwrap();
        assert_eq!(editor.kb.daemon_mode, DaemonMode::OnDemand);
        assert!(editor.kb.daemon_enabled);

        // Invalid value is rejected, state unchanged.
        assert!(editor.set_option("daemon_mode", "nonsense").is_err());
        assert_eq!(editor.kb.daemon_mode, DaemonMode::OnDemand);
    }

    #[test]
    fn collab_config_options_roundtrip_and_reach_editor_state() {
        let mut editor = Editor::new();

        // Defaults match the wired editor.collab fields (single source of truth).
        assert_eq!(
            editor.get_option("collab_command_queue_size").unwrap().0,
            "256"
        );
        assert_eq!(
            editor
                .get_option("collab_force_sync_debounce_secs")
                .unwrap()
                .0,
            "2"
        );
        assert_eq!(
            editor.get_option("collab_daemon_start_grace_ms").unwrap().0,
            "500"
        );
        assert_eq!(
            editor
                .get_option("collab_host_key_prompt_timeout_secs")
                .unwrap()
                .0,
            "120"
        );

        // set_option round-trips through both get_option AND the editor.collab
        // field the network task actually reads (the parity that matters — an
        // option that doesn't reach its use site is theatre).
        editor
            .set_option("collab_command_queue_size", "1024")
            .unwrap();
        assert_eq!(editor.collab.command_queue_size, 1024);
        assert_eq!(
            editor.get_option("collab_command_queue_size").unwrap().0,
            "1024"
        );

        editor
            .set_option("collab_force_sync_debounce_secs", "9")
            .unwrap();
        assert_eq!(editor.collab.force_sync_debounce_secs, 9);

        editor
            .set_option("collab_daemon_start_grace_ms", "0")
            .unwrap();
        assert_eq!(editor.collab.daemon_start_grace_ms, 0);

        editor
            .set_option("collab_host_key_prompt_timeout_secs", "300")
            .unwrap();
        assert_eq!(editor.collab.host_key_prompt_timeout_secs, 300);
    }

    #[test]
    fn collab_config_options_clamp_hostile_values() {
        let mut editor = Editor::new();

        // Queue size must keep at least one slot even if set to 0 (else the
        // bounded channel would panic / deadlock).
        editor.set_option("collab_command_queue_size", "0").unwrap();
        assert!(
            editor.collab.command_queue_size >= 1,
            "queue size floored to >=1, got {}",
            editor.collab.command_queue_size
        );

        // The host-key prompt wait must not be settable to an instant-reject
        // value (fail-closed floor) — that would silently defeat the trust prompt.
        editor
            .set_option("collab_host_key_prompt_timeout_secs", "0")
            .unwrap();
        assert!(
            editor.collab.host_key_prompt_timeout_secs >= 5,
            "prompt wait floored, got {}",
            editor.collab.host_key_prompt_timeout_secs
        );

        // Non-numeric input is rejected, state unchanged.
        let before = editor.collab.daemon_start_grace_ms;
        assert!(editor
            .set_option("collab_daemon_start_grace_ms", "not-a-number")
            .is_err());
        assert_eq!(editor.collab.daemon_start_grace_ms, before);
    }

    #[test]
    fn legacy_daemon_enabled_aliases_daemon_mode() {
        let mut editor = Editor::new();
        // true ⇒ on-demand
        editor.set_option("daemon_enabled", "true").unwrap();
        assert_eq!(editor.kb.daemon_mode, DaemonMode::OnDemand);
        assert_eq!(editor.get_option("daemon_mode").unwrap().0, "on-demand");
        assert_eq!(editor.get_option("daemon_enabled").unwrap().0, "true");

        // false ⇒ off
        editor.set_option("daemon_enabled", "false").unwrap();
        assert_eq!(editor.kb.daemon_mode, DaemonMode::Off);
        assert_eq!(editor.get_option("daemon_enabled").unwrap().0, "false");

        // Setting the mode reflects back through the legacy bool.
        editor.set_option("daemon_mode", "shared").unwrap();
        assert_eq!(editor.get_option("daemon_enabled").unwrap().0, "true");
    }
}

#[cfg(test)]
mod ai_option_tests {
    use crate::editor::Editor;

    #[test]
    fn ai_agent_login_shell_option_registered_and_roundtrips() {
        let mut editor = Editor::new();
        assert!(editor
            .option_registry
            .find("ai_agent_login_shell")
            .is_some());
        assert!(editor
            .option_registry
            .find("ai-agent-login-shell")
            .is_some());

        // Defaults to enabled (fixes the reported bug out of the box).
        assert_eq!(editor.get_option("ai_agent_login_shell").unwrap().0, "true");
        assert!(editor.ai.agent_login_shell);

        editor.set_option("ai_agent_login_shell", "false").unwrap();
        assert_eq!(
            editor.get_option("ai_agent_login_shell").unwrap().0,
            "false"
        );
        assert!(!editor.ai.agent_login_shell);

        editor.set_option("ai_agent_login_shell", "true").unwrap();
        assert!(editor.ai.agent_login_shell);
    }
}
