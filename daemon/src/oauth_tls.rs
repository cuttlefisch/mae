//! The OAuth HTTPS listener's server certificate, re-read when its files change
//! (ADR-111 P1).
//!
//! The listener used to load its PEM pair once, at startup. The platform it
//! sits behind issues certificates that live about a week, so every renewal
//! meant a restart -- and a missed restart meant clients refusing an expired
//! certificate. This is the same move `mae_mcp::tls::ReloadingAuthorizedKeys`
//! made for the collab listener's trust store (re-read per handshake so a
//! `revoke` takes effect without a restart), applied to the certificate: a
//! rustls [`ResolvesServerCert`] that checks the pair's file stamps on every
//! handshake and re-parses only when they changed.
//!
//! @ai-caution: [tls] A pair that fails to load -- unreadable, unparseable,
//! empty, or a certificate whose public key does not match the private key, as
//! mid-renewal when one file has been replaced and the other not yet -- MUST
//! leave the previous certificate in service. Failing the handshake instead
//! turns a routine renewal race into an outage for every client. The failed
//! stamp is remembered, so a broken pair is parsed once, not once per
//! handshake; the next write to either file triggers another attempt.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use rustls::crypto::CryptoProvider;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

/// What "changed" means: a file's modification time and length. Cheap (one
/// `stat` per file per handshake) and enough for a renewal, which lands hours
/// or days after the previous one. `None` = the file could not be stat'ed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

type PairStamp = (Option<FileStamp>, Option<FileStamp>);

fn stamp(path: &Path) -> Option<FileStamp> {
    std::fs::metadata(path).ok().map(|m| FileStamp {
        modified: m.modified().ok(),
        len: m.len(),
    })
}

/// Parse a PEM certificate chain + private key into a [`CertifiedKey`],
/// refusing a key that does not match the certificate. Supports PKCS8, PKCS1
/// (RSA) and SEC1 keys -- whichever `rustls-pemfile` finds first, matching how
/// most CAs/`certbot`/`mkcert` output either shape.
fn load_certified_key(
    cert_path: &Path,
    key_path: &Path,
    provider: &CryptoProvider,
) -> Result<CertifiedKey, String> {
    let cert_bytes =
        std::fs::read(cert_path).map_err(|e| format!("reading {}: {e}", cert_path.display()))?;
    let key_bytes =
        std::fs::read(key_path).map_err(|e| format!("reading {}: {e}", key_path.display()))?;
    let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut cert_bytes.as_slice())
            .collect::<Result<_, _>>()
            .map_err(|e| format!("parsing cert chain: {e}"))?;
    if certs.is_empty() {
        return Err(format!("no certificates found in {}", cert_path.display()));
    }
    let key = rustls_pemfile::private_key(&mut key_bytes.as_slice())
        .map_err(|e| format!("parsing private key: {e}"))?
        .ok_or_else(|| format!("no private key found in {}", key_path.display()))?;
    // `from_der` also checks that the key matches the certificate's public key
    // -- the half-renewed pair case.
    CertifiedKey::from_der(certs, key, provider).map_err(|e| format!("certificate/key pair: {e}"))
}

struct Current {
    seen: PairStamp,
    key: Arc<CertifiedKey>,
}

/// A server certificate that follows its files on disk. See the module docs.
pub(crate) struct ReloadingCertResolver {
    cert_path: PathBuf,
    key_path: PathBuf,
    provider: Arc<CryptoProvider>,
    current: Mutex<Current>,
}

impl std::fmt::Debug for ReloadingCertResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReloadingCertResolver")
            .field("cert_path", &self.cert_path)
            .field("key_path", &self.key_path)
            .finish_non_exhaustive()
    }
}

impl ReloadingCertResolver {
    /// Load the initial pair. Unlike a later reload, a failure here IS an
    /// error: there is no previous certificate to fall back on, and a listener
    /// that starts with none would fail every handshake.
    pub(crate) fn new(
        cert_path: &Path,
        key_path: &Path,
        provider: Arc<CryptoProvider>,
    ) -> Result<Self, String> {
        // Stamp BEFORE loading: a write landing between the two then shows up
        // as a changed stamp on the next handshake, rather than being missed.
        let seen = (stamp(cert_path), stamp(key_path));
        let key = Arc::new(load_certified_key(cert_path, key_path, &provider)?);
        Ok(ReloadingCertResolver {
            cert_path: cert_path.to_path_buf(),
            key_path: key_path.to_path_buf(),
            provider,
            current: Mutex::new(Current { seen, key }),
        })
    }

    /// The certificate to present now, reloading first if either file changed.
    fn current(&self) -> Arc<CertifiedKey> {
        let now = (stamp(&self.cert_path), stamp(&self.key_path));
        let mut cur = self.current.lock().unwrap_or_else(|p| p.into_inner());
        if now != cur.seen {
            cur.seen = now;
            match load_certified_key(&self.cert_path, &self.key_path, &self.provider) {
                Ok(key) => {
                    cur.key = Arc::new(key);
                    tracing::info!(cert = %self.cert_path.display(), "OAuth listener: reloaded TLS certificate");
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    cert = %self.cert_path.display(),
                    "OAuth listener: certificate files changed but did not load; \
                     still serving the previous certificate"
                ),
            }
        }
        Arc::clone(&cur.key)
    }
}

impl ResolvesServerCert for ReloadingCertResolver {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current())
    }
}
