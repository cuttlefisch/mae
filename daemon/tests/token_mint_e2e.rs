//! `mae-daemon token mint` — the operator-issued, scoped, expiring token of
//! ADR-111 P1: how a headless service account (or anyone without collab mTLS)
//! gets a bearer token for the HTTPS listener's `kb/query.*` surface.
//!
//! It is an OPERATOR command on purpose (CLAUDE.md principle #16): whoever can
//! run it holds the daemon's signing key, so it is not an MCP tool and not an
//! RPC. These tests drive the real binary exactly as an operator would, in a
//! per-test temp dir with HOME/XDG pointed inside it, and never touch a running
//! daemon or the real environment.
//!
//! The attacker/mistake cases each test pins:
//! - a mint on a host with no identity must not quietly CREATE one (issue #652:
//!   `--check-config` once minted a root-owned key as a side effect);
//! - a TTL above the configured maximum is refused, not clamped silently;
//! - a token from this command is accepted by the real listener, and one
//!   signed by any other daemon's key is refused.

use std::path::{Path, PathBuf};
use std::process::Output;

use mae_mcp::identity::Identity;

const AUD: &str = "https://127.0.0.1/mcp";

fn free_tcp_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// TOML-safe path (Windows backslashes are escapes in a basic string).
fn toml_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "\\\\")
}

/// A per-test daemon home: config, identity dir, TLS pair, all under `tmp`.
struct Home {
    tmp: tempfile::TempDir,
    config: PathBuf,
    identity_dir: PathBuf,
    oauth_port: u16,
}

/// Knobs a test varies; everything else is a working self-issued deployment.
struct Opts {
    seed_identity: bool,
    self_issued_tokens_enabled: bool,
    default_ttl_secs: u64,
    max_ttl_secs: u64,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            seed_identity: true,
            self_issued_tokens_enabled: true,
            default_ttl_secs: 900,
            max_ttl_secs: 3600,
        }
    }
}

fn make_home(opts: &Opts) -> (Home, Option<Identity>) {
    let tmp = tempfile::tempdir().unwrap();
    let identity_dir = tmp.path().join("identity");
    let identity = opts.seed_identity.then(|| {
        let id = Identity::generate("daemon-under-test");
        id.save(&identity_dir).unwrap();
        id
    });
    let ak = tmp.path().join("authorized_keys");
    std::fs::write(
        &ak,
        format!("{}\n", Identity::generate("client").public().to_line()),
    )
    .unwrap();
    let ck = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
    let (cert, key) = (tmp.path().join("tls.crt"), tmp.path().join("tls.key"));
    std::fs::write(&cert, ck.cert.pem()).unwrap();
    std::fs::write(&key, ck.signing_key.serialize_pem()).unwrap();

    let oauth_port = free_tcp_port();
    let config = tmp.path().join("daemon.toml");
    std::fs::write(
        &config,
        format!(
            r#"
[collab]
enabled = true
bind = "127.0.0.1:{collab_port}"

[collab.auth]
mode = "key"
identity_dir = "{identity_dir}"
authorized_keys = "{ak}"

[oauth]
enabled = true
bind = "127.0.0.1:{oauth_port}"
canonical_resource_uri = "{AUD}"
cert_path = "{cert}"
key_path = "{key}"
kb_query_enabled = true
self_issued_tokens_enabled = {enabled}
self_issued_token_ttl_secs = {ttl}
self_issued_token_max_ttl_secs = {max}
"#,
            collab_port = free_tcp_port(),
            identity_dir = toml_path(&identity_dir),
            ak = toml_path(&ak),
            cert = toml_path(&cert),
            key = toml_path(&key),
            enabled = opts.self_issued_tokens_enabled,
            ttl = opts.default_ttl_secs,
            max = opts.max_ttl_secs,
        ),
    )
    .unwrap();
    (
        Home {
            tmp,
            config,
            identity_dir,
            oauth_port,
        },
        identity,
    )
}

/// Run `mae-daemon --config <home> <args…>` with every ambient location pointed
/// inside the temp dir, so nothing can land in the real HOME.
///
/// Bounded: an administrative command exits promptly, and a binary that does
/// NOT recognise the subcommand falls through to SERVING forever -- which is
/// exactly what the pre-`token` binary does. That must fail the test, not hang
/// it, so the child is killed at the deadline and reported as a failure.
fn run(home: &Home, args: &[&str]) -> Output {
    let root = home.tmp.path();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .arg("--config")
        .arg(&home.config)
        .args(args)
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_RUNTIME_DIR", root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run mae-daemon");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let out = child.wait_with_output().unwrap();
            panic!(
                "`mae-daemon {args:?}` did not exit -- not an administrative command? \
                 stderr: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Every file under `dir`, recursively.
fn all_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(all_files(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// Claims of a JWT, read WITHOUT verification (the tests verify through the
/// real listener; this only inspects what was minted).
fn claims(token: &str) -> serde_json::Value {
    use base64::Engine;
    let payload = token.split('.').nth(1).expect("a JWT has a payload");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// stdout is the token and nothing else -- one line, three JWT segments -- so
/// `mae-daemon token mint … | pass insert -e …` stores exactly the credential.
fn minted(o: &Output) -> String {
    assert!(o.status.success(), "mint failed: {}", stderr(o));
    let out = stdout(o);
    let token = out.strip_suffix('\n').unwrap_or(&out);
    assert!(
        !token.contains('\n'),
        "stdout must be the token only: {out:?}"
    );
    assert_eq!(token.split('.').count(), 3, "not a JWT: {out:?}");
    token.to_string()
}

#[test]
fn mint_refuses_without_an_identity_and_creates_none() {
    let (home, _) = make_home(&Opts {
        seed_identity: false,
        ..Opts::default()
    });
    let before = all_files(home.tmp.path());
    let out = run(&home, &["token", "mint", "--sub", "svc:indexer"]);
    assert!(!out.status.success(), "must refuse with no identity");
    assert_eq!(stdout(&out), "", "nothing on stdout when refusing");
    assert!(
        stderr(&out).contains("identity"),
        "the refusal must say what is missing: {}",
        stderr(&out)
    );
    assert!(
        !home.identity_dir.exists(),
        "a mint must never create the identity directory"
    );
    assert_eq!(
        all_files(home.tmp.path()),
        before,
        "a refused mint must write nothing at all"
    );
}

/// The configured maximum is a hard ceiling, on both sides of the boundary,
/// and the TTL minted is the TTL asked for (read back from the token, not
/// assumed). Units are varied so the parser, not one spelling, is under test.
#[test]
fn mint_honours_the_requested_ttl_and_refuses_one_above_the_maximum() {
    let (home, _) = make_home(&Opts::default()); // max 3600
    for (ttl, secs) in [("3600", 3600u64), ("60m", 3600), ("1h", 3600), ("45s", 45)] {
        let token = minted(&run(
            &home,
            &["token", "mint", "--sub", "svc:a", "--ttl", ttl],
        ));
        let c = claims(&token);
        assert_eq!(
            c["exp"].as_u64().unwrap() - c["iat"].as_u64().unwrap(),
            secs,
            "--ttl {ttl}"
        );
        assert_eq!(c["sub"], "svc:a");
        assert_eq!(c["aud"], AUD, "audience comes from config");
    }
    for ttl in ["3601", "61m", "2h", "1d", "0", "-5", "soon", ""] {
        let out = run(&home, &["token", "mint", "--sub", "svc:a", "--ttl", ttl]);
        assert!(!out.status.success(), "--ttl {ttl:?} must be refused");
        assert_eq!(stdout(&out), "", "--ttl {ttl:?}: nothing on stdout");
    }
}

/// With no `--ttl`, the configured `self_issued_token_ttl_secs` applies (900
/// here, deliberately not the built-in default, so a hardcoded value fails).
#[test]
fn mint_defaults_to_the_configured_short_ttl() {
    let (home, _) = make_home(&Opts::default());
    let c = claims(&minted(&run(&home, &["token", "mint", "--sub", "svc:b"])));
    assert_eq!(c["exp"].as_u64().unwrap() - c["iat"].as_u64().unwrap(), 900);
}

/// A token the listener would never accept is refused at mint time, not
/// handed out to fail later; and a missing `--sub` is a usage error.
#[test]
fn mint_refuses_when_the_listener_would_not_accept_the_token() {
    let (home, _) = make_home(&Opts {
        self_issued_tokens_enabled: false,
        ..Opts::default()
    });
    let out = run(&home, &["token", "mint", "--sub", "svc:c"]);
    assert!(!out.status.success());
    assert_eq!(stdout(&out), "");
    assert!(
        stderr(&out).contains("self_issued_tokens_enabled"),
        "{}",
        stderr(&out)
    );

    let (home, _) = make_home(&Opts {
        default_ttl_secs: 7200,
        max_ttl_secs: 3600,
        ..Opts::default()
    });
    let out = run(&home, &["token", "mint", "--sub", "svc:c"]);
    assert!(
        !out.status.success(),
        "a configured default above the maximum must not slip through"
    );

    let (home, _) = make_home(&Opts::default());
    for args in [
        &["token", "mint"][..],
        &["token", "mint", "--sub", ""],
        &["token"],
    ] {
        let out = run(&home, args);
        assert_eq!(out.status.code(), Some(2), "{args:?} is a usage error");
        assert_eq!(stdout(&out), "");
    }
}

/// The end-to-end property: a token from this command is ACCEPTED by the real
/// HTTPS listener for a KB-query request, and one minted the same way by a
/// DIFFERENT daemon's key is REFUSED.
#[tokio::test]
async fn the_minted_token_is_accepted_by_the_listener_and_a_foreign_one_is_not() {
    let (home, _) = make_home(&Opts::default());
    let token = minted(&run(&home, &["token", "mint", "--sub", "svc:reader"]));
    let (other, _) = make_home(&Opts::default());
    let foreign = minted(&run(&other, &["token", "mint", "--sub", "svc:reader"]));

    let root = home.tmp.path();
    let _daemon = tokio::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .arg("--config")
        .arg(&home.config)
        .args(["--data-dir", root.to_str().unwrap()])
        .args(["--socket", root.join("kb.sock").to_str().unwrap()])
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_RUNTIME_DIR", root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let addr: std::net::SocketAddr = format!("127.0.0.1:{}", home.oauth_port).parse().unwrap();
    let up =
        mae_mcp::ready::wait_until(|| async { tokio::net::TcpStream::connect(addr).await.is_ok() })
            .await;
    assert!(up, "{}", mae_mcp::ready::timeout_message("OAuth listener"));

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    let rpc = serde_json::json!({
        "jsonrpc": "2.0", "id": 7, "method": "kb/query.capabilities",
        "params": {"kb_id": "no-such-kb"}
    });

    let resp = client
        .post(format!("https://{addr}/"))
        .bearer_auth(&token)
        .json(&rpc)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "the minted token must authenticate");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["id"], 7,
        "the request reached kb/query dispatch, which answered it: {body}"
    );

    let probe: serde_json::Value = client
        .get(format!("https://{addr}/"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(probe["principal"], "svc:reader", "the principal is --sub");

    let resp = client
        .post(format!("https://{addr}/"))
        .bearer_auth(&foreign)
        .json(&rpc)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "another daemon's key must not be accepted"
    );
}
