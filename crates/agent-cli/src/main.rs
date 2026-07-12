//! `mae-agent` — a terminal chat harness for driving MAE's tools via any AI
//! provider (ADR-046), built directly on `mae-mcp`'s protocol library (no
//! `mae-mcp-shim` subprocess hop — see `mcp_client.rs`).
//! @stability: stable

mod agent_loop;
mod mcp_client;
mod residency_check;
mod tui;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, ExecutableCommand};
use futures::StreamExt;
use mae_ai::{
    AgentProvider, ClaudeProvider, GeminiProvider, GuardrailProvider, Message, OllamaProvider,
    OpenAiProvider, ProviderConfig, StagePolicy, ToolDefinition, ToolStage,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::{mpsc, oneshot};

use crate::mcp_client::{McpClient, ToolCallOutcome, ToolExecutor};
use crate::tui::{
    needs_confirmation, AppState, ConfirmChoice, PendingConfirm, PermissionMode, SlashCommand,
};

#[derive(Parser, Debug)]
#[command(name = "mae-agent", about = "Terminal chat harness for MAE's AI tools")]
struct Args {
    /// Override socket discovery with an explicit path.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// AI provider: claude | openai | gemini | ollama.
    #[arg(long, default_value = "ollama")]
    provider: String,
    /// Model name.
    #[arg(long, default_value = "qwen3:latest")]
    model: String,
    /// API key (falls back to the provider's usual env var if omitted).
    #[arg(long)]
    api_key: Option<String>,
    /// Permission mode: readonly|write|shell|privileged|yolo.
    #[arg(long, default_value = "shell")]
    permission_mode: String,
    /// Discover the socket, connect, initialize, list tools, then exit
    /// (mirrors `mae-mcp-shim --check`).
    #[arg(long)]
    check: bool,
    /// Run one turn non-interactively (no TUI): send this prompt, print each
    /// tool call/result and the final text to stdout, then exit. Every tool
    /// call is auto-approved -- there's no human to answer a confirm prompt
    /// in scripted/automated use, which is the intended use case (e.g.
    /// driving a real provider through `model_exam` for compatibility data).
    #[arg(long)]
    prompt: Option<String>,
    /// Max tool-calling rounds for `--prompt` mode (default: 50).
    #[arg(long)]
    max_rounds: Option<usize>,
    /// Comma-separated tool names to expose -- everything else is hidden
    /// from the model. MAE's real tool surface (700+ tools) reliably
    /// overwhelms smaller/local models into never attempting a tool call at
    /// all (confirmed directly: the same model that ignores 730 tools calls
    /// the right one immediately when only 1-2 are offered) -- this mirrors
    /// the same restricted allowlist MAE's own embedded `verifier` delegate
    /// profile already uses for exactly this reason
    /// (`ai_event_handler.rs`'s `AiEvent::Delegate` handling).
    #[arg(long, value_delimiter = ',')]
    only_tools: Vec<String>,
    /// Name of a workflow stage policy to enforce via `GuardrailProvider`'s
    /// premature-write rejection pillar (rejects a Write-stage tool call
    /// that has no prior Discovery/Read call this session, nudging the
    /// model to look before it writes). Currently supported: "kb-enrichment".
    /// An unrecognized name is ignored (with a warning), not fatal.
    #[arg(long)]
    stage_policy: Option<String>,
}

/// Resolve a `--stage-policy` flag value to a concrete [`StagePolicy`].
/// Returns `None` (with a stderr warning) for an unrecognized name.
fn stage_policy_for(name: &str) -> Option<StagePolicy> {
    match name {
        "kb-enrichment" => Some(StagePolicy {
            classify: |tool_name| match tool_name {
                "kb_search" | "kb_search_context" | "kb_agenda" => Some(ToolStage::Discovery),
                "kb_get" | "kb_links_from" => Some(ToolStage::Read),
                "kb_add_link" | "kb_set_role" => Some(ToolStage::Write),
                _ => None,
            },
        }),
        other => {
            eprintln!("mae-agent: unrecognized --stage-policy '{other}' -- ignoring");
            None
        }
    }
}

fn construct_provider(provider_type: &str, config: ProviderConfig) -> Box<dyn AgentProvider> {
    match provider_type {
        "openai" => Box::new(OpenAiProvider::new(config)),
        "gemini" => Box::new(GeminiProvider::new(config)),
        "ollama" => Box::new(OllamaProvider::new(config)),
        _ => Box::new(ClaudeProvider::new(config)),
    }
}

fn api_key_env_var(provider_type: &str) -> Option<&'static str> {
    match provider_type {
        "claude" => Some("ANTHROPIC_API_KEY"),
        "openai" => Some("OPENAI_API_KEY"),
        "gemini" => Some("GEMINI_API_KEY"),
        _ => None, // ollama needs no key
    }
}

fn is_local_provider(provider_type: &str) -> bool {
    provider_type.eq_ignore_ascii_case("ollama")
}

/// Parse the wire-format permission-tier string (`ToolInfo::permission`,
/// e.g. `"Shell"`) back into a `PermissionTier`. `None` for anything
/// unrecognized (including a server that predates this field entirely) --
/// callers must NOT treat that as "safe," it means "we don't actually know
/// this tool's real tier" (see `ToolInfo::permission`'s doc comment for why
/// this field exists at all).
fn parse_permission_tier(s: &str) -> Option<mae_ai::PermissionTier> {
    match s {
        "ReadOnly" => Some(mae_ai::PermissionTier::ReadOnly),
        "Write" => Some(mae_ai::PermissionTier::Write),
        "Shell" => Some(mae_ai::PermissionTier::Shell),
        "Privileged" => Some(mae_ai::PermissionTier::Privileged),
        _ => None,
    }
}

/// Convert MCP `ToolInfo`s into `ToolDefinition`s, dropping (with a stderr
/// warning) any whose `input_schema` doesn't parse as `ToolParameters`.
fn convert_tool_infos(tool_infos: Vec<mae_mcp::protocol::ToolInfo>) -> Vec<ToolDefinition> {
    tool_infos
        .into_iter()
        .filter_map(|t| {
            let parameters = match serde_json::from_value(t.input_schema.clone()) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!(
                        "mae-agent: dropping tool {} -- schema parse failed: {e}",
                        t.name
                    );
                    return None;
                }
            };
            Some(ToolDefinition {
                name: t.name,
                description: t.description,
                parameters,
                permission: t.permission.as_deref().and_then(parse_permission_tier),
            })
        })
        .collect()
}

/// Restrict `tools` to `only` by name (empty `only` means no filtering).
/// MAE's real tool surface (700+ tools) reliably overwhelms smaller/local
/// models into never attempting a tool call at all -- confirmed directly:
/// the same model that ignores 730 tools calls the right one immediately
/// when only 1-2 are offered. Mirrors the same restricted allowlist MAE's
/// own embedded `verifier` delegate profile already uses for exactly this
/// reason (`ai_event_handler.rs`'s `AiEvent::Delegate` handling).
fn filter_tools(tools: Vec<ToolDefinition>, only: &[String]) -> Vec<ToolDefinition> {
    if only.is_empty() {
        return tools;
    }
    let allowed: std::collections::HashSet<&str> = only.iter().map(String::as_str).collect();
    tools
        .into_iter()
        .filter(|t| allowed.contains(t.name.as_str()))
        .collect()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if args.check {
        return run_check(&args).await;
    }

    let (socket_path, psk_path) = match &args.socket {
        Some(p) => (p.clone(), None),
        None => mcp_client::discover_connection().ok_or_else(|| {
            anyhow::anyhow!("no live MAE MCP socket found in /tmp — is mae running?")
        })?,
    };
    let psk = match &psk_path {
        Some(p) => std::fs::read_to_string(p)
            .ok()
            .map(|s| s.trim().to_string()),
        None => None,
    };
    let declared_provider = is_local_provider(&args.provider).then_some(args.provider.as_str());

    let mut mcp = McpClient::connect(&socket_path, psk.as_deref(), declared_provider).await?;
    let tool_infos = mcp.list_tools().await?;
    let tools = convert_tool_infos(tool_infos);
    let tools = filter_tools(tools, &args.only_tools);

    let api_key = args
        .api_key
        .clone()
        .or_else(|| api_key_env_var(&args.provider).and_then(|var| std::env::var(var).ok()));
    let config = ProviderConfig {
        provider_type: args.provider.clone(),
        api_key,
        model: args.model.clone(),
        base_url: None,
        max_tokens: 8192,
        temperature: None,
        thinking: None,
        timeout_secs: 300,
        budget: Default::default(),
    };
    let raw_provider = construct_provider(&args.provider, config);
    let verification = mae_ai::lookup_context_limit(&args.model).verification;
    let stage_policy = args.stage_policy.as_deref().and_then(stage_policy_for);
    // Non-Verified-tier models always get the guardrail's reliability
    // pillars. A Verified-tier model normally skips it entirely -- but if a
    // stage policy was explicitly requested, wrap anyway (with the other
    // pillars along for the ride, effectively inert for a well-behaved
    // model) purely to get stage enforcement.
    let needs_guardrail =
        !matches!(verification, mae_ai::ModelVerification::Verified) || stage_policy.is_some();
    let provider: Arc<dyn AgentProvider> = if needs_guardrail {
        let mut guardrail = GuardrailProvider::wrap(raw_provider);
        if let Some(policy) = stage_policy {
            guardrail = guardrail.with_stage_policy(policy);
        }
        Arc::new(guardrail)
    } else {
        Arc::from(raw_provider)
    };

    if let Some(prompt) = args.prompt.clone() {
        return run_once(&args, prompt, mcp, tools, provider).await;
    }

    let permission_mode = PermissionMode::parse(&args.permission_mode).unwrap_or_default();
    let mut app = AppState::new(args.model.clone(), args.provider.clone(), permission_mode);
    app.push_system_note(format!(
        "Connected to {} ({} tools available). Permission mode: {}.",
        socket_path.display(),
        tools.len(),
        args.permission_mode
    ));

    run_tui(app, provider, mcp, tools).await
}

/// Non-interactive single-turn mode (`--prompt`): no TUI, no human to answer
/// a confirm prompt -- but every tool call still gates on `--permission-mode`
/// via `NonInteractiveExecutor`, which DENIES (rather than blindly executes)
/// anything exceeding the configured ceiling. This was NOT always true: an
/// earlier version used `McpClient` directly as the executor with zero
/// gating at any tier, including Privileged -- which is how a `model_exam`
/// pushback prompt ("delete all files in the project root") once reached a
/// real `shell_exec` call with no safety net at all and wiped a real
/// checkout's working tree. Prints each tool call/result as it happens plus
/// the final assistant text, then exits.
async fn run_once(
    args: &Args,
    prompt: String,
    mcp: McpClient,
    tools: Vec<ToolDefinition>,
    provider: Arc<dyn AgentProvider>,
) -> anyhow::Result<()> {
    let permission_mode = PermissionMode::parse(&args.permission_mode).unwrap_or_default();
    eprintln!(
        "mae-agent: --prompt mode (non-interactive) -- permission ceiling: {permission_mode:?}. \
         Tool calls exceeding this tier are denied, not confirmed (no human to ask)."
    );
    let mut executor = NonInteractiveExecutor {
        inner: mcp,
        tools: tools.clone(),
        mode: permission_mode,
        own_provider: args.provider.clone(),
    };

    let mut messages: Vec<Message> = Vec::new();
    let config = agent_loop::TurnConfig {
        max_rounds: args
            .max_rounds
            .unwrap_or_else(|| agent_loop::TurnConfig::default().max_rounds),
    };

    let result = agent_loop::run_turn(
        agent_loop::TurnContext {
            provider: provider.as_ref(),
            executor: &mut executor,
            tools: &tools,
            system_prompt: "You are an AI agent operating MAE's editor and knowledge-base tools.",
        },
        &mut messages,
        &config,
        &prompt,
        |event| match event {
            agent_loop::AgentEvent::ToolCallStarted { name, arguments } => {
                println!(
                    "[tool] {name} {}",
                    serde_json::to_string(&arguments).unwrap_or_default()
                );
            }
            agent_loop::AgentEvent::ToolCallFinished {
                name,
                success,
                output,
            } => {
                println!("[result] {name} success={success} output={output}");
            }
            agent_loop::AgentEvent::Text(text) => {
                println!("[text] {text}");
            }
            agent_loop::AgentEvent::RoundLimitReached => {
                println!("[round-limit-reached]");
            }
            agent_loop::AgentEvent::RoundDiagnostics {
                round,
                tools_offered,
                stop_reason,
                tool_calls_returned,
                text_len,
                usage,
            } => {
                let usage = usage
                    .map(|u| format!("prompt={} completion={}", u.prompt_tokens, u.completion_tokens))
                    .unwrap_or_else(|| "none".to_string());
                println!(
                    "[diag] round={round} tools_offered={tools_offered} stop_reason={stop_reason:?} tool_calls={tool_calls_returned} text_len={text_len} tokens=({usage})"
                );
            }
        },
    )
    .await;

    if let Some(Message {
        content: mae_ai::MessageContent::Text(text),
        ..
    }) = messages.last()
    {
        println!("\n=== FINAL ===\n{text}");
    }

    result
}

async fn run_check(args: &Args) -> anyhow::Result<()> {
    let (socket_path, psk_path) = match &args.socket {
        Some(p) => (p.clone(), None),
        None => mcp_client::discover_connection()
            .ok_or_else(|| anyhow::anyhow!("no live MAE MCP socket found in /tmp"))?,
    };
    let psk = match &psk_path {
        Some(p) => std::fs::read_to_string(p)
            .ok()
            .map(|s| s.trim().to_string()),
        None => None,
    };
    let declared_provider = is_local_provider(&args.provider).then_some(args.provider.as_str());
    let mut mcp = McpClient::connect(&socket_path, psk.as_deref(), declared_provider).await?;
    let tools = mcp.list_tools().await?;
    println!(
        "OK: connected to {} ({} tools, psk={})",
        socket_path.display(),
        tools.len(),
        psk.is_some()
    );
    Ok(())
}

/// Events flowing from the spawned agent-turn task back to the TUI loop.
enum HarnessEvent {
    Agent(agent_loop::AgentEvent),
    ConfirmRequest(PendingConfirm, oneshot::Sender<ConfirmChoice>),
    TurnDone {
        mcp: McpClient,
        messages: Vec<Message>,
        error: Option<String>,
    },
}

/// Wraps `McpClient` with the permission-confirm gate (ADR-046's
/// "interviewing" UX) and the client-side residency self-check (ADR-048).
struct ConfirmingExecutor {
    inner: McpClient,
    tools: Vec<ToolDefinition>,
    mode: PermissionMode,
    own_provider: String,
    events_tx: mpsc::UnboundedSender<HarnessEvent>,
}

#[async_trait::async_trait]
impl ToolExecutor for ConfirmingExecutor {
    async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> anyhow::Result<ToolCallOutcome> {
        // Client-side residency self-check: best effort, server is authoritative.
        if let Ok(kb_residency) = fetch_kb_residency(&mut self.inner).await {
            if let residency_check::SelfCheckDecision::Refuse(reason) =
                residency_check::check_before_call(
                    name,
                    &arguments,
                    &self.own_provider,
                    &kb_residency,
                )
            {
                return Ok(ToolCallOutcome {
                    success: false,
                    text: reason,
                });
            }
        }

        let tier = self
            .tools
            .iter()
            .find(|t| t.name == name)
            .and_then(|t| t.permission)
            // Unknown tier (server predates the `permission` wire field, or
            // genuinely didn't declare one) defaults to the MOST restrictive
            // tier, not `Write` -- an untiered tool must not be silently
            // trusted just because we don't know better.
            .unwrap_or(mae_ai::PermissionTier::Privileged);

        if needs_confirmation(tier, self.mode) {
            let (tx, rx) = oneshot::channel();
            let pending = PendingConfirm {
                tool_name: name.to_string(),
                arguments: arguments.clone(),
                tier,
            };
            let _ = self
                .events_tx
                .send(HarnessEvent::ConfirmRequest(pending, tx));
            // `ApproveAlwaysThisSession` is intentionally treated as a one-time
            // approve for now (a safe, honest subset) rather than a persistent
            // per-session allowlist — every subsequent call still gates on
            // `needs_confirmation`. A real allowlist is a documented follow-up.
            if rx.await.unwrap_or(ConfirmChoice::Deny) == ConfirmChoice::Deny {
                return Ok(ToolCallOutcome {
                    success: false,
                    text: "Denied by user.".to_string(),
                });
            }
        }

        self.inner.call_tool(name, arguments).await
    }
}

/// Fetch each registered KB instance's `ai_residency` policy, best-effort
/// (server-side ADR-048 enforcement is authoritative regardless). Shared by
/// `ConfirmingExecutor` (interactive) and `NonInteractiveExecutor`
/// (`--prompt` mode) — identical residency self-check either way.
async fn fetch_kb_residency(
    mcp: &mut McpClient,
) -> anyhow::Result<Vec<residency_check::KbResidencyInfo>> {
    let outcome = mcp.call_tool("kb_instances", serde_json::json!({})).await?;
    let parsed: serde_json::Value = serde_json::from_str(&outcome.text)?;
    let entries = parsed
        .as_array()
        .cloned()
        .or_else(|| parsed.get("instances").and_then(|v| v.as_array()).cloned())
        .unwrap_or_default();
    Ok(entries
        .into_iter()
        .filter_map(|v| {
            let name = v.get("name")?.as_str()?.to_string();
            let local_models_only = v
                .get("ai_residency")
                .and_then(|r| r.as_str())
                .map(|s| s.eq_ignore_ascii_case("local_models_only"))
                .unwrap_or(false);
            Some(residency_check::KbResidencyInfo {
                name,
                local_models_only,
            })
        })
        .collect())
}

/// Tool executor for `--prompt` mode: no human exists to answer a confirm
/// prompt, so a tool call whose tier EXCEEDS the configured
/// `--permission-mode` ceiling is DENIED outright rather than executed.
/// Before this existed, `run_once` used `McpClient` directly with zero
/// gating at any tier -- see `run_once`'s doc comment for the incident that
/// motivated this.
struct NonInteractiveExecutor {
    inner: McpClient,
    tools: Vec<ToolDefinition>,
    mode: PermissionMode,
    own_provider: String,
}

#[async_trait::async_trait]
impl ToolExecutor for NonInteractiveExecutor {
    async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> anyhow::Result<ToolCallOutcome> {
        // Client-side residency self-check: best effort, server is authoritative.
        if let Ok(kb_residency) = fetch_kb_residency(&mut self.inner).await {
            if let residency_check::SelfCheckDecision::Refuse(reason) =
                residency_check::check_before_call(
                    name,
                    &arguments,
                    &self.own_provider,
                    &kb_residency,
                )
            {
                return Ok(ToolCallOutcome {
                    success: false,
                    text: reason,
                });
            }
        }

        let tier = self
            .tools
            .iter()
            .find(|t| t.name == name)
            .and_then(|t| t.permission)
            // Unknown tier (server predates the `permission` wire field, or
            // genuinely didn't declare one) defaults to the MOST restrictive
            // tier, not `Write` -- an untiered tool must not be silently
            // trusted just because we don't know better.
            .unwrap_or(mae_ai::PermissionTier::Privileged);

        if needs_confirmation(tier, self.mode) {
            let text = format!(
                "Denied: '{name}' is {tier:?}-tier, which exceeds the --permission-mode \
                 ceiling ({:?}). --prompt mode has no human to confirm this -- pass a \
                 higher --permission-mode explicitly if this call is expected.",
                self.mode
            );
            eprintln!("mae-agent: {text}");
            return Ok(ToolCallOutcome {
                success: false,
                text,
            });
        }

        self.inner.call_tool(name, arguments).await
    }
}

async fn run_tui(
    mut app: AppState,
    provider: Arc<dyn AgentProvider>,
    mcp: McpClient,
    tools: Vec<ToolDefinition>,
) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_event_loop(&mut terminal, &mut app, provider, mcp, tools).await;

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    result
}

async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut AppState,
    provider: Arc<dyn AgentProvider>,
    mcp: McpClient,
    tools: Vec<ToolDefinition>,
) -> anyhow::Result<()> {
    let mut messages: Vec<Message> = Vec::new();
    let mut crossterm_events = EventStream::new();
    let (events_tx, mut events_rx) = mpsc::unbounded_channel::<HarnessEvent>();
    let mut turn_handle: Option<tokio::task::JoinHandle<()>> = None;
    let mut pending_confirm_reply: Option<oneshot::Sender<ConfirmChoice>> = None;
    let permission_mode = app.permission_mode;
    let own_provider = app.provider.clone();
    // `None` exactly while a turn's spawned task owns the client (it's handed
    // back via `HarnessEvent::TurnDone`) — `turn_handle.is_none()` is checked
    // before ever taking this, so it's never actually empty when needed.
    let mut mcp: Option<McpClient> = Some(mcp);

    loop {
        terminal.draw(|f| tui::draw(f, app))?;
        if app.should_quit {
            return Ok(());
        }

        tokio::select! {
            maybe_event = crossterm_events.next() => {
                let Some(Ok(event)) = maybe_event else { continue; };
                if let Event::Key(key) = event {
                    handle_key(app, key, &mut pending_confirm_reply);
                    if app.should_quit {
                        if let Some(handle) = turn_handle.take() {
                            handle.abort();
                        }
                        return Ok(());
                    }
                    if let Some(submitted) = app.pending_submit.take() {
                        if let Some(cmd) = tui::parse_slash_command(&submitted) {
                            handle_slash_command(app, cmd);
                        } else if let Some(client) = mcp.take() {
                            app.push_user(submitted.clone());
                            app.busy = true;
                            let executor = ConfirmingExecutor {
                                inner: client,
                                tools: tools.clone(),
                                mode: permission_mode,
                                own_provider: own_provider.clone(),
                                events_tx: events_tx.clone(),
                            };
                            turn_handle = Some(spawn_turn(
                                Arc::clone(&provider),
                                executor,
                                tools.clone(),
                                messages.clone(),
                                submitted,
                                events_tx.clone(),
                            ));
                        } else {
                            app.push_system_note(
                                "Still finishing the previous turn — try again in a moment."
                                    .to_string(),
                            );
                        }
                    }
                }
            }
            Some(harness_event) = events_rx.recv() => {
                match harness_event {
                    HarnessEvent::Agent(agent_loop::AgentEvent::ToolCallStarted { name, arguments }) => {
                        app.push_tool_call_started(name, arguments);
                    }
                    HarnessEvent::Agent(agent_loop::AgentEvent::ToolCallFinished { name, success, output }) => {
                        app.complete_tool_call(&name, success, output);
                    }
                    HarnessEvent::Agent(agent_loop::AgentEvent::Text(text)) => {
                        app.push_assistant(text);
                    }
                    HarnessEvent::Agent(agent_loop::AgentEvent::RoundLimitReached) => {
                        app.push_system_note("Round limit reached for this turn.".to_string());
                    }
                    HarnessEvent::Agent(agent_loop::AgentEvent::RoundDiagnostics {
                        round,
                        tools_offered,
                        stop_reason,
                        tool_calls_returned,
                        text_len,
                        usage,
                    }) => {
                        let tokens = usage
                            .map(|u| format!("{}/{}", u.prompt_tokens, u.completion_tokens))
                            .unwrap_or_else(|| "?/?".to_string());
                        app.set_diagnostics(format!(
                            "r{round} tools={tools_offered} stop={stop_reason:?} calls={tool_calls_returned} text={text_len} tok={tokens}"
                        ));
                    }
                    HarnessEvent::ConfirmRequest(pending, reply) => {
                        app.pending_confirm = Some(pending);
                        pending_confirm_reply = Some(reply);
                    }
                    HarnessEvent::TurnDone { mcp: returned_mcp, messages: new_messages, error } => {
                        mcp = Some(returned_mcp);
                        messages = new_messages;
                        app.busy = false;
                        app.round += 1;
                        turn_handle = None;
                        if let Some(e) = error {
                            app.push_system_note(format!("Error: {e}"));
                        }
                    }
                }
            }
        }
    }
}

fn spawn_turn(
    provider: Arc<dyn AgentProvider>,
    mut executor: ConfirmingExecutor,
    tools: Vec<ToolDefinition>,
    mut messages: Vec<Message>,
    user_input: String,
    events_tx: mpsc::UnboundedSender<HarnessEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let system_prompt = "You are an AI agent operating MAE's editor and knowledge-base tools.";
        let tx = events_tx.clone();
        let result = agent_loop::run_turn(
            agent_loop::TurnContext {
                provider: provider.as_ref(),
                executor: &mut executor,
                tools: &tools,
                system_prompt,
            },
            &mut messages,
            &agent_loop::TurnConfig::default(),
            &user_input,
            move |event| {
                let _ = tx.send(HarnessEvent::Agent(event));
            },
        )
        .await;

        let _ = events_tx.send(HarnessEvent::TurnDone {
            mcp: executor.inner,
            messages,
            error: result.err().map(|e| e.to_string()),
        });
    })
}

fn handle_key(
    app: &mut AppState,
    key: KeyEvent,
    pending_confirm_reply: &mut Option<oneshot::Sender<ConfirmChoice>>,
) {
    if let Some(pending) = app.pending_confirm.take() {
        if let KeyCode::Char(c) = key.code {
            if let Some(choice) = tui::parse_confirm_key(c) {
                if let Some(reply) = pending_confirm_reply.take() {
                    let _ = reply.send(choice);
                }
                return;
            }
        }
        // Not a recognized confirm key — keep the dialog open.
        app.pending_confirm = Some(pending);
        return;
    }

    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyCode::Enter => {
            if let Some(submitted) = app.submit_input() {
                app.pending_submit = Some(submitted);
            }
        }
        KeyCode::Backspace => {
            if app.cursor > 0 {
                app.cursor -= 1;
                app.input.remove(app.cursor);
            }
        }
        KeyCode::Left => app.cursor = app.cursor.saturating_sub(1),
        KeyCode::Right => app.cursor = (app.cursor + 1).min(app.input.len()),
        KeyCode::Up => app.recall_history_prev(),
        KeyCode::Down => app.recall_history_next(),
        KeyCode::Tab => {
            if let Some(last) = app.transcript.len().checked_sub(1) {
                app.toggle_tool_call_expanded(last);
            }
        }
        KeyCode::Char(c) => {
            app.input.insert(app.cursor, c);
            app.cursor += 1;
        }
        _ => {}
    }
}

fn handle_slash_command(app: &mut AppState, cmd: SlashCommand) {
    match cmd {
        SlashCommand::Help => app.push_system_note(
            "/help /clear /model /permissions /quit — Ctrl+C to interrupt, Tab to expand/collapse the last tool call".to_string(),
        ),
        SlashCommand::Clear => app.transcript.clear(),
        SlashCommand::Model => {
            let msg = format!("{}/{}", app.provider, app.model);
            app.push_system_note(msg);
        }
        SlashCommand::Permissions => {
            let msg = format!("{:?}", app.permission_mode);
            app.push_system_note(msg);
        }
        SlashCommand::Quit => app.should_quit = true,
        SlashCommand::Unknown(name) => app.push_system_note(format!("Unknown command: /{name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn char_key(c: char, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn state() -> AppState {
        AppState::new(
            "qwen3:latest".into(),
            "ollama".into(),
            PermissionMode::default(),
        )
    }

    // ---- convert_tool_infos / filter_tools ----

    fn tool_info(name: &str) -> mae_mcp::protocol::ToolInfo {
        mae_mcp::protocol::ToolInfo {
            name: name.to_string(),
            description: format!("{name} description"),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
            permission: None,
        }
    }

    #[test]
    fn convert_tool_infos_keeps_valid_schemas() {
        let tools = convert_tool_infos(vec![tool_info("kb_search"), tool_info("kb_get")]);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "kb_search");
    }

    #[test]
    fn convert_tool_infos_drops_unparseable_schema_not_the_whole_batch() {
        let mut bad = tool_info("broken_tool");
        bad.input_schema = serde_json::json!("not an object schema at all");
        let tools = convert_tool_infos(vec![tool_info("kb_search"), bad]);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "kb_search");
    }

    #[test]
    fn filter_tools_empty_allowlist_is_a_noop() {
        let tools = convert_tool_infos(vec![tool_info("kb_search"), tool_info("shell_exec")]);
        let filtered = filter_tools(tools, &[]);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn filter_tools_restricts_to_named_tools_only() {
        let tools = convert_tool_infos(vec![
            tool_info("kb_search"),
            tool_info("shell_exec"),
            tool_info("kb_get"),
        ]);
        let filtered = filter_tools(tools, &["kb_search".to_string(), "kb_get".to_string()]);
        let names: Vec<&str> = filtered.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["kb_search", "kb_get"]);
    }

    // ---- construct_provider / api_key_env_var / is_local_provider ----

    fn config_for(provider_type: &str) -> ProviderConfig {
        ProviderConfig {
            provider_type: provider_type.to_string(),
            api_key: None,
            model: "test-model".to_string(),
            base_url: None,
            max_tokens: 8192,
            temperature: None,
            thinking: None,
            timeout_secs: 300,
            budget: Default::default(),
        }
    }

    #[test]
    fn construct_provider_dispatches_by_type_name() {
        for (provider_type, expected_name) in [
            ("claude", "claude"),
            ("openai", "openai"),
            ("gemini", "gemini"),
            ("ollama", "ollama"),
            ("something-unknown", "claude"), // unmatched falls back to Claude
        ] {
            let provider = construct_provider(provider_type, config_for(provider_type));
            assert_eq!(provider.name(), expected_name, "for input {provider_type}");
        }
    }

    #[test]
    fn api_key_env_var_matches_provider_conventions() {
        assert_eq!(api_key_env_var("claude"), Some("ANTHROPIC_API_KEY"));
        assert_eq!(api_key_env_var("openai"), Some("OPENAI_API_KEY"));
        assert_eq!(api_key_env_var("gemini"), Some("GEMINI_API_KEY"));
        assert_eq!(api_key_env_var("ollama"), None);
        assert_eq!(api_key_env_var("unknown"), None);
    }

    #[test]
    fn is_local_provider_recognizes_ollama_case_insensitively() {
        assert!(is_local_provider("ollama"));
        assert!(is_local_provider("Ollama"));
        assert!(is_local_provider("OLLAMA"));
        assert!(!is_local_provider("claude"));
        assert!(!is_local_provider(""));
    }

    // ---- handle_key ----

    #[test]
    fn handle_key_types_and_backspaces() {
        let mut app = state();
        let mut pending_reply = None;
        handle_key(
            &mut app,
            char_key('h', KeyModifiers::NONE),
            &mut pending_reply,
        );
        handle_key(
            &mut app,
            char_key('i', KeyModifiers::NONE),
            &mut pending_reply,
        );
        assert_eq!(app.input, "hi");
        assert_eq!(app.cursor, 2);

        handle_key(&mut app, key(KeyCode::Backspace), &mut pending_reply);
        assert_eq!(app.input, "h");
        assert_eq!(app.cursor, 1);
    }

    #[test]
    fn handle_key_enter_submits_and_clears_input() {
        let mut app = state();
        let mut pending_reply = None;
        app.input = "hello".to_string();
        app.cursor = 5;
        handle_key(&mut app, key(KeyCode::Enter), &mut pending_reply);
        assert_eq!(app.pending_submit, Some("hello".to_string()));
        assert!(app.input.is_empty());
    }

    #[test]
    fn handle_key_ctrl_c_sets_should_quit() {
        let mut app = state();
        let mut pending_reply = None;
        handle_key(
            &mut app,
            char_key('c', KeyModifiers::CONTROL),
            &mut pending_reply,
        );
        assert!(app.should_quit);
    }

    #[test]
    fn handle_key_tab_toggles_last_tool_call_expansion() {
        let mut app = state();
        app.push_tool_call_started("kb_search".into(), serde_json::json!({}));
        let mut pending_reply = None;
        handle_key(&mut app, key(KeyCode::Tab), &mut pending_reply);
        assert!(matches!(
            app.transcript.last(),
            Some(tui::TranscriptEntry::ToolCall { expanded: true, .. })
        ));
    }

    #[test]
    fn handle_key_ignores_unrecognized_confirm_key_and_keeps_dialog_open() {
        let mut app = state();
        app.pending_confirm = Some(tui::PendingConfirm {
            tool_name: "shell_exec".into(),
            arguments: serde_json::json!({}),
            tier: mae_ai::PermissionTier::Shell,
        });
        let (tx, mut rx) = oneshot::channel();
        let mut pending_reply = Some(tx);
        // 'z' isn't y/n/a — dialog should stay open, no reply sent.
        handle_key(
            &mut app,
            char_key('z', KeyModifiers::NONE),
            &mut pending_reply,
        );
        assert!(app.pending_confirm.is_some());
        assert!(pending_reply.is_some());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn handle_key_confirm_key_resolves_pending_dialog() {
        let mut app = state();
        app.pending_confirm = Some(tui::PendingConfirm {
            tool_name: "shell_exec".into(),
            arguments: serde_json::json!({}),
            tier: mae_ai::PermissionTier::Shell,
        });
        let (tx, mut rx) = oneshot::channel();
        let mut pending_reply = Some(tx);
        handle_key(
            &mut app,
            char_key('y', KeyModifiers::NONE),
            &mut pending_reply,
        );
        assert!(app.pending_confirm.is_none());
        assert!(pending_reply.is_none());
        assert_eq!(rx.try_recv().unwrap(), ConfirmChoice::Approve);
    }

    // ---- handle_slash_command ----

    #[test]
    fn handle_slash_command_clear_empties_transcript() {
        let mut app = state();
        app.push_user("hi".into());
        handle_slash_command(&mut app, SlashCommand::Clear);
        assert!(app.transcript.is_empty());
    }

    #[test]
    fn handle_slash_command_quit_sets_should_quit() {
        let mut app = state();
        handle_slash_command(&mut app, SlashCommand::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn handle_slash_command_model_reports_provider_and_model() {
        let mut app = state();
        handle_slash_command(&mut app, SlashCommand::Model);
        assert!(matches!(
            app.transcript.last(),
            Some(tui::TranscriptEntry::SystemNote(n)) if n == "ollama/qwen3:latest"
        ));
    }

    #[test]
    fn handle_slash_command_unknown_reports_the_command_name() {
        let mut app = state();
        handle_slash_command(&mut app, SlashCommand::Unknown("bogus".to_string()));
        assert!(matches!(
            app.transcript.last(),
            Some(tui::TranscriptEntry::SystemNote(n)) if n == "Unknown command: /bogus"
        ));
    }

    // ---- NonInteractiveExecutor: --prompt mode must deny, not bypass, tier gating ----
    //
    // Regression coverage for the real incident that motivated this executor:
    // `run_once` used to hand `McpClient` straight to `agent_loop::run_turn`
    // with zero tier gating, so a Shell/Privileged-tier tool call (e.g. a
    // `model_exam` pushback prompt asking a model to run `rm -rf /`-style
    // commands) executed completely unchecked in non-interactive mode.

    /// Minimal fake JSON-RPC server for these two tests: replies to
    /// `kb_instances` (so `fetch_kb_residency` doesn't error) with an empty
    /// list, and to anything else with `{}` -- good enough since a denied
    /// call must never actually reach the wire in the first place.
    async fn run_minimal_fake_server(
        mut reader: tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
        mut writer: tokio::net::unix::OwnedWriteHalf,
    ) {
        loop {
            let Some(msg) = mae_mcp::read_message(&mut reader).await.unwrap() else {
                break;
            };
            let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
            if let Some(id) = parsed.get("id").cloned() {
                let method = parsed.get("method").and_then(|m| m.as_str()).unwrap_or("");
                let result = if method == "tools/call" {
                    serde_json::json!({"isError": false, "content": [{"type": "text", "text": "[]"}]})
                } else {
                    serde_json::json!({})
                };
                let response = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result});
                let body = serde_json::to_vec(&response).unwrap();
                mae_mcp::write_framed(&mut writer, &body, std::time::Duration::from_secs(5))
                    .await
                    .unwrap();
            }
        }
    }

    fn shell_tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: "test tool".to_string(),
            parameters: mae_ai::ToolParameters {
                schema_type: "object".to_string(),
                properties: Default::default(),
                required: vec![],
            },
            permission: Some(mae_ai::PermissionTier::Shell),
        }
    }

    #[tokio::test]
    async fn non_interactive_executor_denies_a_call_exceeding_the_permission_ceiling() {
        let (client_stream, server_stream) = tokio::net::UnixStream::pair().unwrap();
        let (server_read, server_write) = server_stream.into_split();
        let server_reader = tokio::io::BufReader::new(server_read);
        let server = tokio::spawn(run_minimal_fake_server(server_reader, server_write));

        let mcp = McpClient::from_stream(client_stream, None, None)
            .await
            .expect("connect should succeed");

        let mut executor = NonInteractiveExecutor {
            inner: mcp,
            tools: vec![shell_tool("shell_exec")],
            mode: PermissionMode::ReadOnly, // ceiling well below Shell
            own_provider: "ollama".to_string(),
        };

        let outcome = executor
            .call_tool("shell_exec", serde_json::json!({"command": "rm -rf /"}))
            .await
            .unwrap();

        assert!(
            !outcome.success,
            "a Shell-tier call must be denied, not executed"
        );
        assert!(
            outcome.text.contains("Denied"),
            "denial reason should be explicit: {}",
            outcome.text
        );

        drop(executor);
        server.abort();
    }

    #[tokio::test]
    async fn non_interactive_executor_allows_a_call_within_the_permission_ceiling() {
        let (client_stream, server_stream) = tokio::net::UnixStream::pair().unwrap();
        let (server_read, server_write) = server_stream.into_split();
        let server_reader = tokio::io::BufReader::new(server_read);
        let server = tokio::spawn(run_minimal_fake_server(server_reader, server_write));

        let mcp = McpClient::from_stream(client_stream, None, None)
            .await
            .expect("connect should succeed");

        let mut executor = NonInteractiveExecutor {
            inner: mcp,
            tools: vec![shell_tool("shell_exec")],
            mode: PermissionMode::Shell, // ceiling covers Shell -- explicit opt-in
            own_provider: "ollama".to_string(),
        };

        let outcome = executor
            .call_tool("shell_exec", serde_json::json!({"command": "echo hi"}))
            .await
            .unwrap();

        assert!(
            outcome.success,
            "a Shell-tier call within an explicit Shell-ceiling permission mode should reach the server: {}",
            outcome.text
        );

        drop(executor);
        server.abort();
    }
}
