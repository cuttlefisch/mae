//! `mae --sign-identity-challenge <challenge | ->`: the key holder's half of
//! binding an identity-provider subject to this editor's collab key (ADR-098
//! Amendment 1, C2).
//!
//! The person asks the daemon for a challenge in the program holding their
//! identity-provider session (a browser), carries it here, and pastes the
//! printed body into `POST /identity/bind`. The challenge may be the text
//! itself or the daemon's whole answer; `-` reads it from stdin.
//!
//! stdout carries the bind body only (`challenge_id`, `public_key`,
//! `signature`), so it can be piped; stderr says which subject is being bound
//! to which key, for the person to check before pasting.
//!
//! @ai-caution: [security] The member key also signs membership and content
//! ops. This command signs ONLY what `mae_mcp::identity_challenge::
//! sign_challenge` accepts — a canonical, unexpired challenge naming this key
//! — never arbitrary text, and it never prints or logs the secret key. It
//! loads the identity without creating one.

use std::io::{self, Read};
use std::path::Path;

use mae_mcp::identity::Identity;
use mae_mcp::identity_challenge::{sign_challenge, MAX_CHALLENGE_BYTES};

const FLAG: &str = "--sign-identity-challenge";

/// The label an identity loaded only to sign carries; it is never written.
const LABEL: &str = "mae-editor";

/// Dispatch `--sign-identity-challenge`; `None` when the flag is absent.
pub(crate) fn handle(args: &[String]) -> Option<io::Result<()>> {
    let pos = args.iter().position(|a| a == FLAG)?;
    let input = match args.get(pos + 1).map(String::as_str) {
        Some("-") => {
            let mut buf = String::new();
            let limit = (2 * MAX_CHALLENGE_BYTES + 1) as u64;
            if let Err(e) = io::stdin().take(limit).read_to_string(&mut buf) {
                fail(&format!("reading the challenge from stdin: {e}"));
            }
            buf
        }
        Some(text) => text.to_string(),
        None => fail(&format!("usage: mae {FLAG} <challenge | ->")),
    };
    let Some(dir) = mae_mcp::identity::default_collab_dir() else {
        fail("cannot resolve the collab dir (set XDG_DATA_HOME or HOME)");
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    match sign_in(&dir, &input, now) {
        Ok((summary, body)) => {
            eprintln!("{summary}");
            println!("{body}");
            Some(Ok(()))
        }
        Err(e) => fail(&e),
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}

/// Sign `input` with the identity in `dir`: `(summary for stderr, bind body
/// for stdout)`. Refuses, signing nothing, when there is no identity or the
/// input is not a challenge for it.
pub(crate) fn sign_in(dir: &Path, input: &str, now: u64) -> Result<(String, String), String> {
    let identity = Identity::load_secret(dir, LABEL).ok_or_else(|| {
        format!(
            "no collab identity in {} (`mae --collab-identity` creates one)",
            dir.display()
        )
    })?;
    let (c, body) = sign_challenge(&identity, input, now).map_err(|e| e.to_string())?;
    let summary = format!(
        "Binds identity-provider subject {:?} (issuer {:?}) to this editor's key {} on {}.\n\
         Paste the line below into the binding page only if that subject is you.",
        c.sub, c.iss, c.fingerprint, c.aud
    );
    Ok((summary, body.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_mcp::identity_challenge::challenge_text;

    const NOW: u64 = 1_900_000_000;

    fn challenge_for(id: &Identity) -> String {
        challenge_text(
            "https://idp.example.com",
            "alice@idp",
            "https://mae.example.com",
            &id.fingerprint(),
            "bm9uY2Utbm9uY2Utbm9uY2Utbm9uY2Utbm9uY2Utbm9u",
            NOW + 60,
        )
    }

    #[test]
    fn signs_a_challenge_for_the_stored_identity_without_printing_its_secret() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::generate("editor");
        id.save(dir.path()).unwrap();
        let (summary, body) = sign_in(dir.path(), &challenge_for(&id), NOW).unwrap();
        assert!(summary.contains("alice@idp") && summary.contains(&id.fingerprint()));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["public_key"], id.public().encoded());
        let secret = hex::encode(id.secret_bytes());
        assert!(!body.contains(&secret) && !summary.contains(&secret));
    }

    /// Not a signing oracle: arbitrary text, and a challenge naming another
    /// key, are refused with nothing signed.
    #[test]
    fn refuses_anything_that_is_not_a_challenge_for_this_key() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::generate("editor");
        id.save(dir.path()).unwrap();
        for input in [
            "arbitrary text to sign".to_string(),
            "{\"domain\":\"mae-identity-binding-v1\"}".to_string(),
            challenge_for(&Identity::generate("someone-else")),
        ] {
            assert!(sign_in(dir.path(), &input, NOW).is_err(), "signed {input}");
        }
    }

    /// No identity: refused, and none is created as a side effect.
    #[test]
    fn without_an_identity_it_refuses_and_creates_none() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::generate("editor");
        let e = sign_in(dir.path(), &challenge_for(&id), NOW).unwrap_err();
        assert!(e.contains("--collab-identity"), "{e}");
        assert!(!dir.path().join("id_ed25519").exists());
    }
}
