//! Connect tickets for browsers (ADR-099 Amendment 1, B1).
//!
//! The browser WebSocket API cannot set `Authorization`, so a browser first
//! calls `POST /ws/ticket` with its bearer header and then presents the ticket
//! in the subprotocol list. A ticket carries 30 seconds of single-use authority
//! in place of the long-lived token, which never travels in a header a proxy can
//! log (the rejected Kubernetes scheme) or in a URL (the rejected Slack shape).
//!
//! Properties, each pinned by a test in `connect_ticket_tests.rs`:
//! - 256 random bits, as 43 base64url characters; valid for the store's TTL (at most
//!   [`MAX_TICKET_TTL`], `oauth.ws_ticket_ttl_secs`); single use — removed on its first
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

/// How long a ticket is valid after minting, at most and by default (B1: 30
/// seconds). `oauth.ws_ticket_ttl_secs` may shorten it, never lengthen it.
pub(crate) const MAX_TICKET_TTL: Duration = Duration::from_secs(30);

/// Unused tickets one principal may hold at once. A browser needs one ticket
/// per connection attempt and a reconnect burst inside 30 seconds needs a few;
/// the bound exists so an authenticated caller cannot grow the store without
/// limit. A refusal is recoverable: a ticket frees its slot on use or expiry.
pub(crate) const MAX_UNUSED_TICKETS_PER_PRINCIPAL: usize = 8;

/// What a redeemed ticket vouches for: the minting token's identity and expiry,
/// and the mint request's `Origin`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TicketGrant {
    /// The minting principal's kind, for the activity record (ADR-120 D2).
    pub actor: mae_daemon::activity::event::ActorKind,
    pub issuer: String,
    pub subject: String,
    pub principal: String,
    /// The minting token's `exp` (unix seconds), uncapped. Spending the ticket
    /// (`upgrade::spend_ticket`) refuses it once this has passed, and makes it
    /// the session's deadline, capped there for a bound IdP token (E2).
    pub token_expires_at: u64,
    pub origin: Option<String>,
    /// Minted by a bound identity-provider token: redemption re-reads the
    /// binding (ADR-099 Amendment 2, E1).
    pub bound_idp: bool,
}

/// The principal already holds its quota of unused tickets.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TooManyTickets;

struct Entry {
    grant: TicketGrant,
    valid_until: Instant,
}

/// The in-memory ticket store. One per listener.
pub(crate) struct TicketStore {
    entries: Mutex<HashMap<[u8; 32], Entry>>,
    ttl: Duration,
}

impl Default for TicketStore {
    /// A store whose tickets live [`MAX_TICKET_TTL`].
    fn default() -> Self {
        TicketStore::with_ttl(MAX_TICKET_TTL)
    }
}

fn key(ticket: &str) -> [u8; 32] {
    Sha256::digest(ticket.as_bytes()).into()
}

impl TicketStore {
    /// A store whose tickets live `ttl`. The caller has checked it against
    /// [`MAX_TICKET_TTL`] (`ws::ticket_ttl_from_config`).
    pub(crate) fn with_ttl(ttl: Duration) -> Self {
        TicketStore {
            entries: Mutex::default(),
            ttl,
        }
    }

    /// How long a ticket lives: the mint response's `expires_in`.
    pub(crate) fn ttl(&self) -> Duration {
        self.ttl
    }

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
        // script (`tests/data/c-script-v3.json`) expects.
        let ticket = data_encoding::BASE64URL_NOPAD.encode(&bytes);
        entries.insert(
            key(&ticket),
            Entry {
                grant,
                valid_until: now + self.ttl,
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
