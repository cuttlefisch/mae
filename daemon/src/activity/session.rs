//! The daemon's session ids (ADR-120 D4).
//!
//! | Transport | The session is | Its id |
//! |---|---|---|
//! | HTTPS with a bearer token | the token | `HMAC-SHA256(session_key, SHA-256(token))`, 128 bits |
//! | `/ws`, mTLS, mesh, KB socket | the connection | 128 random bits |
//! | `cli` | the command's run | 128 random bits |
//!
//! Every id is the 128 bits in RFC 4648 base32, lowercase, unpadded: 26
//! characters of `[a-z2-7]`, sent as `Mae-Activity-Session`.
//!
//! @ai-caution: [security] An id is an identifier, never a credential. No
//! route, method or socket accepts one as authentication or resumes anything
//! by it (`no_entry_point_reads_the_session_header`), and it cannot be turned
//! back into the token.

use std::path::Path;

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

/// The response header that carries the id (D4).
pub const HEADER: &str = "mae-activity-session";
/// The id's length: 128 bits in unpadded base32.
pub const ID_LEN: usize = 26;

/// The HMAC key session ids are derived with.
#[derive(Clone)]
pub struct SessionKey([u8; 32]);

impl std::fmt::Debug for SessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionKey(..)")
    }
}

impl SessionKey {
    /// A fresh key, for tests and a first start.
    pub fn generate() -> Self {
        let mut k = [0u8; 32];
        rand::Rng::fill_bytes(&mut rand::rng(), &mut k);
        SessionKey(k)
    }

    /// Load the key at `path`, creating it (mode `0600`) if absent. A file of
    /// any other length is refused rather than replaced: replacing it would
    /// give every live token a second session id.
    pub fn load_or_create(path: &Path) -> Result<Self, String> {
        match std::fs::read(path) {
            Ok(bytes) => <[u8; 32]>::try_from(bytes.as_slice())
                .map(SessionKey)
                .map_err(|_| format!("{}: not a 32-byte key", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = SessionKey::generate();
                write_private(path, &key.0)?;
                Ok(key)
            }
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// A bearer session's id: `HMAC-SHA256(key, SHA-256(token))`, truncated.
    pub fn bearer_id(&self, token: &str) -> String {
        let digest = Sha256::digest(token.as_bytes());
        encode(&self.mac(&[&digest[..]])[..16])
    }

    /// The `sign_ins` key of `(iss, sub, auth_time)` (D3). Keyed, so the table
    /// holds no guessable trace of who signed in when.
    pub fn sign_in_key(&self, iss: &str, sub: &str, auth_time: Option<u64>) -> String {
        let tuple = serde_json::json!([iss, sub, auth_time]);
        let canonical = crate::review::chain::canonical(&tuple).unwrap_or_default();
        hex::encode(self.mac(&[b"maeactivity-sign-in/v1\n", &canonical]))
    }

    fn mac(&self, parts: &[&[u8]]) -> [u8; 32] {
        let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.0)
            .expect("HMAC accepts a key of any length");
        for p in parts {
            m.update(p);
        }
        m.finalize().into_bytes().into()
    }
}

/// A connection's or a command's id: 128 random bits.
pub fn random_id() -> String {
    let mut b = [0u8; 16];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut b);
    encode(&b)
}

fn encode(bits128: &[u8]) -> String {
    data_encoding::BASE32_NOPAD
        .encode(bits128)
        .to_ascii_lowercase()
}

/// Whether `s` has the id's exact shape, `^[a-z2-7]{26}$`.
pub fn is_session_id(s: &str) -> bool {
    s.len() == ID_LEN && s.bytes().all(|c| matches!(c, b'a'..=b'z' | b'2'..=b'7'))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(bytes)
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))
}
