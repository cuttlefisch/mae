//! OAuth 2.1 resource-server configuration (ADR-052), split out of `config.rs`
//! (pure code motion — that file was over its structural ceiling).
//!
//! Kept together with its `Default` because the two are read as a pair: several
//! fields are security-relevant by their ABSENCE (`issuer: None` disables `iss`
//! validation), so the default is part of the contract rather than a detail.

use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;

/// OAuth 2.1 resource-server configuration (ADR-052). Never on by default
/// (principle #12 — daemon value is earned by an explicit need, not
/// assumed) — an operator opts in by setting `enabled = true` and pointing
/// `jwks_url`/`issuer` at their chosen external authorization server.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OAuthConfig {
    /// Whether the OAuth HTTPS listener starts at all.
    pub enabled: bool,
    /// TCP bind address for the OAuth-protected HTTPS listener — separate
    /// from `collab.bind` (the mTLS/PSK JSON-RPC listener).
    pub bind: SocketAddr,
    /// This server's own canonical resource URI (RFC 8707 `resource` /
    /// RFC 9728 protected-resource identifier). MUST be set by the
    /// operator to a real, stable, externally-reachable URL before
    /// `enabled = true` is meaningful — there is no safe default to infer
    /// this from.
    pub canonical_resource_uri: String,
    /// URL to fetch the authorization server's JWKS from.
    pub jwks_url: String,
    /// The authorization server's issuer, checked against each token's
    /// `iss` claim. Strongly recommended to set; `None` skips issuer
    /// validation.
    pub issuer: Option<String>,
    /// Which JWT claim becomes the mapped `kb_access` principal.
    pub principal_claim: String,
    /// PEM-encoded TLS certificate chain path for the HTTPS listener.
    pub cert_path: PathBuf,
    /// PEM-encoded TLS private key path for the HTTPS listener.
    pub key_path: PathBuf,
    /// ADR-053/Phase G (#382): whether the `kb/query.get`/`search`/`graph`/
    /// `capabilities` RPC family is reachable on this listener at all.
    /// Independently toggleable from `enabled` — an operator may want the
    /// OAuth listener up (e.g. for the plain bearer-verification diagnostic)
    /// without exposing the KB-query surface yet. Default false (principle
    /// #12 — never on by default). Also requires `collab.enabled` (a
    /// `DocStore` must exist to serve from — see `main.rs`'s
    /// `doc_store_for_query` wiring); this flag alone does not create one.
    pub kb_query_enabled: bool,
    /// Cap on the raw size (bytes) of an incoming authenticated request
    /// body, enforced BEFORE it's read into memory at all (`http_body_util
    /// ::Limited`, which errors mid-stream rather than buffering past the
    /// limit) and regardless of `kb_query_enabled` — a validly-authenticated
    /// caller hitting ANY endpoint on this listener must not be able to
    /// force unbounded server-side buffering merely by sending a large body.
    /// Distinct from `kb_query_max_body_bytes` below, which bounds a
    /// *response*'s node content, not the request itself.
    pub max_request_body_bytes: usize,
    /// Cap on a single `kb/query.get` response body's node-body size, bytes
    /// (unencrypted KBs only — an E2E KB's response is raw ciphertext,
    /// capped by nothing since the daemon can't inspect it to truncate it
    /// meaningfully; the op-set itself is already bounded elsewhere).
    /// Prevents a single "get" from being a disguised bulk-content vector.
    pub kb_query_max_body_bytes: usize,
    /// Cap on how many nodes a single `kb/query.search` call will
    /// materialize and scan (unencrypted KBs only) — bounds server-side
    /// cost and is the literal "prevent search from being a disguised
    /// full-dump vector" cap (ADR-053 decision 3), independent of
    /// `kb_query_max_search_results` below (a cap on the *scan*, not just
    /// the returned count).
    pub kb_query_max_scan_nodes: usize,
    /// Cap on the number of results a single `kb/query.search` call returns.
    pub kb_query_max_search_results: usize,
    /// Hard cap on concurrent connections this listener will accept
    /// (ADR-054's `#342` failure class — found missing here via an
    /// independent security review: every OTHER listener in this daemon
    /// gained this cap, but the OAuth HTTPS listener, added later, never
    /// did). Same shape as `collab.max_connections`/`p2p.max_connections`
    /// (`conn_limit::ConnLimiter`, `0` = unlimited).
    pub max_connections: usize,
    /// ADR-073/Phase E (#547): whether `GET /kb/{kb_id}/view` — the live,
    /// network-shareable HTML KB view — is reachable on this listener.
    /// Independently toggleable from `kb_query_enabled` (a sibling
    /// capability on the SAME listener, following that field's own
    /// established pattern exactly), default `false` (principle #12 — a
    /// real capability is earned by an explicit operator opt-in). Also
    /// requires `collab.enabled` for the same reason `kb_query_enabled`
    /// does: this view is a pure new *consumer* of `kb/query.*`, with no
    /// content path of its own.
    pub webview_enabled: bool,
    /// ADR-067 Phase D3: whether this daemon mints AND accepts its own
    /// self-issued bearer tokens (`kid: "self"`, EdDSA-signed with this
    /// daemon's own Ed25519 identity) for the `kb/query.*` surface, instead
    /// of requiring a real external authorization server. What makes a
    /// self-pointing `RemoteHubQueryLayer` (ADR-062) work for a
    /// `QueryOnly`-restricted member with no external AS available. Default
    /// `false` (principle #12) — also requires `collab.auth.mode = "key"`
    /// (an Ed25519 identity must exist to sign with; psk/none auth has
    /// none) and `enabled = true` on this same listener; a daemon with
    /// neither is a silent no-op, not an error, mirroring `kb_query_enabled`
    /// and `webview_enabled`'s own established pattern.
    pub self_issued_tokens_enabled: bool,
    /// Self-issued token lifetime, seconds. Config-driven (principle #7),
    /// never hardcoded — an operator running a long-lived restricted-viewer
    /// integration may want longer than the default; one running a
    /// short-lived CLI probe may want shorter.
    pub self_issued_token_ttl_secs: u64,
}

impl Default for OAuthConfig {
    fn default() -> Self {
        OAuthConfig {
            enabled: false,
            bind: "127.0.0.1:9474".parse().unwrap(),
            canonical_resource_uri: String::new(),
            jwks_url: String::new(),
            issuer: None,
            principal_claim: "sub".to_string(),
            cert_path: PathBuf::new(),
            key_path: PathBuf::new(),
            kb_query_enabled: false,
            max_request_body_bytes: 1_048_576,
            kb_query_max_body_bytes: 65_536,
            kb_query_max_scan_nodes: 500,
            kb_query_max_search_results: 20,
            max_connections: 256,
            webview_enabled: false,
            self_issued_tokens_enabled: false,
            self_issued_token_ttl_secs: 3600,
        }
    }
}
