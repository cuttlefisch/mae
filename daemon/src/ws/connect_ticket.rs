//! Connect tickets for browsers (ADR-099 Amendment 1, B1).
//!
//! The browser WebSocket API cannot set `Authorization`, so a browser first
//! calls `POST /ws/ticket` with its bearer header and then presents the ticket
//! in the subprotocol list. A ticket carries 30 seconds of single-use authority
//! in place of the long-lived token, which never travels in a header a proxy can
//! log (the rejected Kubernetes scheme) or in a URL (the rejected Slack shape).
//!
//! Properties, each pinned by a test in `connect_ticket_tests.rs`:
//! - 256 random bits, as 43 base64url characters; valid [`TICKET_TTL`]; single use — removed on its first
//!   presentation whatever happens next;
//! - in memory only, so a restart voids every ticket;
//! - bound to the minting token's issuer, subject, principal and expiry, and to
//!   the mint request's `Origin` when it had one. Tenant resolution is a pure
//!   function of the principal fixed at startup (`TenantRegistry` has no live
//!   reload), so binding the principal binds the tenant;
//! - at most [`MAX_UNUSED_TICKETS_PER_PRINCIPAL`] unused tickets per principal;
//! - unknown, used and expired tickets are indistinguishable (`redeem` returns
//!   `None` for all three, and the caller has one refusal for `None`);
//! - never bound to the client's IP: behind ADR-111's TLS-terminating proxy every
//!   client arrives from the proxy's address, so an IP binding would bind nothing.
//!
//! @ai-caution: [security] Never log a ticket. The store is keyed by the
//! SHA-256 of the ticket, so the raw value is not even held after minting.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// How long a ticket is valid after minting (B1: 30 seconds).
pub(crate) const TICKET_TTL: Duration = Duration::from_secs(30);

/// Unused tickets one principal may hold at once. A browser needs one ticket
/// per connection attempt and a reconnect burst inside 30 seconds needs a few;
/// the bound exists so an authenticated caller cannot grow the store without
/// limit. A refusal is recoverable: a ticket frees its slot on use or expiry.
pub(crate) const MAX_UNUSED_TICKETS_PER_PRINCIPAL: usize = 8;

/// What a redeemed ticket vouches for: the minting token's identity and expiry,
/// and the mint request's `Origin`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TicketGrant {
    pub issuer: String,
    pub subject: String,
    pub principal: String,
    /// The minting token's `exp` (unix seconds): the session's deadline.
    pub token_expires_at: u64,
    pub origin: Option<String>,
}

/// The principal already holds its quota of unused tickets.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TooManyTickets;

struct Entry {
    grant: TicketGrant,
    valid_until: Instant,
}

/// The in-memory ticket store. One per listener.
#[derive(Default)]
pub(crate) struct TicketStore {
    entries: Mutex<HashMap<[u8; 32], Entry>>,
}

fn key(ticket: &str) -> [u8; 32] {
    Sha256::digest(ticket.as_bytes()).into()
}

impl TicketStore {
    /// Mint a ticket for `grant`. Expired entries are purged first, so a
    /// principal's quota counts only tickets that could still be used.
    pub(crate) fn mint(&self, grant: TicketGrant, now: Instant) -> Result<String, TooManyTickets> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|_, e| e.valid_until > now);
        let held = entries
            .values()
            .filter(|e| e.grant.principal == grant.principal)
            .count();
        if held >= MAX_UNUSED_TICKETS_PER_PRINCIPAL {
            return Err(TooManyTickets);
        }
        let mut bytes = [0u8; 32];
        rand::Rng::fill_bytes(&mut rand::rng(), &mut bytes);
        // base64url without padding: 43 characters, every one a valid
        // subprotocol token character (RFC 7230 `tchar`), as the client's
        // script (`tests/data/c-guion-v1.json`) expects.
        let ticket = data_encoding::BASE64URL_NOPAD.encode(&bytes);
        entries.insert(
            key(&ticket),
            Entry {
                grant,
                valid_until: now + TICKET_TTL,
            },
        );
        Ok(ticket)
    }

    /// Present a ticket. It is removed whether or not it is still valid, so
    /// it can never be presented twice. `None` for unknown, used and expired
    /// alike.
    pub(crate) fn redeem(&self, ticket: &str, now: Instant) -> Option<TicketGrant> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = entries.remove(&key(ticket))?;
        (entry.valid_until > now).then_some(entry.grant)
    }
}

#[cfg(test)]
#[path = "connect_ticket_tests.rs"]
mod connect_ticket_tests;
