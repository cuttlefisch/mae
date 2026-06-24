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

use iroh::endpoint::presets;
use iroh::{Endpoint, RelayMode, SecretKey};
use mae_mcp::identity::Identity;

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
}
