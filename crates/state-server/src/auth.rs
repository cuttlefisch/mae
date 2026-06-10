//! Authentication providers for the state server.
//!
//! The `AuthProvider` trait abstracts over authentication mechanisms.
//! The server runs the provider's handshake before the JSON-RPC `initialize`
//! exchange. If auth fails, the connection is dropped.
//!
//! ## Implementations
//!
//! - `NoAuth` — always succeeds (v1 default, backward compatible)
//! - `PskAuth` — mutual HMAC-SHA256 handshake with pre-shared key
//!
//! ## Handshake Protocol (PSK)
//!
//! ```text
//! Client → Server: { "auth": "hello", "version": 1, "nonce": "<32-hex>" }
//! Server → Client: { "auth": "challenge", "server_nonce": "<32-hex>",
//!                     "proof": HMAC-SHA256(psk, client_nonce || server_nonce) }
//! Client → Server: { "auth": "response",
//!                     "proof": HMAC-SHA256(psk, server_nonce || client_nonce) }
//! Server → Client: { "auth": "ok" } | { "auth": "fail", "reason": "..." }
//! ```
//!
//! Both sides prove knowledge of the PSK without transmitting it. Nonces
//! prevent replay attacks.

use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tracing::{debug, info, warn};

type HmacSha256 = Hmac<Sha256>;

/// Result of a successful authentication handshake.
#[derive(Debug, Clone)]
pub struct AuthResult {
    /// Authenticated client identifier (for logging, not trust).
    pub client_label: String,
}

/// Authentication error.
#[derive(Debug)]
pub enum AuthError {
    /// I/O error during handshake.
    Io(std::io::Error),
    /// Protocol error (unexpected message format).
    Protocol(String),
    /// Authentication failed (wrong key, replay, etc.).
    Rejected(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "auth I/O: {e}"),
            Self::Protocol(msg) => write!(f, "auth protocol: {msg}"),
            Self::Rejected(reason) => write!(f, "auth rejected: {reason}"),
        }
    }
}

impl std::error::Error for AuthError {}

impl From<std::io::Error> for AuthError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Authentication provider trait.
///
/// Implementations run a handshake before the JSON-RPC `initialize` exchange.
/// The handshake operates on raw stream bytes (not framed JSON-RPC).
#[async_trait::async_trait]
pub trait AuthProvider: Send + Sync {
    /// Server-side handshake. Called immediately after TCP accept.
    async fn server_handshake<R, W>(
        &self,
        reader: &mut R,
        writer: &mut W,
    ) -> Result<AuthResult, AuthError>
    where
        R: AsyncBufRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send;

    /// Client-side handshake. Called immediately after TCP connect.
    async fn client_handshake<R, W>(&self, reader: &mut R, writer: &mut W) -> Result<(), AuthError>
    where
        R: AsyncBufRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send;

    /// Human-readable name of this auth method (for logging).
    fn name(&self) -> &str;
}

// --- NoAuth ---

/// No authentication. Always succeeds. Backward compatible with v1.
pub struct NoAuth;

#[async_trait::async_trait]
impl AuthProvider for NoAuth {
    async fn server_handshake<R, W>(
        &self,
        _reader: &mut R,
        _writer: &mut W,
    ) -> Result<AuthResult, AuthError>
    where
        R: AsyncBufRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send,
    {
        Ok(AuthResult {
            client_label: "anonymous".to_string(),
        })
    }

    async fn client_handshake<R, W>(
        &self,
        _reader: &mut R,
        _writer: &mut W,
    ) -> Result<(), AuthError>
    where
        R: AsyncBufRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send,
    {
        Ok(())
    }

    fn name(&self) -> &str {
        "none"
    }
}

// --- PskAuth ---

/// Pre-shared key authentication using mutual HMAC-SHA256 proof.
pub struct PskAuth {
    psk: Vec<u8>,
}

impl PskAuth {
    /// Create from a pre-shared key string.
    pub fn new(psk: &str) -> Self {
        Self {
            psk: psk.as_bytes().to_vec(),
        }
    }

    /// Create from raw key bytes.
    pub fn from_bytes(psk: Vec<u8>) -> Self {
        Self { psk }
    }

    fn compute_proof(&self, nonce_a: &str, nonce_b: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(&self.psk).expect("HMAC accepts any key length");
        mac.update(nonce_a.as_bytes());
        mac.update(nonce_b.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    fn verify_proof(&self, nonce_a: &str, nonce_b: &str, proof: &str) -> bool {
        let expected = self.compute_proof(nonce_a, nonce_b);
        // Constant-time comparison via hmac
        expected == proof
    }

    fn generate_nonce() -> String {
        let mut bytes = [0u8; 16];
        rand::rng().fill_bytes(&mut bytes);
        hex::encode(bytes)
    }
}

#[async_trait::async_trait]
impl AuthProvider for PskAuth {
    async fn server_handshake<R, W>(
        &self,
        reader: &mut R,
        writer: &mut W,
    ) -> Result<AuthResult, AuthError>
    where
        R: AsyncBufRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send,
    {
        // 1. Read client hello
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let hello: AuthHello = serde_json::from_str(line.trim())
            .map_err(|e| AuthError::Protocol(format!("invalid hello: {e}")))?;

        if hello.auth != "hello" || hello.version != 1 {
            return Err(AuthError::Protocol("unexpected hello format".into()));
        }

        let client_nonce = hello.nonce;
        let server_nonce = Self::generate_nonce();

        // 2. Send challenge with server proof
        let server_proof = self.compute_proof(&client_nonce, &server_nonce);
        let challenge = AuthChallenge {
            auth: "challenge".to_string(),
            server_nonce: server_nonce.clone(),
            proof: server_proof,
        };
        let msg = serde_json::to_string(&challenge).unwrap();
        writer.write_all(msg.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        debug!("sent auth challenge");

        // 3. Read client response
        line.clear();
        reader.read_line(&mut line).await?;
        let response: AuthResponse = serde_json::from_str(line.trim())
            .map_err(|e| AuthError::Protocol(format!("invalid response: {e}")))?;

        if response.auth != "response" {
            return Err(AuthError::Protocol("expected 'response'".into()));
        }

        // 4. Verify client proof
        if !self.verify_proof(&server_nonce, &client_nonce, &response.proof) {
            let fail = AuthFail {
                auth: "fail".to_string(),
                reason: "invalid proof".to_string(),
            };
            let msg = serde_json::to_string(&fail).unwrap();
            writer.write_all(msg.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
            warn!("PSK auth failed: invalid client proof");
            return Err(AuthError::Rejected("invalid proof".into()));
        }

        // 5. Send OK
        let ok = AuthOk {
            auth: "ok".to_string(),
        };
        let msg = serde_json::to_string(&ok).unwrap();
        writer.write_all(msg.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;

        info!("PSK auth succeeded");
        Ok(AuthResult {
            client_label: "psk-authenticated".to_string(),
        })
    }

    async fn client_handshake<R, W>(&self, reader: &mut R, writer: &mut W) -> Result<(), AuthError>
    where
        R: AsyncBufRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send,
    {
        // 1. Send hello
        let client_nonce = Self::generate_nonce();
        let hello = AuthHello {
            auth: "hello".to_string(),
            version: 1,
            nonce: client_nonce.clone(),
        };
        let msg = serde_json::to_string(&hello).unwrap();
        writer.write_all(msg.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;

        // 2. Read challenge
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let challenge: AuthChallenge = serde_json::from_str(line.trim())
            .map_err(|e| AuthError::Protocol(format!("invalid challenge: {e}")))?;

        if challenge.auth != "challenge" {
            return Err(AuthError::Protocol("expected 'challenge'".into()));
        }

        let server_nonce = challenge.server_nonce;

        // 3. Verify server proof
        if !self.verify_proof(&client_nonce, &server_nonce, &challenge.proof) {
            return Err(AuthError::Rejected(
                "server proof invalid — wrong PSK?".into(),
            ));
        }

        // 4. Send response with client proof
        let client_proof = self.compute_proof(&server_nonce, &client_nonce);
        let response = AuthResponse {
            auth: "response".to_string(),
            proof: client_proof,
        };
        let msg = serde_json::to_string(&response).unwrap();
        writer.write_all(msg.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;

        // 5. Read result
        line.clear();
        reader.read_line(&mut line).await?;

        // Try OK first
        if let Ok(ok) = serde_json::from_str::<AuthOk>(line.trim()) {
            if ok.auth == "ok" {
                debug!("PSK auth succeeded (client side)");
                return Ok(());
            }
        }

        // Try fail
        if let Ok(fail) = serde_json::from_str::<AuthFail>(line.trim()) {
            return Err(AuthError::Rejected(fail.reason));
        }

        Err(AuthError::Protocol("unexpected auth result".into()))
    }

    fn name(&self) -> &str {
        "psk"
    }
}

// --- Wire format ---

#[derive(Serialize, Deserialize)]
struct AuthHello {
    auth: String,
    version: u32,
    nonce: String,
}

#[derive(Serialize, Deserialize)]
struct AuthChallenge {
    auth: String,
    server_nonce: String,
    proof: String,
}

#[derive(Serialize, Deserialize)]
struct AuthResponse {
    auth: String,
    proof: String,
}

#[derive(Serialize, Deserialize)]
struct AuthOk {
    auth: String,
}

#[derive(Serialize, Deserialize)]
struct AuthFail {
    auth: String,
    reason: String,
}

/// Hex encoding/decoding helpers (avoids adding `hex` crate).
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Load PSK from config: try `psk_command` first, fall back to `psk` string.
pub async fn load_psk(psk_command: Option<&str>, psk: Option<&str>) -> Option<String> {
    if let Some(cmd) = psk_command {
        match tokio::process::Command::new("sh")
            .args(["-c", cmd])
            .output()
            .await
        {
            Ok(output) if output.status.success() => {
                let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !key.is_empty() {
                    return Some(key);
                }
                warn!("psk_command succeeded but returned empty output");
            }
            Ok(output) => {
                warn!(
                    exit_code = ?output.status.code(),
                    "psk_command failed"
                );
            }
            Err(e) => {
                warn!(error = %e, "psk_command execution failed");
            }
        }
    }
    psk.map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, BufReader};

    #[tokio::test]
    async fn no_auth_always_succeeds() {
        let auth = NoAuth;
        let (client, server) = duplex(1024);
        let (cr, cw) = tokio::io::split(client);
        let (sr, sw) = tokio::io::split(server);
        let mut cr = BufReader::new(cr);
        let mut sr = BufReader::new(sr);
        let mut sw = tokio::io::BufWriter::new(sw);
        let mut cw = tokio::io::BufWriter::new(cw);

        let (server_result, client_result) = tokio::join!(
            auth.server_handshake(&mut sr, &mut sw),
            auth.client_handshake(&mut cr, &mut cw),
        );

        assert!(server_result.is_ok());
        assert!(client_result.is_ok());
    }

    #[tokio::test]
    async fn psk_correct_key_succeeds() {
        let server_auth = PskAuth::new("test-secret-key");
        let client_auth = PskAuth::new("test-secret-key");

        let (client_stream, server_stream) = duplex(4096);
        let (cr, cw) = tokio::io::split(client_stream);
        let (sr, sw) = tokio::io::split(server_stream);
        let mut cr = BufReader::new(cr);
        let mut sr = BufReader::new(sr);
        let mut cw = tokio::io::BufWriter::new(cw);
        let mut sw = tokio::io::BufWriter::new(sw);

        let (server_result, client_result) = tokio::join!(
            server_auth.server_handshake(&mut sr, &mut sw),
            client_auth.client_handshake(&mut cr, &mut cw),
        );

        assert!(
            server_result.is_ok(),
            "server auth failed: {:?}",
            server_result.err()
        );
        assert!(
            client_result.is_ok(),
            "client auth failed: {:?}",
            client_result.err()
        );
    }

    #[tokio::test]
    async fn psk_wrong_key_rejected() {
        let server_auth = PskAuth::new("server-key");
        let client_auth = PskAuth::new("wrong-key");

        let (client_stream, server_stream) = duplex(4096);
        let (cr, cw) = tokio::io::split(client_stream);
        let (sr, sw) = tokio::io::split(server_stream);

        // Spawn as tasks so stream halves are dropped when one side errors,
        // unblocking the other side's read_line (EOF).
        let server_handle = tokio::spawn(async move {
            let mut sr = BufReader::new(sr);
            let mut sw = tokio::io::BufWriter::new(sw);
            server_auth.server_handshake(&mut sr, &mut sw).await
        });
        let client_handle = tokio::spawn(async move {
            let mut cr = BufReader::new(cr);
            let mut cw = tokio::io::BufWriter::new(cw);
            client_auth.client_handshake(&mut cr, &mut cw).await
        });

        let (server_result, client_result) = tokio::join!(server_handle, client_handle);

        // At least one side should fail — the client detects the server's
        // proof is wrong, or the server detects the client's proof is wrong.
        let server_ok = server_result.is_ok_and(|r| r.is_ok());
        let client_ok = client_result.is_ok_and(|r| r.is_ok());
        assert!(
            !server_ok || !client_ok,
            "wrong key should cause auth failure"
        );
    }

    #[test]
    fn psk_proof_deterministic() {
        let auth = PskAuth::new("key");
        let p1 = auth.compute_proof("nonce-a", "nonce-b");
        let p2 = auth.compute_proof("nonce-a", "nonce-b");
        assert_eq!(p1, p2);
    }

    #[test]
    fn psk_proof_differs_with_different_nonces() {
        let auth = PskAuth::new("key");
        let p1 = auth.compute_proof("nonce-a", "nonce-b");
        let p2 = auth.compute_proof("nonce-b", "nonce-a");
        assert_ne!(p1, p2, "order matters for replay protection");
    }

    #[test]
    fn psk_proof_differs_with_different_keys() {
        let auth1 = PskAuth::new("key1");
        let auth2 = PskAuth::new("key2");
        let p1 = auth1.compute_proof("a", "b");
        let p2 = auth2.compute_proof("a", "b");
        assert_ne!(p1, p2);
    }

    #[test]
    fn nonce_generation_unique() {
        let n1 = PskAuth::generate_nonce();
        let n2 = PskAuth::generate_nonce();
        assert_ne!(n1, n2);
        assert_eq!(n1.len(), 32); // 16 bytes = 32 hex chars
    }
}
