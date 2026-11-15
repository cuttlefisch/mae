//! The member-key backup envelope, version 1: a member's 32-byte Ed25519
//! seed sealed under a passphrase. The browser writes it; a native MAE
//! (phase B) imports exactly this format, so the format is the contract and
//! is spelled out here in full.
//!
//! # Format (v1)
//!
//! The envelope is ONE line of JSON, exactly this text (keys in this order,
//! no whitespace), optionally surrounded by ASCII whitespace (a trailing
//! newline in a file is fine):
//!
//! ```text
//! {"v":1,"kdf":"argon2id","m":M,"t":T,"p":P,"salt":"SALT","nonce":"NONCE","ct":"CT"}
//! ```
//!
//! - `M`, `T`, `P`: Argon2id (RFC 9106, version 0x13) memory in KiB, passes
//!   and lanes, as decimal integers. Accepted ranges: `M` 19456..=262144,
//!   `T` 2..=10, `P` 1..=4 (the floor is OWASP's Argon2id minimum; the
//!   ceiling bounds what a hostile envelope can make an opener allocate).
//!   Sealed with `M`=65536, `T`=3, `P`=4 (RFC 9106 §4, second recommendation).
//! - `SALT`: 16 random bytes; `NONCE`: 24 random bytes; `CT`: 48 bytes. All
//!   standard base64 with padding (RFC 4648 §4), canonical.
//! - key = Argon2id(password = the passphrase's UTF-8 bytes, salt = SALT,
//!   M, T, P, output 32 bytes, no secret, no associated data).
//! - The HEADER is the envelope text up to the end of `NONCE`'s closing quote,
//!   followed by `}`: `{"v":1,"kdf":"argon2id","m":M,"t":T,"p":P,"salt":"SALT","nonce":"NONCE"}`.
//! - CT = XChaCha20-Poly1305(key, NONCE, plaintext = the 32-byte seed,
//!   associated data = the HEADER's bytes): 32 bytes of ciphertext then the
//!   16-byte tag.
//!
//! The passphrase is NFC-normalised by the caller before it gets here (the
//! JS loader does it); this module takes its bytes as given. 1..=1024 bytes.
//!
//! Opening refuses, in this order: over [`MAX_ENVELOPE_BYTES`]; not a JSON
//! object with a numeric `v`; `v` other than 1 (an unknown version is never
//! guessed at); any other key set or a non-canonical text; `kdf` other than
//! `argon2id`; parameters out of range; a field of the wrong length; and
//! finally the AEAD tag, which fails for a wrong passphrase, any changed
//! byte of salt, nonce or ciphertext, and any changed header value (a
//! wrong passphrase and a tampered envelope are indistinguishable).
//!
//! @ai-caution: [security] The envelope is the one form in which the seed
//! may leave the client at rest. Never weaken [`KdfParams::MIN`], drop the
//! header from the associated data (an attacker could then lower `m`/`t` on
//! a copy, or swap salts, without detection), or accept a version this
//! module does not implement.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::rng;

/// The envelope version this module writes and opens.
pub const VERSION: u64 = 1;
/// The one key-derivation function of version 1.
pub const KDF: &str = "argon2id";
/// Salt length in bytes.
pub const SALT_LEN: usize = 16;
/// XChaCha20-Poly1305 nonce length in bytes.
pub const NONCE_LEN: usize = 24;
/// The sealed seed: 32 bytes and a 16-byte tag.
pub const CT_LEN: usize = 32 + 16;
/// No v1 envelope comes near this; anything longer is refused unparsed.
pub const MAX_ENVELOPE_BYTES: usize = 1024;
/// The longest passphrase accepted, in UTF-8 bytes.
pub const MAX_PASSPHRASE_BYTES: usize = 1024;

/// Argon2id cost parameters: memory `m` in KiB, `t` passes, `p` lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub m: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfParams {
    /// What [`seal`] uses unless told otherwise (RFC 9106 §4, second option).
    pub const DEFAULT: Self = Self {
        m: 65536,
        t: 3,
        p: 4,
    };
    /// The weakest parameters sealed or opened (OWASP's Argon2id floor).
    pub const MIN: Self = Self {
        m: 19456,
        t: 2,
        p: 1,
    };
    /// The costliest parameters opened: bounds what an envelope from
    /// someone else can make the opener allocate and compute.
    pub const MAX: Self = Self {
        m: 262144,
        t: 10,
        p: 4,
    };

    /// Within [`Self::MIN`]..=[`Self::MAX`], each parameter on its own.
    pub fn check(&self) -> Result<(), BackupError> {
        let (lo, hi) = (Self::MIN, Self::MAX);
        if (lo.m..=hi.m).contains(&self.m)
            && (lo.t..=hi.t).contains(&self.t)
            && (lo.p..=hi.p).contains(&self.p)
        {
            Ok(())
        } else {
            Err(BackupError::Params)
        }
    }
}

/// Why a backup was not sealed or opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupError {
    /// Over [`MAX_ENVELOPE_BYTES`].
    TooLong,
    /// Not a v1 envelope's text: not JSON, a key missing or extra, a field
    /// of the wrong type or length, or not the canonical text.
    NotAnEnvelope(String),
    /// A version other than [`VERSION`].
    UnknownVersion(u64),
    /// A key-derivation function other than [`KDF`].
    UnknownKdf,
    /// KDF parameters outside [`KdfParams::MIN`]..=[`KdfParams::MAX`].
    Params,
    /// An empty passphrase, or one over [`MAX_PASSPHRASE_BYTES`].
    Passphrase,
    /// The AEAD refused: a wrong passphrase or a tampered envelope.
    Open,
    /// No secure random source (sealing only).
    RandomUnavailable,
}

impl BackupError {
    /// A stable, machine-readable name (the WASM API's error `code`).
    pub fn code(&self) -> &'static str {
        match self {
            Self::TooLong => "too-long",
            Self::NotAnEnvelope(_) => "not-an-envelope",
            Self::UnknownVersion(_) => "unknown-version",
            Self::UnknownKdf => "unknown-kdf",
            Self::Params => "kdf-params",
            Self::Passphrase => "passphrase",
            Self::Open => "open",
            Self::RandomUnavailable => "no-entropy",
        }
    }
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLong => write!(f, "longer than any key backup"),
            Self::NotAnEnvelope(e) => write!(f, "not a key backup: {e}"),
            Self::UnknownVersion(v) => write!(f, "key backup version {v} is not supported"),
            Self::UnknownKdf => write!(f, "the key backup uses an unknown key derivation"),
            Self::Params => write!(f, "the key backup's cost parameters are out of range"),
            Self::Passphrase => write!(
                f,
                "the passphrase must be 1 to {MAX_PASSPHRASE_BYTES} bytes"
            ),
            Self::Open => write!(
                f,
                "cannot open the key backup: wrong passphrase, or the backup was altered"
            ),
            Self::RandomUnavailable => write!(f, "no secure random source is available"),
        }
    }
}

/// The header text (the AEAD's associated data) for these values.
pub fn header_text(params: KdfParams, salt: &[u8], nonce: &[u8]) -> String {
    format!(
        "{{\"v\":{VERSION},\"kdf\":\"{KDF}\",\"m\":{},\"t\":{},\"p\":{},\"salt\":\"{}\",\"nonce\":\"{}\"}}",
        params.m,
        params.t,
        params.p,
        STANDARD.encode(salt),
        STANDARD.encode(nonce)
    )
}

fn envelope_text(header: &str, ct: &[u8]) -> String {
    // The header without its closing brace, then the ciphertext.
    let open = header.strip_suffix('}').unwrap_or(header);
    format!("{open},\"ct\":\"{}\"}}", STANDARD.encode(ct))
}

fn check_passphrase(passphrase: &str) -> Result<(), BackupError> {
    if passphrase.is_empty() || passphrase.len() > MAX_PASSPHRASE_BYTES {
        return Err(BackupError::Passphrase);
    }
    Ok(())
}

fn derive_key(
    passphrase: &str,
    salt: &[u8],
    params: KdfParams,
) -> Result<Zeroizing<[u8; 32]>, BackupError> {
    let argon_params = argon2::Params::new(params.m, params.t, params.p, Some(32))
        .map_err(|_| BackupError::Params)?;
    let argon = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon_params,
    );
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut key[..])
        .map_err(|_| BackupError::Params)?;
    Ok(key)
}

fn cipher(key: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new_from_slice(key)
        .expect("a 32-byte key is XChaCha20-Poly1305's key length")
}

/// Seals `seed` under `passphrase` with `params`, returning the envelope
/// text. Fresh salt and nonce from [`crate::rng`] every call.
pub fn seal(seed: &[u8; 32], passphrase: &str, params: KdfParams) -> Result<String, BackupError> {
    params.check()?;
    check_passphrase(passphrase)?;
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    rng::fill(&mut salt).map_err(|_| BackupError::RandomUnavailable)?;
    rng::fill(&mut nonce).map_err(|_| BackupError::RandomUnavailable)?;
    let key = derive_key(passphrase, &salt, params)?;
    let header = header_text(params, &salt, &nonce);
    let payload = Payload {
        msg: seed,
        aad: header.as_bytes(),
    };
    let ct = cipher(&key)
        .encrypt(&XNonce::from(nonce), payload)
        .map_err(|_| BackupError::Open)?;
    Ok(envelope_text(&header, &ct))
}

/// The envelope's fields as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvelopeV1 {
    // Checked through `VersionProbe` first; here so the key is not unknown.
    #[allow(dead_code)]
    v: u64,
    kdf: String,
    m: u32,
    t: u32,
    p: u32,
    salt: String,
    nonce: String,
    ct: String,
}

/// Only the version, read before anything else so an unknown version is
/// reported as such rather than as a malformed v1 envelope.
#[derive(Deserialize)]
struct VersionProbe {
    v: u64,
}

fn not_an_envelope(e: impl std::fmt::Display) -> BackupError {
    BackupError::NotAnEnvelope(e.to_string())
}

fn decode_exact<const N: usize>(field: &str, b64: &str) -> Result<[u8; N], BackupError> {
    let bytes = STANDARD
        .decode(b64)
        .map_err(|e| not_an_envelope(format!("{field}: {e}")))?;
    let n = bytes.len();
    bytes
        .try_into()
        .map_err(|_| not_an_envelope(format!("{field} is {n} bytes, not {N}")))
}

/// A v1 envelope's checked fields, before the passphrase is tried.
struct Parsed {
    params: KdfParams,
    salt: [u8; SALT_LEN],
    nonce: [u8; NONCE_LEN],
    ct: [u8; CT_LEN],
}

fn parse(envelope: &str) -> Result<Parsed, BackupError> {
    if envelope.len() > MAX_ENVELOPE_BYTES {
        return Err(BackupError::TooLong);
    }
    let text = envelope.trim_matches(|c: char| c.is_ascii_whitespace());
    let probe: VersionProbe = serde_json::from_str(text).map_err(not_an_envelope)?;
    if probe.v != VERSION {
        return Err(BackupError::UnknownVersion(probe.v));
    }
    let e: EnvelopeV1 = serde_json::from_str(text).map_err(not_an_envelope)?;
    if e.kdf != KDF {
        return Err(BackupError::UnknownKdf);
    }
    let params = KdfParams {
        m: e.m,
        t: e.t,
        p: e.p,
    };
    params.check()?;
    let parsed = Parsed {
        params,
        salt: decode_exact("salt", &e.salt)?,
        nonce: decode_exact("nonce", &e.nonce)?,
        ct: decode_exact("ct", &e.ct)?,
    };
    let canonical = envelope_text(
        &header_text(params, &parsed.salt, &parsed.nonce),
        &parsed.ct,
    );
    if canonical != text {
        return Err(not_an_envelope("not the canonical v1 text"));
    }
    Ok(parsed)
}

/// Opens `envelope` with `passphrase`, returning the seed (wiped on drop).
pub fn open(envelope: &str, passphrase: &str) -> Result<Zeroizing<[u8; 32]>, BackupError> {
    let p = parse(envelope)?;
    check_passphrase(passphrase)?;
    let key = derive_key(passphrase, &p.salt, p.params)?;
    let header = header_text(p.params, &p.salt, &p.nonce);
    let payload = Payload {
        msg: &p.ct,
        aad: header.as_bytes(),
    };
    let plain = Zeroizing::new(
        cipher(&key)
            .decrypt(&XNonce::from(p.nonce), payload)
            .map_err(|_| BackupError::Open)?,
    );
    let mut seed = Zeroizing::new([0u8; 32]);
    if plain.len() != seed.len() {
        return Err(BackupError::Open);
    }
    seed.copy_from_slice(&plain);
    Ok(seed)
}

/// The KDF parameters an envelope states, checked, without opening it: for
/// a caller to say how long opening will take, or to refuse up front.
pub fn params_of(envelope: &str) -> Result<KdfParams, BackupError> {
    parse(envelope).map(|p| p.params)
}

#[cfg(test)]
#[path = "backup_tests.rs"]
mod backup_tests;
