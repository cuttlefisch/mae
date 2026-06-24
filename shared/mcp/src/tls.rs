//! Native mutual-TLS transport for collab `key` mode (ADR-017).
//!
//! Each peer presents a self-signed X.509 certificate whose SubjectPublicKeyInfo
//! is its existing Ed25519 [`Identity`] key. TLS 1.3 provides confidentiality and
//! proves possession of the private key; peer trust and pinning move into custom
//! certificate verifiers:
//!
//! - The **daemon** verifies the client cert's Ed25519 pubkey is in
//!   [`AuthorizedKeys`] ([`Ed25519ClientVerifier`]).
//! - The **editor** TOFU-pins the daemon cert's Ed25519 pubkey via a
//!   [`HostKeyVerifier`] ([`Ed25519ServerVerifier`]).
//!
//! We use the **ring** crypto backend with an explicit [`CryptoProvider`] so we
//! never clash with the editor's reqwest (which installs an aws-lc-rs default).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    ClientConfig, DigitallySignedStruct, DistinguishedName, ServerConfig, SignatureScheme,
};

use crate::identity::{AuthorizedKeys, HostKeyVerifier, Identity, PeerIdentity, PublicKey};

// Re-export the tokio-rustls types so the daemon + editor don't need a direct
// dep on tokio-rustls (they already depend on mae-mcp).
pub use rustls::pki_types::ServerName;
pub use tokio_rustls::{TlsAcceptor, TlsConnector};

/// The ring-based crypto provider (explicit, never the process default).
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A fixed SNI name; we ignore server names in our verifiers (identity is the
/// Ed25519 key, not the DNS name), but rustls requires one to connect.
pub const SNI: &str = "mae-daemon";

fn app_verify_failure() -> rustls::Error {
    rustls::Error::InvalidCertificate(rustls::CertificateError::ApplicationVerificationFailure)
}

/// Build a self-signed cert + private key from an Ed25519 [`Identity`].
pub fn cert_and_key(
    id: &Identity,
) -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>), String> {
    let pkcs8 = id.pkcs8_der()?;
    let key_pair = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(
        &PrivatePkcs8KeyDer::from(pkcs8.clone()),
        &rcgen::PKCS_ED25519,
    )
    .map_err(|e| format!("rcgen keypair: {e}"))?;

    let mut params = rcgen::CertificateParams::new(Vec::<String>::new())
        .map_err(|e| format!("rcgen params: {e}"))?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, id.label());

    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| format!("rcgen self_signed: {e}"))?;

    let cert_der = cert.der().clone();
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8));
    Ok((cert_der, key_der))
}

/// Extract the raw 32-byte Ed25519 public key from a peer certificate.
/// Returns `None` unless the SPKI algorithm is Ed25519 (OID `1.3.101.112`) and
/// the key is exactly 32 bytes. This is the trust-critical extraction.
pub fn ed25519_pubkey_from_cert(cert: &CertificateDer) -> Option<[u8; 32]> {
    use x509_parser::prelude::*;
    let (_, parsed) = X509Certificate::from_der(cert.as_ref()).ok()?;
    let spki = parsed.public_key();
    // Ed25519 SPKI algorithm OID is 1.3.101.112.
    if spki.algorithm.algorithm.to_id_string() != "1.3.101.112" {
        return None;
    }
    let raw = spki.subject_public_key.data.as_ref();
    if raw.len() != 32 {
        return None;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(raw);
    Some(out)
}

/// Recover the authenticated [`PeerIdentity`] from the peer's certificate chain
/// (call after the TLS handshake; the cert is already verified).
pub fn peer_identity_from_tls(
    certs: &[CertificateDer],
    authorized: &AuthorizedKeys,
) -> Option<PeerIdentity> {
    let cert = certs.first()?;
    let pubkey = ed25519_pubkey_from_cert(cert)?;
    let pk = PublicKey::from_bytes(&pubkey, None)?;
    let label = authorized.authorize(&pubkey).unwrap_or_default();
    let fingerprint = pk.fingerprint();
    Some(PeerIdentity {
        label: if label.is_empty() {
            fingerprint.clone()
        } else {
            label
        },
        fingerprint,
        pubkey,
    })
}

// --- Server side: verify client cert against AuthorizedKeys ---

/// Source the client-cert verifier consults on **every** handshake so that
/// `authorize`/`revoke` changes take effect on a running daemon without a
/// restart (I-10). The startup-snapshot model — baking a fixed
/// `Arc<AuthorizedKeys>` into the rustls `ServerConfig` — meant a revoked key
/// stayed trusted until the process bounced, which is unacceptable for a
/// multi-user service (revocation must be timely).
pub trait ClientAuthSource: Send + Sync + std::fmt::Debug {
    /// The currently-trusted key set. May re-read from disk.
    fn snapshot(&self) -> Arc<AuthorizedKeys>;
}

/// Static set — fixed for the lifetime of the config (tests, callers that don't
/// need live reload). Preserves the original `server_config` behavior.
#[derive(Debug)]
pub struct StaticAuth(pub Arc<AuthorizedKeys>);

impl ClientAuthSource for StaticAuth {
    fn snapshot(&self) -> Arc<AuthorizedKeys> {
        self.0.clone()
    }
}

/// File-backed authorized set re-read from disk on **every** handshake, so
/// `authorize`/`revoke` take effect immediately on a running daemon. Collab
/// connections are infrequent, so a small re-parse per handshake is negligible
/// — and "always reflects disk" avoids any cache-staleness window. If the file
/// is missing/unreadable the set is empty (fail-secure: deny).
#[derive(Debug)]
pub struct ReloadingAuthorizedKeys {
    path: PathBuf,
}

impl ReloadingAuthorizedKeys {
    /// Build from a path (contents are read fresh on each handshake).
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }
}

impl ClientAuthSource for ReloadingAuthorizedKeys {
    fn snapshot(&self) -> Arc<AuthorizedKeys> {
        Arc::new(AuthorizedKeys::load(&self.path))
    }
}

#[derive(Debug)]
struct Ed25519ClientVerifier {
    authorized: Arc<dyn ClientAuthSource>,
    provider: Arc<CryptoProvider>,
}

impl ClientCertVerifier for Ed25519ClientVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        let pubkey = ed25519_pubkey_from_cert(end_entity).ok_or_else(app_verify_failure)?;
        // Re-read the trusted set per handshake (live revoke/authorize, I-10).
        if self.authorized.snapshot().authorize(&pubkey).is_some() {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(app_verify_failure())
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

// --- Client side: verify server cert via the HostKeyVerifier (TOFU) ---

#[derive(Debug)]
struct Ed25519ServerVerifier {
    addr: String,
    verifier: Arc<dyn HostKeyVerifier>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Ed25519ServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let pubkey = ed25519_pubkey_from_cert(end_entity).ok_or_else(app_verify_failure)?;
        let pk = PublicKey::from_bytes(&pubkey, None).ok_or_else(app_verify_failure)?;
        if self.verifier.verify(&self.addr, &pk) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(app_verify_failure())
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Build a daemon [`ServerConfig`] requiring mutual auth: the client cert's
/// Ed25519 key must be in `authorized`.
pub fn server_config(
    id: &Identity,
    authorized: Arc<AuthorizedKeys>,
) -> Result<Arc<ServerConfig>, String> {
    server_config_with_auth(id, Arc::new(StaticAuth(authorized)))
}

/// Build a server [`ServerConfig`] whose client-cert verifier consults a
/// file-backed [`ReloadingAuthorizedKeys`], so `authorize`/`revoke` take effect
/// on a running server without a restart (I-10). Use this for the daemon.
pub fn server_config_reloading(
    id: &Identity,
    authorized_keys_path: impl AsRef<Path>,
) -> Result<Arc<ServerConfig>, String> {
    server_config_with_auth(
        id,
        Arc::new(ReloadingAuthorizedKeys::new(authorized_keys_path)),
    )
}

/// Build a server [`ServerConfig`] with an arbitrary [`ClientAuthSource`].
pub fn server_config_with_auth(
    id: &Identity,
    authorized: Arc<dyn ClientAuthSource>,
) -> Result<Arc<ServerConfig>, String> {
    let provider = provider();
    let (cert, key) = cert_and_key(id)?;
    let cfg = ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls versions: {e}"))?
        .with_client_cert_verifier(Arc::new(Ed25519ClientVerifier {
            authorized,
            provider,
        }))
        .with_single_cert(vec![cert], key)
        .map_err(|e| format!("server cert: {e}"))?;
    Ok(Arc::new(cfg))
}

/// Build an editor [`ClientConfig`] that presents `id`'s cert and pins the
/// daemon's key via `verifier` (TOFU / known_hosts).
pub fn client_config(
    id: &Identity,
    addr: String,
    verifier: Arc<dyn HostKeyVerifier>,
) -> Result<Arc<ClientConfig>, String> {
    let provider = provider();
    let (cert, key) = cert_and_key(id)?;
    let cfg = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls versions: {e}"))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Ed25519ServerVerifier {
            addr,
            verifier,
            provider,
        }))
        .with_client_auth_cert(vec![cert], key)
        .map_err(|e| format!("client cert: {e}"))?;
    Ok(Arc::new(cfg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pubkey_roundtrips_through_cert() {
        // THE risk test: a cert built from a known Identity must yield back that
        // identity's exact 32 Ed25519 bytes.
        let id = Identity::generate("framework");
        let (cert, _key) = cert_and_key(&id).unwrap();
        let recovered = ed25519_pubkey_from_cert(&cert).expect("ed25519 pubkey extracted");
        assert_eq!(recovered, id.public().to_bytes(), "pubkey must round-trip");
    }

    #[test]
    fn peer_identity_resolves_label() {
        let dir = std::env::temp_dir().join(format!("mae-tls-pid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let client = Identity::generate("laptop");
        let mut ak = AuthorizedKeys::load(&dir.join("authorized_keys"));
        ak.add(client.public()).unwrap();

        let (cert, _key) = cert_and_key(&client).unwrap();
        let pid = peer_identity_from_tls(&[cert], &ak).unwrap();
        assert_eq!(pid.label, "laptop");
        assert!(pid.is_authenticated());
        assert_eq!(pid.pubkey, client.public().to_bytes());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn configs_build() {
        let dir = std::env::temp_dir().join(format!("mae-tls-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let server_id = Identity::generate("daemon");
        let client_id = Identity::generate("laptop");
        let ak = Arc::new(AuthorizedKeys::load(&dir.join("authorized_keys")));
        assert!(server_config(&server_id, ak).is_ok());

        let verifier = Arc::new(crate::identity::FileHostKeyVerifier::new(
            dir.join("known_hosts"),
            crate::identity::HostKeyPolicy::AcceptNew,
        ));
        assert!(client_config(&client_id, "h:9473".into(), verifier).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- Full in-process mTLS handshake over a real socket ---

    use crate::identity::HostKeyVerifier;

    #[derive(Debug)]
    struct StubHostVerifier {
        accept: bool,
    }
    impl HostKeyVerifier for StubHostVerifier {
        fn verify(&self, _addr: &str, _pk: &PublicKey) -> bool {
            self.accept
        }
    }

    /// Drive one mTLS handshake; return (server_peer_identity, client_result).
    async fn handshake(
        server_id: Identity,
        client_id: Identity,
        authorized: AuthorizedKeys,
        host_accept: bool,
    ) -> (Option<PeerIdentity>, Result<(), String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio_rustls::{TlsAcceptor, TlsConnector};

        let authorized = Arc::new(authorized);
        let scfg = server_config(&server_id, authorized.clone()).unwrap();
        let verifier = Arc::new(StubHostVerifier {
            accept: host_accept,
        });
        let ccfg = client_config(&client_id, SNI.into(), verifier).unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let acceptor = TlsAcceptor::from(scfg);
        let authorized_for_server = authorized.clone();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            match acceptor.accept(stream).await {
                Ok(mut tls) => {
                    let pid = {
                        let (_, conn) = tls.get_ref();
                        conn.peer_certificates()
                            .and_then(|c| peer_identity_from_tls(c, &authorized_for_server))
                    };
                    let _ = tls.write_all(b"ok").await;
                    pid
                }
                Err(_) => None,
            }
        });

        let connector = TlsConnector::from(ccfg);
        let client = tokio::spawn(async move {
            let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
            let server_name = ServerName::try_from(SNI).unwrap();
            match connector.connect(server_name, tcp).await {
                Ok(mut tls) => {
                    // In TLS 1.3 the client cert is verified late, so a server
                    // rejection only surfaces here (alert on the first read).
                    let mut buf = [0u8; 2];
                    tls.read_exact(&mut buf).await.map_err(|e| e.to_string())?;
                    Ok(())
                }
                Err(e) => Err(e.to_string()),
            }
        });

        let (s, c) = tokio::join!(server, client);
        (s.unwrap(), c.unwrap())
    }

    #[tokio::test]
    async fn mtls_authorized_client_succeeds_and_identity_recovered() {
        let dir = std::env::temp_dir().join(format!("mae-mtls-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let server_id = Identity::generate("daemon");
        let client_id = Identity::generate("laptop");
        let mut ak = AuthorizedKeys::load(&dir.join("authorized_keys"));
        ak.add(client_id.public()).unwrap();
        let client_pub = client_id.public().to_bytes();

        let (pid, client_res) = handshake(server_id, client_id, ak, true).await;
        assert!(client_res.is_ok(), "client: {client_res:?}");
        let pid = pid.expect("server recovered peer identity");
        assert_eq!(pid.label, "laptop");
        assert_eq!(pid.pubkey, client_pub);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn mtls_unauthorized_client_rejected() {
        let dir = std::env::temp_dir().join(format!("mae-mtls-no-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Empty authorized_keys → client cert not trusted.
        let ak = AuthorizedKeys::load(&dir.join("authorized_keys"));
        let (pid, client_res) = handshake(
            Identity::generate("daemon"),
            Identity::generate("intruder"),
            ak,
            true,
        )
        .await;
        assert!(
            pid.is_none(),
            "server must not accept an unauthorized client"
        );
        assert!(client_res.is_err(), "client handshake must fail");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// I-10: the daemon's reloading verifier must honor a `revoke` (and an
    /// `authorize`) on the **same** `ServerConfig` — no restart. We build one
    /// config via `server_config_reloading`, run a handshake while the client is
    /// authorized (succeeds), rewrite the file to drop the key, and run a second
    /// handshake on the SAME acceptor (must now be rejected).
    #[tokio::test]
    async fn mtls_reloading_verifier_honors_live_revoke() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio_rustls::{TlsAcceptor, TlsConnector};

        let dir = std::env::temp_dir().join(format!("mae-mtls-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ak_path = dir.join("authorized_keys");

        let server_id = Identity::generate("daemon");
        let client_id = Identity::generate("laptop");

        // Authorize the client on disk, then build ONE reloading server config.
        let mut ak = AuthorizedKeys::load(&ak_path);
        ak.add(client_id.public()).unwrap(); // persists to ak_path
        let scfg = server_config_reloading(&server_id, &ak_path).unwrap();

        // Helper: one handshake against the given acceptor; Ok = client got bytes.
        async fn one(scfg: Arc<ServerConfig>, client_id: &Identity) -> Result<(), String> {
            let acceptor = TlsAcceptor::from(scfg);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                if let Ok((stream, _)) = listener.accept().await {
                    if let Ok(mut tls) = acceptor.accept(stream).await {
                        let _ = tls.write_all(b"ok").await;
                    }
                }
            });
            let verifier = Arc::new(StubHostVerifier { accept: true });
            let ccfg = client_config(client_id, SNI.into(), verifier).unwrap();
            let connector = TlsConnector::from(ccfg);
            let res = async {
                let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
                let mut tls = connector
                    .connect(ServerName::try_from(SNI).unwrap(), tcp)
                    .await
                    .map_err(|e| e.to_string())?;
                let mut buf = [0u8; 2];
                tls.read_exact(&mut buf).await.map_err(|e| e.to_string())?;
                Ok::<(), String>(())
            }
            .await;
            let _ = server.await;
            res
        }

        // 1) Authorized → succeeds.
        assert!(
            one(scfg.clone(), &client_id).await.is_ok(),
            "authorized client should connect"
        );

        // 2) Revoke on disk (rewrite the file without the client's key).
        let mut ak = AuthorizedKeys::load(&ak_path);
        ak.revoke_by_fingerprint(&client_id.public().fingerprint())
            .unwrap();
        assert!(ak.is_empty(), "revoke should empty the file");

        // 3) SAME config, second handshake → rejected, with NO restart.
        assert!(
            one(scfg.clone(), &client_id).await.is_err(),
            "revoked client must be rejected on reconnect without a server restart"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn mtls_client_rejects_untrusted_host() {
        let dir = std::env::temp_dir().join(format!("mae-mtls-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let client_id = Identity::generate("laptop");
        let mut ak = AuthorizedKeys::load(&dir.join("authorized_keys"));
        ak.add(client_id.public()).unwrap();
        // host_accept = false → client aborts on the server cert.
        let (_pid, client_res) =
            handshake(Identity::generate("daemon"), client_id, ak, false).await;
        assert!(
            client_res.is_err(),
            "client must reject an untrusted host key"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
