//! MCP stdio <-> Unix socket shim.
//!
//! Claude Code (or any MCP client) spawns this binary as its MCP server
//! process. It reads `MAE_MCP_SOCKET` from the environment and bridges
//! stdin/stdout to the MAE editor's Unix socket.

use std::env;
use tokio::io::{AsyncWriteExt, BufReader};
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
    // Use tokio::io::copy for raw, robust, unbuffered proxying.
    // We use join! so both directions run concurrently until EOF/error.
    let _ = tokio::join!(
        async {
            if let Err(e) = tokio::io::copy(&mut stdin_reader, &mut socket_writer).await {
                eprintln!("mae-mcp-shim: stdin -> socket error: {}", e);
            }
            let _ = socket_writer.shutdown().await;
        },
        async {
            if let Err(e) = tokio::io::copy(&mut socket_reader, &mut stdout).await {
                eprintln!("mae-mcp-shim: socket -> stdout error: {}", e);
            }
            let _ = stdout.flush().await;
        }
    );
}
