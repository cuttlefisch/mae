//! `mae --sign-identity-challenge <challenge | -> --issuer <iss> --subject
//! <sub>`: the key holder's half of binding an identity-provider subject to
//! this editor's collab key (ADR-098 Amendment 1, C2).
//!
//! The person asks the daemon for a challenge in the program holding their
//! identity-provider session (a browser), carries it here, and pastes the
//! printed body into `POST /identity/bind`. The challenge may be the text
//! itself or the daemon's whole answer; `-` reads it from stdin.
//!
//! `--issuer` and `--subject` are the person's own identity-provider
//! identity, stated by them: the daemon issues a challenge for any key to
//! any authenticated subject, so a challenge naming this key says nothing
//! about who asked for it. One issued to anyone else is refused.
//!
//! stderr says what is being bound (issuer, subject, audience, key) before
//! anything is signed; stdout carries the bind body only (`challenge_id`,
//! `public_key`, `signature`), so it can be piped.
//!
//! @ai-caution: [security] The member key also signs membership and content
//! ops. This command signs ONLY what `mae_mcp::identity_challenge::
//! sign_challenge` accepts — a canonical, unexpired challenge naming this key
//! AND the issuer and subject the person stated — never arbitrary text, and
//! it never prints or logs the secret key. It loads the identity without
//! creating one.

use std::io::{self, Read};
use std::path::Path;

use mae_mcp::identity::Identity;
use mae_mcp::identity_challenge::{check_challenge, sign_challenge, Claimant, MAX_CHALLENGE_BYTES};

const FLAG: &str = "--sign-identity-challenge";
const USAGE: &str =
    "usage: mae --sign-identity-challenge <challenge | -> --issuer <iss> --subject <sub>";

/// The label an identity loaded only to sign carries; it is never written.
const LABEL: &str = "mae-editor";

/// Dispatch `--sign-identity-challenge`; `None` when the flag is absent.
pub(crate) fn handle(args: &[String]) -> Option<io::Result<()>> {
    let pos = args.iter().position(|a| a == FLAG)?;
    let parsed = match parse_args(&args[pos + 1..]) {
        Ok(p) => p,
        Err(e) => fail(&e),
    };
    let input = match parsed.challenge.as_str() {
        "-" => {
            let mut buf = String::new();
            let limit = (2 * MAX_CHALLENGE_BYTES + 1) as u64;
            if let Err(e) = io::stdin().take(limit).read_to_string(&mut buf) {
                fail(&format!("reading the challenge from stdin: {e}"));
            }
            buf
        }
        text => text.to_string(),
    };
    let Some(dir) = mae_mcp::identity::default_collab_dir() else {
        fail("cannot resolve the collab dir (set XDG_DATA_HOME or HOME)");
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let claimant = Claimant {
        issuer: &parsed.issuer,
        subject: &parsed.subject,
    };
    match sign_in(&dir, claimant, &input, now, &mut |s| eprintln!("{s}")) {
        Ok(body) => {
            println!("{body}");
            Some(Ok(()))
        }
        Err(e) => fail(&e),
    }
}

/// The command's arguments after the flag.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Args {
    pub challenge: String,
    pub issuer: String,
    pub subject: String,
}

/// Parse the arguments after the flag: one challenge (or `-`) and both
/// `--issuer` and `--subject`, each given once and non-empty.
pub(crate) fn parse_args(rest: &[String]) -> Result<Args, String> {
    let (mut challenge, mut issuer, mut subject) = (None, None, None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let slot = match a.as_str() {
            "--issuer" => &mut issuer,
            "--subject" => &mut subject,
            _ => &mut challenge,
        };
        let value = if a == "--issuer" || a == "--subject" {
            it.next()
                .ok_or_else(|| format!("{a} needs a value; {USAGE}"))?
        } else {
            a
        };
        if slot.replace(value.clone()).is_some() {
            return Err(format!("{a} given twice; {USAGE}"));
        }
    }
    let need = |v: Option<String>, what: &str| match v {
        Some(v) if !v.is_empty() => Ok(v),
        _ => Err(format!("{what} is required; {USAGE}")),
    };
    Ok(Args {
        challenge: need(challenge, "the challenge")?,
        issuer: need(issuer, "--issuer (your identity provider)")?,
        subject: need(subject, "--subject (your subject at it)")?,
    })
}

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}

/// Sign `input` with the identity in `dir`, held by `claimant`, and return
/// the bind body for stdout. `announce` is given what is being bound before
/// anything is signed. Refuses, signing nothing, when there is no identity or
/// the input is not a challenge for this key and this claimant.
pub(crate) fn sign_in(
    dir: &Path,
    claimant: Claimant<'_>,
    input: &str,
    now: u64,
    announce: &mut dyn FnMut(&str),
) -> Result<String, String> {
    let identity = Identity::load_secret(dir, LABEL).ok_or_else(|| {
        format!(
            "no collab identity in {} (`mae --collab-identity` creates one)",
            dir.display()
        )
    })?;
    let c = check_challenge(&identity, claimant, input, now).map_err(|e| e.to_string())?;
    announce(&format!(
        "Binding this editor's key {} to identity-provider subject {:?} (issuer {:?}) \
         on daemon {:?}.\nPaste the line below into the binding page only if you asked \
         for this challenge yourself.",
        c.fingerprint, c.sub, c.iss, c.aud
    ));
    let (_, body) = sign_challenge(&identity, claimant, input, now).map_err(|e| e.to_string())?;
    Ok(body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_mcp::identity_challenge::challenge_text;

    const NOW: u64 = 1_900_000_000;
    const ISS: &str = "https://idp.example.com";
    const ME: Claimant<'static> = Claimant {
        issuer: ISS,
        subject: "alice@idp",
    };

    fn challenge_for(id: &Identity) -> String {
        issued_to(id, "alice@idp")
    }

    fn issued_to(id: &Identity, sub: &str) -> String {
        challenge_text(
            ISS,
            sub,
            "https://mae.example.com",
            &id.fingerprint(),
            "bm9uY2Utbm9uY2Utbm9uY2Utbm9uY2Utbm9uY2Utbm9u",
            NOW + 60,
        )
    }

    /// `sign_in`, collecting what it announces.
    fn sign(dir: &Path, who: Claimant<'_>, input: &str) -> (Result<String, String>, Vec<String>) {
        let mut said = Vec::new();
        let r = sign_in(dir, who, input, NOW, &mut |s| said.push(s.to_string()));
        (r, said)
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn signs_a_challenge_for_the_stored_identity_without_printing_its_secret() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::generate("editor");
        id.save(dir.path()).unwrap();
        let (body, said) = sign(dir.path(), ME, &challenge_for(&id));
        let body = body.unwrap();
        let summary = said.concat();
        let fp = id.fingerprint();
        for shown in ["alice@idp", ISS, "https://mae.example.com", fp.as_str()] {
            assert!(summary.contains(shown), "{shown} not shown: {summary}");
        }
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
            assert!(sign(dir.path(), ME, &input).0.is_err(), "signed {input}");
        }
    }

    /// No identity: refused, and none is created as a side effect.
    #[test]
    fn without_an_identity_it_refuses_and_creates_none() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::generate("editor");
        let e = sign(dir.path(), ME, &challenge_for(&id)).0.unwrap_err();
        assert!(e.contains("--collab-identity"), "{e}");
        assert!(!dir.path().join("id_ed25519").exists());
    }

    /// The finding this guards: a challenge someone else requested for THIS
    /// key — canonical, unexpired, naming the local key — is refused unless it
    /// names the subject and issuer the key holder stated, and nothing is
    /// announced as about to be signed.
    #[test]
    fn a_challenge_issued_to_another_subject_is_refused_though_it_names_this_key() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::generate("editor");
        id.save(dir.path()).unwrap();
        let theirs = issued_to(&id, "mallory@idp");
        let (r, said) = sign(dir.path(), ME, &theirs);
        let e = r.unwrap_err();
        assert!(e.contains("another subject"), "{e}");
        assert!(said.is_empty(), "announced a binding it refused: {said:?}");
        let elsewhere = Claimant {
            issuer: "https://idp.example.org",
            subject: "alice@idp",
        };
        let e = sign(dir.path(), elsewhere, &challenge_for(&id))
            .0
            .unwrap_err();
        assert!(e.contains("another identity provider"), "{e}");
        // The same challenge, stated by the subject it was issued to, signs.
        let mallory = Claimant {
            issuer: ISS,
            subject: "mallory@idp",
        };
        assert!(sign(dir.path(), mallory, &theirs).0.is_ok());
    }

    #[test]
    fn issuer_and_subject_are_required_and_non_empty() {
        let ok = parse_args(&strings(&["-", "--issuer", ISS, "--subject", "alice@idp"])).unwrap();
        assert_eq!(
            ok,
            Args {
                challenge: "-".into(),
                issuer: ISS.into(),
                subject: "alice@idp".into(),
            }
        );
        for bad in [
            vec!["-"],
            vec!["-", "--issuer", ISS],
            vec!["-", "--subject", "alice@idp"],
            vec!["-", "--issuer", ISS, "--subject", ""],
            vec!["-", "--issuer", "", "--subject", "alice@idp"],
            vec!["-", "--issuer", ISS, "--subject"],
            vec!["--issuer", ISS, "--subject", "alice@idp"],
            vec!["-", "x", "--issuer", ISS, "--subject", "alice@idp"],
            vec!["-", "--issuer", ISS, "--subject", "a", "--subject", "b"],
        ] {
            assert!(parse_args(&strings(&bad)).is_err(), "accepted {bad:?}");
        }
    }
}
