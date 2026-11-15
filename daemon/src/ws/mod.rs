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
//! else is `issuer_not_supported`. Amendment 2 (E1/E2) adds one exception behind
//! `oauth.ws_accept_bound_idp`: an identity-provider token whose `(iss, sub)` is
//! bound to a member key, whose principal is then that fingerprint. Which tokens
//! qualify is decided in one place, `credential`. The session principal is
//! bound as a bearer `PeerIdentity`, so `session/hello`'s principal and
//! `kb/join`'s are the same value.
//!
//! @ai-caution: [security] This is a network-facing WRITE path. Never log a
//! token or a ticket. Every refusal before the upgrade goes through the
//! listener's own response builders (`oauth::unauthorized`, `with_no_store`).

mod connect_ticket;
mod credential;
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
    /// Largest message the daemon sends (`ws_max_outbound_bytes`, else `DocStore::message_bound`).
    pub max_outbound_bytes: usize,
    /// `ws_accept_bound_idp` (ADR-099 Amendment 2, E1).
    pub accept_bound_idp: bool,
    /// `ws_max_idp_session_secs` (Amendment 2, E2); `0` = no cap.
    pub max_idp_session_secs: u64,
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
    if !idp_cap_leaves_room_to_renew(oauth.ws_max_idp_session_secs, oauth.ws_renew_lead_secs) {
        tracing::error!(
            "oauth.ws_max_idp_session_secs must exceed oauth.ws_renew_lead_secs (or be 0): a \
             shorter cap would ask every bound-IdP session to renew as it opens — /ws disabled"
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
            .unwrap_or_else(|| deps.doc_store.message_bound()),
        accept_bound_idp: oauth.ws_accept_bound_idp,
        max_idp_session_secs: oauth.ws_max_idp_session_secs,
    };
    if settings.accept_bound_idp {
        tracing::warn!(
            "oauth.ws_accept_bound_idp: /ws accepts bound identity-provider tokens; until hub \
             signature verification (#727) is enforced their writes are trusted on the channel \
             alone (ADR-099 Amendment 2, E3) — keep token lifetimes short"
        );
    }
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

/// A bound-IdP session cap (`ws_max_idp_session_secs`) must leave room for a
/// renewal: at or under `ws_renew_lead_secs`, `session/renew_needed` would be
/// due the moment each session opens, and again after every renewal.
fn idp_cap_leaves_room_to_renew(cap_secs: u64, renew_lead_secs: u64) -> bool {
    cap_secs == 0 || cap_secs > renew_lead_secs
}

/// Current unix time, seconds.
pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cap at or under the renewal lead would demand a renewal on open; the
    /// default (900 over a 60 s lead) and "no cap" are both fine.
    #[test]
    fn an_idp_session_cap_must_leave_room_to_renew() {
        assert!(idp_cap_leaves_room_to_renew(900, 60));
        assert!(idp_cap_leaves_room_to_renew(61, 60));
        assert!(idp_cap_leaves_room_to_renew(0, 60), "0 is no cap");
        assert!(!idp_cap_leaves_room_to_renew(60, 60));
        assert!(!idp_cap_leaves_room_to_renew(30, 60));
        let d = crate::oauth_config::OAuthConfig::default();
        assert!(idp_cap_leaves_room_to_renew(
            d.ws_max_idp_session_secs,
            d.ws_renew_lead_secs
        ));
    }
}
