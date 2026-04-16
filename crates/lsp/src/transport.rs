//! Content-Length framed transport for LSP.
//!
//! LSP uses the same wire format as DAP: HTTP-style `Content-Length` headers
//! over a byte stream (stdio). The header section ends with `\r\n\r\n`,
//! followed by exactly `Content-Length` bytes of UTF-8 JSON.
//!
//! This is intentionally a near-clone of `mae-dap::transport`. We own both
//! crates and the slight duplication is preferable to a shared dependency
//! for ~100 lines of framing code.

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::protocol::Message;

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
pub struct LspTransport<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> {
    reader: BufReader<R>,
    writer: W,
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> LspTransport<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        LspTransport {
            reader: BufReader::new(reader),
            writer,
        }
    }

    /// Read one LSP message. Parses Content-Length header, reads body, deserializes.
    pub async fn read_message(&mut self) -> Result<Message, TransportError> {
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

        let length = content_length
            .ok_or_else(|| TransportError::InvalidHeader("missing Content-Length header".into()))?;

        let mut body = vec![0u8; length];
        self.reader.read_exact(&mut body).await?;

        let msg: Message = serde_json::from_slice(&body)?;
        Ok(msg)
    }

    /// Write one LSP message. Serializes to JSON, prepends Content-Length header.
    pub async fn write_message(&mut self, msg: &Message) -> Result<(), TransportError> {
        let body = serde_json::to_vec(msg)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());

        self.writer.write_all(header.as_bytes()).await?;
        self.writer.write_all(&body).await?;
        self.writer.flush().await?;

        Ok(())
    }

    /// Write a raw JSON-RPC message (for sending typed Request/Notification directly).
    pub async fn write_raw(&mut self, body: &[u8]) -> Result<(), TransportError> {
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.writer.write_all(header.as_bytes()).await?;
        self.writer.write_all(body).await?;
        self.writer.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Notification, Request, RequestId, Response};

    #[tokio::test]
    async fn write_and_read_request_round_trip() {
        let (client, server) = tokio::io::duplex(4096);
        let (server_read, server_write) = tokio::io::split(server);
        let (_, client_write) = tokio::io::split(client);

        let mut writer = LspTransport::new(tokio::io::empty(), client_write);
        let mut reader = LspTransport::new(server_read, server_write);

        let req = Request::new(1, "initialize", Some(serde_json::json!({"processId": 42})));
        let msg = Message::Request(req);

        writer.write_message(&msg).await.unwrap();
        let received = reader.read_message().await.unwrap();

        match received {
            Message::Request(req) => {
                assert_eq!(req.id, RequestId::Integer(1));
                assert_eq!(req.method, "initialize");
                assert_eq!(req.params.unwrap()["processId"], 42);
            }
            _ => panic!("expected Request"),
        }
    }

    #[tokio::test]
    async fn write_and_read_notification_round_trip() {
        let (client, server) = tokio::io::duplex(4096);
        let (server_read, server_write) = tokio::io::split(server);
        let (_, client_write) = tokio::io::split(client);

        let mut writer = LspTransport::new(tokio::io::empty(), client_write);
        let mut reader = LspTransport::new(server_read, server_write);

        let notif = Notification::new("initialized", None);
        let msg = Message::Notification(notif);

        writer.write_message(&msg).await.unwrap();
        let received = reader.read_message().await.unwrap();

        match received {
            Message::Notification(n) => {
                assert_eq!(n.method, "initialized");
                assert!(n.params.is_none());
            }
            _ => panic!("expected Notification"),
        }
    }

    #[tokio::test]
    async fn write_and_read_response_round_trip() {
        let (client, server) = tokio::io::duplex(4096);
        let (server_read, server_write) = tokio::io::split(server);
        let (_, client_write) = tokio::io::split(client);

        let mut writer = LspTransport::new(tokio::io::empty(), client_write);
        let mut reader = LspTransport::new(server_read, server_write);

        let resp = Response::ok(
            RequestId::Integer(1),
            serde_json::json!({"capabilities": {}}),
        );
        let msg = Message::Response(resp);

        writer.write_message(&msg).await.unwrap();
        let received = reader.read_message().await.unwrap();

        match received {
            Message::Response(r) => {
                assert_eq!(r.id, RequestId::Integer(1));
                assert!(r.result.is_some());
                assert!(r.error.is_none());
            }
            _ => panic!("expected Response"),
        }
    }

    #[tokio::test]
    async fn read_missing_content_length_returns_error() {
        let data = b"X-Custom: foo\r\n\r\n{}";
        let reader = &data[..];
        let writer = tokio::io::sink();

        let mut transport = LspTransport::new(reader, writer);
        let result = transport.read_message().await;
        assert!(result.is_err());
        match result.unwrap_err() {
            TransportError::InvalidHeader(msg) => {
                assert!(msg.contains("missing Content-Length"));
            }
            other => panic!("expected InvalidHeader, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn read_connection_closed_returns_error() {
        let data: &[u8] = b"";
        let writer = tokio::io::sink();

        let mut transport = LspTransport::new(data, writer);
        let result = transport.read_message().await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TransportError::ConnectionClosed
        ));
    }
}
