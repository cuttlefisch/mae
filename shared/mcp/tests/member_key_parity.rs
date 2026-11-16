//! The browser's member key (`mae_crypto::member`, built into the WASM
//! module) against this crate's `Identity`: the same seed must give the same
//! public key, the same wire encoding, the same fingerprint and the same
//! signatures, or a key generated in the browser would not be the key the
//! daemon and the editor know. `identity.rs` computes its forms on its own,
//! so this compares two implementations, not one with itself. Seeds are
//! fresh per run.

use mae_crypto::member::{self, MemberKey};
use mae_mcp::identity::{Identity, PublicKey};

#[test]
fn the_same_seed_gives_the_same_key_forms_and_signatures() {
    for i in 0..64 {
        let id = Identity::generate("parity");
        let seed = id.secret_bytes();
        let mk = MemberKey::from_seed(&seed);
        assert_eq!(mk.public_key(), id.public().to_bytes(), "public key #{i}");
        assert_eq!(mk.public_key_b64(), id.public().encoded(), "encoding #{i}");
        assert_eq!(mk.fingerprint(), id.fingerprint(), "fingerprint #{i}");
        assert_eq!(
            member::fingerprint(&mk.public_key()),
            id.public().fingerprint()
        );
        let msg = format!("message {i}: {}", "x".repeat(i));
        assert_eq!(mk.sign(msg.as_bytes()).to_vec(), id.sign(msg.as_bytes()));
        assert!(id.public().verify(msg.as_bytes(), &mk.sign(msg.as_bytes())));
        // And the other direction: a key made by mae-crypto loads as an Identity.
        let fresh = MemberKey::generate().unwrap();
        let loaded = Identity::from_seed(&fresh.seed(), "parity");
        assert_eq!(loaded.fingerprint(), fresh.fingerprint());
        let decoded = PublicKey::from_encoded(&fresh.public_key_b64(), None).unwrap();
        assert_eq!(decoded.to_bytes(), fresh.public_key());
    }
}

/// A challenge signed in the browser (mae-crypto over `MemberKey`) is the
/// same body the editor produces (mae-mcp over `Identity`) for the same key:
/// one rule set, two signers.
#[test]
fn a_challenge_signs_identically_through_both_signers() {
    use mae_mcp::identity_challenge::{challenge_text, sign_challenge, Claimant};
    let id = Identity::generate("parity");
    let mk = MemberKey::from_seed(&id.secret_bytes());
    let me = Claimant {
        issuer: "https://idp.example.com",
        subject: "alice@idp",
        audience: "https://mae.example.com",
    };
    let text = challenge_text(
        me.issuer,
        me.subject,
        me.audience,
        &id.fingerprint(),
        "bm9uY2U",
        1_900_000_600,
    );
    let (_, editor) = sign_challenge(&id, me, &text, 1_900_000_000).unwrap();
    let (_, browser) =
        mae_crypto::identity_challenge::sign_challenge(&mk, me, &text, 1_900_000_000).unwrap();
    assert_eq!(editor, browser);
}
