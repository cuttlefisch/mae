//! The ticket store's properties (ADR-099 B1, B7 "Tickets"). The over-the-wire
//! behaviour — one refusal for unknown/used, a restart voiding every ticket — is
//! in `daemon/tests/ws_ticket_e2e.rs`; the 30-second expiry is pinned here
//! against the real store with an injected clock, where it is exact.

use super::*;

fn grant(principal: &str) -> TicketGrant {
    TicketGrant {
        issuer: "self".into(),
        subject: principal.into(),
        principal: principal.into(),
        token_expires_at: 4_000_000_000,
        origin: Some("https://app.example".into()),
    }
}

#[test]
fn a_ticket_redeems_once_and_carries_its_grant() {
    let store = TicketStore::default();
    let now = Instant::now();
    let t = store.mint(grant("SHA256:alice"), now).unwrap();
    assert_eq!(store.redeem(&t, now), Some(grant("SHA256:alice")));
    assert_eq!(
        store.redeem(&t, now),
        None,
        "a second presentation is refused"
    );
}

#[test]
fn a_ticket_is_valid_for_exactly_thirty_seconds() {
    assert_eq!(TICKET_TTL, Duration::from_secs(30), "B1 fixes the lifetime");
    let store = TicketStore::default();
    let now = Instant::now();
    let early = store.mint(grant("SHA256:alice"), now).unwrap();
    let late = store.mint(grant("SHA256:alice"), now).unwrap();
    let just_inside = now + TICKET_TTL - Duration::from_millis(1);
    assert!(store.redeem(&early, just_inside).is_some());
    assert_eq!(
        store.redeem(&late, now + TICKET_TTL),
        None,
        "expired at 30 s"
    );
}

/// An expired ticket is consumed too: a later presentation, even with a clock
/// that would call it valid, finds nothing.
#[test]
fn an_expired_presentation_still_consumes_the_ticket() {
    let store = TicketStore::default();
    let now = Instant::now();
    let t = store.mint(grant("SHA256:alice"), now).unwrap();
    assert_eq!(store.redeem(&t, now + TICKET_TTL), None);
    assert_eq!(store.redeem(&t, now), None);
}

#[test]
fn tickets_are_256_random_bits_and_never_repeat() {
    let store = TicketStore::default();
    let now = Instant::now();
    let a = store.mint(grant("SHA256:a"), now).unwrap();
    let b = store.mint(grant("SHA256:b"), now).unwrap();
    assert_eq!(a.len(), 43, "32 bytes, base64url without padding: {a}");
    let bytes = data_encoding::BASE64URL_NOPAD.decode(a.as_bytes()).unwrap();
    assert_eq!(bytes.len(), 32);
    assert_ne!(a, b);
}

/// Unknown, used and expired give the same answer, so the caller has one
/// refusal and a probe learns nothing.
#[test]
fn unknown_used_and_expired_are_indistinguishable() {
    let store = TicketStore::default();
    let now = Instant::now();
    let used = store.mint(grant("SHA256:a"), now).unwrap();
    store.redeem(&used, now);
    let expired = store.mint(grant("SHA256:a"), now).unwrap();
    let unknown = "00".repeat(32);
    let later = now + TICKET_TTL;
    assert_eq!(store.redeem(&used, now), None);
    assert_eq!(store.redeem(&expired, later), None);
    assert_eq!(store.redeem(&unknown, now), None);
}

#[test]
fn unused_tickets_are_bounded_per_principal_and_freed_by_use_or_expiry() {
    let store = TicketStore::default();
    let now = Instant::now();
    let held: Vec<String> = (0..MAX_UNUSED_TICKETS_PER_PRINCIPAL)
        .map(|_| store.mint(grant("SHA256:alice"), now).unwrap())
        .collect();
    assert_eq!(
        store.mint(grant("SHA256:alice"), now),
        Err(TooManyTickets),
        "one more unused ticket than the bound is refused"
    );
    assert!(
        store.mint(grant("SHA256:bob"), now).is_ok(),
        "the bound is per principal, not global"
    );
    store.redeem(&held[0], now);
    assert!(
        store.mint(grant("SHA256:alice"), now).is_ok(),
        "use frees a slot"
    );
    assert!(
        store.mint(grant("SHA256:alice"), now + TICKET_TTL).is_ok(),
        "expiry frees every slot"
    );
}
