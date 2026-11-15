//! The size bound on one message between a client and the daemon.
//!
//! A message carries document state as base64, so the bound is derived from
//! the document size limit (ADR-109 D6): one document at the limit always fits
//! one message, in either direction (ADR-022 Amendment 1, E3). A whole KB does
//! not have to fit, because `kb/share` and `kb/join` are paged.

/// Room for a message's JSON around its largest field: ids, keys, state
/// vectors, the collection state riding a `kb/join`.
pub const ENVELOPE_BYTES: usize = 1 << 20;

/// The largest message, for a document size limit of `max_document_bytes`
/// (0 = unlimited): `4 * ceil(limit / 3)` bytes of base64 plus the envelope.
/// Never below the 10 MiB every message was held to before, so nothing that
/// worked stops working under a small document limit; an unlimited daemon
/// gets the bound a native client reads replies with.
pub fn message_bound(max_document_bytes: usize) -> usize {
    if max_document_bytes == 0 {
        return mae_mcp::MAX_DAEMON_RESPONSE_SIZE;
    }
    let base64 = max_document_bytes.div_ceil(3).saturating_mul(4);
    base64
        .saturating_add(ENVELOPE_BYTES)
        .max(mae_mcp::MAX_MESSAGE_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bound must hold what is really on the wire for one document at the
    /// limit: its state through the same base64 encoder every message uses,
    /// plus the envelope. Checked at the stock limit and around it.
    #[test]
    fn one_document_at_the_limit_always_fits() {
        for limit in [10_485_760, 12_000_000, 30_000_001] {
            let state = vec![0xA5u8; limit];
            let on_wire = mae_sync::encoding::update_to_base64(&state).len();
            assert!(
                on_wire + ENVELOPE_BYTES <= message_bound(limit),
                "limit {limit}: {on_wire} B of base64 does not fit {}",
                message_bound(limit)
            );
        }
    }

    /// Never below 10 MiB; unlimited documents get the native reply bound.
    #[test]
    fn the_floor_and_the_unlimited_case() {
        assert_eq!(message_bound(1_000_000), mae_mcp::MAX_MESSAGE_SIZE);
        assert_eq!(message_bound(0), mae_mcp::MAX_DAEMON_RESPONSE_SIZE);
    }
}
