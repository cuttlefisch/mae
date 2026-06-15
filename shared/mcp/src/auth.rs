//! Authentication providers for MCP transport.
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

/// One key the auth provider knows about: an optional name plus secret bytes.
struct KeyMaterial {
    name: Option<String>,
    secret: Vec<u8>,
}

/// Pre-shared key authentication using mutual HMAC-SHA256 proof.
///
/// A *client* holds exactly one key (the first in `keys`) and may advertise its
/// name via `offer_id` so a multi-key server can pick the matching secret. A
/// *server* may hold a SET of trusted keys (a `trusted_keys` keystore): it
/// selects the one named by the client's `key_id`, or the first key when the
/// client offers none (preserving single-key backward compatibility).
pub struct PskAuth {
    keys: Vec<KeyMaterial>,
    /// Client-side: the `key_id` to advertise in the hello (`None` = unnamed).
    offer_id: Option<String>,
}

impl PskAuth {
    /// Create from a single pre-shared key string (unnamed). Backward compatible
    /// with the original single-key constructor.
    pub fn new(psk: &str) -> Self {
        Self {
            keys: vec![KeyMaterial {
                name: None,
                secret: psk.as_bytes().to_vec(),
            }],
            offer_id: None,
        }
    }

    /// Create from raw key bytes (unnamed, single key).
    pub fn from_bytes(psk: Vec<u8>) -> Self {
        Self {
            keys: vec![KeyMaterial {
                name: None,
                secret: psk,
            }],
            offer_id: None,
        }
    }

    /// Build a server-side provider trusting a SET of keys. Each entry is an
    /// optional name plus its secret. A client authenticates if it proves
    /// knowledge of any trusted key (selected by `key_id`, else the first).
    pub fn from_keys<I, S>(keys: I) -> Self
    where
        I: IntoIterator<Item = (Option<String>, S)>,
        S: Into<String>,
    {
        Self {
            keys: keys
                .into_iter()
                .map(|(name, secret)| KeyMaterial {
                    name,
                    secret: secret.into().into_bytes(),
                })
                .collect(),
            offer_id: None,
        }
    }

    /// Client-side: advertise this `key_id` in the hello so a multi-key server
    /// selects the matching key. `None` leaves the hello unnamed.
    pub fn offering(mut self, id: Option<String>) -> Self {
        self.offer_id = id;
        self
    }

    /// True when this provider holds no keys (a misconfigured server).
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Server-side key selection: named lookup, else the first (default) key.
    fn select(&self, key_id: Option<&str>) -> Option<&KeyMaterial> {
        match key_id {
            Some(id) => self.keys.iter().find(|k| k.name.as_deref() == Some(id)),
            None => self.keys.first(),
        }
    }

    /// HMAC-SHA256(secret, nonce_a || nonce_b), hex-encoded.
    fn proof_with(secret: &[u8], nonce_a: &str, nonce_b: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
        mac.update(nonce_a.as_bytes());
        mac.update(nonce_b.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    /// Constant-time verification of a hex proof against `secret`.
    fn verify_with(secret: &[u8], nonce_a: &str, nonce_b: &str, proof_hex: &str) -> bool {
        let proof = match hex::decode(proof_hex) {
            Some(p) => p,
            None => return false,
        };
        let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
        mac.update(nonce_a.as_bytes());
        mac.update(nonce_b.as_bytes());
        // `verify_slice` is constant-time (defends against proof-timing oracles).
        mac.verify_slice(&proof).is_ok()
    }

    /// Compute a proof with the primary (first) key. Used in unit tests.
    #[cfg(test)]
    fn compute_proof(&self, nonce_a: &str, nonce_b: &str) -> String {
        let secret = self
            .keys
            .first()
            .map(|k| k.secret.as_slice())
            .unwrap_or(&[]);
        Self::proof_with(secret, nonce_a, nonce_b)
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

        // Select the trusted key the client is presenting (named, else default).
        let key = match self.select(hello.key_id.as_deref()) {
            Some(k) => k,
            None => {
                let fail = AuthFail {
                    auth: "fail".to_string(),
                    reason: "unknown key id".to_string(),
                };
                let msg = serde_json::to_string(&fail).unwrap();
                writer.write_all(msg.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                writer.flush().await?;
                warn!(key_id = ?hello.key_id, "PSK auth failed: unknown key id");
                return Err(AuthError::Rejected("unknown key id".into()));
            }
        };
        let client_label = key.name.clone().unwrap_or_else(|| "psk".to_string());

        let client_nonce = hello.nonce;
        let server_nonce = Self::generate_nonce();

        // 2. Send challenge with server proof
        let server_proof = Self::proof_with(&key.secret, &client_nonce, &server_nonce);
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
        if !Self::verify_with(&key.secret, &server_nonce, &client_nonce, &response.proof) {
            let fail = AuthFail {
                auth: "fail".to_string(),
                reason: "invalid proof".to_string(),
            };
            let msg = serde_json::to_string(&fail).unwrap();
            writer.write_all(msg.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
            warn!(key = %client_label, "PSK auth failed: invalid client proof");
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

        info!(key = %client_label, "PSK auth succeeded");
        Ok(AuthResult { client_label })
    }

    async fn client_handshake<R, W>(&self, reader: &mut R, writer: &mut W) -> Result<(), AuthError>
    where
        R: AsyncBufRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send,
    {
        // The client presents exactly one key (the first).
        let secret = self
            .keys
            .first()
            .map(|k| k.secret.clone())
            .ok_or_else(|| AuthError::Protocol("client has no PSK".into()))?;

        // 1. Send hello (advertising our key_id so a multi-key server can pick).
        let client_nonce = Self::generate_nonce();
        let hello = AuthHello {
            auth: "hello".to_string(),
            version: 1,
            nonce: client_nonce.clone(),
            key_id: self.offer_id.clone(),
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
        if !Self::verify_with(&secret, &client_nonce, &server_nonce, &challenge.proof) {
            return Err(AuthError::Rejected(
                "server proof invalid — wrong key?".into(),
            ));
        }

        // 4. Send response with client proof
        let client_proof = Self::proof_with(&secret, &server_nonce, &client_nonce);
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

// --- KeyAuth (asymmetric Ed25519, ADR-017) ---

use crate::identity::{AuthorizedKeys, HostKeyVerifier, Identity, PublicKey};
use std::sync::Arc;

/// Asymmetric peer authentication: a mutual signed-challenge handshake over
/// Ed25519 keys. The server trusts a set of client public keys
/// (`authorized_keys`); the client pins the server's key via a
/// [`HostKeyVerifier`] (TOFU). See ADR-017.
pub struct KeyAuth {
    identity: Arc<Identity>,
    role: KeyRole,
}

enum KeyRole {
    Server {
        authorized: Arc<AuthorizedKeys>,
    },
    Client {
        addr: String,
        verifier: Arc<dyn HostKeyVerifier>,
    },
}

impl KeyAuth {
    /// Build a server-side provider trusting the given client keys.
    pub fn server(identity: Arc<Identity>, authorized: Arc<AuthorizedKeys>) -> Self {
        Self {
            identity,
            role: KeyRole::Server { authorized },
        }
    }

    /// Build a client-side provider that verifies the daemon at `addr` via
    /// `verifier` (TOFU / known_hosts policy).
    pub fn client(
        identity: Arc<Identity>,
        addr: String,
        verifier: Arc<dyn HostKeyVerifier>,
    ) -> Self {
        Self {
            identity,
            role: KeyRole::Client { addr, verifier },
        }
    }

    fn gen_nonce_b64() -> String {
        let mut bytes = [0u8; 16];
        rand::rng().fill_bytes(&mut bytes);
        b64::encode(&bytes)
    }
}

#[async_trait::async_trait]
impl AuthProvider for KeyAuth {
    async fn server_handshake<R, W>(
        &self,
        reader: &mut R,
        writer: &mut W,
    ) -> Result<AuthResult, AuthError>
    where
        R: AsyncBufRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send,
    {
        let authorized = match &self.role {
            KeyRole::Server { authorized } => authorized,
            KeyRole::Client { .. } => {
                return Err(AuthError::Protocol("KeyAuth client used as server".into()))
            }
        };

        // 1. Read client hello.
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let hello: KeyHello = serde_json::from_str(line.trim())
            .map_err(|e| AuthError::Protocol(format!("invalid key hello: {e}")))?;
        if hello.auth != "key-hello" || hello.v != 1 {
            return Err(AuthError::Protocol("unexpected key hello".into()));
        }
        let client_pub = PublicKey::from_encoded(&hello.client_pub, None)
            .ok_or_else(|| AuthError::Protocol("bad client public key".into()))?;
        let client_nonce = b64::decode(&hello.client_nonce)
            .ok_or_else(|| AuthError::Protocol("bad nonce".into()))?;

        // 2. Send offer with server proof (signature over the transcript).
        let server_nonce_b64 = Self::gen_nonce_b64();
        let server_nonce = b64::decode(&server_nonce_b64).unwrap();
        let server_pub = self.identity.public();
        let t = crate::identity::transcript(
            &client_pub.to_bytes(),
            &server_pub.to_bytes(),
            &client_nonce,
            &server_nonce,
        );
        let sig_s = self.identity.sign(&t);
        let offer = KeyOffer {
            auth: "key-offer".to_string(),
            server_pub: server_pub.encoded(),
            server_nonce: server_nonce_b64,
            sig: b64::encode(&sig_s),
        };
        write_line(writer, &serde_json::to_string(&offer).unwrap()).await?;

        // 3. Read client auth (its signature over the same transcript).
        line.clear();
        reader.read_line(&mut line).await?;
        let auth_msg: KeyAuthMsg = serde_json::from_str(line.trim())
            .map_err(|e| AuthError::Protocol(format!("invalid key auth: {e}")))?;
        let sig_c = b64::decode(&auth_msg.sig)
            .ok_or_else(|| AuthError::Protocol("bad client sig".into()))?;

        // 4. Verify the client owns its key.
        if !client_pub.verify(&t, &sig_c) {
            send_fail(writer, "invalid client signature").await?;
            warn!("KeyAuth: client signature invalid");
            return Err(AuthError::Rejected("invalid client signature".into()));
        }

        // 5. Is the client key authorized?
        match authorized.authorize(&client_pub.to_bytes()) {
            Some(label) => {
                let client_label = if label.is_empty() {
                    client_pub.fingerprint()
                } else {
                    label
                };
                send_ok(writer).await?;
                info!(client = %client_label, fp = %client_pub.fingerprint(), "KeyAuth succeeded");
                Ok(AuthResult { client_label })
            }
            None => {
                send_fail(writer, "public key not authorized").await?;
                warn!(fp = %client_pub.fingerprint(), "KeyAuth: client key not authorized");
                Err(AuthError::Rejected(format!(
                    "client key not authorized (fingerprint {})",
                    client_pub.fingerprint()
                )))
            }
        }
    }

    async fn client_handshake<R, W>(&self, reader: &mut R, writer: &mut W) -> Result<(), AuthError>
    where
        R: AsyncBufRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send,
    {
        let (addr, verifier) = match &self.role {
            KeyRole::Client { addr, verifier } => (addr, verifier),
            KeyRole::Server { .. } => {
                return Err(AuthError::Protocol("KeyAuth server used as client".into()))
            }
        };

        // 1. Send hello.
        let client_nonce_b64 = Self::gen_nonce_b64();
        let client_nonce = b64::decode(&client_nonce_b64).unwrap();
        let client_pub = self.identity.public();
        let hello = KeyHello {
            auth: "key-hello".to_string(),
            v: 1,
            client_pub: client_pub.encoded(),
            client_nonce: client_nonce_b64,
        };
        write_line(writer, &serde_json::to_string(&hello).unwrap()).await?;

        // 2. Read offer.
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let offer: KeyOffer = serde_json::from_str(line.trim())
            .map_err(|e| AuthError::Protocol(format!("invalid key offer: {e}")))?;
        if offer.auth != "key-offer" {
            return Err(AuthError::Protocol("expected key offer".into()));
        }
        let server_pub = PublicKey::from_encoded(&offer.server_pub, None)
            .ok_or_else(|| AuthError::Protocol("bad server public key".into()))?;
        let server_nonce = b64::decode(&offer.server_nonce)
            .ok_or_else(|| AuthError::Protocol("bad nonce".into()))?;
        let sig_s =
            b64::decode(&offer.sig).ok_or_else(|| AuthError::Protocol("bad server sig".into()))?;

        let t = crate::identity::transcript(
            &client_pub.to_bytes(),
            &server_pub.to_bytes(),
            &client_nonce,
            &server_nonce,
        );

        // 3. Verify the server owns the key it presented.
        if !server_pub.verify(&t, &sig_s) {
            return Err(AuthError::Rejected("server signature invalid".into()));
        }

        // 4. TOFU / known_hosts policy.
        if !verifier.verify(addr, &server_pub) {
            return Err(AuthError::Rejected(format!(
                "daemon host key not trusted (fingerprint {})",
                server_pub.fingerprint()
            )));
        }

        // 5. Prove we own our key.
        let sig_c = self.identity.sign(&t);
        let auth_msg = KeyAuthMsg {
            auth: "key-auth".to_string(),
            sig: b64::encode(&sig_c),
        };
        write_line(writer, &serde_json::to_string(&auth_msg).unwrap()).await?;

        // 6. Read result.
        line.clear();
        reader.read_line(&mut line).await?;
        if let Ok(ok) = serde_json::from_str::<AuthOk>(line.trim()) {
            if ok.auth == "ok" {
                return Ok(());
            }
        }
        if let Ok(fail) = serde_json::from_str::<AuthFail>(line.trim()) {
            return Err(AuthError::Rejected(fail.reason));
        }
        Err(AuthError::Protocol("unexpected key auth result".into()))
    }

    fn name(&self) -> &str {
        "key"
    }
}

async fn write_line<W>(writer: &mut W, s: &str) -> Result<(), AuthError>
where
    W: AsyncWrite + Unpin + Send,
{
    writer.write_all(s.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

async fn send_ok<W>(writer: &mut W) -> Result<(), AuthError>
where
    W: AsyncWrite + Unpin + Send,
{
    write_line(
        writer,
        &serde_json::to_string(&AuthOk { auth: "ok".into() }).unwrap(),
    )
    .await
}

async fn send_fail<W>(writer: &mut W, reason: &str) -> Result<(), AuthError>
where
    W: AsyncWrite + Unpin + Send,
{
    let fail = AuthFail {
        auth: "fail".to_string(),
        reason: reason.to_string(),
    };
    write_line(writer, &serde_json::to_string(&fail).unwrap()).await
}

/// base64 helpers for the key handshake wire form.
mod b64 {
    use base64::prelude::{Engine as _, BASE64_STANDARD};
    pub fn encode(bytes: &[u8]) -> String {
        BASE64_STANDARD.encode(bytes)
    }
    pub fn decode(s: &str) -> Option<Vec<u8>> {
        BASE64_STANDARD.decode(s).ok()
    }
}

#[derive(Serialize, Deserialize)]
struct KeyHello {
    auth: String,
    v: u32,
    client_pub: String,
    client_nonce: String,
}

#[derive(Serialize, Deserialize)]
struct KeyOffer {
    auth: String,
    server_pub: String,
    server_nonce: String,
    sig: String,
}

#[derive(Serialize, Deserialize)]
struct KeyAuthMsg {
    auth: String,
    sig: String,
}

// --- Wire format ---

#[derive(Serialize, Deserialize)]
struct AuthHello {
    auth: String,
    version: u32,
    nonce: String,
    /// Optional key name so a multi-key server can select the matching secret.
    /// Absent for unnamed keys; older servers ignore it (backward compatible).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_id: Option<String>,
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

    /// Decode a hex string to bytes. Returns `None` on odd length or non-hex.
    pub fn decode(s: &str) -> Option<Vec<u8>> {
        if !s.len().is_multiple_of(2) {
            return None;
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
            .collect()
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

    /// Run a full handshake between a server provider and a client provider,
    /// returning both results.
    async fn run_handshake(
        server_auth: PskAuth,
        client_auth: PskAuth,
    ) -> (Result<AuthResult, AuthError>, Result<(), AuthError>) {
        let (client_stream, server_stream) = duplex(4096);
        let (cr, cw) = tokio::io::split(client_stream);
        let (sr, sw) = tokio::io::split(server_stream);
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
        let (s, c) = tokio::join!(server_handle, client_handle);
        (s.unwrap(), c.unwrap())
    }

    #[tokio::test]
    async fn multikey_server_selects_named_key() {
        // Server trusts two named peers; client offers "phone".
        let server = PskAuth::from_keys([
            (Some("laptop".to_string()), "key-laptop"),
            (Some("phone".to_string()), "key-phone"),
        ]);
        let client = PskAuth::new("key-phone").offering(Some("phone".to_string()));
        let (s, c) = run_handshake(server, client).await;
        assert!(s.is_ok(), "server: {:?}", s.err());
        assert!(c.is_ok(), "client: {:?}", c.err());
        assert_eq!(s.unwrap().client_label, "phone");
    }

    #[tokio::test]
    async fn multikey_unknown_key_id_rejected() {
        let server = PskAuth::from_keys([(Some("laptop".to_string()), "key-laptop")]);
        let client = PskAuth::new("whatever").offering(Some("nonexistent".to_string()));
        let (s, c) = run_handshake(server, client).await;
        assert!(s.is_err(), "server must reject unknown key id");
        assert!(c.is_err(), "client must see rejection");
    }

    #[tokio::test]
    async fn multikey_named_key_wrong_secret_rejected() {
        // Right name, wrong secret → invalid proof.
        let server = PskAuth::from_keys([(Some("laptop".to_string()), "real-secret")]);
        let client = PskAuth::new("wrong-secret").offering(Some("laptop".to_string()));
        let (s, c) = run_handshake(server, client).await;
        let server_ok = s.is_ok();
        let client_ok = c.is_ok();
        assert!(!server_ok || !client_ok, "wrong secret must fail");
    }

    #[tokio::test]
    async fn unnamed_client_uses_default_key_backward_compat() {
        // Old-style client (no key_id) against a single-key server still works.
        let server = PskAuth::from_keys([(None, "shared")]);
        let client = PskAuth::new("shared"); // no offering()
        let (s, c) = run_handshake(server, client).await;
        assert!(s.is_ok(), "server: {:?}", s.err());
        assert!(c.is_ok(), "client: {:?}", c.err());
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(hex::decode("00ff10").unwrap(), vec![0x00, 0xff, 0x10]);
        assert!(hex::decode("xyz").is_none());
        assert!(hex::decode("abc").is_none(), "odd length rejected");
        let bytes = [1u8, 2, 250, 3];
        assert_eq!(hex::decode(&hex::encode(bytes)).unwrap(), bytes);
    }

    // --- KeyAuth (asymmetric) ---

    use crate::identity::{AuthorizedKeys, HostKeyVerifier, Identity, PublicKey};

    /// Test verifier with a fixed accept/reject decision and optional expected key.
    #[derive(Debug)]
    struct StubVerifier {
        accept: bool,
    }
    impl HostKeyVerifier for StubVerifier {
        fn verify(&self, _addr: &str, _server_pub: &PublicKey) -> bool {
            self.accept
        }
    }

    fn empty_authorized() -> AuthorizedKeys {
        // A path that does not exist → empty trust store (no I/O on load).
        AuthorizedKeys::load(std::path::Path::new(
            "/nonexistent/mae-test/authorized_keys",
        ))
    }

    async fn run_key_handshake(
        server: KeyAuth,
        client: KeyAuth,
    ) -> (Result<AuthResult, AuthError>, Result<(), AuthError>) {
        let (client_stream, server_stream) = duplex(8192);
        let (cr, cw) = tokio::io::split(client_stream);
        let (sr, sw) = tokio::io::split(server_stream);
        let sh = tokio::spawn(async move {
            let mut sr = BufReader::new(sr);
            let mut sw = tokio::io::BufWriter::new(sw);
            server.server_handshake(&mut sr, &mut sw).await
        });
        let ch = tokio::spawn(async move {
            let mut cr = BufReader::new(cr);
            let mut cw = tokio::io::BufWriter::new(cw);
            client.client_handshake(&mut cr, &mut cw).await
        });
        let (s, c) = tokio::join!(sh, ch);
        (s.unwrap(), c.unwrap())
    }

    #[tokio::test]
    async fn keyauth_authorized_client_succeeds() {
        let server_id = Arc::new(Identity::generate("daemon"));
        let client_id = Arc::new(Identity::generate("laptop"));

        // Server authorizes the client's public key (add() persists to disk).
        let dir = std::env::temp_dir().join(format!("mae-ka-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut authorized = AuthorizedKeys::load(&dir.join("authorized_keys"));
        authorized.add(client_id.public()).unwrap();
        let authorized = Arc::new(authorized);

        let server = KeyAuth::server(server_id.clone(), authorized);
        let client = KeyAuth::client(
            client_id.clone(),
            "daemon:9473".to_string(),
            Arc::new(StubVerifier { accept: true }),
        );

        let (s, c) = run_key_handshake(server, client).await;
        assert!(s.is_ok(), "server: {:?}", s.err());
        assert!(c.is_ok(), "client: {:?}", c.err());
        assert_eq!(s.unwrap().client_label, "laptop");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn keyauth_unauthorized_client_rejected() {
        let server_id = Arc::new(Identity::generate("daemon"));
        let client_id = Arc::new(Identity::generate("intruder"));
        let server = KeyAuth::server(server_id, Arc::new(empty_authorized()));
        let client = KeyAuth::client(
            client_id,
            "daemon:9473".to_string(),
            Arc::new(StubVerifier { accept: true }),
        );
        let (s, c) = run_key_handshake(server, client).await;
        assert!(s.is_err(), "unauthorized client must be rejected by server");
        assert!(c.is_err(), "client must see the rejection");
    }

    #[tokio::test]
    async fn keyauth_client_rejects_untrusted_host() {
        // Client's verifier rejects the host key → client aborts before authorizing.
        let server_id = Arc::new(Identity::generate("daemon"));
        let client_id = Arc::new(Identity::generate("laptop"));
        let dir = std::env::temp_dir().join(format!("mae-ka-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut authorized = AuthorizedKeys::load(&dir.join("authorized_keys"));
        authorized.add(client_id.public()).unwrap();
        let server = KeyAuth::server(server_id, Arc::new(authorized));
        let client = KeyAuth::client(
            client_id,
            "daemon:9473".to_string(),
            Arc::new(StubVerifier { accept: false }),
        );
        let (_s, c) = run_key_handshake(server, client).await;
        assert!(c.is_err(), "client must abort on untrusted host key");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
