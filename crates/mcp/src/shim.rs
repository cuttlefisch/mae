//! MCP stdio <-> Unix socket shim.
//!
//! Claude Code (or any MCP client) spawns this binary as its MCP server
//! process. It reads `MAE_MCP_SOCKET` from the environment and bridges
//! stdin/stdout to the MAE editor's Unix socket.
//!
//! **Stdio side** (to/from Claude Code): newline-delimited JSON per MCP spec.
//! **Socket side** (to/from MAE): Content-Length framing (LSP-style).
//!
//! Set `MAE_MCP_SHIM_LOG=/path/to/file.log` to override the default log path.
//! Default log: `/tmp/mae-shim.log`.
//!
//! Flags:
//!   --version   Print version and exit
//!   --check     Connectivity diagnostic (discover, connect, ping, exit)

use std::env;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

const VERSION: &str = env!("CARGO_PKG_VERSION");

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

/// Append a line to the debug log file (if configured).
fn log(file: &Option<std::sync::Arc<std::sync::Mutex<std::fs::File>>>, msg: &str) {
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

fn open_log() -> Option<std::sync::Arc<std::sync::Mutex<std::fs::File>>> {
    let path = env::var("MAE_MCP_SHIM_LOG").unwrap_or_else(|_| "/tmp/mae-shim.log".to_string());
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()
        .map(|f| std::sync::Arc::new(std::sync::Mutex::new(f)))
}

/// Write a Content-Length framed message (for the socket side to MAE).
async fn write_framed<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    body: &[u8],
) -> Result<(), std::io::Error> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(body).await?;
    writer.flush().await
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

/// Run `--check` diagnostic: discover socket, connect, send initialize + ping, report results.
async fn run_check() {
    eprintln!("mae-mcp-shim --check  v{}", VERSION);
    eprintln!();

    // Step 1: Discover socket
    let socket_path = match env::var("MAE_MCP_SOCKET") {
        Ok(p) => {
            eprintln!("[1/4] socket (env): {}", p);
            p
        }
        Err(_) => match discover_socket() {
            Some(p) => {
                eprintln!("[1/4] socket (discovered): {}", p);
                p
            }
            None => {
                eprintln!("[1/4] FAIL: no live mae socket found in /tmp/");
                eprintln!("  Hint: start mae first, or set MAE_MCP_SOCKET=/tmp/mae-<PID>.sock");
                std::process::exit(1);
            }
        },
    };

    // Step 2: Connect
    let stream = match UnixStream::connect(&socket_path).await {
        Ok(s) => {
            eprintln!("[2/4] connected");
            s
        }
        Err(e) => {
            eprintln!("[2/4] FAIL: connect error: {}", e);
            std::process::exit(1);
        }
    };

    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // Step 3: Send initialize
    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "mae-mcp-shim-check", "version": VERSION }
        }
    });
    let init_bytes = serde_json::to_string(&init_req).unwrap();
    if let Err(e) = write_framed(&mut writer, init_bytes.as_bytes()).await {
        eprintln!("[3/4] FAIL: write initialize: {}", e);
        std::process::exit(1);
    }

    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        mae_mcp::read_message(&mut reader),
    )
    .await
    {
        Ok(Ok(Some(resp))) => {
            eprintln!("[3/4] initialize OK ({}B response)", resp.len());
        }
        Ok(Ok(None)) => {
            eprintln!("[3/4] FAIL: server closed connection");
            std::process::exit(1);
        }
        Ok(Err(e)) => {
            eprintln!("[3/4] FAIL: read error: {}", e);
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("[3/4] FAIL: timeout (5s) waiting for initialize response");
            std::process::exit(1);
        }
    }

    // Send notifications/initialized
    let notif = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    let notif_bytes = serde_json::to_string(&notif).unwrap();
    let _ = write_framed(&mut writer, notif_bytes.as_bytes()).await;

    // Step 4: Send $/ping
    let ping_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "$/ping"
    });
    let ping_bytes = serde_json::to_string(&ping_req).unwrap();
    if let Err(e) = write_framed(&mut writer, ping_bytes.as_bytes()).await {
        eprintln!("[4/4] FAIL: write ping: {}", e);
        std::process::exit(1);
    }

    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        mae_mcp::read_message(&mut reader),
    )
    .await
    {
        Ok(Ok(Some(resp))) => {
            if resp.contains("pong") {
                eprintln!("[4/4] ping -> pong OK");
            } else {
                eprintln!("[4/4] ping response (unexpected): {}", resp);
            }
        }
        Ok(Ok(None)) => {
            eprintln!("[4/4] FAIL: server closed connection");
            std::process::exit(1);
        }
        Ok(Err(e)) => {
            eprintln!("[4/4] FAIL: read error: {}", e);
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("[4/4] FAIL: timeout (5s) waiting for ping response");
            std::process::exit(1);
        }
    }

    eprintln!();
    eprintln!("All checks passed.");
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

    let socket_path = env::var("MAE_MCP_SOCKET").unwrap_or_else(|_| match discover_socket() {
        Some(path) => {
            eprintln!("mae-mcp-shim: discovered {}", path);
            log(&logfile, &format!("discovered {}", path));
            path
        }
        None => {
            eprintln!("mae-mcp-shim: error: no live mae socket found in /tmp/");
            eprintln!("  Hint: start mae first, or set MAE_MCP_SOCKET=/tmp/mae-<PID>.sock");
            log(&logfile, "error: no live mae socket found");
            std::process::exit(1);
        }
    });

    log(&logfile, &format!("connecting to {}", socket_path));

    let stream = match UnixStream::connect(&socket_path).await {
        Ok(s) => {
            eprintln!("mae-mcp-shim: connected to {}", socket_path);
            log(&logfile, "connected");
            s
        }
        Err(e) => {
            let msg = format!("error: connect to {}: {}", socket_path, e);
            eprintln!("mae-mcp-shim: {}", msg);
            log(&logfile, &msg);
            std::process::exit(1);
        }
    };

    let (socket_reader, mut socket_writer) = stream.into_split();
    let mut socket_reader = BufReader::new(socket_reader);

    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut stdin_reader = BufReader::new(stdin);

    let log_in = logfile.clone();
    let log_out = logfile.clone();

    let relay_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let relay_flag_out = relay_flag.clone();

    let _ = tokio::join!(
        // stdin -> socket: read newline-delimited JSON from Claude Code,
        // forward with Content-Length framing to the MAE Unix socket.
        async {
            loop {
                let mut line = String::new();
                match stdin_reader.read_line(&mut line).await {
                    Ok(0) => {
                        log(&log_in, "stdin EOF");
                        eprintln!("mae-mcp-shim: stdin EOF, shutting down");
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        log(&log_in, &format!("C->S: {}", trimmed));
                        if let Err(e) = write_framed(&mut socket_writer, trimmed.as_bytes()).await {
                            log(&log_in, &format!("write error: {}", e));
                            eprintln!("mae-mcp-shim: error: socket write: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        log(&log_in, &format!("stdin read error: {}", e));
                        eprintln!("mae-mcp-shim: error: stdin read: {}", e);
                        break;
                    }
                }
            }
            let _ = socket_writer.shutdown().await;
        },
        // socket -> stdout: read Content-Length framed messages from MAE,
        // write as newline-delimited JSON to Claude Code's stdout.
        async {
            loop {
                match mae_mcp::read_message(&mut socket_reader).await {
                    Ok(Some(msg)) => {
                        log(&log_out, &format!("S->C: {}", msg));
                        if let Err(e) = write_jsonl(&mut stdout, msg.as_bytes()).await {
                            log(&log_out, &format!("stdout write error: {}", e));
                            eprintln!("mae-mcp-shim: error: stdout write: {}", e);
                            break;
                        }
                        if !relay_flag_out.load(std::sync::atomic::Ordering::Relaxed) {
                            relay_flag_out.store(true, std::sync::atomic::Ordering::Relaxed);
                            eprintln!("mae-mcp-shim: ready (first message relayed)");
                        }
                    }
                    Ok(None) => {
                        log(&log_out, "socket EOF");
                        eprintln!("mae-mcp-shim: socket EOF");
                        break;
                    }
                    Err(e) => {
                        log(&log_out, &format!("socket read error: {}", e));
                        eprintln!("mae-mcp-shim: error: socket read: {}", e);
                        break;
                    }
                }
            }
        }
    );

    log(&logfile, "shim exiting");
    eprintln!("mae-mcp-shim: exiting");
}
