//! MCP stdio <-> Unix socket shim.
//!
//! Claude Code (or any MCP client) spawns this binary as its MCP server
//! process. It reads `MAE_MCP_SOCKET` from the environment and bridges
//! stdin/stdout to the MAE editor's Unix socket.
//!
//! **Stdio side** (to/from Claude Code): newline-delimited JSON per MCP spec.
//! **Socket side** (to/from MAE): Content-Length framing (LSP-style).
//!
//! **Reconnect behavior** (#356): stdin reading runs on its own long-lived
//! task, independent of the socket connection -- a dropped/restarted editor
//! instance does not end the shim process. On socket EOF/error the shim
//! rediscovers a live socket and reconnects with bounded exponential backoff
//! (mirrors `daemon/src/dialer.rs`'s mesh-peer reconnect policy, the closest
//! existing precedent for a persistent reconnecting client relay in this
//! codebase). Stdin EOF is the shim's actual terminal-shutdown signal: the
//! MCP host closed the pipe, so the process exits instead of reconnecting.
//!
//! Set `MAE_MCP_SHIM_LOG=/path/to/file.log` to override the default log path.
//! Default log: `/tmp/mae-shim.log`.
//!
//! Set `MAE_MCP_PERMISSION_CEILING=<ReadOnly|Write|Shell|Privileged>` to have
//! the shim forward a `permissionCeiling` in its `initialize` request params
//! (ADR-051). This can only ever *tighten* the effective policy on the
//! editor side (`effective_permission_policy` takes a `min()` against global
//! config) -- there is no server-side field this env var could set to
//! *loosen* a session's tier, so it's safe to trust unconditionally, exactly
//! like any hand-rolled MCP client that set the same field directly. Exists
//! because the shim itself otherwise has no way to set this (a gap ADR-050
//! D1/Phase I's "MAE for VS Code" extension needed closed).
//!
//! Flags:
//!   --version   Print version and exit
//!   --check     Connectivity diagnostic (discover, connect, verify, exit)

use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{split as io_split, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

const VERSION: &str = env!("CARGO_PKG_VERSION");

type LogHandle = Option<Arc<Mutex<std::fs::File>>>;

/// Reconnect backoff floor after the editor socket drops (grows toward
/// `RECONNECT_MAX`). Named consts, not magic numbers, mirroring
/// `daemon/src/dialer.rs`'s `RECONNECT_MIN`/`RECONNECT_MAX` rationale: this
/// binary has no `OptionRegistry` to register a user-facing option against,
/// so these are fixed operational constants.
const RECONNECT_MIN: Duration = Duration::from_secs(2);
/// Reconnect backoff ceiling -- caps retry spacing so a long-dead editor
/// instance is still retried roughly once a minute rather than spinning.
const RECONNECT_MAX: Duration = Duration::from_secs(60);
/// Per-step timeout for the connect-time handshake verification.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Per-message write timeout to the editor socket -- a stuck/hung editor is
/// dropped (triggering reconnect), not blocked on forever.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Scan /tmp/mae-*.sock for a socket whose PID is still alive.
/// Returns the most recently modified match.
fn discover_socket() -> Option<String> {
    let tmp = std::path::Path::new("/tmp");
    let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

    let entries = std::fs::read_dir(tmp).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with("mae-") || !name_str.ends_with(".sock") {
            continue;
        }
        // Extract PID and check if alive.
        let pid_str = name_str
            .strip_prefix("mae-")
            .and_then(|s| s.strip_suffix(".sock"));
        if let Some(pid_str) = pid_str {
            if let Ok(pid) = pid_str.parse::<u32>() {
                let proc_path = format!("/proc/{}", pid);
                if std::path::Path::new(&proc_path).exists() {
                    if let Ok(meta) = entry.metadata() {
                        let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                        candidates.push((entry.path(), mtime));
                    }
                }
            }
        }
    }

    candidates.sort_by_key(|c| std::cmp::Reverse(c.1));
    candidates
        .first()
        .map(|(p, _)| p.to_string_lossy().to_string())
}

/// Resolve which socket to try next: the pinned `MAE_MCP_SOCKET` env var if
/// set (respected verbatim on every attempt -- an explicit user override),
/// otherwise a fresh `discover_socket()` scan. Re-running discovery on every
/// reconnect attempt (rather than caching the first result) is what lets a
/// restarted editor's *new* PID/socket get picked up automatically --
/// `discover_socket()` already filters to live PIDs, so a dead instance's
/// stale socket is never a candidate.
fn resolve_socket_path() -> Option<String> {
    match env::var("MAE_MCP_SOCKET") {
        Ok(p) => Some(p),
        Err(_) => discover_socket(),
    }
}

/// Append a line to the debug log file (if configured).
fn log(file: &LogHandle, msg: &str) {
    if let Some(f) = file {
        use std::io::Write;
        if let Ok(mut f) = f.lock() {
            let _ = writeln!(f, "[{}] {}", chrono_now(), msg);
            let _ = f.flush();
        }
    }
}

fn chrono_now() -> String {
    // Simple timestamp without chrono dependency.
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}", d.as_secs(), d.subsec_millis())
}

fn open_log() -> LogHandle {
    let path = env::var("MAE_MCP_SHIM_LOG").unwrap_or_else(|_| "/tmp/mae-shim.log".to_string());
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()
        .map(|f| Arc::new(Mutex::new(f)))
}

/// Write a newline-delimited JSON message (for the stdio side to Claude Code).
async fn write_jsonl<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    body: &[u8],
) -> Result<(), std::io::Error> {
    writer.write_all(body).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

/// Connect to `socket_path` and verify a healthy MAE process is behind it
/// (initialize -> notifications/initialized -> $/ping, each with a bounded
/// timeout) -- not just that the socket file accepts connections. Shared by
/// `--check` and the reconnect loop in `main()`. Before #356, `main()`'s
/// connect path did zero handshake verification: a freshly discovered but
/// stale socket file could be "connected" to and then silently relay
/// nothing.
async fn connect_and_verify(socket_path: &str) -> Result<BufReader<UnixStream>, String> {
    let stream = UnixStream::connect(socket_path)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let mut stream = BufReader::new(stream);

    let ceiling_env = env::var("MAE_MCP_PERMISSION_CEILING").ok();
    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": mae_mcp::build_shim_initialize_params(ceiling_env.as_deref())
    });
    mae_mcp::write_framed(
        &mut stream,
        init_req.to_string().as_bytes(),
        HANDSHAKE_TIMEOUT,
    )
    .await
    .map_err(|e| format!("write initialize: {e}"))?;
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, mae_mcp::read_message(&mut stream)).await {
        Ok(Ok(Some(_))) => {}
        Ok(Ok(None)) => return Err("server closed connection during initialize".into()),
        Ok(Err(e)) => return Err(format!("read initialize response: {e}")),
        Err(_) => return Err("timeout waiting for initialize response".into()),
    }

    let notif = serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    mae_mcp::write_framed(&mut stream, notif.to_string().as_bytes(), HANDSHAKE_TIMEOUT)
        .await
        .map_err(|e| format!("write notifications/initialized: {e}"))?;

    let ping = serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "$/ping" });
    mae_mcp::write_framed(&mut stream, ping.to_string().as_bytes(), HANDSHAKE_TIMEOUT)
        .await
        .map_err(|e| format!("write ping: {e}"))?;
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, mae_mcp::read_message(&mut stream)).await {
        Ok(Ok(Some(_))) => {}
        Ok(Ok(None)) => return Err("server closed connection during ping".into()),
        Ok(Err(e)) => return Err(format!("read ping response: {e}")),
        Err(_) => return Err("timeout waiting for ping response".into()),
    }

    Ok(stream)
}

/// Run `--check` diagnostic: discover socket, connect + verify, report result.
async fn run_check() {
    eprintln!("mae-mcp-shim --check  v{}", VERSION);
    eprintln!();

    let socket_path = match env::var("MAE_MCP_SOCKET") {
        Ok(p) => {
            eprintln!("[1/2] socket (env): {}", p);
            p
        }
        Err(_) => match discover_socket() {
            Some(p) => {
                eprintln!("[1/2] socket (discovered): {}", p);
                p
            }
            None => {
                eprintln!("[1/2] FAIL: no live mae socket found in /tmp/");
                eprintln!("  Hint: start mae first, or set MAE_MCP_SOCKET=/tmp/mae-<PID>.sock");
                std::process::exit(1);
            }
        },
    };

    match connect_and_verify(&socket_path).await {
        Ok(_) => {
            eprintln!("[2/2] connect + initialize + ping -> pong OK");
            eprintln!();
            eprintln!("All checks passed.");
        }
        Err(e) => {
            eprintln!("[2/2] FAIL: {e}");
            std::process::exit(1);
        }
    }
}

/// Why a relay session ended -- drives `main()`'s reconnect-vs-exit decision.
enum SessionEnd {
    /// The MCP host closed stdin (or stdout is broken, meaning the host is
    /// gone too) -- nothing left to relay for, exit for good.
    StdinClosed,
    /// The editor-side socket dropped or errored -- reconnect.
    SocketDropped(String),
}

/// Relay one connected session: stdin lines (received via `stdin_rx`, fed by
/// the independent stdin task) -> socket with Content-Length framing;
/// socket messages -> stdout as newline-delimited JSON. Returns why the
/// session ended so `main()` knows whether to reconnect or shut down.
async fn run_session<R, W>(
    socket_reader: &mut BufReader<R>,
    mut socket_writer: W,
    stdin_rx: &mut mpsc::Receiver<String>,
    stdout: &mut tokio::io::Stdout,
    logfile: &LogHandle,
    relay_flag: &Arc<AtomicBool>,
) -> SessionEnd
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    loop {
        tokio::select! {
            line = stdin_rx.recv() => match line {
                Some(l) => {
                    if let Err(e) = mae_mcp::write_framed(&mut socket_writer, l.as_bytes(), WRITE_TIMEOUT).await {
                        log(logfile, &format!("socket write error: {}", e));
                        return SessionEnd::SocketDropped(format!("write: {e}"));
                    }
                }
                None => return SessionEnd::StdinClosed,
            },
            msg = mae_mcp::read_message(socket_reader) => match msg {
                Ok(Some(m)) => {
                    log(logfile, &format!("S->C: {}", m));
                    if let Err(e) = write_jsonl(stdout, m.as_bytes()).await {
                        log(logfile, &format!("stdout write error: {}", e));
                        eprintln!("mae-mcp-shim: error: stdout write: {}", e);
                        // stdout is broken -- the MCP host is gone, not just
                        // the editor. Reconnecting to the editor wouldn't help.
                        return SessionEnd::StdinClosed;
                    }
                    if !relay_flag.load(Ordering::Relaxed) {
                        relay_flag.store(true, Ordering::Relaxed);
                        eprintln!("mae-mcp-shim: ready (first message relayed)");
                    }
                }
                Ok(None) => return SessionEnd::SocketDropped("socket EOF".into()),
                Err(e) => return SessionEnd::SocketDropped(format!("read: {e}")),
            },
        }
    }
}

/// Wait out `backoff` before the next reconnect attempt, but return early
/// (`true`) if stdin closes in the meantime -- the MCP host closing the pipe
/// means "shut down", not "keep waiting for the editor to come back". A line
/// arriving on stdin while there's no live socket to forward it to is
/// unavoidably dropped (and logged) -- there's nothing to relay it to yet.
async fn wait_or_stdin_closed(
    stdin_rx: &mut mpsc::Receiver<String>,
    backoff: Duration,
    logfile: &LogHandle,
) -> bool {
    let sleep = tokio::time::sleep(backoff);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            _ = &mut sleep => return false,
            line = stdin_rx.recv() => match line {
                Some(l) => {
                    log(logfile, &format!("dropped (no live editor socket): {}", l));
                }
                None => return true,
            },
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Handle --version and --check before anything else.
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("mae-mcp-shim {}", VERSION);
        return;
    }
    if args.iter().any(|a| a == "--check") {
        run_check().await;
        return;
    }

    let logfile = open_log();

    log(&logfile, &format!("mae-mcp-shim v{} starting", VERSION));
    eprintln!("mae-mcp-shim: v{} starting", VERSION);

    // Stdin reading is decoupled from the socket connection lifecycle: it
    // must survive across reconnects (a transient editor-side drop is not a
    // reason to stop relaying the MCP host's requests), and stdin EOF is the
    // shim's actual terminal-shutdown signal (#356).
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(64);
    let log_stdin = logfile.clone();
    tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(0) => {
                    log(&log_stdin, "stdin EOF");
                    eprintln!("mae-mcp-shim: stdin EOF, shutting down");
                    break;
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    log(&log_stdin, &format!("C->S: {}", trimmed));
                    if stdin_tx.send(trimmed.to_string()).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    log(&log_stdin, &format!("stdin read error: {}", e));
                    eprintln!("mae-mcp-shim: error: stdin read: {}", e);
                    break;
                }
            }
        }
        // Dropping `stdin_tx` here closes the channel -- the session/backoff
        // loops' `stdin_rx.recv() -> None` is how they learn to stop.
    });

    let mut stdout = tokio::io::stdout();
    let relay_flag = Arc::new(AtomicBool::new(false));
    let mut backoff = RECONNECT_MIN;

    loop {
        let Some(socket_path) = resolve_socket_path() else {
            eprintln!(
                "mae-mcp-shim: no live mae socket found in /tmp/, retrying in {}s...",
                backoff.as_secs()
            );
            log(&logfile, "no live mae socket found, waiting to retry");
            if wait_or_stdin_closed(&mut stdin_rx, backoff, &logfile).await {
                break;
            }
            backoff = (backoff * 2).min(RECONNECT_MAX);
            continue;
        };

        log(&logfile, &format!("connecting to {}", socket_path));
        match connect_and_verify(&socket_path).await {
            Ok(stream) => {
                backoff = RECONNECT_MIN;
                eprintln!("mae-mcp-shim: connected to {}", socket_path);
                log(&logfile, "connected");

                // `tokio::io::split` (not `UnixStream::into_split()`) so the
                // handshake's `BufReader` -- and any bytes it already
                // buffered from the socket -- is preserved rather than
                // discarded; both halves share the same underlying object.
                let (socket_read_half, socket_writer) = io_split(stream);
                let mut socket_reader = BufReader::new(socket_read_half);
                match run_session(
                    &mut socket_reader,
                    socket_writer,
                    &mut stdin_rx,
                    &mut stdout,
                    &logfile,
                    &relay_flag,
                )
                .await
                {
                    SessionEnd::StdinClosed => break,
                    SessionEnd::SocketDropped(reason) => {
                        eprintln!(
                            "mae-mcp-shim: editor instance disappeared ({}), reconnecting...",
                            reason
                        );
                        log(
                            &logfile,
                            &format!("socket dropped ({}), reconnecting", reason),
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "mae-mcp-shim: connect to {} failed: {} (retrying in {}s)",
                    socket_path,
                    e,
                    backoff.as_secs()
                );
                log(
                    &logfile,
                    &format!("connect to {} failed: {}", socket_path, e),
                );
            }
        }

        if wait_or_stdin_closed(&mut stdin_rx, backoff, &logfile).await {
            break;
        }
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }

    log(&logfile, "shim exiting");
    eprintln!("mae-mcp-shim: exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UnixListener;

    fn test_socket_path(dir: &tempfile::TempDir, name: &str) -> String {
        dir.path().join(name).to_string_lossy().to_string()
    }

    /// A minimal fake MAE server: accepts one connection, replies to
    /// `initialize` and `$/ping` with a bare JSON-RPC result (ignores
    /// `notifications/initialized`, which has no response).
    async fn serve_one_handshake(path: String) {
        let listener = UnixListener::bind(&path).unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        loop {
            let Ok(Some(msg)) = mae_mcp::read_message(&mut stream).await else {
                return;
            };
            let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
            let Some(id) = v.get("id") else {
                continue; // notification, no response
            };
            let resp = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {}});
            mae_mcp::write_framed(
                &mut stream,
                resp.to_string().as_bytes(),
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn connect_and_verify_succeeds_against_a_healthy_handshake() {
        let dir = tempfile::tempdir().unwrap();
        let path = test_socket_path(&dir, "healthy.sock");
        tokio::spawn(serve_one_handshake(path.clone()));
        // Give the listener a moment to bind before dialing.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let result = connect_and_verify(&path).await;
        assert!(
            result.is_ok(),
            "expected a healthy handshake to verify OK, got: {:?}",
            result.err()
        );
    }

    /// Adversarial: a socket that merely *accepts* the connection but never
    /// answers isn't sufficient proof of a healthy MAE process behind it --
    /// `connect_and_verify` must time out, not hang forever or report success.
    #[tokio::test]
    async fn connect_and_verify_times_out_against_a_silent_acceptor() {
        let dir = tempfile::tempdir().unwrap();
        let path = test_socket_path(&dir, "silent.sock");
        let listener = UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            // Accept and then just hold the connection open, never responding.
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let result = connect_and_verify(&path).await;
        assert!(
            result.is_err(),
            "a socket that accepts but never responds must not verify as healthy"
        );
        assert!(
            result.unwrap_err().contains("timeout"),
            "expected a timeout-specific error"
        );
    }

    #[tokio::test]
    async fn connect_and_verify_fails_against_no_listener() {
        let dir = tempfile::tempdir().unwrap();
        let path = test_socket_path(&dir, "nothing-here.sock");
        let result = connect_and_verify(&path).await;
        assert!(
            result.is_err(),
            "connecting to a nonexistent socket must fail"
        );
    }

    /// #356 reconnect decision logic: a closed stdin channel ends the session
    /// (`StdinClosed`) even while the socket side would otherwise keep
    /// waiting for data -- this is what lets `main()` distinguish "the MCP
    /// host is gone, exit for good" from "the editor dropped, reconnect".
    #[tokio::test]
    async fn run_session_ends_stdin_closed_when_stdin_channel_closes() {
        let dir = tempfile::tempdir().unwrap();
        let path = test_socket_path(&dir, "session-stdin-closed.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            // Hold the connection open, sending nothing -- the stdin side
            // must be what ends the session here, not the socket.
            std::future::pending::<()>().await;
        });

        let client = UnixStream::connect(&path).await.unwrap();
        let (read_half, write_half) = io_split(client);
        let mut socket_reader = BufReader::new(read_half);

        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(1);
        drop(stdin_tx); // closes the channel immediately -- simulates stdin EOF

        let mut stdout = tokio::io::stdout();
        let relay_flag = Arc::new(AtomicBool::new(false));
        let end = run_session(
            &mut socket_reader,
            write_half,
            &mut stdin_rx,
            &mut stdout,
            &None,
            &relay_flag,
        )
        .await;

        assert!(matches!(end, SessionEnd::StdinClosed));
        server.abort();
    }

    /// The mirror case: a live stdin channel (never closed) but a dropped
    /// socket must end the session as `SocketDropped`, driving a reconnect
    /// rather than a shutdown.
    #[tokio::test]
    async fn run_session_ends_socket_dropped_on_socket_eof() {
        let dir = tempfile::tempdir().unwrap();
        let path = test_socket_path(&dir, "session-socket-eof.sock");
        let listener = UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            drop(stream); // immediate EOF from the client's perspective
        });

        let client = UnixStream::connect(&path).await.unwrap();
        let (read_half, write_half) = io_split(client);
        let mut socket_reader = BufReader::new(read_half);

        // Keep the sender alive (unused) so `stdin_rx.recv()` stays pending
        // for the whole test -- the socket branch must be what resolves.
        let (_stdin_tx, mut stdin_rx) = mpsc::channel::<String>(1);

        let mut stdout = tokio::io::stdout();
        let relay_flag = Arc::new(AtomicBool::new(false));
        let end = tokio::time::timeout(
            Duration::from_secs(5),
            run_session(
                &mut socket_reader,
                write_half,
                &mut stdin_rx,
                &mut stdout,
                &None,
                &relay_flag,
            ),
        )
        .await
        .expect("run_session should resolve promptly on socket EOF");

        assert!(matches!(end, SessionEnd::SocketDropped(_)));
    }

    #[tokio::test]
    async fn wait_or_stdin_closed_returns_true_immediately_on_channel_close() {
        let (tx, mut rx) = mpsc::channel::<String>(1);
        drop(tx);
        let closed = wait_or_stdin_closed(&mut rx, Duration::from_secs(30), &None).await;
        assert!(
            closed,
            "must return true promptly, not wait out the full backoff"
        );
    }

    #[tokio::test]
    async fn wait_or_stdin_closed_returns_false_after_backoff_elapses() {
        let (_tx, mut rx) = mpsc::channel::<String>(1);
        let closed = wait_or_stdin_closed(&mut rx, Duration::from_millis(20), &None).await;
        assert!(
            !closed,
            "must return false once backoff elapses with the channel still open"
        );
    }
}
