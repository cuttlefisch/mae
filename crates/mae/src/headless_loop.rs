//! Headless event loop (ADR-055) — the same substantive event-processing
//! `run_terminal_loop` does (MCP tool dispatch, AI/LSP/DAP/collab events,
//! health-check/autosave/daemon-supervision), with everything rendering-
//! and-terminal-input-specific removed: no `TerminalRenderer` (its `new()`
//! requires a real TTY and hard-fails under systemd), no crossterm
//! `EventStream`, no frame-rate limiting, no per-iteration viewport/scroll/
//! visual-rows-cache bookkeeping (nothing to scroll with no display
//! attached). A `NullRenderer` is still used for the one thing that
//! genuinely needs *some* `&dyn Renderer` — embedded shell/PTY sizing for
//! the `shell_exec`/`terminal_spawn` MCP tools.
//!
//! Runs until SIGTERM (or Ctrl-C, for local/manual use) — graceful shutdown
//! mirrors `run_terminal_loop`'s `!editor.running` branch: persist history/
//! AI session, send `Shutdown` to the AI/LSP/DAP tasks, then return.

use std::io;

use mae_ai::{AiCommand, AiEvent};
use mae_core::Editor;
use mae_dap::DapCommand;
use mae_lsp::LspCommand;
use mae_renderer::NullRenderer;
use mae_scheme::SchemeRuntime;
use tracing::{info, warn};

use crate::ai_event_handler;
use crate::bootstrap::save_history_on_exit;
use crate::config;
use crate::dap_bridge::{drain_dap_intents, handle_dap_event};
use crate::lsp_bridge::{drain_lsp_intents, handle_lsp_event};
use crate::shell_lifecycle;

/// Same cadence `run_terminal_loop` uses for health-check/autosave/daemon
/// supervision (kept identical rather than re-tuned, since nothing about
/// running headless changes what that cadence is for).
const HEALTH_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// A short, fixed timeout for probing whether something is already
/// listening on a candidate stable socket path -- long enough that a real
/// (even briefly busy) `mae --headless` instance would answer, short enough
/// that a genuinely stale/nothing-there path doesn't make startup feel hung.
const STABLE_SOCKET_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(300);

/// Stable, project-keyed socket path for long-lived headless instances
/// (ADR-055 decision 2): `~/.local/share/mae/headless/{project-hash}.sock`,
/// XDG-first (`data_dir_candidate`, principle #13). Project-scoped
/// short-lived instances (e.g. one the "MAE for VS Code" extension spawns
/// per workspace) keep using the existing `/tmp/mae-{PID}.sock` convention
/// unchanged -- `mae-mcp-shim`'s auto-discovery already handles that case
/// with no changes. This stable path exists so a long-lived, explicitly-
/// started instance can be found without tracking a PID that changes across
/// restarts.
///
/// `project_root` is hashed (SHA-256 of the canonicalized absolute path,
/// truncated to 16 hex chars -- short enough to stay well under Unix domain
/// socket path length limits, long enough that accidental collision between
/// two different real project paths is not a practical concern) rather than
/// used verbatim, since a raw path can contain characters unsafe or awkward
/// in a filename and can be arbitrarily long. `None` when there's no
/// project root to key on (e.g. a headless instance started outside any
/// project) -- callers should fall back to PID-based discovery only in that
/// case, since a stable path only makes sense once there's something stable
/// to key it on.
pub(crate) fn stable_socket_path(
    project_root: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    let root = project_root?;
    let data_dir = crate::pkg::paths::data_dir_candidate("mae")?;
    Some(stable_socket_path_in(&data_dir, root))
}

/// Pure path computation behind [`stable_socket_path`] — no env var or
/// filesystem access, so it's directly unit-testable without the process-
/// global-env-var races a `$XDG_DATA_HOME`-mutating test would risk under
/// parallel test execution (CLAUDE.md's testing-isolation discipline).
/// `data_dir` is the already-resolved `~/.local/share/mae`-equivalent base
/// (i.e. `data_dir_candidate("mae")`'s result); `project_root` need not be
/// canonicalized by the caller — this canonicalizes it itself so relative
/// or symlinked inputs still hash consistently.
fn stable_socket_path_in(
    data_dir: &std::path::Path,
    project_root: &std::path::Path,
) -> std::path::PathBuf {
    let canonical = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let hash = project_hash(&canonical);
    data_dir.join("headless").join(format!("{hash}.sock"))
}

fn project_hash(canonical_root: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(canonical_root.as_os_str().as_encoded_bytes());
    let digest = hasher.finalize();
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Outcome of attempting to claim a stable socket path for this headless
/// instance.
pub(crate) enum StableSocketClaim {
    /// No project root to key a stable path on -- nothing to bind, the
    /// PID-based socket is this instance's only discovery path. Not an
    /// error.
    NoProjectRoot,
    /// The stable path is free (or was stale and has been cleared) and
    /// ready to bind.
    Claimed(std::path::PathBuf),
    /// Something is already live-listening on the stable path -- a second
    /// headless instance for the same project must fail loudly here, never
    /// silently overwrite or share the first instance's socket (ADR-055's
    /// required adversarial test).
    AlreadyRunning(std::path::PathBuf),
}

/// Attempt to claim `stable_socket_path(project_root)` for this instance.
/// Async because it may probe the existing path with a real connection
/// attempt to distinguish "stale file, safe to clear" from "another
/// instance is genuinely listening here."
pub(crate) async fn claim_stable_socket(
    project_root: Option<&std::path::Path>,
) -> io::Result<StableSocketClaim> {
    let Some(path) = stable_socket_path(project_root) else {
        return Ok(StableSocketClaim::NoProjectRoot);
    };
    claim_stable_socket_at(path).await
}

/// Real logic behind [`claim_stable_socket`], operating on an already-
/// resolved path — split out so tests can exercise the probe/stale-file/
/// live-listener logic directly against a tempdir path, without going
/// through `$XDG_DATA_HOME` at all. Mutating that process-global env var
/// from parallel tests would be a real cross-test race (CLAUDE.md's
/// per-test-fixture isolation discipline), not just a style preference.
async fn claim_stable_socket_at(path: std::path::PathBuf) -> io::Result<StableSocketClaim> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let probe = tokio::time::timeout(
            STABLE_SOCKET_PROBE_TIMEOUT,
            tokio::net::UnixStream::connect(&path),
        )
        .await;
        match probe {
            Ok(Ok(_stream)) => {
                // Something answered -- a live instance owns this path.
                return Ok(StableSocketClaim::AlreadyRunning(path));
            }
            _ => {
                // Connection refused/timed out/errored: a stale file from an
                // ungracefully-terminated previous instance (kill -9, power
                // loss). Safe to clear and rebind -- extends the existing
                // `cleanup_stale_mcp_sockets` philosophy to this convention.
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    Ok(StableSocketClaim::Claimed(path))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_headless_loop(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    ai_event_rx: &mut tokio::sync::mpsc::Receiver<AiEvent>,
    ai_event_tx: &tokio::sync::mpsc::Sender<AiEvent>,
    ai_command_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
    lsp_event_rx: &mut tokio::sync::mpsc::Receiver<mae_lsp::LspTaskEvent>,
    lsp_command_tx: &tokio::sync::mpsc::Sender<LspCommand>,
    dap_event_rx: &mut tokio::sync::mpsc::Receiver<mae_dap::DapTaskEvent>,
    dap_command_tx: &tokio::sync::mpsc::Sender<DapCommand>,
    mcp_tool_rx: &mut tokio::sync::mpsc::Receiver<mae_mcp::McpToolRequest>,
    collab_event_rx: &mut tokio::sync::mpsc::Receiver<crate::collab_bridge::CollabEvent>,
    collab_command_tx: &tokio::sync::mpsc::Sender<crate::collab_bridge::CollabCommand>,
    mcp_socket_path: &str,
    all_tools: &[mae_ai::ToolDefinition],
    permission_policy: &mae_ai::PermissionPolicy,
    app_config: &config::Config,
    mcp_client_mgr: &ai_event_handler::McpClientMgrRef,
    sync_broadcaster: &mae_mcp::broadcast::SharedBroadcaster,
) -> io::Result<()> {
    let renderer: NullRenderer = NullRenderer::default();

    let mut deferred_ai_reply: ai_event_handler::DeferredAiReply = None;
    let mut deferred_dap_reply: ai_event_handler::DeferredDapReply = None;
    let mut pending_interactive_event: Option<ai_event_handler::PendingInteractiveEvent> = None;
    let mut deferred_mcp_reply: ai_event_handler::DeferredMcpReply = Vec::new();
    let mut last_mcp_activity: Option<tokio::time::Instant> = None;

    let mut shell_terminals: std::collections::HashMap<usize, mae_shell::ShellTerminal> =
        std::collections::HashMap::new();
    let mut shell_last_dims: std::collections::HashMap<usize, (u16, u16)> =
        std::collections::HashMap::new();

    let mut health_check_interval = tokio::time::interval(HEALTH_CHECK_INTERVAL);
    health_check_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    info!("headless event loop started");

    loop {
        editor
            .heartbeat
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        if !editor.running {
            break;
        }

        let mcp_idle_tick = async {
            if last_mcp_activity.is_some() {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await
            } else {
                std::future::pending::<()>().await
            }
        };

        tokio::select! {
            biased;

            _ = shutdown_signal() => {
                info!("headless: shutdown signal received");
                editor.running = false;
            }

            _ = health_check_interval.tick() => {
                shell_lifecycle::health_check(
                    editor,
                    &mut shell_terminals,
                    deferred_ai_reply.is_some(),
                    last_mcp_activity.is_some() || !deferred_mcp_reply.is_empty(),
                );
                for removed_idx in std::mem::take(&mut editor.pending_buffer_removals) {
                    mae_core::editor::rekey_after_remove(&mut shell_terminals, removed_idx);
                    mae_core::editor::rekey_after_remove(&mut shell_last_dims, removed_idx);
                }
                editor.try_autosave();
                crate::daemon_supervisor::supervise_daemon(editor);
            }

            // No Editor::on_idle_tick() call here (ADR-055 P4, audited not
            // assumed): its only two consumers are the which-key popup and
            // the KB-link hover preview popup -- purely visual, and both are
            // structurally unreachable headless (no leader-keypad, no mouse
            // hover). Calling it on a timer would just be periodic wasted
            // work with no observable effect, exactly what P4 exists to
            // catch; omitting it is the audited-correct answer, not an
            // oversight.

            _ = mcp_idle_tick => {
                if let Some(ts) = last_mcp_activity {
                    if ts.elapsed() > std::time::Duration::from_millis(500)
                        && deferred_mcp_reply.is_empty()
                    {
                        editor.ai.input_lock = mae_core::InputLock::None;
                        last_mcp_activity = None;
                    }
                }
            }

            Some(ai_event) = ai_event_rx.recv() => {
                let ctx = ai_event_handler::AiEventContext {
                    all_tools,
                    permission_policy,
                    deferred_ai_reply: &mut deferred_ai_reply,
                    deferred_dap_reply: &mut deferred_dap_reply,
                    pending_interactive_event: &mut pending_interactive_event,
                    lsp_command_tx,
                    dap_command_tx,
                    ai_event_tx,
                    scheme,
                    mcp_client_mgr,
                };
                ai_event_handler::handle_ai_event(editor, ai_event, ctx);
            }

            Some(lsp_event) = lsp_event_rx.recv() => {
                ai_event_handler::try_resolve_deferred(editor, &lsp_event, &mut deferred_ai_reply);
                if ai_event_handler::try_resolve_deferred_mcp(&lsp_event, &mut deferred_mcp_reply) {
                    last_mcp_activity = Some(tokio::time::Instant::now());
                }
                handle_lsp_event(editor, lsp_command_tx, lsp_event);
            }

            Some(dap_event) = dap_event_rx.recv() => {
                let dap_action = ai_event_handler::try_resolve_deferred_dap(
                    editor, &dap_event, &mut deferred_dap_reply,
                );
                handle_dap_event(editor, dap_event);
                if dap_action == ai_event_handler::DapResolveAction::TransitionedToStackTrace {
                    drain_dap_intents(editor, dap_command_tx);
                }
            }

            Some(mcp_req) = mcp_tool_rx.recv() => {
                editor.ai.input_lock = mae_core::InputLock::McpBusy;
                last_mcp_activity = Some(tokio::time::Instant::now());
                let immediate = ai_event_handler::handle_mcp_request(
                    editor, mcp_req, all_tools, permission_policy,
                    lsp_command_tx, &mut deferred_mcp_reply, scheme,
                );
                if immediate && deferred_mcp_reply.is_empty() {
                    editor.ai.input_lock = mae_core::InputLock::None;
                    last_mcp_activity = None;
                }
                crate::key_handling::drain_hook_evals(editor, scheme);
                crate::sync_broadcast::drain_and_broadcast(editor, sync_broadcaster, Some(collab_command_tx));
            }

            Some(collab_event) = collab_event_rx.recv() => {
                crate::collab_bridge::handle_collab_event(editor, collab_event);
            }
        }

        ai_event_handler::timeout_deferred_reply(editor, &mut deferred_ai_reply);
        ai_event_handler::timeout_deferred_dap_reply(editor, &mut deferred_dap_reply);
        ai_event_handler::timeout_deferred_mcp_reply(editor, &mut deferred_mcp_reply);

        crate::scheme_lsp_bridge::drain_scheme_lsp_intents(editor, scheme);
        drain_lsp_intents(editor, lsp_command_tx);
        crate::scheme_dap_bridge::drain_scheme_dap_intents(editor, scheme);
        drain_dap_intents(editor, dap_command_tx);
        crate::collab_bridge::drain_collab_intents(editor, collab_command_tx);
        crate::collab_bridge::queue_awareness_update(editor);
        crate::collab_bridge::cleanup_stale_awareness(editor);

        shell_lifecycle::drain_agent_setup(editor);
        shell_lifecycle::spawn_pending_shells(
            editor,
            &mut shell_terminals,
            &mut shell_last_dims,
            &renderer,
            mcp_socket_path,
            app_config,
        );
        shell_lifecycle::resize_shells(editor, &renderer, &shell_terminals, &mut shell_last_dims);
        shell_lifecycle::manage_shell_lifecycle(editor, &mut shell_terminals);
        for removed_idx in std::mem::take(&mut editor.pending_buffer_removals) {
            mae_core::editor::rekey_after_remove(&mut shell_terminals, removed_idx);
            mae_core::editor::rekey_after_remove(&mut shell_last_dims, removed_idx);
        }

        let reloads = std::mem::take(&mut editor.pending_module_reloads);
        for module_name in reloads {
            if module_name == "__all__" {
                crate::bootstrap::reload_everything(scheme, editor, None);
            } else if let Some(flavor) = module_name.strip_prefix("__flavor:") {
                crate::bootstrap::switch_keymap_flavor(scheme, editor, flavor);
            } else {
                crate::bootstrap::reload_module(&module_name, scheme, editor);
            }
        }

        if !editor.running {
            info!("headless: editor shutting down");
            editor.fire_hook("app-exit");
            if editor.kb.daemon_hosts_primary() {
                editor.kb_snapshot_primary_to_store();
            }
            if !editor.clean_mode {
                if let Err(e) = save_history_on_exit(editor) {
                    tracing::error!(error = %e, "failed to save history");
                }
                if let Some(data_dir) = editor.mae_data_dir() {
                    crate::bootstrap::save_project_list_on_exit(editor, &data_dir);
                }
            }
            if editor.restore_session {
                if let Some(root) = editor.active_project_root() {
                    let session_path = root.join(".mae/conversation.json");
                    if let Some(parent) = session_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Err(e) = editor.ai_save(&session_path) {
                        if !e.contains("No conversation buffer") {
                            warn!(path = %session_path.display(), error = %e, "failed to persist AI session");
                        }
                    }
                }
            }
            if let Some(ref tx) = ai_command_tx {
                let _ = tx.try_send(AiCommand::Shutdown);
            }
            let _ = lsp_command_tx.try_send(LspCommand::Shutdown);
            let _ = dap_command_tx.try_send(DapCommand::Shutdown);
            break;
        }
    }

    info!("headless event loop exited");
    Ok(())
}

/// Resolves when the process should shut down: SIGTERM (the systemd/launchd
/// stop signal) or Ctrl-C (local/manual `mae --headless` runs). Unix-only —
/// gate G3 scopes ADR-055 to Linux + macOS; Windows service-host shutdown is
/// explicitly out of scope (see issue #386).
#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
    tokio::select! {
        _ = sigterm.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- project_hash / stable_socket_path_in (pure, no env/filesystem) ---

    #[test]
    fn project_hash_is_deterministic() {
        let path = std::path::Path::new("/home/alice/projects/widget");
        assert_eq!(project_hash(path), project_hash(path));
    }

    #[test]
    fn project_hash_differs_for_different_paths() {
        let a = project_hash(std::path::Path::new("/home/alice/projects/widget"));
        let b = project_hash(std::path::Path::new("/home/alice/projects/gadget"));
        assert_ne!(a, b);
    }

    #[test]
    fn project_hash_is_fixed_length_hex() {
        let hash = project_hash(std::path::Path::new("/anything"));
        assert_eq!(hash.len(), 16, "expected 8 bytes -> 16 hex chars");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn stable_socket_path_in_is_project_keyed_and_ends_in_sock() {
        let data_dir = std::path::Path::new("/fake/data/mae");
        let tmp = tempfile::tempdir().unwrap();
        let project_a = tmp.path().join("project-a");
        let project_b = tmp.path().join("project-b");
        std::fs::create_dir_all(&project_a).unwrap();
        std::fs::create_dir_all(&project_b).unwrap();

        let path_a = stable_socket_path_in(data_dir, &project_a);
        let path_b = stable_socket_path_in(data_dir, &project_b);

        assert_ne!(
            path_a, path_b,
            "different projects must get different stable socket paths"
        );
        assert!(path_a.starts_with(data_dir.join("headless")));
        assert_eq!(path_a.extension().unwrap(), "sock");
    }

    #[test]
    fn stable_socket_path_in_is_stable_across_repeated_calls_for_the_same_project() {
        let data_dir = std::path::Path::new("/fake/data/mae");
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("same-project");
        std::fs::create_dir_all(&project).unwrap();

        let first = stable_socket_path_in(data_dir, &project);
        let second = stable_socket_path_in(data_dir, &project);
        assert_eq!(
            first, second,
            "the same project must always resolve to the same stable socket path across restarts"
        );
    }

    #[test]
    fn stable_socket_path_is_none_with_no_project_root() {
        assert!(stable_socket_path(None).is_none());
    }

    // --- claim_stable_socket: the ADR-055 adversarial tests ---

    #[tokio::test]
    async fn claim_stable_socket_with_no_project_root_is_none() {
        let claim = claim_stable_socket(None).await.unwrap();
        assert!(matches!(claim, StableSocketClaim::NoProjectRoot));
    }

    #[tokio::test]
    async fn claim_stable_socket_claims_a_free_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("headless").join("free.sock");

        let claim = claim_stable_socket_at(path.clone()).await.unwrap();
        match claim {
            StableSocketClaim::Claimed(claimed) => {
                assert_eq!(claimed, path);
                assert!(
                    !claimed.exists(),
                    "claiming a free path must not itself create the file"
                );
            }
            _ => panic!("expected Claimed for a fresh path"),
        }
    }

    /// Adversarial test (ADR-055's required "two headless instances racing
    /// to bind the same stable socket path" case): a REAL listener already
    /// bound at the computed path must cause the second claim attempt to
    /// report `AlreadyRunning`, never silently succeed.
    #[tokio::test]
    async fn claim_stable_socket_refuses_when_a_real_listener_is_already_bound() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("headless").join("taken.sock");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _listener =
            tokio::net::UnixListener::bind(&path).expect("bind should succeed on a free path");

        let claim = claim_stable_socket_at(path.clone()).await.unwrap();

        match claim {
            StableSocketClaim::AlreadyRunning(reported_path) => {
                assert_eq!(reported_path, path);
            }
            StableSocketClaim::Claimed(_) => panic!(
                "expected AlreadyRunning with a real listener bound, got Claimed -- a second instance would proceed"
            ),
            StableSocketClaim::NoProjectRoot => panic!("unexpected NoProjectRoot"),
        }
        // The real listener's socket file must be left completely alone —
        // a refused claim must never touch the winning instance's socket.
        assert!(path.exists());
    }

    /// Orphaned-socket-cleanup adversarial test: a stale socket FILE left
    /// behind by an ungracefully-terminated previous instance (kill -9) —
    /// present on disk but nothing listening — must be cleared and claimed
    /// by the next instance, not mistaken for a live one.
    #[tokio::test]
    async fn claim_stable_socket_clears_a_stale_file_with_no_live_listener() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("headless").join("stale.sock");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Simulate an orphaned socket file: bind a listener, then drop it
        // WITHOUT removing the file, matching what a kill -9'd process
        // leaves behind on disk (a Unix listener does not unlink on drop).
        {
            let listener = tokio::net::UnixListener::bind(&path).unwrap();
            drop(listener);
        }
        assert!(
            path.exists(),
            "the stale file must still be present after drop"
        );

        let claim = claim_stable_socket_at(path.clone()).await.unwrap();

        match claim {
            StableSocketClaim::Claimed(claimed_path) => {
                assert_eq!(claimed_path, path);
            }
            _ => panic!("a stale, unconnectable socket file must self-heal into a Claimed result"),
        }
    }
}
