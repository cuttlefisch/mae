//! The sync WebSocket on the OAuth HTTPS listener (ADR-099, Amendment 1).
//!
//! `GET /ws` upgrades to a WebSocket carrying MAE's own JSON-RPC envelope, one
//! message per text frame, multiplexing any number of documents (D2). `POST
//! /ws/ticket` mints the single-use connect ticket a browser presents instead of
//! a bearer header (B1).
//!
//! The socket reaches the SAME session loop and dispatch every collab transport
//! uses (`collab_handler::handle_client_governed` → `run_session`), through an
//! in-memory byte pipe in `session`: each text frame becomes one framed message
//! on the pipe, and each framed message the session writes becomes one text
//! frame. So every `sync/*` and `kb/*` method keeps its one authorization path
//! and per-frame authorization (B4) is inherited, not re-implemented. What this
//! module adds is only what the transport owns: the upgrade and its credential
//! (B1/B2), the renewal deadline (B3), the frame cap, and the B6 gate.
//!
//! B6, while it holds: off by default (`oauth.ws_enabled`), and only
//! daemon-issued tokens (`kid = "self"`) are accepted on both routes — anything
//! else is `issuer_not_supported`. The session principal is the token subject,
//! bound as a bearer `PeerIdentity`, so `session/hello`'s principal and
//! `kb/join`'s are the same value.
//!
//! @ai-caution: [security] This is a network-facing WRITE path. Never log a
//! token or a ticket. Every refusal before the upgrade goes through the
//! listener's own response builders (`oauth::unauthorized`, `with_no_store`).

mod connect_ticket;
mod session;
mod upgrade;

use std::time::Duration;

use mae_daemon::collab_handler::SessionDeps;

pub(crate) use connect_ticket::TicketStore;
pub(crate) use upgrade::handle;

/// The one protocol version this daemon speaks (B2).
pub(crate) const PROTOCOL_V1: &str = "mae.v1";
/// Every version this daemon speaks, as `protocol_unsupported` lists them.
pub(crate) const SUPPORTED_PROTOCOLS: &[&str] = &[PROTOCOL_V1];

/// Is `path` one of this module's routes?
pub(crate) fn is_ws_path(path: &str) -> bool {
    path == "/ws" || path == "/ws/ticket"
}

/// The `[oauth] ws_*` settings, resolved (see `OAuthConfig` for each).
#[derive(Debug, Clone)]
pub(crate) struct WsSettings {
    pub allowed_origins: Vec<String>,
    pub renew_lead: Duration,
    /// Keepalive ping period (`None` = no pings), ADR-111 D6.
    pub ping_every: Option<Duration>,
    pub max_open_docs: usize,
    pub max_frame_bytes: usize,
    /// Largest message the daemon sends; see [`outbound_bound`].
    pub max_outbound_bytes: usize,
}

/// Room for a response's JSON around its largest field: ids, keys, state
/// vectors, the collection state riding a `kb/join`.
const OUTBOUND_ENVELOPE_BYTES: usize = 1 << 20;

/// The default largest message the daemon sends on a socket, for a document
/// size limit of `max_document_bytes` (0 = unlimited).
///
/// A reply carries document state base64-encoded, so one document at the limit
/// is `4 * ceil(limit / 3)` bytes on the wire plus its envelope; the bound must
/// hold that, or the largest document a member may legitimately write cannot be
/// fetched. Never below the 10 MiB every reply was held to before (a join that
/// worked must keep working), and an unlimited daemon gets the bound a native
/// client reads with.
pub(crate) fn outbound_bound(max_document_bytes: usize) -> usize {
    if max_document_bytes == 0 {
        return mae_mcp::MAX_DAEMON_RESPONSE_SIZE;
    }
    let base64 = max_document_bytes.div_ceil(3).saturating_mul(4);
    base64
        .saturating_add(OUTBOUND_ENVELOPE_BYTES)
        .max(mae_mcp::MAX_MESSAGE_SIZE)
}

/// Everything the routes need beyond the listener's own state.
pub(crate) struct WsState {
    pub settings: WsSettings,
    pub tickets: TicketStore,
    pub deps: SessionDeps,
}

/// The routes' state, when `ws_enabled` and its prerequisites hold. A missing
/// prerequisite is logged and leaves the routes absent: a socket on which no
/// token could ever authenticate, or with no session loop to reach, is a
/// misconfiguration to report, not a listener to start.
pub(crate) fn state_from_config(
    oauth: &crate::oauth_config::OAuthConfig,
    deps: Option<SessionDeps>,
    self_issue_available: bool,
) -> Option<WsState> {
    if !oauth.ws_enabled {
        return None;
    }
    let Some(deps) = deps else {
        tracing::error!("oauth.ws_enabled needs collab.enabled (the socket reaches the collab session loop) — /ws disabled");
        return None;
    };
    if !self_issue_available {
        tracing::error!(
            "oauth.ws_enabled accepts only self-issued tokens (ADR-099 B6), which are unavailable \
             (oauth.self_issued_tokens_enabled and collab.auth.mode = \"key\") — /ws disabled"
        );
        return None;
    }
    let settings = WsSettings {
        allowed_origins: oauth.ws_allowed_origins.clone(),
        renew_lead: Duration::from_secs(oauth.ws_renew_lead_secs),
        ping_every: (oauth.ws_ping_secs > 0).then(|| Duration::from_secs(oauth.ws_ping_secs)),
        max_open_docs: oauth.ws_max_open_docs,
        max_frame_bytes: oauth
            .ws_max_frame_bytes
            .unwrap_or(oauth.max_request_body_bytes),
        max_outbound_bytes: oauth
            .ws_max_outbound_bytes
            .unwrap_or_else(|| outbound_bound(deps.doc_store.max_document_size())),
    };
    tracing::info!(
        origins = settings.allowed_origins.len(),
        "sync WebSocket enabled on the OAuth listener (/ws)"
    );
    Some(WsState {
        settings,
        tickets: TicketStore::default(),
        deps,
    })
}

/// Current unix time, seconds.
pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod outbound_bound_tests {
    use super::*;

    /// The bound must hold what the daemon really puts on the wire for one
    /// document at the limit: its state through the same base64 encoder every
    /// reply uses, plus the envelope. Checked at the stock limit and around it.
    #[test]
    fn one_document_at_the_limit_always_fits() {
        for limit in [10_485_760, 12_000_000, 30_000_001] {
            let state = vec![0xA5u8; limit];
            let on_wire = mae_sync::encoding::update_to_base64(&state).len();
            assert!(
                on_wire + OUTBOUND_ENVELOPE_BYTES <= outbound_bound(limit),
                "limit {limit}: {on_wire} B of base64 does not fit {}",
                outbound_bound(limit)
            );
        }
    }

    /// Never below the 10 MiB every reply was held to before, so a join that
    /// worked keeps working under a small document limit; unlimited documents
    /// get the bound a native client reads with.
    #[test]
    fn the_floor_and_the_unlimited_case() {
        assert_eq!(outbound_bound(1_000_000), mae_mcp::MAX_MESSAGE_SIZE);
        assert_eq!(outbound_bound(0), mae_mcp::MAX_DAEMON_RESPONSE_SIZE);
    }
}
