//! DapManager — coordinates a single DAP session and surfaces a
//! normalized event/command stream to the editor.
//!
//! Unlike the LSP manager which juggles multiple servers (one per
//! language), DAP sessions are one-at-a-time: you're debugging one
//! program. The manager therefore owns at most one `DapClient`.
//!
//! The editor holds a command channel to a long-running task (see
//! `run_dap_task`). The task owns the manager, drains the client's
//! event channel, and emits `DapTaskEvent`s the editor translates
//! into updates to `mae_core::DebugState` + renderer markers.
//!
//! Design parity note: this mirrors the shape of `LspCommand`/
//! `LspTaskEvent`/`run_lsp_task` so the editor's event loop stays
//! uniform across LSP/DAP/AI.

use tokio::sync::mpsc;

use crate::client::{DapClient, DapEventKind, DapServerConfig};
use crate::protocol::{
    Capabilities, DapScope, DapStackFrame, DapThread, DapVariable, SourceBreakpoint,
};

/// Commands the editor sends to the DAP task.
#[derive(Debug)]
pub enum DapCommand {
    /// Spawn an adapter and run through initialize → launch/attach →
    /// configurationDone. `args` is the adapter-specific launch/attach
    /// JSON payload (program, args, cwd, etc.).
    StartSession {
        config: DapServerConfig,
        launch_args: serde_json::Value,
        /// If true, send `attach` instead of `launch`.
        attach: bool,
    },
    /// Replace the breakpoints for `source_path`.
    SetBreakpoints {
        source_path: String,
        breakpoints: Vec<SourceBreakpoint>,
    },
    /// Resume execution on `thread_id`.
    Continue { thread_id: i64 },
    /// Step over on `thread_id`.
    Next { thread_id: i64 },
    /// Step in on `thread_id`.
    StepIn { thread_id: i64 },
    /// Step out on `thread_id`.
    StepOut { thread_id: i64 },
    /// Pull fresh threads + stack frames for the current stopped state.
    /// Emits ThreadsResult / StackTraceResult.
    RefreshThreadsAndStack {
        /// Optional: if provided, request stack for this thread;
        /// otherwise fetch stack for the first thread in the list.
        thread_id: Option<i64>,
    },
    /// Pull scopes for a stack frame. Emits ScopesResult.
    RequestScopes { frame_id: i64 },
    /// Pull variables for a variables_reference. Emits VariablesResult.
    RequestVariables {
        /// Scope name (used to group variables on the editor side).
        scope_name: String,
        variables_reference: i64,
    },
    /// Soft terminate the debuggee.
    Terminate,
    /// Hard disconnect (terminates the adapter process).
    Disconnect { terminate_debuggee: bool },
    /// Shutdown the manager task cleanly (no disconnect sent).
    Shutdown,
}

/// Events the DAP task forwards to the editor.
#[derive(Debug)]
pub enum DapTaskEvent {
    /// Session is initialized and configurationDone has succeeded.
    SessionStarted {
        adapter_id: String,
        capabilities: Option<Capabilities>,
    },
    /// Session failed to start (initialize, launch, or configurationDone).
    SessionStartFailed { error: String },
    /// Adapter emitted a `stopped` event.
    Stopped {
        reason: String,
        thread_id: Option<i64>,
        text: Option<String>,
    },
    /// Adapter emitted a `continued` event.
    Continued {
        thread_id: i64,
        all_threads: bool,
    },
    /// Adapter emitted a `thread` event (started/exited).
    ThreadEvent { reason: String, thread_id: i64 },
    /// Adapter emitted an `output` event.
    Output { category: String, output: String },
    /// Adapter emitted `terminated` — debuggee finished normally.
    Terminated,
    /// Adapter process exited.
    AdapterExited,
    /// Generic error (from a failed request or a transport issue).
    Error { message: String },
    /// Response to RefreshThreadsAndStack.
    ThreadsResult { threads: Vec<DapThread> },
    /// Response to RefreshThreadsAndStack.
    StackTraceResult {
        thread_id: i64,
        frames: Vec<DapStackFrame>,
    },
    /// Response to RequestScopes.
    ScopesResult {
        frame_id: i64,
        scopes: Vec<DapScope>,
    },
    /// Response to RequestVariables.
    VariablesResult {
        scope_name: String,
        variables: Vec<DapVariable>,
    },
    /// setBreakpoints round-tripped. Verified/unverified status per line.
    BreakpointsSet {
        source_path: String,
        breakpoints: Vec<crate::protocol::DapBreakpoint>,
    },
}

/// Wraps a started DAP client with a captured event receiver.
struct Session {
    client: DapClient,
    event_rx: mpsc::Receiver<DapEventKind>,
    /// Retained for tracing/diagnostics — not currently surfaced.
    #[allow(dead_code)]
    adapter_id: String,
}

impl Session {
    /// Take ownership of the client's event receiver so the manager can
    /// drain it alongside command processing.
    fn new(mut client: DapClient, adapter_id: String) -> Self {
        // Swap a dummy receiver in so we own the real one here.
        let (_dummy_tx, dummy_rx) = mpsc::channel::<DapEventKind>(1);
        let event_rx = std::mem::replace(&mut client.event_rx, dummy_rx);
        Session {
            client,
            event_rx,
            adapter_id,
        }
    }
}

/// Long-running DAP task. Owns a single session at a time.
///
/// Exits when `cmd_rx` is closed or `DapCommand::Shutdown` is received.
pub async fn run_dap_task(
    mut cmd_rx: mpsc::Receiver<DapCommand>,
    event_tx: mpsc::Sender<DapTaskEvent>,
) {
    let mut session: Option<Session> = None;

    loop {
        tokio::select! {
            // Prefer commands when both are ready — keeps responses
            // flowing to user actions while events are still drained.
            biased;

            cmd = cmd_rx.recv() => {
                match cmd {
                    None => break,
                    Some(DapCommand::Shutdown) => break,
                    Some(c) => handle_command(&mut session, c, &event_tx).await,
                }
            }

            // Drain adapter events — only polled if a session is active.
            evt = async {
                match session.as_mut() {
                    Some(s) => s.event_rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(evt) = evt {
                    handle_adapter_event(evt, &mut session, &event_tx).await;
                }
            }
        }
    }

    // Clean shutdown: best-effort disconnect.
    if let Some(s) = session.take() {
        let _ = s.client.disconnect(false).await;
    }
}

async fn handle_command(
    session: &mut Option<Session>,
    cmd: DapCommand,
    event_tx: &mpsc::Sender<DapTaskEvent>,
) {
    match cmd {
        DapCommand::StartSession {
            config,
            launch_args,
            attach,
        } => {
            if session.is_some() {
                let _ = event_tx
                    .send(DapTaskEvent::SessionStartFailed {
                        error: "a DAP session is already active".into(),
                    })
                    .await;
                return;
            }
            let adapter_id = config.adapter_id.clone();
            match DapClient::start(config).await {
                Ok(client) => {
                    let caps = client.capabilities.clone();
                    let mut sess = Session::new(client, adapter_id.clone());

                    // Wait for the `initialized` event before configurationDone.
                    // Other events that arrive in the meantime are forwarded
                    // unchanged to the editor.
                    let initialized_seen = wait_for_initialized(&mut sess, event_tx).await;

                    // Send launch/attach concurrently with initialized wait is
                    // technically allowed by the spec, but keeping the order
                    // sequential (wait → launch → configurationDone) is the
                    // most broadly-compatible flow.
                    let launch_result = if attach {
                        sess.client.attach(launch_args).await
                    } else {
                        sess.client.launch(launch_args).await
                    };
                    if let Err(e) = launch_result {
                        let _ = event_tx
                            .send(DapTaskEvent::SessionStartFailed {
                                error: format!("launch/attach failed: {}", e),
                            })
                            .await;
                        let _ = sess.client.disconnect(true).await;
                        return;
                    }

                    // Only send configurationDone if the adapter advertises it
                    // AND we've seen the initialized event.
                    let supports_cfg_done = caps
                        .as_ref()
                        .map(|c| c.supports_configuration_done_request)
                        .unwrap_or(false);
                    if initialized_seen && supports_cfg_done {
                        if let Err(e) = sess.client.configuration_done().await {
                            let _ = event_tx
                                .send(DapTaskEvent::SessionStartFailed {
                                    error: format!("configurationDone failed: {}", e),
                                })
                                .await;
                            let _ = sess.client.disconnect(true).await;
                            return;
                        }
                    }
                    sess.client.mark_initialized();
                    let _ = event_tx
                        .send(DapTaskEvent::SessionStarted {
                            adapter_id,
                            capabilities: caps,
                        })
                        .await;
                    *session = Some(sess);
                }
                Err(e) => {
                    let _ = event_tx
                        .send(DapTaskEvent::SessionStartFailed { error: e })
                        .await;
                }
            }
        }
        DapCommand::SetBreakpoints {
            source_path,
            breakpoints,
        } => {
            let Some(sess) = session.as_ref() else {
                let _ = event_tx
                    .send(DapTaskEvent::Error {
                        message: "no DAP session active".into(),
                    })
                    .await;
                return;
            };
            match sess.client.set_breakpoints(&source_path, breakpoints).await {
                Ok(body) => {
                    let _ = event_tx
                        .send(DapTaskEvent::BreakpointsSet {
                            source_path,
                            breakpoints: body.breakpoints,
                        })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(DapTaskEvent::Error { message: e })
                        .await;
                }
            }
        }
        DapCommand::Continue { thread_id } => {
            forward_exec(session, event_tx, "continue", thread_id).await;
        }
        DapCommand::Next { thread_id } => {
            forward_exec(session, event_tx, "next", thread_id).await;
        }
        DapCommand::StepIn { thread_id } => {
            forward_exec(session, event_tx, "stepIn", thread_id).await;
        }
        DapCommand::StepOut { thread_id } => {
            forward_exec(session, event_tx, "stepOut", thread_id).await;
        }
        DapCommand::RefreshThreadsAndStack { thread_id } => {
            let Some(sess) = session.as_ref() else {
                let _ = event_tx
                    .send(DapTaskEvent::Error {
                        message: "no DAP session active".into(),
                    })
                    .await;
                return;
            };
            let threads = match sess.client.threads().await {
                Ok(t) => t,
                Err(e) => {
                    let _ = event_tx
                        .send(DapTaskEvent::Error { message: e })
                        .await;
                    return;
                }
            };
            let _ = event_tx
                .send(DapTaskEvent::ThreadsResult {
                    threads: threads.clone(),
                })
                .await;

            // Pick the first thread if the caller didn't specify one.
            let target_thread = thread_id.or_else(|| threads.first().map(|t| t.id));
            if let Some(tid) = target_thread {
                match sess.client.stack_trace(tid, Some(64)).await {
                    Ok(body) => {
                        let _ = event_tx
                            .send(DapTaskEvent::StackTraceResult {
                                thread_id: tid,
                                frames: body.stack_frames,
                            })
                            .await;
                    }
                    Err(e) => {
                        let _ = event_tx
                            .send(DapTaskEvent::Error { message: e })
                            .await;
                    }
                }
            }
        }
        DapCommand::RequestScopes { frame_id } => {
            let Some(sess) = session.as_ref() else {
                let _ = event_tx
                    .send(DapTaskEvent::Error {
                        message: "no DAP session active".into(),
                    })
                    .await;
                return;
            };
            match sess.client.scopes(frame_id).await {
                Ok(scopes) => {
                    let _ = event_tx
                        .send(DapTaskEvent::ScopesResult { frame_id, scopes })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(DapTaskEvent::Error { message: e })
                        .await;
                }
            }
        }
        DapCommand::RequestVariables {
            scope_name,
            variables_reference,
        } => {
            let Some(sess) = session.as_ref() else {
                let _ = event_tx
                    .send(DapTaskEvent::Error {
                        message: "no DAP session active".into(),
                    })
                    .await;
                return;
            };
            match sess.client.variables(variables_reference).await {
                Ok(variables) => {
                    let _ = event_tx
                        .send(DapTaskEvent::VariablesResult {
                            scope_name,
                            variables,
                        })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(DapTaskEvent::Error { message: e })
                        .await;
                }
            }
        }
        DapCommand::Terminate => {
            if let Some(sess) = session.as_ref() {
                if let Err(e) = sess.client.terminate().await {
                    let _ = event_tx
                        .send(DapTaskEvent::Error { message: e })
                        .await;
                }
            }
        }
        DapCommand::Disconnect { terminate_debuggee } => {
            if let Some(sess) = session.take() {
                let _ = sess.client.disconnect(terminate_debuggee).await;
            }
        }
        DapCommand::Shutdown => {
            // handled in run_dap_task loop
        }
    }
}

async fn forward_exec(
    session: &Option<Session>,
    event_tx: &mpsc::Sender<DapTaskEvent>,
    cmd_name: &str,
    thread_id: i64,
) {
    let Some(sess) = session.as_ref() else {
        let _ = event_tx
            .send(DapTaskEvent::Error {
                message: "no DAP session active".into(),
            })
            .await;
        return;
    };
    let result = match cmd_name {
        "continue" => sess.client.continue_(thread_id).await,
        "next" => sess.client.next(thread_id).await,
        "stepIn" => sess.client.step_in(thread_id).await,
        "stepOut" => sess.client.step_out(thread_id).await,
        _ => unreachable!("unknown exec command: {}", cmd_name),
    };
    if let Err(e) = result {
        let _ = event_tx
            .send(DapTaskEvent::Error { message: e })
            .await;
    }
}

/// Wait for the `initialized` event with a short timeout. Returns whether
/// we saw it. Other events that arrive while waiting are forwarded to
/// the editor unchanged so nothing is lost.
async fn wait_for_initialized(
    sess: &mut Session,
    event_tx: &mpsc::Sender<DapTaskEvent>,
) -> bool {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        match tokio::time::timeout(remaining, sess.event_rx.recv()).await {
            Ok(Some(DapEventKind::Event(e))) if e.event == "initialized" => return true,
            Ok(Some(DapEventKind::Event(e))) => {
                forward_adapter_event(e, event_tx).await;
            }
            Ok(Some(DapEventKind::AdapterExited)) => {
                let _ = event_tx.send(DapTaskEvent::AdapterExited).await;
                return false;
            }
            Ok(Some(DapEventKind::Error(msg))) => {
                let _ = event_tx
                    .send(DapTaskEvent::Error { message: msg })
                    .await;
            }
            Ok(Some(DapEventKind::OrphanResponse(_))) => continue,
            Ok(Some(DapEventKind::ReverseRequest(_))) => continue,
            Ok(None) => return false,
            Err(_) => return false,
        }
    }
}

async fn handle_adapter_event(
    evt: DapEventKind,
    session: &mut Option<Session>,
    event_tx: &mpsc::Sender<DapTaskEvent>,
) {
    match evt {
        DapEventKind::Event(e) => forward_adapter_event(e, event_tx).await,
        DapEventKind::AdapterExited => {
            *session = None;
            let _ = event_tx.send(DapTaskEvent::AdapterExited).await;
        }
        DapEventKind::Error(msg) => {
            let _ = event_tx
                .send(DapTaskEvent::Error { message: msg })
                .await;
        }
        DapEventKind::OrphanResponse(_) => {}
        DapEventKind::ReverseRequest(_) => {}
    }
}

async fn forward_adapter_event(e: crate::protocol::DapEvent, event_tx: &mpsc::Sender<DapTaskEvent>) {
    use crate::protocol::{OutputEventBody, StoppedEventBody, TerminatedEventBody};

    match e.event.as_str() {
        "stopped" => {
            if let Some(body) = e
                .body
                .clone()
                .and_then(|v| serde_json::from_value::<StoppedEventBody>(v).ok())
            {
                let _ = event_tx
                    .send(DapTaskEvent::Stopped {
                        reason: body.reason,
                        thread_id: body.thread_id,
                        text: body.text,
                    })
                    .await;
            }
        }
        "continued" => {
            let thread_id = e
                .body
                .as_ref()
                .and_then(|v| v.get("threadId"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let all_threads = e
                .body
                .as_ref()
                .and_then(|v| v.get("allThreadsContinued"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let _ = event_tx
                .send(DapTaskEvent::Continued {
                    thread_id,
                    all_threads,
                })
                .await;
        }
        "thread" => {
            let reason = e
                .body
                .as_ref()
                .and_then(|v| v.get("reason"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let thread_id = e
                .body
                .as_ref()
                .and_then(|v| v.get("threadId"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let _ = event_tx
                .send(DapTaskEvent::ThreadEvent { reason, thread_id })
                .await;
        }
        "output" => {
            if let Some(body) = e
                .body
                .clone()
                .and_then(|v| serde_json::from_value::<OutputEventBody>(v).ok())
            {
                let _ = event_tx
                    .send(DapTaskEvent::Output {
                        category: body.category.unwrap_or_else(|| "console".into()),
                        output: body.output,
                    })
                    .await;
            }
        }
        "terminated" => {
            // Body is optional — we don't expose the restart hint.
            let _ = e
                .body
                .as_ref()
                .and_then(|v| serde_json::from_value::<TerminatedEventBody>(v.clone()).ok());
            let _ = event_tx.send(DapTaskEvent::Terminated).await;
        }
        _ => {
            // Drop other events silently (exited, breakpoint, module, ...)
            // — editor doesn't need them yet.
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::DapResponse;
    use crate::transport::DapTransport;

    /// Scripted mock adapter: reads messages, produces responses keyed by
    /// the command name (not by order). Events can be emitted at will.
    ///
    /// This is a richer variant of the mock used in `client::tests` — the
    /// manager tests need interleaved responses and events.
    struct MockAdapter {
        tx: mpsc::Sender<MockAction>,
    }

    enum MockAction {
        /// Reply to the next matching `command` with success + this body.
        RespondTo {
            command: String,
            body: Option<serde_json::Value>,
            success: bool,
            message: Option<String>,
        },
        /// Emit an unsolicited event.
        Emit {
            event: String,
            body: Option<serde_json::Value>,
        },
    }

    fn spawn_mock() -> (
        impl tokio::io::AsyncRead + Unpin + Send + 'static,
        impl tokio::io::AsyncWrite + Unpin + Send + 'static,
        MockAdapter,
    ) {
        let (client_side, adapter_side) = tokio::io::duplex(16384);
        let (client_read, client_write) = tokio::io::split(client_side);
        let (adapter_read, adapter_write) = tokio::io::split(adapter_side);

        let (action_tx, mut action_rx) = mpsc::channel::<MockAction>(32);

        tokio::spawn(async move {
            let mut transport = DapTransport::new(adapter_read, adapter_write);
            let mut out_seq: i64 = 1000;
            // Pre-load actions. For any RespondTo action, we block until a
            // matching request arrives before sending. Emit actions fire
            // immediately. We poll both the action channel and the transport
            // round-robin.
            let mut pending_responses: Vec<(String, Option<serde_json::Value>, bool, Option<String>)> = Vec::new();

            loop {
                tokio::select! {
                    biased;
                    act = action_rx.recv() => {
                        match act {
                            Some(MockAction::Emit { event, body }) => {
                                let msg = crate::protocol::DapMessage::Event(
                                    crate::protocol::DapEvent {
                                        seq: out_seq,
                                        event,
                                        body,
                                    },
                                );
                                out_seq += 1;
                                if transport.write_message(&msg).await.is_err() {
                                    break;
                                }
                            }
                            Some(MockAction::RespondTo {
                                command,
                                body,
                                success,
                                message,
                            }) => {
                                pending_responses.push((command, body, success, message));
                            }
                            None => break,
                        }
                    }
                    msg = transport.read_message() => {
                        match msg {
                            Ok(crate::protocol::DapMessage::Request(r)) => {
                                if let Some(pos) =
                                    pending_responses.iter().position(|p| p.0 == r.command)
                                {
                                    let (cmd, body, success, message) =
                                        pending_responses.remove(pos);
                                    let resp = crate::protocol::DapMessage::Response(
                                        DapResponse {
                                            seq: out_seq,
                                            request_seq: r.seq,
                                            success,
                                            command: cmd,
                                            message,
                                            body,
                                        },
                                    );
                                    out_seq += 1;
                                    if transport.write_message(&resp).await.is_err() {
                                        break;
                                    }
                                } else {
                                    // No pending response → answer with a generic success
                                    let resp = crate::protocol::DapMessage::Response(
                                        DapResponse {
                                            seq: out_seq,
                                            request_seq: r.seq,
                                            success: true,
                                            command: r.command,
                                            message: None,
                                            body: None,
                                        },
                                    );
                                    out_seq += 1;
                                    if transport.write_message(&resp).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                }
            }
        });

        (
            client_read,
            client_write,
            MockAdapter { tx: action_tx },
        )
    }

    impl MockAdapter {
        async fn respond(&self, command: &str, body: serde_json::Value) {
            self.tx
                .send(MockAction::RespondTo {
                    command: command.into(),
                    body: Some(body),
                    success: true,
                    message: None,
                })
                .await
                .unwrap();
        }

        async fn respond_ok(&self, command: &str) {
            self.tx
                .send(MockAction::RespondTo {
                    command: command.into(),
                    body: None,
                    success: true,
                    message: None,
                })
                .await
                .unwrap();
        }

        async fn emit(&self, event: &str, body: serde_json::Value) {
            self.tx
                .send(MockAction::Emit {
                    event: event.into(),
                    body: Some(body),
                })
                .await
                .unwrap();
        }
    }

    /// Start a manager in its own task, then synthesize a completed session
    /// by hand-crafting a DapClient from streams and injecting it. This
    /// exercises the command/event translation without the StartSession
    /// spawn-subprocess path (which requires a real `command` on $PATH).
    async fn manager_with_session() -> (
        mpsc::Sender<DapCommand>,
        mpsc::Receiver<DapTaskEvent>,
        MockAdapter,
    ) {
        let (r, w, adapter) = spawn_mock();

        // Script the initialize handshake.
        adapter
            .respond(
                "initialize",
                serde_json::json!({
                    "supportsConfigurationDoneRequest": true,
                }),
            )
            .await;
        adapter.emit("initialized", serde_json::json!({})).await;

        // Launch path inside the manager will send launch + configurationDone.
        adapter.respond_ok("launch").await;
        adapter.respond_ok("configurationDone").await;

        let (cmd_tx, cmd_rx) = mpsc::channel::<DapCommand>(16);
        let (evt_tx, evt_rx) = mpsc::channel::<DapTaskEvent>(32);

        // Build the client first (so the initialize handshake succeeds
        // synchronously before the manager takes over the event channel).
        let client = DapClient::from_streams(r, w, "mock").await.unwrap();

        // Inject the already-running client as a manual session by spawning
        // a variant of run_dap_task that starts with a preloaded session.
        tokio::spawn(async move {
            let mut sess = Session::new(client, "mock".into());

            // Drain the rest of the initialize handshake: wait for the
            // `initialized` event, then issue launch + configurationDone.
            let _ = wait_for_initialized(&mut sess, &evt_tx).await;
            let _ = sess.client.launch(serde_json::json!({})).await;
            let _ = sess.client.configuration_done().await;
            sess.client.mark_initialized();
            let _ = evt_tx
                .send(DapTaskEvent::SessionStarted {
                    adapter_id: "mock".into(),
                    capabilities: sess.client.capabilities.clone(),
                })
                .await;

            let mut session = Some(sess);
            let mut cmd_rx = cmd_rx;
            loop {
                tokio::select! {
                    biased;
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            None => break,
                            Some(DapCommand::Shutdown) => break,
                            Some(c) => handle_command(&mut session, c, &evt_tx).await,
                        }
                    }
                    evt = async {
                        match session.as_mut() {
                            Some(s) => s.event_rx.recv().await,
                            None => std::future::pending().await,
                        }
                    } => {
                        if let Some(evt) = evt {
                            handle_adapter_event(evt, &mut session, &evt_tx).await;
                        }
                    }
                }
            }
        });

        (cmd_tx, evt_rx, adapter)
    }

    async fn recv_with_timeout(
        rx: &mut mpsc::Receiver<DapTaskEvent>,
    ) -> DapTaskEvent {
        tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
            .await
            .expect("timed out waiting for DapTaskEvent")
            .expect("DapTaskEvent channel closed")
    }

    #[tokio::test]
    async fn session_started_event_after_handshake() {
        let (_cmd, mut evt, _adapter) = manager_with_session().await;
        let e = recv_with_timeout(&mut evt).await;
        match e {
            DapTaskEvent::SessionStarted { adapter_id, .. } => assert_eq!(adapter_id, "mock"),
            other => panic!("expected SessionStarted, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn stopped_event_translated() {
        let (_cmd, mut evt, adapter) = manager_with_session().await;
        // Drain SessionStarted
        let _ = recv_with_timeout(&mut evt).await;

        adapter
            .emit(
                "stopped",
                serde_json::json!({"reason": "breakpoint", "threadId": 7}),
            )
            .await;

        let e = recv_with_timeout(&mut evt).await;
        match e {
            DapTaskEvent::Stopped {
                reason, thread_id, ..
            } => {
                assert_eq!(reason, "breakpoint");
                assert_eq!(thread_id, Some(7));
            }
            other => panic!("expected Stopped, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn continued_event_translated() {
        let (_cmd, mut evt, adapter) = manager_with_session().await;
        let _ = recv_with_timeout(&mut evt).await;
        adapter
            .emit(
                "continued",
                serde_json::json!({"threadId": 3, "allThreadsContinued": true}),
            )
            .await;
        let e = recv_with_timeout(&mut evt).await;
        match e {
            DapTaskEvent::Continued {
                thread_id,
                all_threads,
            } => {
                assert_eq!(thread_id, 3);
                assert!(all_threads);
            }
            other => panic!("expected Continued, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn output_event_translated() {
        let (_cmd, mut evt, adapter) = manager_with_session().await;
        let _ = recv_with_timeout(&mut evt).await;
        adapter
            .emit(
                "output",
                serde_json::json!({"category": "stdout", "output": "hello\n"}),
            )
            .await;
        match recv_with_timeout(&mut evt).await {
            DapTaskEvent::Output { category, output } => {
                assert_eq!(category, "stdout");
                assert_eq!(output, "hello\n");
            }
            other => panic!("expected Output, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn set_breakpoints_round_trip() {
        let (cmd, mut evt, adapter) = manager_with_session().await;
        let _ = recv_with_timeout(&mut evt).await;

        adapter
            .respond(
                "setBreakpoints",
                serde_json::json!({
                    "breakpoints": [
                        {"id": 11, "verified": true, "line": 42}
                    ]
                }),
            )
            .await;

        cmd.send(DapCommand::SetBreakpoints {
            source_path: "/tmp/x.rs".into(),
            breakpoints: vec![SourceBreakpoint {
                line: 42,
                condition: None,
                hit_condition: None,
            }],
        })
        .await
        .unwrap();

        match recv_with_timeout(&mut evt).await {
            DapTaskEvent::BreakpointsSet {
                source_path,
                breakpoints,
            } => {
                assert_eq!(source_path, "/tmp/x.rs");
                assert_eq!(breakpoints.len(), 1);
                assert_eq!(breakpoints[0].line, Some(42));
                assert!(breakpoints[0].verified);
            }
            other => panic!("expected BreakpointsSet, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn continue_forwards_to_adapter() {
        let (cmd, mut evt, _adapter) = manager_with_session().await;
        let _ = recv_with_timeout(&mut evt).await;
        // Mock adapter auto-replies with success to unlisted commands.
        cmd.send(DapCommand::Continue { thread_id: 1 }).await.unwrap();
        // No event expected on success. A timeout here means success.
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(100), evt.recv()).await;
        assert!(result.is_err(), "unexpected event after continue");
    }

    #[tokio::test]
    async fn refresh_threads_and_stack_emits_both() {
        let (cmd, mut evt, adapter) = manager_with_session().await;
        let _ = recv_with_timeout(&mut evt).await;

        adapter
            .respond(
                "threads",
                serde_json::json!({
                    "threads": [{"id": 1, "name": "main"}]
                }),
            )
            .await;
        adapter
            .respond(
                "stackTrace",
                serde_json::json!({
                    "stackFrames": [
                        {"id": 100, "name": "main", "line": 5, "column": 0}
                    ]
                }),
            )
            .await;

        cmd.send(DapCommand::RefreshThreadsAndStack { thread_id: None })
            .await
            .unwrap();

        match recv_with_timeout(&mut evt).await {
            DapTaskEvent::ThreadsResult { threads } => {
                assert_eq!(threads.len(), 1);
                assert_eq!(threads[0].id, 1);
            }
            other => panic!("expected ThreadsResult, got: {:?}", other),
        }
        match recv_with_timeout(&mut evt).await {
            DapTaskEvent::StackTraceResult { thread_id, frames } => {
                assert_eq!(thread_id, 1);
                assert_eq!(frames.len(), 1);
                assert_eq!(frames[0].id, 100);
                assert_eq!(frames[0].line, 5);
            }
            other => panic!("expected StackTraceResult, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn scopes_and_variables_round_trip() {
        let (cmd, mut evt, adapter) = manager_with_session().await;
        let _ = recv_with_timeout(&mut evt).await;

        adapter
            .respond(
                "scopes",
                serde_json::json!({
                    "scopes": [
                        {"name": "Locals", "variablesReference": 1001, "expensive": false}
                    ]
                }),
            )
            .await;
        adapter
            .respond(
                "variables",
                serde_json::json!({
                    "variables": [
                        {"name": "x", "value": "42", "variablesReference": 0}
                    ]
                }),
            )
            .await;

        cmd.send(DapCommand::RequestScopes { frame_id: 100 })
            .await
            .unwrap();
        match recv_with_timeout(&mut evt).await {
            DapTaskEvent::ScopesResult { frame_id, scopes } => {
                assert_eq!(frame_id, 100);
                assert_eq!(scopes.len(), 1);
                assert_eq!(scopes[0].name, "Locals");
            }
            other => panic!("expected ScopesResult, got: {:?}", other),
        }

        cmd.send(DapCommand::RequestVariables {
            scope_name: "Locals".into(),
            variables_reference: 1001,
        })
        .await
        .unwrap();
        match recv_with_timeout(&mut evt).await {
            DapTaskEvent::VariablesResult {
                scope_name,
                variables,
            } => {
                assert_eq!(scope_name, "Locals");
                assert_eq!(variables.len(), 1);
                assert_eq!(variables[0].name, "x");
                assert_eq!(variables[0].value, "42");
            }
            other => panic!("expected VariablesResult, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn command_without_session_emits_error() {
        // Fresh run_dap_task without any session.
        let (cmd_tx, cmd_rx) = mpsc::channel::<DapCommand>(8);
        let (evt_tx, mut evt_rx) = mpsc::channel::<DapTaskEvent>(8);
        tokio::spawn(run_dap_task(cmd_rx, evt_tx));

        cmd_tx
            .send(DapCommand::Continue { thread_id: 1 })
            .await
            .unwrap();

        match tokio::time::timeout(std::time::Duration::from_millis(500), evt_rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            DapTaskEvent::Error { message } => {
                assert!(message.contains("no DAP session"));
            }
            other => panic!("expected Error, got: {:?}", other),
        }

        drop(cmd_tx);
    }

    #[tokio::test]
    async fn run_dap_task_shuts_down_on_command() {
        let (cmd_tx, cmd_rx) = mpsc::channel::<DapCommand>(8);
        let (evt_tx, _evt_rx) = mpsc::channel::<DapTaskEvent>(8);
        let handle = tokio::spawn(run_dap_task(cmd_rx, evt_tx));
        cmd_tx.send(DapCommand::Shutdown).await.unwrap();
        drop(cmd_tx);
        let res =
            tokio::time::timeout(std::time::Duration::from_secs(1), handle).await;
        assert!(res.is_ok(), "run_dap_task didn't exit on Shutdown");
    }
}
