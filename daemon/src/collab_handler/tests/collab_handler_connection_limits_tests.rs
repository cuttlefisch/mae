//! #342 regression coverage: the collab TCP accept loop previously had no deadline
//! on completing the auth handshake and no cap on concurrent connections — a client
//! that opened the socket and never sent its hello parked a task+socket forever,
//! with nothing bounding how many could accumulate.
//!
//! The connection-cap enforcement itself lives inline in `main.rs`'s accept loop
//! (no extractable pure-function seam — same class of thing as the write-failure
//! tests elsewhere in this codebase), so this file covers what IS directly
//! testable at the `collab_handler` library level: the handshake timeout via a
//! real, genuinely-silent TCP connection against the real `handle_client_with_auth`.

use super::*;
use mae_mcp::auth::PskAuth;
use std::time::Duration;
use tokio::net::TcpListener;

#[tokio::test]
async fn silent_client_is_dropped_within_the_handshake_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let store = test_doc_store();
    let bc = test_broadcaster();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (r, w) = stream.into_split();
        let reader = tokio::io::BufReader::new(r);
        let auth = PskAuth::new("test-psk");
        crate::collab_handler::handle_client_with_auth(
            reader,
            w,
            &auth,
            store,
            bc,
            std::time::Instant::now(),
            mae_sync::kb::Transport::Hub,
        )
        .await;
    });

    // A real client that connects and then sends NOTHING — the exact scenario
    // #342 fixes. Held alive (not dropped) for the whole wait so the server sees
    // a genuinely stalled connection, not a closed one (which would already
    // return quickly via EOF, without needing the timeout fix at all).
    let _silent_client = tokio::net::TcpStream::connect(addr).await.unwrap();

    // The server task must return on its own within the timeout + a small
    // margin — if the fix regressed (no timeout), this outer wrapper times out
    // and the test fails with a clear message instead of hanging forever.
    let outcome = tokio::time::timeout(
        Duration::from_secs(crate::collab_handler::HANDSHAKE_TIMEOUT_SECS + 5),
        server,
    )
    .await;
    assert!(
        outcome.is_ok(),
        "handle_client_with_auth did not return within HANDSHAKE_TIMEOUT_SECS ({}s) + 5s margin \
         for a client that never sent its hello — the handshake timeout regressed",
        crate::collab_handler::HANDSHAKE_TIMEOUT_SECS
    );
}
