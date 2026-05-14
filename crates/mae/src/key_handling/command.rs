use super::dispatch_command;
use crate::ai_event_handler::PendingInteractiveEvent;
use crate::bootstrap::load_ai_config;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mae_ai::AiCommand;
use mae_core::{Editor, KeyPress, Mode};
use mae_scheme::SchemeRuntime;
use tracing::{debug, error, info, warn};

fn apply_tab_completion(editor: &mut Editor) {
    if editor.tab_completions.is_empty() {
        return;
    }
    let completion = editor.tab_completions[editor.tab_completion_idx].clone();
    if let Some(space_pos) = editor.command_line.find(' ') {
        let prefix = editor.command_line[..=space_pos].to_string();
        editor.command_line = format!("{}{}", prefix, completion);
    } else {
        editor.command_line = completion;
    }
    editor.command_cursor = editor.command_line.len();
}

pub fn handle_command_mode(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
    ai_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
    pending_interactive_event: &mut Option<PendingInteractiveEvent>,
) {
    pending_keys.clear();
    match key.code {
        KeyCode::Esc => {
            editor.file_tree_action = None;
            editor.set_mode(Mode::Normal);
            editor.command_line.clear();
            editor.command_cursor = 0;
        }
        KeyCode::Enter => {
            let cmd = editor.command_line.clone();
            editor.set_mode(Mode::Normal);
            editor.command_line.clear();
            editor.command_cursor = 0;

            // File tree action (rename/create) — intercept before normal dispatch.
            if let Some(action) = editor.file_tree_action.take() {
                match action {
                    mae_core::file_tree::FileTreeAction::Rename(old_path) => {
                        if cmd.is_empty() {
                            editor.set_status("Rename cancelled");
                            return;
                        }
                        let new_path = old_path
                            .parent()
                            .unwrap_or(std::path::Path::new("."))
                            .join(&cmd);
                        match std::fs::rename(&old_path, &new_path) {
                            Ok(()) => {
                                // Refresh file tree
                                let tree_idx = editor
                                    .buffers
                                    .iter()
                                    .position(|b| b.kind == mae_core::BufferKind::FileTree);
                                if let Some(ti) = tree_idx {
                                    if let Some(ft) = editor.buffers[ti].file_tree_mut() {
                                        ft.refresh();
                                    }
                                }
                                editor.set_status(format!("Renamed to {}", cmd));
                            }
                            Err(e) => editor.set_status(format!("Rename failed: {}", e)),
                        }
                        return;
                    }
                    mae_core::file_tree::FileTreeAction::Create(parent) => {
                        if cmd.is_empty() {
                            editor.set_status("Create cancelled");
                            return;
                        }
                        let target = parent.join(&cmd);
                        let result = if cmd.ends_with('/') {
                            std::fs::create_dir_all(&target)
                        } else {
                            // Ensure parent dirs exist
                            if let Some(p) = target.parent() {
                                let _ = std::fs::create_dir_all(p);
                            }
                            std::fs::write(&target, "")
                        };
                        match result {
                            Ok(()) => {
                                let tree_idx = editor
                                    .buffers
                                    .iter()
                                    .position(|b| b.kind == mae_core::BufferKind::FileTree);
                                if let Some(ti) = tree_idx {
                                    if let Some(ft) = editor.buffers[ti].file_tree_mut() {
                                        ft.refresh();
                                    }
                                }
                                editor.set_status(format!("Created {}", cmd));
                            }
                            Err(e) => editor.set_status(format!("Create failed: {}", e)),
                        }
                        return;
                    }
                }
            }

            // Record in command history before executing
            editor.push_command_history(&cmd);

            // :ai-accept — approve proposed changes
            if cmd == "ai-accept" {
                if let Some(event) = pending_interactive_event.take() {
                    match event {
                        PendingInteractiveEvent::ProposeChanges(reply) => {
                            let _ = reply.send(true);
                            editor.set_status("[AI] Changes accepted");
                        }
                        PendingInteractiveEvent::AskUser(reply) => {
                            let _ = reply.send("User accepted without typing".into());
                            editor.set_status("[AI] User accepted");
                        }
                    }
                } else {
                    editor.set_status("No pending AI interaction to accept");
                }
                return;
            }

            // :ai-reject — reject proposed changes
            if cmd == "ai-reject" {
                if let Some(event) = pending_interactive_event.take() {
                    match event {
                        PendingInteractiveEvent::ProposeChanges(reply) => {
                            let _ = reply.send(false);
                            editor.set_status("[AI] Changes rejected");
                        }
                        PendingInteractiveEvent::AskUser(reply) => {
                            let _ = reply.send("User rejected/cancelled".into());
                            editor.set_status("[AI] User rejected");
                        }
                    }
                } else {
                    editor.set_status("No pending AI interaction to reject");
                }
                return;
            }

            // :ai-status — show AI configuration + session metrics
            if cmd == "ai-status" {
                let config = load_ai_config(editor);
                if let Some(ref cfg) = config {
                    let connected = ai_tx.is_some();
                    let mut parts = vec![format!(
                        "AI: provider={}, model={}, connected={}",
                        cfg.provider_type, cfg.model, connected
                    )];
                    if connected {
                        if editor.ai_session_cost_usd > 0.0 {
                            parts.push(format!("${:.4}", editor.ai_session_cost_usd));
                        }
                        if editor.ai_session_tokens_in > 0 || editor.ai_session_tokens_out > 0 {
                            parts.push(format!(
                                "tokens: {}in/{}out",
                                editor.ai_session_tokens_in, editor.ai_session_tokens_out
                            ));
                        }
                        if editor.ai_context_window > 0 && editor.ai_context_used_tokens > 0 {
                            let pct = (editor.ai_context_used_tokens as f64
                                / editor.ai_context_window as f64
                                * 100.0) as u64;
                            parts.push(format!("ctx: {}%", pct));
                        }
                        if editor.ai_cache_read_tokens > 0 {
                            let total_cache =
                                editor.ai_cache_read_tokens + editor.ai_cache_creation_tokens;
                            let hit_pct = if total_cache > 0 {
                                (editor.ai_cache_read_tokens as f64 / total_cache as f64 * 100.0)
                                    as u64
                            } else {
                                0
                            };
                            parts.push(format!("cache: {}%", hit_pct));
                        }
                    }
                    editor.set_status(parts.join(" | "));
                } else {
                    editor.set_status("AI not configured \u{2014} :help ai-setup for setup guide");
                }
                return;
            }

            // :ai-status! — detailed AI diagnostics buffer
            if cmd == "ai-status!" {
                let content = build_ai_status_report(editor, ai_tx);
                let mut buf = mae_core::buffer::Buffer::new();
                buf.name = "*AI Status*".to_string();
                buf.replace_contents(&content);
                buf.modified = false;
                buf.read_only = true;
                let buf_idx = editor.buffers.len();
                editor.buffers.push(buf);
                editor.display_buffer(buf_idx);
                return;
            }

            // :ai-ping — network connectivity check (no LLM round-trip)
            if cmd == "ai-ping" {
                if let Some(tx) = ai_tx {
                    let config = load_ai_config(editor);
                    let base_url = config.as_ref().and_then(|c| c.base_url.clone());
                    if tx.try_send(AiCommand::PingNetwork { base_url }).is_err() {
                        editor.set_status("[AI] Ping failed \u{2014} channel closed");
                    } else {
                        editor.set_status("[AI] Pinging...");
                    }
                } else {
                    editor.set_status("AI not configured \u{2014} :help ai-setup for setup guide");
                }
                return;
            }

            // :verify [objective] — spawn verifier sub-agent (direct delegate, no LLM round-trip)
            if cmd == "verify" || cmd.starts_with("verify ") {
                let objective = cmd.strip_prefix("verify").unwrap_or("").trim();
                let objective = if objective.is_empty() {
                    "Run all tests and report results"
                } else {
                    objective
                };
                if let Some(tx) = ai_tx {
                    if tx
                        .try_send(AiCommand::Delegate {
                            profile: "verifier".to_string(),
                            objective: objective.to_string(),
                        })
                        .is_err()
                    {
                        editor.set_status("[AI] Verify failed \u{2014} channel closed");
                    } else {
                        editor.set_status("[AI] Verifier spawned...");
                    }
                } else {
                    editor.set_status("AI not configured \u{2014} :help ai-setup for setup guide");
                }
                return;
            }

            // :ai <prompt> — send to AI agent
            if let Some(prompt) = cmd.strip_prefix("ai ") {
                let prompt = prompt.trim();
                if prompt.is_empty() {
                    editor.set_status("Usage: :ai <prompt>");
                    return;
                }
                if let Some(tx) = ai_tx {
                    info!(
                        prompt_len = prompt.len(),
                        "sending AI prompt via command mode"
                    );
                    if tx.try_send(AiCommand::Prompt(prompt.to_string())).is_err() {
                        warn!("AI command channel full or closed — prompt dropped");
                    }
                    editor.set_status("[AI] Thinking...");
                } else {
                    warn!("AI prompt submitted but no AI provider configured");
                    editor.set_status("AI not configured \u{2014} :help ai-setup for setup guide");
                }
                return;
            }

            // :self-test [categories] — AI-driven e2e validation
            if cmd == "self-test" || cmd.starts_with("self-test ") {
                let categories = cmd.strip_prefix("self-test").unwrap().trim();
                if let Some(tx) = ai_tx {
                    // Lock input so user keystrokes don't interfere with test state.
                    editor.input_lock = mae_core::InputLock::AiBusy;
                    // Ensure *AI* buffer exists and is visible so the user
                    // can watch self-test progress (tool calls, results, report).
                    editor.open_conversation_buffer();
                    let prompt = build_self_test_prompt(categories);
                    if tx.try_send(AiCommand::Prompt(prompt)).is_err() {
                        warn!("AI self-test prompt dropped");
                        editor.input_lock = mae_core::InputLock::None;
                    }
                    info!(
                        "self-test started, categories={:?}",
                        if categories.is_empty() {
                            "all"
                        } else {
                            categories
                        }
                    );
                    editor.set_status("[AI BUSY — Esc to cancel] Running self-test...");
                } else {
                    editor.set_status("AI not configured \u{2014} :help ai-setup for setup guide");
                }
                return;
            }

            // :model-exam — run model validation exam via verifier sub-agent
            if cmd == "model-exam" {
                if let Some(tx) = ai_tx {
                    if tx
                        .try_send(AiCommand::Delegate {
                            profile: "verifier".to_string(),
                            objective: "Run the model validation exam: call model_exam with action='plan' to get the test plan, execute each test by sending the prompt and recording which tools are called, then call model_exam with action='grade' and provide the results array. Report the final ExamResult with verdict.".to_string(),
                        })
                        .is_err()
                    {
                        editor.set_status("[AI] Model exam failed \u{2014} channel closed");
                    } else {
                        editor.set_status("[AI] Model exam started...");
                    }
                } else {
                    editor.set_status("AI not configured \u{2014} :help ai-setup for setup guide");
                }
                return;
            }

            // :eval <scheme> — Scheme REPL
            if let Some(code) = cmd.strip_prefix("eval ") {
                let code = code.trim();
                if code.is_empty() {
                    editor.set_status("eval: no expression given");
                    return;
                }
                debug!(code, "evaluating scheme expression");
                scheme.inject_editor_state(editor);
                match scheme.eval(code) {
                    Ok(result) => {
                        scheme.apply_to_editor(editor);
                        debug!(result = %result, "scheme eval succeeded");
                        if result.is_empty() {
                            editor.set_status("(ok)");
                        } else {
                            editor.set_status(result);
                        }
                    }
                    Err(e) => {
                        error!(code, error = %e, "scheme eval failed");
                        editor.set_status(format!("Scheme error: {}", e));
                    }
                }
                return;
            }

            // Try ex-command handler first (handles args like `:theme dracula`,
            // `:e file.txt`, `:help topic`, etc.), then fall back to registered
            // command dispatch for bare names like `:move-down`.
            if !editor.execute_command(&cmd) {
                let cmd_name = cmd.split_whitespace().next().unwrap_or("");
                if editor.commands.contains(cmd_name) {
                    dispatch_command(editor, scheme, cmd_name);
                } else {
                    editor.set_status(format!("Unknown command: {}", cmd));
                }
            }
        }
        KeyCode::Tab => {
            if editor.tab_completions.is_empty() {
                editor.tab_completions = editor.cmdline_completions();
                editor.tab_completion_idx = 0;
            } else {
                editor.tab_completion_idx =
                    (editor.tab_completion_idx + 1) % editor.tab_completions.len();
            }
            apply_tab_completion(editor);
        }
        KeyCode::BackTab => {
            if editor.tab_completions.is_empty() {
                editor.tab_completions = editor.cmdline_completions();
                if !editor.tab_completions.is_empty() {
                    editor.tab_completion_idx = editor.tab_completions.len() - 1;
                }
            } else {
                let len = editor.tab_completions.len();
                editor.tab_completion_idx = (editor.tab_completion_idx + len - 1) % len;
            }
            apply_tab_completion(editor);
        }
        KeyCode::Up => {
            editor.command_history_prev();
        }
        KeyCode::Down => {
            editor.command_history_next();
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.command_history_prev();
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.command_history_next();
        }
        KeyCode::Left => {
            editor.cmdline_move_backward();
        }
        KeyCode::Right => {
            editor.cmdline_move_forward();
        }
        KeyCode::Home => {
            editor.cmdline_move_home();
        }
        KeyCode::End => {
            editor.cmdline_move_end();
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.cmdline_move_home();
        }
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.cmdline_move_end();
        }
        KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.cmdline_move_backward();
        }
        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.cmdline_move_forward();
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.cmdline_delete_word_backward();
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.cmdline_kill_to_start();
        }
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.cmdline_kill_to_end();
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if editor.command_line.is_empty() {
                // C-d on empty line = abort (like in shells)
                editor.set_mode(Mode::Normal);
            } else {
                editor.cmdline_delete_forward();
            }
        }
        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if editor.command_line.is_empty() {
                editor.set_mode(Mode::Normal);
            } else {
                editor.cmdline_backspace();
            }
        }
        KeyCode::Backspace => {
            if editor.command_line.is_empty() {
                editor.set_mode(Mode::Normal);
            } else {
                editor.cmdline_backspace();
            }
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.cmdline_insert_char(ch);
        }
        _ => {}
    }
}

/// Build the self-test prompt from the embedded template.
///
/// If `categories` is empty, all test categories run. Otherwise only the
/// named categories execute and everything else is reported as SKIP.
pub fn build_self_test_prompt(categories: &str) -> String {
    let base = include_str!("../self_test_prompt.md");
    if categories.is_empty() {
        format!(
            "You are running MAE's self-test suite. Execute ALL test categories.\n\n{}",
            base
        )
    } else {
        format!(
            "You are running MAE's self-test suite. Execute ONLY these categories: {}. \
             Report all others as SKIP.\n\n{}",
            categories, base
        )
    }
}

fn build_ai_status_report(
    editor: &Editor,
    ai_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
) -> String {
    let config = load_ai_config(editor);
    let mut lines = vec![
        "MAE AI Status Report".to_string(),
        "====================".to_string(),
        String::new(),
        // Provider
        "Provider".to_string(),
        "--------".to_string(),
    ];
    if let Some(ref cfg) = config {
        lines.push(format!("  Type:       {}", cfg.provider_type));
        lines.push(format!("  Model:      {}", cfg.model));
        let key_set = cfg.api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false);
        lines.push(format!(
            "  API Key:    {}",
            if key_set { "*** (set)" } else { "not set" }
        ));
        if let Some(ref url) = cfg.base_url {
            lines.push(format!("  Base URL:   {}", url));
        }
        lines.push(format!("  Timeout:    {}s", cfg.timeout_secs));
        lines.push(format!("  Max Tokens: {}", cfg.max_tokens));
        lines.push(format!("  Connected:  {}", ai_tx.is_some()));
    } else {
        lines.push("  Not configured — :help ai-setup for setup guide".to_string());
    }
    lines.push(String::new());

    // Permission
    lines.push("Permission".to_string());
    lines.push("----------".to_string());
    lines.push(format!("  Tier:       {}", editor.ai_permission_tier));
    lines.push(format!("  Mode:       {}", editor.ai_mode));
    lines.push(format!("  Profile:    {}", editor.ai_profile));
    lines.push(String::new());

    // Session
    lines.push("Session".to_string());
    lines.push("-------".to_string());
    lines.push(format!(
        "  Cost:           ${:.4}",
        editor.ai_session_cost_usd
    ));
    lines.push(format!("  Tokens In:      {}", editor.ai_session_tokens_in));
    lines.push(format!(
        "  Tokens Out:     {}",
        editor.ai_session_tokens_out
    ));
    if editor.ai_context_window > 0 {
        let pct = (editor.ai_context_used_tokens as f64 / editor.ai_context_window as f64) * 100.0;
        lines.push(format!(
            "  Context:        {}/{} ({:.1}%)",
            editor.ai_context_used_tokens, editor.ai_context_window, pct
        ));
    }
    if editor.ai_cache_read_tokens > 0 || editor.ai_cache_creation_tokens > 0 {
        let total = editor.ai_cache_read_tokens + editor.ai_cache_creation_tokens;
        let hit = if total > 0 {
            (editor.ai_cache_read_tokens as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        lines.push(format!(
            "  Cache Read:     {} ({:.1}% hit rate)",
            editor.ai_cache_read_tokens, hit
        ));
        lines.push(format!(
            "  Cache Created:  {}",
            editor.ai_cache_creation_tokens
        ));
    }
    if let Some(ref cfg) = config {
        let warn = cfg.budget.session_warn_usd.unwrap_or(0.0);
        let cap = cfg.budget.session_hard_cap_usd.unwrap_or(0.0);
        if warn > 0.0 || cap > 0.0 {
            lines.push(format!(
                "  Budget:         warn=${:.2}, cap=${:.2}",
                warn, cap
            ));
        }
    }
    lines.push(String::new());

    // Network
    lines.push("Network".to_string());
    lines.push("-------".to_string());
    lines.push(format!("  API Calls:  {}", editor.ai_api_call_count));
    if let Some(ref instant) = editor.ai_last_api_success {
        let elapsed = instant.elapsed();
        let secs = elapsed.as_secs();
        let ago = if secs < 60 {
            format!("{}s ago", secs)
        } else if secs < 3600 {
            format!("{}m ago", secs / 60)
        } else {
            format!("{}h ago", secs / 3600)
        };
        lines.push(format!("  Last OK:    {}", ago));
    } else {
        lines.push("  Last OK:    (none)".to_string());
    }
    if let Some(ms) = editor.ai_last_api_latency_ms {
        lines.push(format!("  Latency:    {}ms", ms));
    }
    if let Some(ref err) = editor.ai_last_api_error {
        lines.push(format!("  Last Error: {}", err));
    }
    if let Some(ref check) = editor.ai_last_network_check {
        lines.push(String::new());
        lines.push("Connectivity".to_string());
        lines.push("------------".to_string());
        lines.push(format!("  Endpoint:   {}", check.endpoint));
        lines.push(format!(
            "  Reachable:  {}",
            if check.reachable { "OK" } else { "FAIL" }
        ));
        if let Some(status) = check.http_status {
            lines.push(format!("  HTTP:       {}", status));
        }
        lines.push(format!("  Latency:    {}ms", check.latency_ms));
        if let Some(ref err) = check.error {
            lines.push(format!("  Error:      {}", err));
        }
    }
    lines.push(String::new());

    // Scheme Tools
    lines.push("Scheme Tools".to_string());
    lines.push("------------".to_string());
    if editor.scheme_ai_tools.is_empty() {
        lines.push("  (none registered)".to_string());
    } else {
        for st in &editor.scheme_ai_tools {
            lines.push(format!(
                "  {} — {} [{}]",
                st.name, st.description, st.permission
            ));
        }
    }
    lines.push(String::new());

    // Configuration
    lines.push("Configuration".to_string());
    lines.push("-------------".to_string());
    let config_dir = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| std::path::PathBuf::from(h).join(".config"))
                .unwrap_or_default()
        })
        .join("mae");
    let data_dir = std::env::var("XDG_DATA_HOME")
        .ok()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| std::path::PathBuf::from(h).join(".local/share"))
                .unwrap_or_default()
        })
        .join("mae");
    lines.push(format!(
        "  Config:     {}",
        config_dir.join("config.toml").display()
    ));
    lines.push(format!(
        "  Init:       {}",
        config_dir.join("init.scm").display()
    ));
    lines.push(format!("  Data:       {}", data_dir.display()));
    lines.push(format!(
        "  Transcripts: {}",
        data_dir.join("transcripts").display()
    ));

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_self_test_prompt_all_categories() {
        let prompt = build_self_test_prompt("");
        assert!(!prompt.is_empty());
        assert!(prompt.contains("Execute ALL test categories"));
        assert!(prompt.contains("self_test_suite"));
    }

    #[test]
    fn build_self_test_prompt_filtered() {
        let prompt = build_self_test_prompt("editing");
        assert!(prompt.contains("Execute ONLY these categories: editing"));
        assert!(prompt.contains("Report all others as SKIP"));
    }

    #[test]
    fn build_self_test_prompt_multi_category() {
        let prompt = build_self_test_prompt("editing,help");
        assert!(prompt.contains("Execute ONLY these categories: editing,help"));
    }

    #[test]
    fn ai_status_report_has_sections() {
        let editor = mae_core::Editor::new();
        let report = build_ai_status_report(&editor, &None);
        assert!(report.contains("Provider"));
        assert!(report.contains("Permission"));
        assert!(report.contains("Session"));
        assert!(report.contains("Network"));
        assert!(report.contains("Configuration"));
    }

    #[test]
    fn ai_status_report_with_network_check() {
        let mut editor = mae_core::Editor::new();
        editor.ai_last_network_check = Some(mae_core::editor::AiNetworkCheck {
            endpoint: "https://api.anthropic.com".into(),
            reachable: true,
            http_status: Some(200),
            latency_ms: 42,
            error: None,
        });
        let report = build_ai_status_report(&editor, &None);
        assert!(report.contains("Connectivity"));
        assert!(report.contains("https://api.anthropic.com"));
        assert!(report.contains("Reachable:  OK"));
        assert!(report.contains("HTTP:       200"));
        assert!(report.contains("Latency:    42ms"));
    }

    #[test]
    fn ai_status_report_network_with_data() {
        let mut editor = mae_core::Editor::new();
        editor.ai_api_call_count = 5;
        editor.ai_last_api_latency_ms = Some(123);
        editor.ai_last_api_success = Some(std::time::Instant::now());
        editor.ai_last_api_error = Some("timeout".to_string());
        let report = build_ai_status_report(&editor, &None);
        assert!(report.contains("API Calls:  5"));
        assert!(report.contains("Latency:    123ms"));
        assert!(report.contains("Last Error: timeout"));
    }
}
