//! Asymmetric Ed25519 identities and SSH-style trust stores for the collab
//! `key` auth mode (ADR-017).
//!
//! Layout under `$XDG_DATA_HOME/mae/collab/` (mirrors `~/.ssh/`):
//!
//! - `id_ed25519`      — this peer's private key (0600), hex-encoded 32 bytes.
//! - `id_ed25519.pub`  — this peer's public key line: `mae-ed25519 <b64> <label>`.
//! - `known_hosts`     — pinned daemon public keys: `<addr> mae-ed25519 <b64>`.
//! - `authorized_keys` — trusted client public keys: `mae-ed25519 <b64> <label>`.
//!
//! Public keys and signatures travel as base64 in the handshake JSON.
//! Fingerprints are `SHA256:<base64(sha256(pubkey))>`, like OpenSSH.

use std::path::{Path, PathBuf};

use base64::prelude::{Engine as _, BASE64_STANDARD};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

/// Algorithm tag written in key lines and the handshake.
pub const ALGO: &str = "mae-ed25519";

/// Domain-separation prefix bound into every signed transcript.
const TRANSCRIPT_DOMAIN: &[u8] = b"mae-collab-key-auth-v1";

/// The default collab directory: `$XDG_DATA_HOME/mae/collab`
/// (fallback `~/.local/share/mae/collab`).
pub fn default_collab_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("mae/collab"))
}

// --- PublicKey ---

/// A peer's public key plus an optional human label.
#[derive(Clone, Debug)]
pub struct PublicKey {
    key: VerifyingKey,
    pub label: Option<String>,
}

/// An authenticated peer's identity, recovered from a verified key/cert.
/// Authoritative for attribution + membership (ADR-017 strict binding).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerIdentity {
    /// Human label (from authorized_keys / the cert CN), else the fingerprint.
    pub label: String,
    /// SHA256 fingerprint of the public key.
    pub fingerprint: String,
    /// Raw 32-byte Ed25519 public key.
    pub pubkey: [u8; 32],
}

impl PeerIdentity {
    /// A synthetic identity for non-key auth modes (psk/none), so handlers have
    /// a uniform type. `label` comes from the auth result; no real key.
    pub fn synthetic(label: &str) -> Self {
        Self {
            label: label.to_string(),
            fingerprint: String::new(),
            pubkey: [0u8; 32],
        }
    }

    /// True for a real (key-authenticated) identity, false for synthetic.
    pub fn is_authenticated(&self) -> bool {
        self.pubkey != [0u8; 32]
    }
}

impl PublicKey {
    /// The raw 32 public-key bytes.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.key.to_bytes()
    }

    /// Build from raw 32 bytes (rejects non-canonical / invalid points).
    pub fn from_bytes(bytes: &[u8; 32], label: Option<String>) -> Option<Self> {
        VerifyingKey::from_bytes(bytes)
            .ok()
            .map(|key| Self { key, label })
    }

    /// Base64 of the 32 public-key bytes (handshake wire form).
    pub fn encoded(&self) -> String {
        BASE64_STANDARD.encode(self.key.to_bytes())
    }

    /// Parse from the base64 wire form.
    pub fn from_encoded(b64: &str, label: Option<String>) -> Option<Self> {
        let bytes = BASE64_STANDARD.decode(b64).ok()?;
        let arr: [u8; 32] = bytes.try_into().ok()?;
        Self::from_bytes(&arr, label)
    }

    /// A storage line: `mae-ed25519 <b64> [label]`.
    pub fn to_line(&self) -> String {
        match &self.label {
            Some(l) => format!("{ALGO} {} {l}", self.encoded()),
            None => format!("{ALGO} {}", self.encoded()),
        }
    }

    /// Parse a storage line `mae-ed25519 <b64> [label]`. Returns `None` on a
    /// bad algo tag or malformed key.
    pub fn from_line(line: &str) -> Option<Self> {
        let mut toks = line.split_whitespace();
        if toks.next()? != ALGO {
            return None;
        }
        let b64 = toks.next()?;
        let label = toks.next().map(|s| s.to_string());
        Self::from_encoded(b64, label)
    }

    /// OpenSSH-style fingerprint: `SHA256:<base64(sha256(pubkey))>` (no padding).
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(self.key.to_bytes());
        let b64 = base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest);
        format!("SHA256:{b64}")
    }

    /// Verify a 64-byte signature over `msg`.
    pub fn verify(&self, msg: &[u8], sig_bytes: &[u8]) -> bool {
        let arr: [u8; 64] = match sig_bytes.try_into() {
            Ok(a) => a,
            Err(_) => return false,
        };
        let sig = Signature::from_bytes(&arr);
        self.key.verify(msg, &sig).is_ok()
    }
}

// --- Identity (private key) ---

/// This peer's long-lived signing identity.
pub struct Identity {
    signing: SigningKey,
    label: String,
}

impl Identity {
    /// Generate a fresh random identity.
    pub fn generate(label: &str) -> Self {
        use rand::RngCore;
        let mut secret = [0u8; 32];
        rand::rng().fill_bytes(&mut secret);
        let signing = SigningKey::from_bytes(&secret);
        Self {
            signing,
            label: label.to_string(),
        }
    }

    /// This identity's public key (with the same label).
    pub fn public(&self) -> PublicKey {
        PublicKey {
            key: self.signing.verifying_key(),
            label: Some(self.label.clone()),
        }
    }

    /// The label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Fingerprint of the public key.
    pub fn fingerprint(&self) -> String {
        self.public().fingerprint()
    }

    /// Sign `msg`, returning the 64 signature bytes.
    pub fn sign(&self, msg: &[u8]) -> Vec<u8> {
        self.signing.sign(msg).to_bytes().to_vec()
    }

    /// PKCS#8 DER encoding of the private key (for building a TLS keypair).
    pub fn pkcs8_der(&self) -> Result<Vec<u8>, String> {
        use ed25519_dalek::pkcs8::EncodePrivateKey;
        self.signing
            .to_pkcs8_der()
            .map(|d| d.as_bytes().to_vec())
            .map_err(|e| format!("pkcs8 encode failed: {e}"))
    }

    /// Load the identity from `dir/id_ed25519`, generating + persisting one
    /// (0600 private key, public-key line) if absent.
    pub fn load_or_generate(dir: &Path, label: &str) -> std::io::Result<Self> {
        let priv_path = dir.join("id_ed25519");
        if let Ok(content) = std::fs::read_to_string(&priv_path) {
            if let Some(id) = Self::parse_private(content.trim(), label) {
                return Ok(id);
            }
        }
        // Generate + persist.
        std::fs::create_dir_all(dir)?;
        secure_dir(dir);
        let id = Self::generate(label);
        let hex: String = id
            .signing
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        crate::keystore::write_secure(&priv_path, &format!("{hex}\n"))?;
        std::fs::write(
            dir.join("id_ed25519.pub"),
            format!("{}\n", id.public().to_line()),
        )?;
        Ok(id)
    }

    fn parse_private(hex: &str, label: &str) -> Option<Self> {
        let hex = hex.split_whitespace().next()?;
        if hex.len() != 64 {
            return None;
        }
        let mut secret = [0u8; 32];
        for (i, b) in secret.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(Self {
            signing: SigningKey::from_bytes(&secret),
            label: label.to_string(),
        })
    }
}

// --- known_hosts (client pins daemon keys) ---

/// A pinned host entry: `<addr> mae-ed25519 <b64>`.
pub struct KnownHosts {
    path: PathBuf,
    entries: Vec<(String, PublicKey)>,
}

impl KnownHosts {
    /// Load (or start empty if the file is absent).
    pub fn load(path: &Path) -> Self {
        let mut entries = Vec::new();
        if let Ok(content) = std::fs::read_to_string(path) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((addr, rest)) = line.split_once(char::is_whitespace) {
                    if let Some(pk) = PublicKey::from_line(rest.trim()) {
                        entries.push((addr.to_string(), pk));
                    }
                }
            }
        }
        Self {
            path: path.to_path_buf(),
            entries,
        }
    }

    /// The pinned key for `addr`, if any.
    pub fn get(&self, addr: &str) -> Option<&PublicKey> {
        self.entries.iter().find(|(a, _)| a == addr).map(|(_, k)| k)
    }

    /// Pin `pubkey` for `addr` (in memory + appended to the file, 0600).
    pub fn pin(&mut self, addr: &str, pubkey: &PublicKey) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
            secure_dir(parent);
        }
        // Rewrite the whole file so first-creation gets 0600.
        self.entries.push((addr.to_string(), pubkey.clone()));
        crate::keystore::write_secure(&self.path, &self.render())?;
        Ok(())
    }

    fn render(&self) -> String {
        let mut out = String::from("# MAE collab known_hosts — <addr> mae-ed25519 <b64>\n");
        for (addr, pk) in &self.entries {
            out.push_str(&format!("{addr} {ALGO} {}\n", pk.encoded()));
        }
        out
    }
}

// --- authorized_keys (daemon trusts client keys) ---

/// Trusted client public keys (daemon side).
#[derive(Debug)]
pub struct AuthorizedKeys {
    path: PathBuf,
    entries: Vec<PublicKey>,
}

impl AuthorizedKeys {
    /// Load (or start empty if absent).
    pub fn load(path: &Path) -> Self {
        let mut entries = Vec::new();
        if let Ok(content) = std::fs::read_to_string(path) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some(pk) = PublicKey::from_line(line) {
                    entries.push(pk);
                }
            }
        }
        Self {
            path: path.to_path_buf(),
            entries,
        }
    }

    /// Number of trusted keys.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when no keys are trusted.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// If `pubkey_bytes` is trusted, return its label (or `""`).
    pub fn authorize(&self, pubkey_bytes: &[u8; 32]) -> Option<String> {
        self.entries
            .iter()
            .find(|k| &k.to_bytes() == pubkey_bytes)
            .map(|k| k.label.clone().unwrap_or_default())
    }

    /// All entries (for listing).
    pub fn entries(&self) -> &[PublicKey] {
        &self.entries
    }

    /// Add a trusted key (rejecting an exact-bytes duplicate). Persists 0600.
    pub fn add(&mut self, pubkey: PublicKey) -> std::io::Result<()> {
        if self
            .entries
            .iter()
            .any(|k| k.to_bytes() == pubkey.to_bytes())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "public key already authorized",
            ));
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
            secure_dir(parent);
        }
        self.entries.push(pubkey);
        crate::keystore::write_secure(&self.path, &self.render())?;
        Ok(())
    }

    /// Remove the key(s) with `label`. Returns how many were removed. Persists.
    pub fn revoke(&mut self, label: &str) -> std::io::Result<usize> {
        let before = self.entries.len();
        self.entries.retain(|k| k.label.as_deref() != Some(label));
        let removed = before - self.entries.len();
        if removed > 0 {
            crate::keystore::write_secure(&self.path, &self.render())?;
        }
        Ok(removed)
    }

    fn render(&self) -> String {
        let mut out = String::from("# MAE collab authorized_keys — mae-ed25519 <b64> <label>\n");
        for pk in &self.entries {
            out.push_str(&pk.to_line());
            out.push('\n');
        }
        out
    }
}

// --- Host-key verification policy (client side) ---

/// Trust-on-first-use policy for an unknown daemon host key (ADR-017).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyPolicy {
    /// Auto-pin unknown hosts; still abort on a *changed* key. Headless default.
    AcceptNew,
    /// Never auto-pin; unknown or changed host key aborts.
    Strict,
    /// Defer to an interactive prompt (the file verifier rejects; the editor
    /// supplies a prompting verifier).
    Prompt,
}

impl HostKeyPolicy {
    /// Parse from the `collab_host_key_policy` option string.
    pub fn from_str_opt(s: &str) -> HostKeyPolicy {
        match s {
            "strict" => HostKeyPolicy::Strict,
            "accept-new" => HostKeyPolicy::AcceptNew,
            _ => HostKeyPolicy::Prompt,
        }
    }
}

/// Decides whether to trust a daemon's presented public key. Implementations
/// handle `known_hosts` pinning, policy, and (in the editor) prompting.
/// `Debug` is required because this is held inside a rustls cert verifier.
pub trait HostKeyVerifier: Send + Sync + std::fmt::Debug {
    /// Return `true` to proceed with `server_pub` from `addr`, `false` to abort.
    fn verify(&self, addr: &str, server_pub: &PublicKey) -> bool;
}

/// Default `known_hosts`-backed verifier: pins on first use per `policy`, and
/// always aborts when a previously pinned key changes (MITM defense). Cannot
/// prompt — under `Prompt` it rejects unknown hosts (the editor overrides).
#[derive(Debug)]
pub struct FileHostKeyVerifier {
    pub path: PathBuf,
    pub policy: HostKeyPolicy,
}

impl FileHostKeyVerifier {
    pub fn new(path: PathBuf, policy: HostKeyPolicy) -> Self {
        Self { path, policy }
    }
}

impl HostKeyVerifier for FileHostKeyVerifier {
    fn verify(&self, addr: &str, server_pub: &PublicKey) -> bool {
        let mut kh = KnownHosts::load(&self.path);
        match kh.get(addr) {
            Some(pinned) => {
                if pinned.to_bytes() == server_pub.to_bytes() {
                    true
                } else {
                    tracing::error!(
                        addr,
                        expected = %pinned.fingerprint(),
                        got = %server_pub.fingerprint(),
                        "daemon host key CHANGED — aborting (possible MITM)"
                    );
                    false
                }
            }
            None => match self.policy {
                HostKeyPolicy::AcceptNew => {
                    if let Err(e) = kh.pin(addr, server_pub) {
                        tracing::warn!(addr, error = %e, "failed to pin host key");
                        return false;
                    }
                    tracing::info!(addr, fp = %server_pub.fingerprint(), "pinned new daemon host key (accept-new)");
                    true
                }
                HostKeyPolicy::Strict | HostKeyPolicy::Prompt => {
                    tracing::warn!(addr, fp = %server_pub.fingerprint(), policy = ?self.policy, "unknown host key rejected");
                    false
                }
            },
        }
    }
}

/// Build the signed transcript binding both identities + nonces to the session.
/// Both sides MUST compute this identically.
pub fn transcript(
    client_pub: &[u8; 32],
    server_pub: &[u8; 32],
    client_nonce: &[u8],
    server_nonce: &[u8],
) -> Vec<u8> {
    let mut t =
        Vec::with_capacity(TRANSCRIPT_DOMAIN.len() + 64 + client_nonce.len() + server_nonce.len());
    t.extend_from_slice(TRANSCRIPT_DOMAIN);
    t.extend_from_slice(client_pub);
    t.extend_from_slice(server_pub);
    t.extend_from_slice(client_nonce);
    t.extend_from_slice(server_nonce);
    t
}

#[cfg(unix)]
fn secure_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}
#[cfg(not(unix))]
fn secure_dir(_dir: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_roundtrip() {
        let id = Identity::generate("alice");
        let pk = id.public();
        let msg = b"hello world";
        let sig = id.sign(msg);
        assert!(pk.verify(msg, &sig));
        assert!(!pk.verify(b"tampered", &sig));
        // A different identity's key must not verify.
        assert!(!Identity::generate("bob").public().verify(msg, &sig));
    }

    #[test]
    fn pubkey_line_roundtrip() {
        let pk = Identity::generate("framework").public();
        let line = pk.to_line();
        assert!(line.starts_with("mae-ed25519 "));
        let parsed = PublicKey::from_line(&line).unwrap();
        assert_eq!(parsed.to_bytes(), pk.to_bytes());
        assert_eq!(parsed.label.as_deref(), Some("framework"));
        assert!(
            PublicKey::from_line("ssh-rsa AAAA foo").is_none(),
            "wrong algo"
        );
    }

    #[test]
    fn fingerprint_is_stable_and_sshlike() {
        let pk = Identity::generate("x").public();
        let fp = pk.fingerprint();
        assert!(fp.starts_with("SHA256:"));
        assert_eq!(fp, pk.fingerprint(), "deterministic");
        assert_ne!(fp, Identity::generate("y").public().fingerprint());
    }

    #[test]
    fn identity_persists_and_reloads() {
        let dir = std::env::temp_dir().join(format!("mae-id-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = Identity::load_or_generate(&dir, "framework").unwrap();
        let b = Identity::load_or_generate(&dir, "framework").unwrap();
        assert_eq!(
            a.public().to_bytes(),
            b.public().to_bytes(),
            "stable across loads"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("id_ed25519"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "private key must be 0600");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn known_hosts_pin_and_lookup() {
        let dir = std::env::temp_dir().join(format!("mae-kh-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("known_hosts");
        let mut kh = KnownHosts::load(&path);
        assert!(kh.get("10.0.0.5:9473").is_none());
        let server = Identity::generate("daemon").public();
        kh.pin("10.0.0.5:9473", &server).unwrap();
        // Reload from disk.
        let kh2 = KnownHosts::load(&path);
        assert_eq!(
            kh2.get("10.0.0.5:9473").unwrap().to_bytes(),
            server.to_bytes()
        );
        assert!(kh2.get("other:9473").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn authorized_keys_add_authorize_revoke() {
        let dir = std::env::temp_dir().join(format!("mae-ak-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("authorized_keys");
        let mut ak = AuthorizedKeys::load(&path);
        let client = Identity::generate("laptop").public();
        ak.add(client.clone()).unwrap();
        assert_eq!(ak.len(), 1);
        assert_eq!(ak.authorize(&client.to_bytes()).as_deref(), Some("laptop"));
        // Unknown key not authorized.
        assert!(ak
            .authorize(&Identity::generate("z").public().to_bytes())
            .is_none());
        // Duplicate rejected.
        assert!(ak.add(client.clone()).is_err());
        // Reload + revoke.
        let mut ak2 = AuthorizedKeys::load(&path);
        assert_eq!(ak2.revoke("laptop").unwrap(), 1);
        assert!(ak2.authorize(&client.to_bytes()).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_verifier_accept_new_pins_then_detects_change() {
        let dir = std::env::temp_dir().join(format!("mae-fv-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("known_hosts");
        let v = FileHostKeyVerifier::new(path.clone(), HostKeyPolicy::AcceptNew);
        let server = Identity::generate("daemon").public();
        // First sight → accepted + pinned.
        assert!(v.verify("10.0.0.5:9473", &server));
        // Same key again → accepted (pinned match).
        assert!(v.verify("10.0.0.5:9473", &server));
        // Different key for same addr → rejected (possible MITM).
        let imposter = Identity::generate("evil").public();
        assert!(!v.verify("10.0.0.5:9473", &imposter));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_verifier_strict_rejects_unknown() {
        let dir = std::env::temp_dir().join(format!("mae-fvs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let v = FileHostKeyVerifier::new(dir.join("known_hosts"), HostKeyPolicy::Strict);
        assert!(!v.verify("h:9473", &Identity::generate("d").public()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_key_policy_parsing() {
        assert_eq!(HostKeyPolicy::from_str_opt("strict"), HostKeyPolicy::Strict);
        assert_eq!(
            HostKeyPolicy::from_str_opt("accept-new"),
            HostKeyPolicy::AcceptNew
        );
        assert_eq!(HostKeyPolicy::from_str_opt("prompt"), HostKeyPolicy::Prompt);
        assert_eq!(
            HostKeyPolicy::from_str_opt("garbage"),
            HostKeyPolicy::Prompt
        );
    }

    #[test]
    fn transcript_binds_inputs() {
        let cp = [1u8; 32];
        let sp = [2u8; 32];
        let t1 = transcript(&cp, &sp, b"na", b"nb");
        assert_eq!(t1, transcript(&cp, &sp, b"na", b"nb"));
        assert_ne!(
            t1,
            transcript(&sp, &cp, b"na", b"nb"),
            "pubkey order matters"
        );
        assert_ne!(
            t1,
            transcript(&cp, &sp, b"nb", b"na"),
            "nonce order matters"
        );
    }
}
