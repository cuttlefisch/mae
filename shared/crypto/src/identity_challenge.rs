//! The identity-binding challenge (ADR-098 Amendment 1, C2): its one wire
//! format and its one signing rule, shared by the daemon that issues and
//! verifies it, the editor command that signs it (`mae
//! --sign-identity-challenge`, through `mae_mcp::identity_challenge`) and the
//! browser (through mae-crypto-wasm).
//!
//! A challenge is a JSON object with exactly seven fields, written in a fixed
//! order by [`challenge_text`]: the domain tag, `iss`, `sub`, `aud`, the
//! `fingerprint` claimed, a random `nonce` (the daemon's challenge id) and
//! `expires_at` (unix seconds). The key holder signs its UTF-8 bytes.
//!
//! @ai-caution: [security] [`sign_challenge`] signs with the member key, which
//! also signs membership and content ops. It must never become a signing
//! oracle: it signs only text that is BYTE-FOR-BYTE what [`challenge_text`]
//! produces for the fields it parses (so the domain tag leads every signed
//! message, and no extra field, whitespace or duplicate key can ride along),
//! names THIS key, and is unexpired. Loosening any of those makes every
//! signer (editor and browser alike) sign attacker-chosen bytes.
//!
//! @ai-caution: [security] It also signs only for the `(iss, sub, aud)` the
//! key holder states ([`Claimant`]): their own identity-provider identity and
//! the daemon they mean to bind at. The daemon issues a challenge
//! for any fingerprint to any authenticated subject, so a challenge naming
//! this key proves nothing about WHO asked for it: without the comparison, a
//! key holder persuaded to sign someone else's challenge binds their key to
//! that person's subject. Without the audience, a challenge one daemon issued
//! could be relayed to the key holder by another daemon they also use and
//! signed as if it were for that one.
//!
//! The signer also refuses a challenge that stays valid for longer than
//! [`MAX_CHALLENGE_LIFETIME_SECS`] from now: the daemon issues them for
//! minutes, so one valid for days was not issued by a daemon as configured.

use serde::Deserialize;

/// The domain-separation tag: a signature over a challenge can never stand for
/// a signature over anything else the member key signs.
pub const DOMAIN_TAG: &str = "mae-identity-binding-v1";

/// No challenge the daemon issues comes near this; anything longer is refused
/// before it is parsed.
pub const MAX_CHALLENGE_BYTES: usize = 4096;

/// The longest a challenge may remain valid, from the signer's `now`, and
/// still be signed: one hour. The daemon's `identity_challenge_ttl_secs`
/// defaults to 300 s (its tests pin the default within 120..=900), and an
/// hour leaves room for any TTL an operator would reasonably set plus clock
/// skew between the daemon and the signer, while refusing a challenge that
/// would stay signable, and so relayable, for days.
pub const MAX_CHALLENGE_LIFETIME_SECS: u64 = 3600;

/// A parsed challenge.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Challenge {
    pub domain: String,
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub fingerprint: String,
    pub nonce: String,
    pub expires_at: u64,
}

fn quoted(s: &str) -> String {
    serde_json::Value::from(s).to_string()
}

/// The canonical challenge text. Written field by field rather than through a
/// `serde_json::Map`, whose key order depends on a cargo feature the editor and
/// daemon workspaces may resolve differently.
pub fn challenge_text(
    iss: &str,
    sub: &str,
    aud: &str,
    fingerprint: &str,
    nonce: &str,
    expires_at: u64,
) -> String {
    format!(
        "{{\"domain\":{},\"iss\":{},\"sub\":{},\"aud\":{},\"fingerprint\":{},\"nonce\":{},\
         \"expires_at\":{}}}",
        quoted(DOMAIN_TAG),
        quoted(iss),
        quoted(sub),
        quoted(aud),
        quoted(fingerprint),
        quoted(nonce),
        expires_at
    )
}

/// What the key holder states: their own identity-provider identity (a
/// subject is unique only within its issuer, OIDC Core §2, so both) and the
/// daemon they mean to bind at (its canonical resource URI, the challenge's
/// `aud`). All three must match the challenge byte for byte.
#[derive(Debug, Clone, Copy)]
pub struct Claimant<'a> {
    pub issuer: &'a str,
    pub subject: &'a str,
    pub audience: &'a str,
}

/// What [`sign_challenge`] refuses, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChallengeRefusal {
    /// Over [`MAX_CHALLENGE_BYTES`].
    TooLong,
    /// Not the challenge format: not JSON, a field missing, an unknown or
    /// duplicate field, or not byte-for-byte canonical.
    NotAChallenge(String),
    /// The domain tag is not [`DOMAIN_TAG`].
    WrongDomain,
    /// An empty field.
    EmptyField(&'static str),
    /// It names another key than the one signing.
    OtherKey,
    /// It names another issuer than the one the key holder stated.
    OtherIssuer,
    /// It names another subject than the one the key holder stated.
    OtherSubject,
    /// It names another daemon (`aud`) than the one the key holder stated.
    OtherAudience,
    /// `expires_at` is not in the future.
    Expired,
    /// `expires_at` is more than [`MAX_CHALLENGE_LIFETIME_SECS`] away.
    ExpiryTooFar,
}

impl ChallengeRefusal {
    /// A stable, machine-readable name for the refusal (the WASM API's
    /// error `code`).
    pub fn code(&self) -> &'static str {
        match self {
            Self::TooLong => "too-long",
            Self::NotAChallenge(_) => "not-a-challenge",
            Self::WrongDomain => "wrong-domain",
            Self::EmptyField(_) => "empty-field",
            Self::OtherKey => "other-key",
            Self::OtherIssuer => "other-issuer",
            Self::OtherSubject => "other-subject",
            Self::OtherAudience => "other-audience",
            Self::Expired => "expired",
            Self::ExpiryTooFar => "expiry-too-far",
        }
    }
}

impl std::fmt::Display for ChallengeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLong => write!(f, "longer than any challenge"),
            Self::NotAChallenge(e) => write!(f, "not an identity-binding challenge: {e}"),
            Self::WrongDomain => write!(f, "not an identity-binding challenge (domain tag)"),
            Self::EmptyField(k) => write!(f, "the challenge's {k} is empty"),
            Self::OtherKey => write!(f, "the challenge names another key than this identity"),
            Self::OtherIssuer => write!(
                f,
                "the challenge is for another identity provider than the one you stated"
            ),
            Self::OtherSubject => write!(
                f,
                "the challenge would bind this key to another subject than the one you stated"
            ),
            Self::OtherAudience => write!(
                f,
                "the challenge is for another daemon than the one you stated"
            ),
            Self::Expired => write!(f, "the challenge has expired; ask for a new one"),
            Self::ExpiryTooFar => write!(
                f,
                "the challenge stays valid for longer than any daemon issues one"
            ),
        }
    }
}

/// The key a challenge is signed with: `mae_mcp::identity::Identity` in the
/// editor, [`crate::member::MemberKey`] in the browser.
pub trait ChallengeSigner {
    /// `SHA256:<base64, no padding>` of the public key.
    fn fingerprint(&self) -> String;
    /// The public key, standard padded base64.
    fn public_key_b64(&self) -> String;
    /// The 64-byte Ed25519 signature over `msg`.
    fn sign_bytes(&self, msg: &[u8]) -> Vec<u8>;
}

impl ChallengeSigner for crate::member::MemberKey {
    fn fingerprint(&self) -> String {
        crate::member::MemberKey::fingerprint(self)
    }
    fn public_key_b64(&self) -> String {
        crate::member::MemberKey::public_key_b64(self)
    }
    fn sign_bytes(&self, msg: &[u8]) -> Vec<u8> {
        self.sign(msg).to_vec()
    }
}

/// Parse and check `text` for signing by the key whose fingerprint is
/// `fingerprint`, held by `claimant`, at `now` (unix seconds).
pub fn parse_for(
    fingerprint: &str,
    claimant: Claimant<'_>,
    text: &str,
    now: u64,
) -> Result<Challenge, ChallengeRefusal> {
    if text.len() > MAX_CHALLENGE_BYTES {
        return Err(ChallengeRefusal::TooLong);
    }
    let c: Challenge = serde_json::from_str(text).map_err(|e| not_a_challenge(&e))?;
    if c.domain != DOMAIN_TAG {
        return Err(ChallengeRefusal::WrongDomain);
    }
    let canonical = challenge_text(
        &c.iss,
        &c.sub,
        &c.aud,
        &c.fingerprint,
        &c.nonce,
        c.expires_at,
    );
    if canonical != text {
        return Err(ChallengeRefusal::NotAChallenge(
            "not in the canonical form the daemon issues".into(),
        ));
    }
    for (name, v) in [
        ("iss", &c.iss),
        ("sub", &c.sub),
        ("aud", &c.aud),
        ("fingerprint", &c.fingerprint),
        ("nonce", &c.nonce),
    ] {
        if v.is_empty() {
            return Err(ChallengeRefusal::EmptyField(name));
        }
    }
    if c.fingerprint != fingerprint {
        return Err(ChallengeRefusal::OtherKey);
    }
    if c.iss != claimant.issuer {
        return Err(ChallengeRefusal::OtherIssuer);
    }
    if c.sub != claimant.subject {
        return Err(ChallengeRefusal::OtherSubject);
    }
    if c.aud != claimant.audience {
        return Err(ChallengeRefusal::OtherAudience);
    }
    if c.expires_at <= now {
        return Err(ChallengeRefusal::Expired);
    }
    if c.expires_at - now > MAX_CHALLENGE_LIFETIME_SECS {
        return Err(ChallengeRefusal::ExpiryTooFar);
    }
    Ok(c)
}

/// The refusal for text that does not parse as a challenge. The reason is
/// one of a fixed few, never serde's message: that quotes the input, and the
/// browser shows refusals to the person (or logs them).
fn not_a_challenge(e: &serde_json::Error) -> ChallengeRefusal {
    use serde_json::error::Category;
    let reason = match e.classify() {
        Category::Data => "not exactly the challenge's seven fields",
        Category::Syntax | Category::Eof | Category::Io => "not JSON",
    };
    ChallengeRefusal::NotAChallenge(reason.into())
}

/// The challenge in `input` (as [`sign_challenge`] reads it), checked as
/// [`parse_for`] checks it, without signing: for a signer to show the person
/// what they are about to bind before it signs.
pub fn check_challenge(
    fingerprint: &str,
    claimant: Claimant<'_>,
    input: &str,
    now: u64,
) -> Result<Challenge, ChallengeRefusal> {
    parse_for(fingerprint, claimant, &challenge_of(input)?, now)
}

/// Sign a challenge with `key`, held by `claimant`, and return it with the body
/// `POST /identity/bind` expects: `{"challenge_id", "public_key",
/// "signature"}` (base64 key and signature). `input` may be the challenge text
/// or the daemon's whole challenge answer. Nothing secret is in the result.
pub fn sign_challenge(
    key: &impl ChallengeSigner,
    claimant: Claimant<'_>,
    input: &str,
    now: u64,
) -> Result<(Challenge, serde_json::Value), ChallengeRefusal> {
    let text = challenge_of(input)?;
    let c = parse_for(&key.fingerprint(), claimant, &text, now)?;
    use base64::Engine;
    let signature =
        base64::engine::general_purpose::STANDARD.encode(key.sign_bytes(text.as_bytes()));
    let body = serde_json::json!({
        "challenge_id": c.nonce,
        "public_key": key.public_key_b64(),
        "signature": signature,
    });
    Ok((c, body))
}

/// The challenge text in `input`: the text itself, or the `challenge` string of
/// the daemon's answer `{"challenge_id", "challenge", "expires_in"}` (only
/// those three keys; anything else is not an answer the daemon gives).
pub fn challenge_of(input: &str) -> Result<String, ChallengeRefusal> {
    let trimmed = input.trim();
    if trimmed.len() > 2 * MAX_CHALLENGE_BYTES {
        return Err(ChallengeRefusal::TooLong);
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Answer {
        #[allow(dead_code)]
        challenge_id: String,
        challenge: String,
        #[allow(dead_code)]
        expires_in: u64,
    }
    match serde_json::from_str::<Answer>(trimmed) {
        Ok(a) => Ok(a.challenge),
        Err(_) => Ok(trimmed.to_string()),
    }
}

#[cfg(test)]
#[path = "identity_challenge_tests.rs"]
mod identity_challenge_tests;
