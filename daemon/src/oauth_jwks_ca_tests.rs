//! The JWKS client is built fallibly: an image without CA certificates gets
//! an error naming what to do, never a panic, and `oauth.jwks_ca_file` lets
//! the fetch trust a named PEM bundle instead of the system's roots.

use super::JwksCache;
use std::io::Write;

const URL: &str = "https://issuer.invalid/jwks.json";

/// Point the platform's root loading at an empty bundle and an empty
/// directory, as in a container image with no CA certificates. Each nextest
/// test runs in its own process, so this changes nothing for other tests.
fn without_system_roots(dir: &tempfile::TempDir) {
    let empty = dir.path().join("empty.pem");
    std::fs::write(&empty, b"").unwrap();
    std::env::set_var("SSL_CERT_FILE", &empty);
    std::env::set_var("SSL_CERT_DIR", dir.path().join("no-such-dir"));
}

fn ca_pem() -> String {
    let ck = rcgen::generate_simple_self_signed(vec!["issuer.invalid".to_string()]).unwrap();
    ck.cert.pem()
}

#[test]
fn no_system_roots_is_an_error_naming_the_fix_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    without_system_roots(&dir);
    let got = std::panic::catch_unwind(|| JwksCache::new(URL.to_string(), None));
    let result = got.expect("building the JWKS client must not panic");
    let err = result.err().expect("no roots at all must be refused");
    assert!(err.contains("oauth.jwks_url"), "{err}");
    assert!(err.contains("oauth.jwks_ca_file"), "{err}");
}

#[test]
fn a_ca_file_works_where_the_system_has_no_roots() {
    let dir = tempfile::tempdir().unwrap();
    without_system_roots(&dir);
    let path = dir.path().join("ca.pem");
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
