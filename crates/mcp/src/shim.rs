//! MCP stdio <-> Unix socket shim.
//!
//! Claude Code (or any MCP client) spawns this binary as its MCP server
//! process. It reads `MAE_MCP_SOCKET` from the environment and bridges
//! stdin/stdout to the MAE editor's Unix socket.

use std::env;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let socket_path = env::var("MAE_MCP_SOCKET").unwrap_or_else(|_| {
        eprintln!("mae-mcp-shim: MAE_MCP_SOCKET not set");
        std::process::exit(1);
    });

    let stream = match UnixStream::connect(&socket_path).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mae-mcp-shim: failed to connect to {}: {}", socket_path, e);
            std::process::exit(1);
        }
    };

    let (socket_reader, mut socket_writer) = stream.into_split();
    let mut socket_reader = BufReader::new(socket_reader);

    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut stdin_reader = BufReader::new(stdin);

    // Bidirectional pipe: stdin -> socket, socket -> stdout.
    tokio::select! {
        _ = async {
            let mut line = String::new();
            loop {
                line.clear();
                match stdin_reader.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        if socket_writer.write_all(line.as_bytes()).await.is_err() {
                            break;
                        }
                        let _ = socket_writer.flush().await;
                    }
                    Err(_) => break,
                }
            }
        } => {}
        _ = async {
            let mut line = String::new();
            loop {
                line.clear();
                match socket_reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        if stdout.write_all(line.as_bytes()).await.is_err() {
                            break;
                        }
                        let _ = stdout.flush().await;
                    }
                    Err(_) => break,
                }
            }
        } => {}
    }
}
