//! The identity-binding challenge (ADR-098 Amendment 1, C2), over this
//! crate's [`Identity`]: used by the daemon that issues and verifies it and
//! the editor command that signs it (`mae --sign-identity-challenge`).
//!
//! The format and the signing rules are `mae_crypto::identity_challenge`,
//! the one implementation the browser runs too (mae-crypto-wasm); this
//! module only binds them to [`Identity`]. Its tests
//! (`identity_challenge_tests.rs`) run the refusal table through this
//! wrapper, so a wrapper that passed the wrong key or claimant fails them.
//!
//! @ai-caution: [security] [`sign_challenge`] signs with the member key, which
//! also signs membership and content ops. It must never become a signing
//! oracle: the rules (canonical text byte for byte, domain tag, this key,
//! unexpired and not too far off, only for the stated issuer, subject and
//! audience) live in
//! `mae_crypto::identity_challenge` and must not be re-implemented or
//! bypassed here.

pub use mae_crypto::identity_challenge::{
    challenge_text, Challenge, ChallengeRefusal, ChallengeSigner, Claimant, DOMAIN_TAG,
    MAX_CHALLENGE_BYTES, MAX_CHALLENGE_LIFETIME_SECS,
};

use crate::identity::Identity;
use mae_crypto::identity_challenge as shared;

impl ChallengeSigner for Identity {
    fn fingerprint(&self) -> String {
        Identity::fingerprint(self)
    }
    fn public_key_b64(&self) -> String {
        self.public().encoded()
    }
    fn sign_bytes(&self, msg: &[u8]) -> Vec<u8> {
        self.sign(msg)
    }
}

/// Parse and check `text` for signing by `identity`, held by `claimant`, at
/// `now` (unix seconds).
pub fn parse_for(
    identity: &Identity,
    claimant: Claimant<'_>,
    text: &str,
    now: u64,
) -> Result<Challenge, ChallengeRefusal> {
    shared::parse_for(&identity.fingerprint(), claimant, text, now)
}

/// The challenge in `input` (as [`sign_challenge`] reads it), checked as
/// [`parse_for`] checks it, without signing: for a signer to show the person
/// what they are about to bind before it signs.
pub fn check_challenge(
    identity: &Identity,
    claimant: Claimant<'_>,
    input: &str,
    now: u64,
) -> Result<Challenge, ChallengeRefusal> {
    shared::check_challenge(&identity.fingerprint(), claimant, input, now)
}

/// Sign a challenge with `identity`, held by `claimant`, and return it with the body
/// `POST /identity/bind` expects: `{"challenge_id", "public_key",
/// "signature"}` (base64 key and signature). `input` may be the challenge text
/// or the daemon's whole challenge answer. Nothing secret is in the result.
pub fn sign_challenge(
    identity: &Identity,
    claimant: Claimant<'_>,
    input: &str,
    now: u64,
) -> Result<(Challenge, serde_json::Value), ChallengeRefusal> {
    shared::sign_challenge(identity, claimant, input, now)
}

#[cfg(test)]
#[path = "identity_challenge_tests.rs"]
mod identity_challenge_tests;
