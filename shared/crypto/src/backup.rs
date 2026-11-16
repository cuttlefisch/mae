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
//! {"v":1,"fingerprint":"FP","kdf":"argon2id","m":M,"t":T,"p":P,"salt":"SALT","nonce":"NONCE","ct":"CT"}
//! ```
//!
//! - `FP`: the sealed key's fingerprint, `SHA256:` + 43 characters of
//!   unpadded standard base64 of SHA-256 over its 32-byte public key (as
//!   `mae_mcp::identity` writes it). Readable without the passphrase, so a
//!   person can tell which key a file holds; authenticated, and checked
//!   against the opened seed.
//! - `M`, `T`, `P`: Argon2id (RFC 9106, version 0x13) memory in KiB, passes
//!   and lanes, as decimal integers. Accepted ranges: `M` 19456..=262144,
//!   `T` 2..=10, `P` 1..=4 (the floor is OWASP's Argon2id minimum; the
//!   ceiling bounds what a hostile envelope can make an opener allocate).
//!   Sealed with `M`=65536, `T`=3, `P`=4 (RFC 9106 §4, second recommendation).
//! - `SALT`: 16 random bytes; `NONCE`: 24 random bytes; `CT`: 48 bytes. All
//!   standard base64 with padding (RFC 4648 §4), canonical.
//! - The passphrase is normalised to Unicode NFC here, on sealing and on
//!   opening (so the same typed passphrase opens it whatever form an input
//!   method produced); it must then be 1..=1024 UTF-8 bytes, and to SEAL, at
//!   least [`MIN_PASSPHRASE_CHARS`] characters.
//! - key = Argon2id(password = the NFC passphrase's UTF-8 bytes, salt = SALT,
//!   M, T, P, output 32 bytes, no secret, no associated data).
//! - The HEADER is the envelope text without `,"ct":"CT"`:
//!   `{"v":1,"fingerprint":"FP","kdf":"argon2id","m":M,"t":T,"p":P,"salt":"SALT","nonce":"NONCE"}`.
//! - CT = XChaCha20-Poly1305(key, NONCE, plaintext = the 32-byte seed,
//!   associated data = the HEADER's bytes): 32 bytes of ciphertext then the
//!   16-byte tag.
//!
//! Opening refuses, in this order: over [`MAX_ENVELOPE_BYTES`]; not a JSON
//! object with a numeric `v`; `v` other than 1 (an unknown version is never
//! guessed at); any other key set or a non-canonical text; `kdf` other than
//! `argon2id`; parameters out of range; a field of the wrong form or length;
//! a passphrase out of bounds; the AEAD tag, which fails for a wrong
//! passphrase, any changed byte of salt, nonce or ciphertext, and any
//! changed header value, the fingerprint included (a wrong passphrase and a
//! tampered envelope are indistinguishable); and last, a seed whose
//! fingerprint is not the header's (only an envelope sealed with a wrong
//! fingerprint in the first place can reach that).
//!
//! @ai-caution: [security] The envelope is the one form in which the seed
//! may leave the client at rest. Never weaken [`KdfParams::MIN`] or
//! [`MIN_PASSPHRASE_CHARS`], drop the header from the associated data (an
//! attacker could then lower `m`/`t` on a copy, swap salts, or relabel the
//! fingerprint, without detection), or accept a version this module does not
//! implement. Error messages are fixed text: never put input in them.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use serde::Deserialize;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use crate::member::MemberKey;
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
/// The longest passphrase accepted, in UTF-8 bytes after NFC.
pub const MAX_PASSPHRASE_BYTES: usize = 1024;
/// The fewest characters (Unicode scalar values, after NFC) a passphrase
/// must have to SEAL a backup; opening applies no floor, so an envelope made
/// elsewhere still opens. Twelve is OWASP ASVS 4.0.3 V2.1.1's minimum for a
/// user-chosen secret. The envelope is the offline-attackable copy of the
/// member's whole access, so length is the one lever a person controls; the
/// Argon2id cost multiplies whatever entropy those characters carry, it does
/// not replace it. (NIST SP 800-63B-4 asks 15 for a single-factor password;
/// twelve plus a memory-hard KDF is the trade made here, and the floor can
/// only rise.)
pub const MIN_PASSPHRASE_CHARS: usize = 12;

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
    /// A version other than [`VERSION`] (the number is not shown).
    UnknownVersion(u64),
    /// A key-derivation function other than [`KDF`].
    UnknownKdf,
    /// KDF parameters outside [`KdfParams::MIN`]..=[`KdfParams::MAX`].
    Params,
    /// An empty passphrase, or one over [`MAX_PASSPHRASE_BYTES`].
    Passphrase,
    /// Sealing only: fewer than [`MIN_PASSPHRASE_CHARS`] characters.
    PassphraseWeak,
    /// The AEAD refused: a wrong passphrase or a tampered envelope.
    Open,
    /// The opened seed's fingerprint is not the header's.
    FingerprintMismatch,
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
            Self::PassphraseWeak => "passphrase-weak",
            Self::Open => "open",
            Self::FingerprintMismatch => "fingerprint-mismatch",
            Self::RandomUnavailable => "no-entropy",
        }
    }
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLong => write!(f, "longer than any key backup"),
            Self::NotAnEnvelope(e) => write!(f, "not a key backup: {e}"),
            Self::UnknownVersion(_) => write!(f, "this key backup's version is not supported"),
            Self::UnknownKdf => write!(f, "the key backup uses an unknown key derivation"),
            Self::Params => write!(f, "the key backup's cost parameters are out of range"),
            Self::Passphrase => write!(
                f,
                "the passphrase must be 1 to {MAX_PASSPHRASE_BYTES} bytes"
            ),
            Self::PassphraseWeak => write!(
                f,
                "a backup passphrase needs at least {MIN_PASSPHRASE_CHARS} characters"
            ),
            Self::FingerprintMismatch => {
                write!(f, "the key in the backup is not the key its label names")
            }
            Self::Open => write!(
                f,
                "cannot open the key backup: wrong passphrase, or the backup was altered"
            ),
            Self::RandomUnavailable => write!(f, "no secure random source is available"),
        }
    }
}

/// A v1 header: everything but the ciphertext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub fingerprint: String,
    pub params: KdfParams,
    pub salt: [u8; SALT_LEN],
    pub nonce: [u8; NONCE_LEN],
}

/// The header text (the AEAD's associated data).
pub fn header_text(h: &Header) -> String {
    format!(
        "{{\"v\":{VERSION},\"fingerprint\":\"{}\",\"kdf\":\"{KDF}\",\"m\":{},\"t\":{},\"p\":{},\
         \"salt\":\"{}\",\"nonce\":\"{}\"}}",
        h.fingerprint,
        h.params.m,
        h.params.t,
        h.params.p,
        STANDARD.encode(h.salt),
        STANDARD.encode(h.nonce)
    )
}

fn envelope_text(header: &str, ct: &[u8]) -> String {
    // The header without its closing brace, then the ciphertext.
    let open = header.strip_suffix('}').unwrap_or(header);
    format!("{open},\"ct\":\"{}\"}}", STANDARD.encode(ct))
}

/// Whether `s` is a fingerprint in its one canonical form.
fn is_fingerprint(s: &str) -> bool {
    use base64::engine::general_purpose::STANDARD_NO_PAD;
    let Some(body) = s.strip_prefix("SHA256:") else {
        return false;
    };
    body.len() == 43
        && STANDARD_NO_PAD
            .decode(body)
            .is_ok_and(|d| d.len() == 32 && STANDARD_NO_PAD.encode(&d) == body)
}

/// The passphrase in NFC, 1..=[`MAX_PASSPHRASE_BYTES`] bytes. Normalised
/// lazily and stopped at the bound, so a huge input costs no more than a
/// legal one; built in a fixed buffer that is never reallocated, so no
/// unwiped copy is left behind.
fn normalize(passphrase: &str) -> Result<Zeroizing<String>, BackupError> {
    let mut nfc = Zeroizing::new(String::with_capacity(MAX_PASSPHRASE_BYTES + 4));
    for c in passphrase.nfc() {
        if nfc.len() + c.len_utf8() > MAX_PASSPHRASE_BYTES {
            return Err(BackupError::Passphrase);
        }
        nfc.push(c);
    }
    if nfc.is_empty() {
        return Err(BackupError::Passphrase);
    }
    Ok(nfc)
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
    seal_as(
        seed,
        &MemberKey::from_seed(seed).fingerprint(),
        passphrase,
        params,
    )
}

/// [`seal`], labelled with `fingerprint`: the real one, except in the test
/// that proves a mislabelled envelope is refused on opening.
pub(crate) fn seal_as(
    seed: &[u8; 32],
    fingerprint: &str,
    passphrase: &str,
    params: KdfParams,
) -> Result<String, BackupError> {
    params.check()?;
    let passphrase = normalize(passphrase)?;
    if passphrase.chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(BackupError::PassphraseWeak);
    }
    let mut h = Header {
        fingerprint: fingerprint.to_string(),
        params,
        salt: [0u8; SALT_LEN],
        nonce: [0u8; NONCE_LEN],
    };
    rng::fill(&mut h.salt).map_err(|_| BackupError::RandomUnavailable)?;
    rng::fill(&mut h.nonce).map_err(|_| BackupError::RandomUnavailable)?;
    let key = derive_key(&passphrase, &h.salt, params)?;
    let header = header_text(&h);
    let payload = Payload {
        msg: seed,
        aad: header.as_bytes(),
    };
    let ct = cipher(&key)
        .encrypt(&XNonce::from(h.nonce), payload)
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
    fingerprint: String,
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

/// A fixed reason: never serde's message or a decoder's, which quote input.
fn not_an_envelope(reason: &'static str) -> BackupError {
    BackupError::NotAnEnvelope(reason.into())
}

fn decode_exact<const N: usize>(b64: &str, reason: &'static str) -> Result<[u8; N], BackupError> {
    STANDARD
        .decode(b64)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| not_an_envelope(reason))
}

fn parse(envelope: &str) -> Result<(Header, [u8; CT_LEN]), BackupError> {
    if envelope.len() > MAX_ENVELOPE_BYTES {
        return Err(BackupError::TooLong);
    }
    let text = envelope.trim_matches(|c: char| c.is_ascii_whitespace());
    let probe: VersionProbe = serde_json::from_str(text)
        .map_err(|_| not_an_envelope("not a JSON object with a version"))?;
    if probe.v != VERSION {
        return Err(BackupError::UnknownVersion(probe.v));
    }
    let e: EnvelopeV1 = serde_json::from_str(text)
        .map_err(|_| not_an_envelope("not exactly the v1 envelope's fields"))?;
    if e.kdf != KDF {
        return Err(BackupError::UnknownKdf);
    }
    let params = KdfParams {
        m: e.m,
        t: e.t,
        p: e.p,
    };
    params.check()?;
    if !is_fingerprint(&e.fingerprint) {
        return Err(not_an_envelope(
            "the fingerprint is not SHA256: and 43 base64 characters",
        ));
    }
    let h = Header {
        fingerprint: e.fingerprint,
        params,
        salt: decode_exact(&e.salt, "the salt is not 16 bytes of canonical base64")?,
        nonce: decode_exact(&e.nonce, "the nonce is not 24 bytes of canonical base64")?,
    };
    let ct = decode_exact(&e.ct, "the ciphertext is not 48 bytes of canonical base64")?;
    if envelope_text(&header_text(&h), &ct) != text {
        return Err(not_an_envelope("not the canonical v1 text"));
    }
    Ok((h, ct))
}

/// Opens `envelope` with `passphrase`, returning the seed (wiped on drop).
pub fn open(envelope: &str, passphrase: &str) -> Result<Zeroizing<[u8; 32]>, BackupError> {
    let (h, ct) = parse(envelope)?;
    let passphrase = normalize(passphrase)?;
    let key = derive_key(&passphrase, &h.salt, h.params)?;
    let header = header_text(&h);
    let payload = Payload {
        msg: &ct,
        aad: header.as_bytes(),
    };
    let plain = Zeroizing::new(
        cipher(&key)
            .decrypt(&XNonce::from(h.nonce), payload)
            .map_err(|_| BackupError::Open)?,
    );
    let mut seed = Zeroizing::new([0u8; 32]);
    if plain.len() != seed.len() {
        return Err(BackupError::Open);
    }
    seed.copy_from_slice(&plain);
    if MemberKey::from_seed(&seed).fingerprint() != h.fingerprint {
        return Err(BackupError::FingerprintMismatch);
    }
    Ok(seed)
}

/// An envelope's header, checked, without opening it: for a caller to show
/// which key a file holds and how long opening will take, or to refuse it
/// up front.
pub fn header_of(envelope: &str) -> Result<Header, BackupError> {
    parse(envelope).map(|(h, _)| h)
}

#[cfg(test)]
#[path = "backup_tests.rs"]
mod backup_tests;
