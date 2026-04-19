//! Shared shell terminal lifecycle management for terminal and GUI loops.
//!
//! Both event loops manage identical shell lifecycle: spawning, resizing,
//! input draining, viewport caching, event polling, and cleanup. This
//! module provides shared implementations.

use mae_core::{Editor, InputLock, Mode};
use mae_renderer::Renderer;
use std::collections::HashMap;
use tracing::{debug, error, info, warn};

use crate::agents;
use crate::config;

/// Drain pending agent setup requests (:agent-setup / :agent-list).
pub fn drain_agent_setup(editor: &mut Editor) {
    let Some(agent_name) = editor.pending_agent_setup.take() else {
        return;
    };
    if agent_name == "__list__" {
        let list = agents::agent_list_display();
        editor.set_status(format!("Available agents:\n{}", list));
    } else {
        let root = editor
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(|| std::env::current_dir().ok());
        match root {
            Some(root) => match agents::setup_agent(&agent_name, &root) {
                Ok(msg) => editor.set_status(msg),
                Err(msg) => editor.set_status(msg),
            },
            None => editor.set_status("No project root or working directory available"),
        }
    }
}

/// Spawn any pending shell terminals and auto-write agent configs.
pub fn spawn_pending_shells(
    editor: &mut Editor,
    shell_terminals: &mut HashMap<usize, mae_shell::ShellTerminal>,
    shell_last_dims: &mut HashMap<usize, (u16, u16)>,
    renderer: &dyn Renderer,
    mcp_socket_path: &str,
    app_config: &config::Config,
) {
    let shell_spawns = std::mem::take(&mut editor.pending_shell_spawns);
    let agent_spawns = std::mem::take(&mut editor.pending_agent_spawns);
    let had_shell_spawns = !shell_spawns.is_empty() || !agent_spawns.is_empty();

    for buf_idx in shell_spawns {
        let (inner_cols, inner_rows) = crate::shell_dims_for_buffer(editor, renderer, buf_idx);
        let cwd = editor.active_project_root().map(|p| p.to_path_buf());
        let mut extra_env = HashMap::new();
        extra_env.insert("MAE_MCP_SOCKET".to_string(), mcp_socket_path.to_string());
        match mae_shell::ShellTerminal::spawn_with_env(inner_cols, inner_rows, cwd, extra_env) {
            Ok(shell) => {
                debug!(
                    buf_idx,
                    cols = inner_cols,
                    rows = inner_rows,
                    "shell terminal spawned"
                );
                shell_last_dims.insert(buf_idx, (inner_cols, inner_rows));
                shell_terminals.insert(buf_idx, shell);
            }
            Err(e) => {
                error!(buf_idx, error = %e, "failed to spawn shell terminal");
                editor.set_status(format!("Terminal spawn failed: {}", e));
            }
        }
    }

    // Spawn agent shells: command runs directly as PTY program.
    for (buf_idx, command) in agent_spawns {
        let (inner_cols, inner_rows) = crate::shell_dims_for_buffer(editor, renderer, buf_idx);
        let cwd = editor.active_project_root().map(|p| p.to_path_buf());
        let mut extra_env = HashMap::new();
        extra_env.insert("MAE_MCP_SOCKET".to_string(), mcp_socket_path.to_string());
        match mae_shell::ShellTerminal::spawn_command(
            inner_cols, inner_rows, &command, cwd, extra_env,
        ) {
            Ok(shell) => {
                debug!(buf_idx, %command, "agent terminal spawned");
                shell_last_dims.insert(buf_idx, (inner_cols, inner_rows));
                shell_terminals.insert(buf_idx, shell);
            }
            Err(e) => {
                error!(buf_idx, error = %e, "failed to spawn agent terminal");
                editor.set_status(format!("Agent spawn failed: {}", e));
            }
        }
    }

    // Auto-write .mcp.json and agent settings on first shell spawn.
    if had_shell_spawns {
        let root = editor
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(|| std::env::current_dir().ok());
        if let Some(root) = root {
            if app_config.agents.auto_mcp_json_effective() {
                let shim = agents::resolve_shim_path();
                if let Err(e) = agents::write_mcp_json(&root, &shim) {
                    debug!(error = %e, "failed to write .mcp.json");
                }
            }
            if app_config.agents.auto_approve_tools_effective() {
                if let Err(e) = agents::write_agent_settings(&root) {
                    debug!(error = %e, "failed to write agent settings");
                }
            }
        }
    }
}

/// Dynamic resize: check each shell's owning window dims and resize if needed.
pub fn resize_shells(
    editor: &Editor,
    renderer: &dyn Renderer,
    shell_terminals: &HashMap<usize, mae_shell::ShellTerminal>,
    shell_last_dims: &mut HashMap<usize, (u16, u16)>,
) {
    for (buf_idx, shell) in shell_terminals {
        let dims = crate::shell_dims_for_buffer(editor, renderer, *buf_idx);
        if shell_last_dims.get(buf_idx) != Some(&dims) {
            shell.resize(dims.0, dims.1);
            shell_last_dims.insert(*buf_idx, dims);
        }
    }
}

/// Handle shell resets, closes, event polling, exited shells, input draining,
/// and viewport/CWD caching. Called once per loop iteration.
pub fn manage_shell_lifecycle(
    editor: &mut Editor,
    shell_terminals: &mut HashMap<usize, mae_shell::ShellTerminal>,
) {
    // Reset pending shells.
    for buf_idx in std::mem::take(&mut editor.pending_shell_resets) {
        if let Some(shell) = shell_terminals.get(&buf_idx) {
            shell.reset();
        }
    }

    // Close pending shells.
    for buf_idx in std::mem::take(&mut editor.pending_shell_closes) {
        if let Some(shell) = shell_terminals.remove(&buf_idx) {
            shell.shutdown();
        }
        editor.execute_command("force-kill-buffer");
    }

    // Poll shell events (bell, title, exit).
    let mut exited_shells: Vec<usize> = Vec::new();
    for (buf_idx, shell) in shell_terminals.iter_mut() {
        for event in shell.poll_events() {
            match event {
                mae_shell::ShellEvent::Bell => editor.ring_bell(),
                mae_shell::ShellEvent::Title(t) => {
                    editor.set_status(format!("Terminal: {}", t));
                }
                mae_shell::ShellEvent::ChildExit(code) => {
                    info!(buf_idx, code, "shell process exited");
                    exited_shells.push(*buf_idx);
                }
                _ => {}
            }
        }
    }

    // Handle exited shells.
    for buf_idx in exited_shells {
        if editor.active_buffer_idx() == buf_idx && editor.mode == Mode::ShellInsert {
            editor.mode = Mode::Normal;
        }
        if let Some(shell) = shell_terminals.remove(&buf_idx) {
            shell.shutdown();
        }
        if buf_idx < editor.buffers.len() {
            // Auto-close the exited shell buffer (empty rope = useless frame).
            let label = if editor.buffers[buf_idx].agent_shell {
                "AI agent exited — buffer closed"
            } else {
                "Terminal exited — buffer closed"
            };
            if editor.active_buffer_idx() == buf_idx {
                // Switch away before removing
                let alt = editor.alternate_buffer_idx.unwrap_or(0);
                let target = if alt < editor.buffers.len() && alt != buf_idx {
                    alt
                } else {
                    0
                };
                editor.window_mgr.focused_window_mut().buffer_idx = target;
            }
            editor.buffers.remove(buf_idx);
            // Fix up buffer indices in all windows after removal
            for win in editor.window_mgr.iter_windows_mut() {
                if win.buffer_idx > buf_idx {
                    win.buffer_idx -= 1;
                }
            }
            if let Some(alt) = editor.alternate_buffer_idx.as_mut() {
                if *alt > buf_idx {
                    *alt -= 1;
                } else if *alt == buf_idx {
                    *alt = 0;
                }
            }
            editor.set_status(label);
        }
    }

    // Drain pending shell inputs.
    for (buf_idx, text) in std::mem::take(&mut editor.pending_shell_inputs) {
        if let Some(shell) = shell_terminals.get(&buf_idx) {
            shell.write_str(&text);
        }
    }

    // Cache shell viewport snapshots and CWDs for AI tool access.
    for (buf_idx, shell) in shell_terminals.iter() {
        let viewport = shell.read_viewport(100);
        editor.shell_viewports.insert(*buf_idx, viewport);
        if let Some(cwd) = shell.cwd() {
            editor.shell_cwds.insert(*buf_idx, cwd);
        }
    }
    editor
        .shell_viewports
        .retain(|idx, _| shell_terminals.contains_key(idx));
    editor
        .shell_cwds
        .retain(|idx, _| shell_terminals.contains_key(idx));
}

/// Periodic health check (call every ~30s). Belt-and-suspenders cleanup for:
/// - Shell terminals whose child process exited but weren't caught by `ChildExit`
/// - Stale input locks when no AI session or MCP activity is active
pub fn health_check(
    editor: &mut Editor,
    shell_terminals: &mut HashMap<usize, mae_shell::ShellTerminal>,
    ai_event_active: bool,
    mcp_activity_active: bool,
) {
    // Scan for shells with exited children that weren't cleaned up.
    let zombies: Vec<usize> = shell_terminals
        .iter()
        .filter(|(_, shell)| shell.has_exited())
        .map(|(idx, _)| *idx)
        .collect();

    for buf_idx in zombies {
        warn!(buf_idx, "health check: found zombie shell — cleaning up");
        if editor.active_buffer_idx() == buf_idx && editor.mode == Mode::ShellInsert {
            editor.mode = Mode::Normal;
        }
        if let Some(shell) = shell_terminals.remove(&buf_idx) {
            shell.shutdown();
        }
        if buf_idx < editor.buffers.len() {
            // Auto-close zombie shell buffer (same as normal exit)
            if editor.active_buffer_idx() == buf_idx {
                let alt = editor.alternate_buffer_idx.unwrap_or(0);
                let target = if alt < editor.buffers.len() && alt != buf_idx {
                    alt
                } else {
                    0
                };
                editor.window_mgr.focused_window_mut().buffer_idx = target;
            }
            editor.buffers.remove(buf_idx);
            for win in editor.window_mgr.iter_windows_mut() {
                if win.buffer_idx > buf_idx {
                    win.buffer_idx -= 1;
                }
            }
            if let Some(alt) = editor.alternate_buffer_idx.as_mut() {
                if *alt > buf_idx {
                    *alt -= 1;
                } else if *alt == buf_idx {
                    *alt = 0;
                }
            }
        }
    }

    // Clear stale input locks when the process that set them is no longer active.
    match editor.input_lock {
        InputLock::AiBusy if !ai_event_active => {
            warn!("health check: stale AiBusy lock — clearing");
            editor.input_lock = InputLock::None;
            editor.ai_streaming = false;
            editor.set_status("AI lock cleared (session inactive)");
        }
        InputLock::McpBusy if !mcp_activity_active => {
            warn!("health check: stale McpBusy lock — clearing");
            editor.input_lock = InputLock::None;
            editor.set_status("MCP lock cleared (no pending requests)");
        }
        _ => {}
    }
}
