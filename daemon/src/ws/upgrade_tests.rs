//! Pure pieces of the upgrade: the RFC 6455 accept key and subprotocol parsing.
//! The refusals themselves are on the wire, in `daemon/tests/ws_*_e2e.rs`.

use super::*;

/// RFC 6455 §1.3's own worked example: a wrong accept key makes every client
/// refuse the handshake.
#[test]
fn accept_key_matches_the_rfc_example() {
    assert_eq!(
        accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
        "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    );
}

#[test]
fn subprotocols_are_collected_across_headers_and_commas() {
    let mut h = HeaderMap::new();
    h.append(
        hyper::header::SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static("mae.v1, mae.ticket.abc"),
    );
    h.append(
        hyper::header::SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static("mae.v2"),
    );
    assert_eq!(
        offered_subprotocols(&h),
        vec!["mae.v1", "mae.ticket.abc", "mae.v2"]
    );
}
