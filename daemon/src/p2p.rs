//! P2P mesh transport over iroh (ADR-025, Phase 1 / #88).
//!
//! Validated against **iroh v1.0.0** (compiles alongside our rustls/ring/ed25519
//! stack — the ADR-025 integration risk, cleared):
//! - The endpoint's node identity is the daemon's Ed25519 **trusted-peer key**
//!   (`SecretKey::from(Identity::secret_bytes())`), so a peer's `EndpointId` is
//!   exactly the `authorized_keys` principal — no separate P2P identity to manage.
//! - `Connection::open_bi()` / `accept_bi()` yield `SendStream`/`RecvStream` that
//!   impl tokio `AsyncRead`/`AsyncWrite` (iroh's own examples drive them with
//!   `tokio::io::copy`), so they drop into the existing Content-Length framing
//!   (`mae_mcp::{read_message, write_framed}`) behind a `BufReader` — feeding the
//!   same `handle_client_with_auth` the editor's local socket uses.
//! - `Connection::remote_id()` gives the peer's `EndpointId` for the
//!   `authorized_keys` gate at accept time.
//! - `RelayMode::{Default, Custom(RelayMap), Disabled}` covers public relay,
//!   self-hosted relay, and LAN-only (mDNS) — the relay self-host story.
//!
//! This slice provides endpoint construction. The accept loop, peer dialer, and
//! `authorized_keys` gate land in the following Phase-1 steps, gated behind the
//! `collab.p2p` config (#94).

use std::sync::Arc;
use std::time::Instant;

use iroh::endpoint::presets;
use iroh::{Endpoint, RelayMode, SecretKey};
use mae_daemon::collab_handler;
use mae_daemon::doc_store::DocStore;
use mae_mcp::broadcast::SharedBroadcaster;
use mae_mcp::identity::{AuthorizedKeys, Identity, PeerIdentity, PublicKey};
use tracing::{info, warn};

/// ALPN for the MAE collab mesh protocol over iroh/QUIC.
pub const MAE_ALPN: &[u8] = b"mae-sync/0";

/// Bind an iroh endpoint whose node identity is the daemon's Ed25519 trusted-peer
/// key. `relay_mode` selects public relays (`Default`), a self-hosted relay map
/// (`Custom`), or LAN-only with no relay (`Disabled`).
#[allow(dead_code)] // wired into the accept loop + dialer in the next Phase-1 step
pub async fn bind_endpoint(
    identity: &Identity,
    relay_mode: RelayMode,
) -> Result<Endpoint, iroh::endpoint::BindError> {
    let secret_key = SecretKey::from(identity.secret_bytes());
    Endpoint::builder(presets::N0)
        .secret_key(secret_key)
        .alpns(vec![MAE_ALPN.to_vec()])
        .relay_mode(relay_mode)
        .bind()
        .await
}

/// Resolve a connecting peer's **verified** Ed25519 key (`remote_id()`) to a
/// `PeerIdentity`, or `None` if it is not present in `authorized_keys`.
///
/// THIS is the mesh access gate (ADR-025). iroh/QUIC authenticates *which* key
/// dialed us — the connection is cryptographically bound to the peer's secret
/// key — but, unlike the rustls mTLS path (where an unknown client cert is
/// rejected during the handshake by the `ClientAuthSource` verifier), iroh
/// completes the handshake for **any** peer that speaks our ALPN. Membership is
/// therefore ours to enforce here. Label/fingerprint resolution mirrors the
/// mTLS path's `peer_identity_from_tls` so attribution + KB membership see the
/// same identity regardless of transport.
pub(crate) fn authorize_peer(
    pubkey: [u8; 32],
    authorized: &AuthorizedKeys,
) -> Option<PeerIdentity> {
    let entry = authorized.authorize_full(&pubkey)?;
    let fingerprint = entry.fingerprint();
    let label = entry
        .label
        .as_deref()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fingerprint.clone());
    Some(PeerIdentity {
        label,
        fingerprint,
        pubkey,
    })
}

/// Accept loop for the mesh endpoint. Each inbound connection is gated on its
/// `remote_id()` being in `authorized_keys` (see [`authorize_peer`]); authorized
/// peers are handed to the **same** `handle_client_authenticated` the editor's
/// local socket and the TCP collab listener use, over the bidirectional stream
/// the peer opens — the QUIC `RecvStream`/`SendStream` drop straight into the
/// existing Content-Length framing (proven by `two_endpoints_round_trip_*`).
///
/// This is the Phase-1 transport adapter: one bi stream per connection, request/
/// response like the TCP path. Mesh multiplexing/gossip is Phase 2 (#89).
#[allow(dead_code)] // wired into daemon startup behind `collab.p2p` (#94)
pub async fn serve(
    endpoint: Endpoint,
    authorized: Arc<AuthorizedKeys>,
    doc_store: Arc<DocStore>,
    broadcaster: SharedBroadcaster,
    start_time: Instant,
) {
    while let Some(incoming) = endpoint.accept().await {
        let authorized = Arc::clone(&authorized);
        let doc_store = Arc::clone(&doc_store);
        let broadcaster = broadcaster.clone();
        tokio::spawn(async move {
            // Complete the QUIC handshake.
            let conn = match incoming.accept() {
                Ok(accepting) => match accepting.await {
                    Ok(conn) => conn,
                    Err(e) => {
                        warn!(error = %e, "mesh connection failed to establish");
                        return;
                    }
                },
                Err(e) => {
                    warn!(error = %e, "mesh accept rejected");
                    return;
                }
            };

            // Gate on the peer's verified key being authorized.
            let pubkey = *conn.remote_id().as_bytes();
            let Some(peer) = authorize_peer(pubkey, &authorized) else {
                let fp = PublicKey::from_bytes(&pubkey, None)
                    .map(|k| k.fingerprint())
                    .unwrap_or_default();
                warn!(fingerprint = %fp, "rejecting mesh peer absent from authorized_keys");
                conn.close(1u32.into(), b"unauthorized");
                return;
            };
            info!(peer = %peer.label, "mesh peer authenticated");

            // The dialing peer opens one bi stream; feed it to the shared handler.
            let (send, recv) = match conn.accept_bi().await {
                Ok(streams) => streams,
                Err(e) => {
                    warn!(peer = %peer.label, error = %e, "mesh peer opened no stream");
                    return;
                }
            };
            collab_handler::handle_client_authenticated(
                tokio::io::BufReader::new(recv),
                send,
                peer,
                doc_store,
                broadcaster,
                start_time,
            )
            .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mesh endpoint's node identity IS the daemon's trusted-peer Ed25519 key,
    /// so a peer's `EndpointId` equals its `authorized_keys` principal (ADR-025).
    /// This is the load-bearing identity-reuse claim of the iroh integration.
    #[tokio::test]
    async fn endpoint_identity_is_the_trusted_peer_key() {
        let id = Identity::generate("daemon");
        let ep = bind_endpoint(&id, RelayMode::Disabled)
            .await
            .expect("endpoint binds on a free port");
        assert_eq!(
            ep.secret_key().public().as_bytes(),
            &id.public().to_bytes(),
            "iroh endpoint identity must equal the daemon's Ed25519 trusted-peer key"
        );
        ep.close().await;
    }

    /// The mesh access gate (ADR-025): a key in `authorized_keys` resolves to a
    /// real, principal-bearing `PeerIdentity`; any other key is rejected — even
    /// though iroh would have happily completed the QUIC handshake for it. Uses
    /// freshly generated keys + their real fingerprints (no hardcoded values).
    #[test]
    fn authorize_peer_admits_only_authorized_keys() {
        let dir = tempfile::tempdir().unwrap();
        let ak_path = dir.path().join("authorized_keys");

        let trusted = Identity::generate("peer-a");
        let trusted_pub = trusted.public().to_bytes();
        let mut ak = AuthorizedKeys::load(&ak_path);
        ak.add(PublicKey::from_bytes(&trusted_pub, Some("peer-a".to_string())).unwrap())
            .unwrap();

        // Authorized → admitted, carrying its label + a real principal (the
        // fingerprint), exactly like the mTLS path.
        let peer = authorize_peer(trusted_pub, &ak).expect("trusted key is admitted");
        assert_eq!(peer.label, "peer-a");
        assert_eq!(peer.pubkey, trusted_pub);
        assert!(peer.is_authenticated());
        assert_eq!(peer.principal(), Some(peer.fingerprint.as_str()));

        // A different, untrusted key → rejected by the gate.
        let stranger = Identity::generate("stranger").public().to_bytes();
        assert!(
            authorize_peer(stranger, &ak).is_none(),
            "a key absent from authorized_keys must be rejected by the mesh gate"
        );
    }

    /// End-to-end transport proof: two endpoints connect over iroh and round-trip a
    /// **Content-Length-framed** mae_mcp message through `open_bi`/`accept_bi`
    /// streams — confirming the QUIC streams drop into the existing framing, and the
    /// acceptor sees the connector's `EndpointId` == its trusted-peer key (the
    /// `authorized_keys` gate input).
    #[tokio::test]
    async fn two_endpoints_round_trip_a_framed_message() {
        use iroh::{EndpointAddr, TransportAddr};
        use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
        use std::time::Duration;
        use tokio::io::BufReader;

        let acc_id = Identity::generate("acceptor");
        let con_id = Identity::generate("connector");
        let con_pub = con_id.public().to_bytes();

        let acceptor = bind_endpoint(&acc_id, RelayMode::Disabled).await.unwrap();

        // Dial by EXPLICIT direct address — no relay, no discovery. With
        // `RelayMode::Disabled` there is no relay home and DNS/Pkarr discovery
        // never resolves on localhost, so `online()`/`addr()` would block forever
        // (a 20-min hang in CI). Instead, hand the connector the acceptor's bound
        // UDP socket(s) directly, rewriting the unspecified bind IP (0.0.0.0 / [::])
        // to loopback so it is actually dialable. Compute this BEFORE moving the
        // acceptor into the accept task.
        let acc_addr = EndpointAddr::from_parts(
            acceptor.id(),
            acceptor
                .bound_sockets()
                .into_iter()
                .map(|mut sa: SocketAddr| {
                    if sa.ip().is_unspecified() {
                        sa.set_ip(if sa.is_ipv4() {
                            Ipv4Addr::LOCALHOST.into()
                        } else {
                            Ipv6Addr::LOCALHOST.into()
                        });
                    }
                    TransportAddr::Ip(sa)
                }),
        );

        // Acceptor: accept one connection, read a framed message, echo it back.
        let server = tokio::spawn(async move {
            let incoming = acceptor.accept().await.expect("incoming connection");
            let conn = incoming
                .accept()
                .expect("accept")
                .await
                .expect("connection established");
            let peer = conn.remote_id();
            let (mut send, recv) = conn.accept_bi().await.expect("accept_bi");
            let mut reader = BufReader::new(recv);
            let msg = mae_mcp::read_message(&mut reader)
                .await
                .expect("read")
                .expect("a framed message");
            mae_mcp::write_framed(&mut send, msg.as_bytes(), Duration::from_secs(5))
                .await
                .expect("echo write");
            send.finish().expect("finish");
            conn.closed().await;
            peer
        });

        // Connector: dial, open a bi stream, write a framed message, read the echo.
        let connector = bind_endpoint(&con_id, RelayMode::Disabled).await.unwrap();
        let conn = tokio::time::timeout(
            Duration::from_secs(20),
            connector.connect(acc_addr, MAE_ALPN),
        )
        .await
        .expect("relay-less direct dial must not hang")
        .expect("dial acceptor");
        let (mut send, recv) = conn.open_bi().await.expect("open_bi");
        let payload = r#"{"jsonrpc":"2.0","method":"$/ping"}"#;
        mae_mcp::write_framed(&mut send, payload.as_bytes(), Duration::from_secs(5))
            .await
            .expect("write");
        send.finish().expect("finish");
        let mut reader = BufReader::new(recv);
        let echo = mae_mcp::read_message(&mut reader)
            .await
            .expect("read echo")
            .expect("a framed echo");

        assert_eq!(
            echo, payload,
            "framed message round-trips over iroh streams"
        );

        // Signal a graceful close so the acceptor's `conn.closed().await` resolves
        // (iroh's canonical client/server echo handshake — `close` only queues the
        // CONNECTION_CLOSE; the acceptor was holding the connection open for it).
        conn.close(0u32.into(), b"bye");

        let peer = tokio::time::timeout(Duration::from_secs(20), server)
            .await
            .expect("acceptor task must not hang")
            .unwrap();
        assert_eq!(
            peer.as_bytes(),
            &con_pub,
            "acceptor sees the connector's EndpointId = its trusted-peer key"
        );
    }
}
