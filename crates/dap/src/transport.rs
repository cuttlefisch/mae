//! Content-Length framed transport for DAP.
//!
//! DAP uses the same wire format as LSP: HTTP-style `Content-Length` headers
//! over a byte stream (stdio or TCP). The header section ends with `\r\n\r\n`,
//! followed by exactly `Content-Length` bytes of JSON.
//!
//! ```text
//! Content-Length: 119\r\n
//! \r\n
//! {"seq":1,"type":"request","command":"initialize","arguments":{"clientID":"mae",...}}
//! ```

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::protocol::DapMessage;

/// Errors that can occur during transport operations.
#[derive(Debug)]
pub enum TransportError {
    Io(std::io::Error),
    InvalidHeader(String),
    Json(serde_json::Error),
    ConnectionClosed,
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Io(e) => write!(f, "I/O error: {}", e),
            TransportError::InvalidHeader(s) => write!(f, "invalid header: {}", s),
            TransportError::Json(e) => write!(f, "JSON error: {}", e),
            TransportError::ConnectionClosed => write!(f, "connection closed"),
        }
    }
}

impl std::error::Error for TransportError {}

impl From<std::io::Error> for TransportError {
    fn from(e: std::io::Error) -> Self {
        TransportError::Io(e)
    }
}

impl From<serde_json::Error> for TransportError {
    fn from(e: serde_json::Error) -> Self {
        TransportError::Json(e)
    }
}

/// Content-Length framed transport over async reader/writer.
pub struct DapTransport<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> {
    reader: BufReader<R>,
    writer: W,
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> DapTransport<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        DapTransport {
            reader: BufReader::new(reader),
            writer,
        }
    }

    /// Read one DAP message. Parses Content-Length header, reads body, deserializes.
    pub async fn read_message(&mut self) -> Result<DapMessage, TransportError> {
        let mut content_length: Option<usize> = None;

        // Read header lines until empty line
        loop {
            let mut line = String::new();
            let bytes_read = self.reader.read_line(&mut line).await?;
            if bytes_read == 0 {
                return Err(TransportError::ConnectionClosed);
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                // End of headers
                break;
            }

            if let Some(value) = trimmed.strip_prefix("Content-Length:") {
                let len_str = value.trim();
                content_length = Some(len_str.parse::<usize>().map_err(|_| {
                    TransportError::InvalidHeader(format!(
                        "invalid Content-Length value: '{}'",
                        len_str
                    ))
                })?);
            }
            // Ignore other headers (e.g. Content-Type)
        }

        let length = content_length.ok_or_else(|| {
            TransportError::InvalidHeader("missing Content-Length header".into())
        })?;

        // Read exactly `length` bytes
        let mut body = vec![0u8; length];
        self.reader.read_exact(&mut body).await?;

        let msg: DapMessage = serde_json::from_slice(&body)?;
        Ok(msg)
    }

    /// Write one DAP message. Serializes to JSON, prepends Content-Length header.
    pub async fn write_message(&mut self, msg: &DapMessage) -> Result<(), TransportError> {
        let body = serde_json::to_vec(msg)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());

        self.writer.write_all(header.as_bytes()).await?;
        self.writer.write_all(&body).await?;
        self.writer.flush().await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_and_read_round_trip() {
        // duplex: writing to `client` is readable from `server`, and vice versa.
        let (client, server) = tokio::io::duplex(4096);
        let (server_read, server_write) = tokio::io::split(server);
        let (_, client_write) = tokio::io::split(client);

        // Writer writes to client_write → readable from server_read
        let mut writer = DapTransport::new(tokio::io::empty(), client_write);
        let mut reader = DapTransport::new(server_read, server_write);

        let original = DapMessage::Request(crate::protocol::DapRequest {
            seq: 42,
            command: "initialize".into(),
            arguments: Some(serde_json::json!({"clientID": "mae", "linesStartAt1": true})),
        });

        writer.write_message(&original).await.unwrap();
        let received = reader.read_message().await.unwrap();

        match received {
            DapMessage::Request(req) => {
                assert_eq!(req.seq, 42);
                assert_eq!(req.command, "initialize");
                let args = req.arguments.unwrap();
                assert_eq!(args["clientID"], "mae");
            }
            _ => panic!("expected Request"),
        }
    }

    #[tokio::test]
    async fn read_invalid_header_returns_error() {
        // Feed data with no Content-Length header — just an empty header section
        // followed by some garbage
        let data = b"X-Custom: foo\r\n\r\n{}";
        let reader = &data[..];
        let writer = tokio::io::sink();

        let mut transport = DapTransport::new(reader, writer);
        let result = transport.read_message().await;
        assert!(result.is_err());
        match result.unwrap_err() {
            TransportError::InvalidHeader(msg) => {
                assert!(msg.contains("missing Content-Length"));
            }
            other => panic!("expected InvalidHeader, got: {:?}", other),
        }
    }
}
