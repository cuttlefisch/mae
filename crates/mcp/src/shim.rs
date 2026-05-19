//! MCP stdio <-> Unix socket shim.
//!
//! Claude Code (or any MCP client) spawns this binary as its MCP server
//! process. It reads `MAE_MCP_SOCKET` from the environment and bridges
//! stdin/stdout to the MAE editor's Unix socket.
//!
//! Both directions use Content-Length framing (with line-based fallback)
//! via `mae_mcp::read_message`. Set `MAE_MCP_SHIM_LOG=/path/to/file.log`
//! to log all traffic for debugging.

use std::env;
use std::path::PathBuf;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

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
    env::var("MAE_MCP_SHIM_LOG").ok().and_then(|path| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()
            .map(|f| std::sync::Arc::new(std::sync::Mutex::new(f)))
    })
}

/// Write a Content-Length framed message to any async writer.
async fn write_framed<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    body: &[u8],
) -> Result<(), std::io::Error> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(body).await?;
    writer.flush().await
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let logfile = open_log();

    let socket_path = env::var("MAE_MCP_SOCKET").unwrap_or_else(|_| match discover_socket() {
        Some(path) => {
            eprintln!("mae-mcp-shim: auto-discovered {}", path);
            log(&logfile, &format!("auto-discovered {}", path));
            path
        }
        None => {
            eprintln!("mae-mcp-shim: no live mae socket found in /tmp/");
            eprintln!("  Hint: start mae first, or set MAE_MCP_SOCKET=/tmp/mae-<PID>.sock");
            std::process::exit(1);
        }
    });

    log(&logfile, &format!("connecting to {}", socket_path));

    let stream = match UnixStream::connect(&socket_path).await {
        Ok(s) => {
            log(&logfile, "connected");
            s
        }
        Err(e) => {
            let msg = format!("failed to connect to {}: {}", socket_path, e);
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

    let _ = tokio::join!(
        // stdin -> socket: read Content-Length framed messages from Claude Code,
        // re-frame and forward to the MAE Unix socket.
        async {
            loop {
                match mae_mcp::read_message(&mut stdin_reader).await {
                    Ok(Some(msg)) => {
                        log(&log_in, &format!("C->S: {}", msg));
                        if let Err(e) = write_framed(&mut socket_writer, msg.as_bytes()).await {
                            log(&log_in, &format!("write error: {}", e));
                            break;
                        }
                    }
                    Ok(None) => {
                        log(&log_in, "stdin EOF");
                        break;
                    }
                    Err(e) => {
                        log(&log_in, &format!("stdin read error: {}", e));
                        break;
                    }
                }
            }
            let _ = socket_writer.shutdown().await;
        },
        // socket -> stdout: read Content-Length framed messages from MAE,
        // re-frame and forward to Claude Code's stdout.
        async {
            loop {
                match mae_mcp::read_message(&mut socket_reader).await {
                    Ok(Some(msg)) => {
                        log(&log_out, &format!("S->C: {}", msg));
                        if let Err(e) = write_framed(&mut stdout, msg.as_bytes()).await {
                            log(&log_out, &format!("stdout write error: {}", e));
                            break;
                        }
                    }
                    Ok(None) => {
                        log(&log_out, "socket EOF");
                        break;
                    }
                    Err(e) => {
                        log(&log_out, &format!("socket read error: {}", e));
                        break;
                    }
                }
            }
        }
    );

    log(&logfile, "shim exiting");
}
