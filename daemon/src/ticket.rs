//! P2P KB-join tickets — the shareable "magnet link" bootstrap token (ADR-025).
//!
//! A ticket bundles the owner daemon's iroh [`EndpointAddr`] (node-id = the
//! `authorized_keys` principal, plus relay + direct-address routing hints) with
//! the KB-id to join. It is everything a new peer needs to *reach* the owner —
//! even behind NAT, via the relay, before any DNS/Pkarr propagation — and to
//! know *which* KB it is joining.
//!
//! iroh-base 1.0 dropped the built-in node ticket, but `EndpointAddr` is
//! serde-serializable, so the ticket is a small MAE construct — which also lets
//! us carry the KB-id a bare iroh ticket couldn't. Wire form:
//! `mae://join/<base32-nopad(json(JoinTicket))>`.
//!
//! **Security (ADR-025):** the addresses in a ticket are **routing hints only**.
//! Trust comes from the node-id — the dialer verifies the connection's
//! `remote_id()` against `authorized_keys` ([`crate::p2p::authorize_peer`]), so a
//! tampered or spoofed address cannot impersonate the owner; at worst it wastes a
//! dial. In the Phase-2 model the joiner still lands in the owner's pending-approve
//! queue, so a leaked ticket is not by itself a grant.

// The mint/parse primitive lands here first; it is wired into the daemon
// JSON-RPC (`p2p/mint_ticket`, `p2p/join_ticket`) + the editor `kb-share --p2p`
// / `kb-join <ticket>` UX in the following slice.
#![allow(dead_code)]

use std::fmt;
use std::str::FromStr;

use data_encoding::BASE32_NOPAD;
use iroh::{EndpointAddr, EndpointId};
use serde::{Deserialize, Serialize};

/// The scheme prefix carried by a textual ticket.
const TICKET_PREFIX: &str = "mae://join/";

/// A shareable KB-join bootstrap token (the "magnet link").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinTicket {
    /// The owner daemon's address: node-id (= `authorized_keys` principal) plus
    /// relay URL + known direct addrs. The node-id is the identity; the
    /// addresses are reachability hints only.
    pub endpoint: EndpointAddr,
    /// The KB the joiner is being invited into.
    pub kb_id: String,
}

impl JoinTicket {
    /// Build a ticket for `kb_id` reachable at `endpoint`.
    pub fn new(endpoint: EndpointAddr, kb_id: impl Into<String>) -> Self {
        Self {
            endpoint,
            kb_id: kb_id.into(),
        }
    }

    /// The owner's node-id — the Ed25519 fingerprint principal the dialer must
    /// verify `remote_id()` against. Addresses are never trusted on their own.
    pub fn node_id(&self) -> EndpointId {
        self.endpoint.id
    }
}

impl fmt::Display for JoinTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // JSON keeps the wire form stable across iroh patch versions (field
        // names, not field order); base32-nopad makes it case-insensitive and
        // QR/URL-friendly. A tampered body simply fails to decode or to dial.
        let json = serde_json::to_vec(self).map_err(|_| fmt::Error)?;
        write!(f, "{TICKET_PREFIX}{}", BASE32_NOPAD.encode(&json))
    }
}

/// Why a textual ticket failed to parse.
#[derive(Debug, PartialEq, Eq)]
pub enum TicketParseError {
    /// Missing the `mae://join/` scheme prefix.
    BadScheme,
    /// The body is not valid base32 (no-pad).
    BadBase32,
    /// The decoded bytes are not a valid ticket.
    BadPayload,
}

impl fmt::Display for TicketParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            TicketParseError::BadScheme => "not a mae://join/ ticket",
            TicketParseError::BadBase32 => "ticket body is not valid base32",
            TicketParseError::BadPayload => "ticket payload is malformed",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for TicketParseError {}

impl FromStr for JoinTicket {
    type Err = TicketParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let body = s
            .trim()
            .strip_prefix(TICKET_PREFIX)
            .ok_or(TicketParseError::BadScheme)?;
        let bytes = BASE32_NOPAD
            .decode(body.as_bytes())
            .map_err(|_| TicketParseError::BadBase32)?;
        serde_json::from_slice(&bytes).map_err(|_| TicketParseError::BadPayload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::{SecretKey, TransportAddr};
    use mae_mcp::identity::Identity;
    use std::net::{Ipv4Addr, SocketAddr};

    /// Build an `EndpointAddr` from a generated trusted-peer identity + one
    /// direct address, so the node-id is a real Ed25519 key (no hardcoded bytes).
    fn sample_endpoint(label: &str, port: u16) -> EndpointAddr {
        let id = Identity::generate(label);
        let node_id = SecretKey::from(id.secret_bytes()).public();
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        EndpointAddr::from_parts(node_id, [TransportAddr::Ip(addr)])
    }

    #[test]
    fn ticket_round_trips_through_text() {
        let endpoint = sample_endpoint("owner", 9473);
        let ticket = JoinTicket::new(endpoint.clone(), "concept:architecture");

        let encoded = ticket.to_string();
        assert!(
            encoded.starts_with("mae://join/"),
            "ticket carries the mae://join/ scheme: {encoded}"
        );

        let parsed: JoinTicket = encoded.parse().expect("a minted ticket re-parses");
        assert_eq!(parsed, ticket, "round-trip preserves the whole ticket");
        assert_eq!(parsed.kb_id, "concept:architecture");
        // The node-id (identity) survives intact — it is what the dialer verifies.
        assert_eq!(parsed.node_id(), endpoint.id);
    }

    #[test]
    fn parse_rejects_malformed_tickets() {
        // Wrong scheme.
        assert_eq!(
            "https://example.org/x".parse::<JoinTicket>().unwrap_err(),
            TicketParseError::BadScheme
        );
        // Right scheme, body not base32 (`1`/`8`/`9` are not in the base32 alphabet).
        assert_eq!(
            "mae://join/8189".parse::<JoinTicket>().unwrap_err(),
            TicketParseError::BadBase32
        );
        // Right scheme, valid base32, but not a ticket payload.
        let junk = BASE32_NOPAD.encode(b"not a ticket");
        assert_eq!(
            format!("mae://join/{junk}")
                .parse::<JoinTicket>()
                .unwrap_err(),
            TicketParseError::BadPayload
        );
    }

    #[test]
    fn distinct_owners_produce_distinct_tickets() {
        let a = JoinTicket::new(sample_endpoint("owner-a", 9473), "kb:x");
        let b = JoinTicket::new(sample_endpoint("owner-b", 9473), "kb:x");
        assert_ne!(a.node_id(), b.node_id(), "different identities");
        assert_ne!(a.to_string(), b.to_string(), "different tickets");
    }
}
