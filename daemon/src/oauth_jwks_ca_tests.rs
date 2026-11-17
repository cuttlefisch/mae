//! The JWKS client is built fallibly: an image without CA certificates gets
//! an error naming what to do, never a panic, and `oauth.jwks_ca_file` lets
//! the fetch trust a named PEM bundle instead of the system's roots. The
//! no-roots cases run in a child process: they must not change this process's
//! environment, which other tests share under `cargo test`.

use super::JwksCache;
use std::io::Write;

const URL: &str = "https://issuer.invalid/jwks.json";

/// Set in a child test process only: the child runs with the platform's root
/// loading pointed at an empty bundle and directory, as in a container image
/// with no CA certificates.
const CHILD: &str = "MAE_JWKS_CA_TEST_CHILD";

/// Run `test` (an `#[ignore]`d test in this module) in a child process whose
/// environment has no CA roots, and assert it passed. The environment is never
/// changed in THIS process: `cargo test` runs tests as threads of one
/// process, and emptied roots leaked into an unrelated test's HTTPS client.
fn in_a_child_without_system_roots(test: &str) {
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty.pem");
    std::fs::write(&empty, b"").unwrap();
    let name = format!("oauth::oauth_jwks_ca_tests::{test}");
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([name.as_str(), "--exact", "--ignored", "--nocapture"])
        .env(CHILD, dir.path())
        .env("SSL_CERT_FILE", &empty)
        .env("SSL_CERT_DIR", dir.path().join("no-such-dir"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{test} failed in the child:\n{text}");
    assert!(
        text.contains("1 passed"),
        "{test} did not run in the child:\n{text}"
    );
}

/// The child's scratch directory, or `None` when not running as a child (a
/// plain `--ignored` run does nothing).
fn child_dir() -> Option<std::path::PathBuf> {
    std::env::var_os(CHILD).map(std::path::PathBuf::from)
}

fn ca_pem() -> String {
    let ck = rcgen::generate_simple_self_signed(vec!["issuer.invalid".to_string()]).unwrap();
    ck.cert.pem()
}

#[test]
fn no_system_roots_is_an_error_naming_the_fix_not_a_panic() {
    in_a_child_without_system_roots("child_no_system_roots");
}

#[test]
#[ignore = "run by no_system_roots_is_an_error_naming_the_fix_not_a_panic in a child process"]
fn child_no_system_roots() {
    if child_dir().is_none() {
        return;
    }
    let got = std::panic::catch_unwind(|| JwksCache::new(URL.to_string(), None));
    let result = got.expect("building the JWKS client must not panic");
    let err = result.err().expect("no roots at all must be refused");
    assert!(err.contains("oauth.jwks_url"), "{err}");
    assert!(err.contains("oauth.jwks_ca_file"), "{err}");
}

#[test]
fn a_ca_file_works_where_the_system_has_no_roots() {
    in_a_child_without_system_roots("child_ca_file_without_system_roots");
}

#[test]
#[ignore = "run by a_ca_file_works_where_the_system_has_no_roots in a child process"]
fn child_ca_file_without_system_roots() {
    let Some(dir) = child_dir() else { return };
    let path = dir.join("ca.pem");
    std::fs::write(&path, ca_pem()).unwrap();
    assert!(JwksCache::new(URL.to_string(), Some(&path)).is_ok());
}

#[test]
fn a_missing_garbage_or_empty_ca_file_is_refused_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.pem");
    let garbage = dir.path().join("garbage.pem");
    let mut f = std::fs::File::create(&garbage).unwrap();
    f.write_all(b"-----BEGIN CERTIFICATE-----\nnot base64 at all\n-----END CERTIFICATE-----\n")
        .unwrap();
    let empty = dir.path().join("empty.pem");
    std::fs::write(&empty, b"# no certificate here\n").unwrap();
    for path in [&missing, &garbage, &empty] {
        let err = JwksCache::new(URL.to_string(), Some(path))
            .err()
            .expect("refused");
        assert!(
            err.contains("oauth.jwks_ca_file"),
            "{}: {err}",
            path.display()
        );
        assert!(err.contains(&path.display().to_string()), "{err}");
    }
}
