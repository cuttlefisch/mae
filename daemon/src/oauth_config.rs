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
    /// A PEM file of CA certificates the JWKS fetch trusts INSTEAD of the
    /// system's roots. For an image without CA certificates, or an issuer
    /// behind a private CA. Unset: the system's roots.
    pub jwks_ca_file: Option<std::path::PathBuf>,
    /// The authorization server's issuer, checked against each token's
    /// `iss` claim. Strongly recommended to set; `None` skips issuer
    /// validation.
    pub issuer: Option<String>,
    /// Which claim of an external token identifies its subject within its
    /// issuer: with `iss`, the binding table's key (ADR-098 D3). Since ADR-098
    /// Amendment 1 (C1) it never becomes the `kb_access` principal itself; an
    /// external token reaches a member only through a binding.
    pub principal_claim: String,
    /// ADR-098 D4: the claim an external token lists its groups in (a string
    /// array, or one string).
    pub groups_claim: String,
    /// ADR-098 D4: an external token carrying ANY ONE of these groups is
    /// admitted (exact match), checked on every
    /// request before the binding is looked up. Empty (the default): no group
    /// gate. Self-issued tokens carry no groups and are not gated.
    pub allowed_groups: Vec<String>,
    /// ADR-098, for ADR-113 B7: the `amr` values (RFC 8176) that count as a
    /// second factor; any one of them in the token's `amr` meets the rule.
    /// No default, because the values differ by identity provider: empty (the
    /// default) with `second_factor_acr` empty means no sign-in has one, so a
    /// requirement fails closed.
    pub second_factor_amr: Vec<String>,
    /// The `acr` values that count as a second factor (exact match). Some
    /// identity providers emit a constant `acr` that says nothing about the
    /// factors used; prefer `second_factor_amr`. Empty by default.
    pub second_factor_acr: Vec<String>,
    /// ADR-098 Amendment 1, C2: whether `POST /identity/challenge` and `POST
    /// /identity/bind` exist, letting an IdP-authenticated person bind their
    /// subject to a member key by proof of possession. Default `false`
    /// (principle #12). The binding table itself is consulted either way; the
    /// local `mae-daemon binding` CLI administers it regardless.
    pub identity_binding_enabled: bool,
    /// How long a binding challenge stays valid, seconds. Long enough to carry
    /// it from a browser to the program holding the key and back.
    pub identity_challenge_ttl_secs: u64,
    /// Unused binding challenges one subject may hold at once; bounds the
    /// in-memory store against an authenticated caller.
    pub identity_max_pending_challenges: usize,
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
    /// Whether the live KB site (ADR-119 L1a) answers `GET /kb/<kb-id>/<id>` on
    /// this listener: a KB's pages, read-only, to a bearer holder the KB's read
    /// gate admits (the same gate as `kb/query.*`). Default false (principle
    /// #12). Needs a `DocStore`, like `kb_query_enabled`, and is bounded by
    /// `kb_query_max_scan_nodes`.
    pub site_enabled: bool,
    /// The live KB site's locale: `<html lang>` and every string the site
    /// writes (P-16, P-164). Default `en-US`.
    pub site_locale: String,
    /// Namespaces whose references (`ns:rest`) the live site removes from every
    /// page and index (P-13, P-78). Default none.
    pub site_hidden_namespaces: Vec<String>,
    /// Each KB's home page, served at `/kb/<kb-id>/`: KB ID → node ID, under
    /// `[oauth.site_home]` (ADR-119 Amendment 2 B3). Operator configuration,
    /// never content (D3). A KB with no entry, or whose home node the reader
    /// cannot read, gets the tag index there. Default none.
    pub site_home: std::collections::BTreeMap<String, String>,
    /// Each KB's site title (P-16), under `[oauth.site_title]`: KB ID → title.
    /// Shown in every page's `<title>` and in the header linking to the KB's
    /// home. Operator configuration, never content (D3). Never in a refusal:
    /// the not-found is identical for every caller (A5). Default none.
    pub site_title: std::collections::BTreeMap<String, String>,
    /// ADR-115: whether a KB's attachments are served here: `POST
    /// /kb/<kb-id>/blobs` (an editor uploads) and `GET`/`HEAD
    /// /kb/<kb-id>/blobs/<sha256>` (a reader fetches), each gated by the KB's
    /// own access. Default false (principle #12). Needs a `DocStore`, like
    /// `kb_query_enabled`. The store lives under the collab data directory's
    /// `blobs/`.
    pub attachments_enabled: bool,
    /// ADR-115 D3: the largest upload, bytes. A larger declared
    /// `Content-Length` is refused (`413 quota_exceeded`) before any of the
    /// body is read. Default [`DEFAULT_MAX_ATTACHMENT_BYTES`].
    pub max_attachment_bytes: u64,
    /// ADR-115 D3a: an image's width times height, read from its header and
    /// refused above this before anything is decoded. Default 40 megapixels,
    /// the ADR's figure.
    pub max_attachment_pixels: u64,
    /// ADR-115 D3: the longest an upload's body may go without a byte
    /// arriving, seconds. Over it the upload is refused (`408
    /// upload_timeout`), its temporary file deleted and nothing charged.
    /// Default [`DEFAULT_ATTACHMENT_IDLE_TIMEOUT_SECS`]. Must be above 0.
    pub attachment_idle_timeout_secs: u64,
    /// ADR-115 D3: the longest a whole upload body may take to arrive,
    /// seconds, however steadily it trickles (`408 upload_timeout`). Unset
    /// means derived from `max_attachment_bytes`: the time a body at the cap
    /// takes at [`ATTACHMENT_FLOOR_BYTES_PER_SEC`], so raising the cap raises
    /// the deadline with it. Must be above 0 when set.
    pub attachment_upload_deadline_secs: Option<u64>,
    /// ADR-115 D3a: images processed at once. Default
    /// [`DEFAULT_ATTACHMENT_IMAGE_WORKERS`]; see it for the memory each one
    /// costs. Must be above 0.
    pub attachment_image_workers: usize,
    /// ADR-115 D3a: bytes the attachments of principals in no `[[tenant]]`
    /// may total, together. In ADR-108's one-process-per-tenant deployment
    /// no `[[tenant]]` is configured, so this is that tenant's budget;
    /// every principal not listed in a `[[tenant]]` (agents included) is
    /// charged to it, so no uploader is outside a budget. Default
    /// [`crate::config::DEFAULT_MAX_STORAGE_BYTES`], the same as
    /// `[tenant.quota] max_storage_bytes`, for the same reason. `0` is an
    /// explicit opt-out: unlimited, still recorded.
    pub default_storage_bytes: u64,
    /// Each KB's slot components (ADR-119 Amendment 4 H1), under
    /// `[oauth.site_components]`: KB ID → a directory holding
    /// `components.toml` and the files it names. Operator configuration,
    /// never content (D3). Read and checked once, at startup: any problem,
    /// a missing directory included, refuses to start the daemon, naming the
    /// KB, the component and the reason (`mae_daemon::site_components`).
    /// Checked whenever an entry is configured, even with the site off, so a
    /// directory that would fail never waits for the day the site is turned
    /// on. A change takes a restart. Default none.
    pub site_components: std::collections::BTreeMap<String, PathBuf>,
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
    /// Default: the node size backstop, so every node is returned whole
    /// (see `kb_query::DEFAULT_MAX_BODY_BYTES`); set it lower to stop a
    /// live-query-only member reading whole bodies, and `body_truncated`
    /// reports each cut.
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
    /// Deprecated and ignored (ADR-119 Amendment 2 B4): ADR-073's view at
    /// `GET /kb/{kb_id}/view` is retired; the live site (`site_enabled`)
    /// replaces it. Still accepted so an existing config keeps loading, with
    /// a startup warning when set; removed one minor release later.
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
    /// established pattern.
    pub self_issued_tokens_enabled: bool,
    /// Self-issued token lifetime, seconds. Config-driven (principle #7),
    /// never hardcoded — an operator running a long-lived restricted-viewer
    /// integration may want longer than the default; one running a
    /// short-lived CLI probe may want shorter.
    pub self_issued_token_ttl_secs: u64,
    /// Ceiling on the lifetime `mae-daemon token mint --ttl` will issue,
    /// seconds (ADR-111 P1). A self-issued token cannot be revoked
    /// individually -- it lives until it expires or the daemon's identity key
    /// is rotated -- so the operator command refuses, rather than clamps, a
    /// request above this. Also bounds the default above: a configured
    /// `self_issued_token_ttl_secs` greater than this makes `token mint`
    /// without `--ttl` refuse too.
    pub self_issued_token_max_ttl_secs: u64,
    /// ADR-099 Amendment 1, B6: whether `GET /ws` (the sync WebSocket) and
    /// `POST /ws/ticket` exist on this listener at all. Default `false`
    /// (principle #12). While B6 holds this route is a development and
    /// single-operator surface: it accepts ONLY this daemon's own tokens
    /// (`kid = "self"`, so `self_issued_tokens_enabled` is a prerequisite), and
    /// it needs `collab.enabled` — the socket reaches the collab session loop.
    pub ws_enabled: bool,
    /// ADR-099 Amendment 2, E1: whether `/ws` and `/ws/ticket` also accept an
    /// external identity-provider token whose `(iss, sub)` is bound to a member
    /// key (ADR-098). The session's principal is then the bound fingerprint; an
    /// unbound token is refused `identity_not_bound`. Default `false`: only this
    /// daemon's own tokens (B6). Until hub signature verification (#727) is
    /// enforced, a write over such a session is trusted on the channel alone
    /// (E3), so this is for a pilot with short IdP token lifetimes.
    pub ws_accept_bound_idp: bool,
    /// ADR-099 Amendment 2, E2: the longest a bound-IdP socket session runs
    /// before it must renew, seconds, whatever the IdP token's own `exp`. Each
    /// renewal re-reads the binding, so this bounds how long an unbind, a
    /// rebind or a lost group takes to reach an open socket, independently of
    /// the token lifetime the IdP is configured with. Default 900 (15 min):
    /// short enough for the pilot's revocation window, long enough that a
    /// renewal (one validation and one binding lookup per session) every
    /// `900 - ws_renew_lead_secs` seconds costs nothing measurable, and the
    /// client renews with the token it already holds, so a cap shorter than
    /// the token adds no sign-in. Must exceed `ws_renew_lead_secs`, or every
    /// session would be asked to renew as it opens (`/ws` is then disabled
    /// and the reason logged). `0`: no cap, the deadline is the token's `exp`.
    /// Self-issued sessions are not capped.
    pub ws_max_idp_session_secs: u64,
    /// Origins a browser handshake (and a ticket mint) may carry, matched
    /// EXACTLY: no wildcards, no prefix or substring matching (B1). A request
    /// with no `Origin` (a native client) is not origin-checked. Empty means no
    /// browser origin is accepted.
    pub ws_allowed_origins: Vec<String>,
    /// How long a connect ticket from `POST /ws/ticket` stays valid, seconds
    /// (ADR-099 Amendment 1, B1). Default 30, which is also the most allowed:
    /// B1 fixes a ticket at no more than 30 seconds of single-use authority, so
    /// this may only SHORTEN that. A value outside `1..=30` leaves `/ws`
    /// disabled and logs why, like any other misconfiguration of these routes.
    pub ws_ticket_ttl_secs: u64,
    /// Seconds before a session's authentication deadline at which the daemon
    /// sends `session/renew_needed` (B3).
    pub ws_renew_lead_secs: u64,
    /// ADR-111 D6: a WebSocket ping this often, so a proxy's idle timeout
    /// never closes a quiet session. 30 s sits under the 60 s idle timeout
    /// common reverse proxies ship with (nginx `proxy_read_timeout`, HAProxy
    /// `timeout tunnel` examples). `0` disables pings.
    pub ws_ping_secs: u64,
    /// Units one socket session may hold open at once (B4); 0 = unlimited.
    /// A joined KB is ONE unit however many nodes it has (its nodes ride the
    /// join); any other document is one unit. Once a session holds this many,
    /// a request opening a new unit is refused (`too_many_open_docs`).
    pub ws_max_open_docs: usize,
    /// Largest inbound WebSocket frame, bytes; a larger one closes the socket
    /// with `1009` (B4). Unset means `max_request_body_bytes` — the same bound
    /// a request body on this listener has.
    pub ws_max_frame_bytes: Option<usize>,
    /// Largest message the daemon sends on a socket, bytes; a reply over it
    /// closes the socket with `1011` and reason `response_too_large`, never
    /// silently. Unset means derived from `[collab.sync]
    /// max_document_size_bytes` so one document at that limit always fits
    /// (`DocStore::message_bound`). Advertised in `session/hello` as
    /// `limits.max_outbound_bytes`.
    pub ws_max_outbound_bytes: Option<usize>,
}

/// [`OAuthConfig::max_attachment_bytes`]'s default: 10 MiB, the daemon's
/// largest document (`max_document_size_bytes`, ADR-109 D6), which holds a
/// phone photo with room to spare. It is also the most a processing worker
/// reads into memory at once; `the_attachment_defaults_fit_adr_109s_memory_budget`
/// pins what that and the pixel bound cost.
pub const DEFAULT_MAX_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;

/// [`OAuthConfig::max_attachment_pixels`]'s default: ADR-115 D3a's 40
/// megapixels.
pub const DEFAULT_MAX_ATTACHMENT_PIXELS: u64 = 40_000_000;

/// [`OAuthConfig::attachment_idle_timeout_secs`]'s default: 30 seconds. A
/// lossy mobile link's TCP retransmissions back off 1, 2, 4 and 8 seconds
/// before giving up on a segment (15 s in all), so a client that is still
/// there resumes inside this; one that sends nothing for longer has stalled,
/// and holding its connection slot (`max_connections`) and temporary file any
/// longer only serves an attacker holding many. The same figure as the
/// processing timeout (`oauth_blobs::PROCESS_TIMEOUT`): no upload waits on
/// the network longer than it may wait on the CPU.
pub const DEFAULT_ATTACHMENT_IDLE_TIMEOUT_SECS: u64 = 30;

/// The slowest steady upload the derived deadline admits: 64 KiB/s
/// (512 kbit/s), a weak mobile uplink. A body at the default cap then has
/// 160 seconds; anything slower is indistinguishable from a client holding a
/// connection open on purpose.
pub const ATTACHMENT_FLOOR_BYTES_PER_SEC: u64 = 64 * 1024;

/// [`OAuthConfig::attachment_image_workers`]'s default: one. The slowest
/// processing measured (`blob_bench`, a rotated 40-megapixel JPEG) peaks at
/// +229 MiB in 2.5 s; ADR-109 D2 gives a tenant process 8 GB / 20 = 400 MB in
/// all, so a second worker at that peak (~460 MiB together) would pass the
/// process's whole share. Raise it only on a host with memory to match.
pub const DEFAULT_ATTACHMENT_IMAGE_WORKERS: usize = 1;

impl OAuthConfig {
    /// The upload deadline (see
    /// [`OAuthConfig::attachment_upload_deadline_secs`]), seconds: as set,
    /// else the cap's transfer time at the floor throughput, rounded up.
    pub fn attachment_upload_deadline(&self) -> u64 {
        self.attachment_upload_deadline_secs.unwrap_or_else(|| {
            self.max_attachment_bytes
                .div_ceil(ATTACHMENT_FLOOR_BYTES_PER_SEC)
                .max(1)
        })
    }
}

impl Default for OAuthConfig {
    fn default() -> Self {
        OAuthConfig {
            enabled: false,
            bind: "127.0.0.1:9474".parse().unwrap(),
            canonical_resource_uri: String::new(),
            jwks_url: String::new(),
            jwks_ca_file: None,
            issuer: None,
            principal_claim: "sub".to_string(),
            groups_claim: "groups".to_string(),
            allowed_groups: Vec::new(),
            second_factor_amr: Vec::new(),
            second_factor_acr: Vec::new(),
            identity_binding_enabled: false,
            identity_challenge_ttl_secs: 300,
            identity_max_pending_challenges: 4,
            cert_path: PathBuf::new(),
            key_path: PathBuf::new(),
            kb_query_enabled: false,
            site_enabled: false,
            site_locale: "en-US".to_string(),
            site_hidden_namespaces: Vec::new(),
            site_home: std::collections::BTreeMap::new(),
            site_title: std::collections::BTreeMap::new(),
            attachments_enabled: false,
            max_attachment_bytes: DEFAULT_MAX_ATTACHMENT_BYTES,
            max_attachment_pixels: DEFAULT_MAX_ATTACHMENT_PIXELS,
            attachment_idle_timeout_secs: DEFAULT_ATTACHMENT_IDLE_TIMEOUT_SECS,
            attachment_upload_deadline_secs: None,
            attachment_image_workers: DEFAULT_ATTACHMENT_IMAGE_WORKERS,
            default_storage_bytes: crate::config::DEFAULT_MAX_STORAGE_BYTES,
            site_components: std::collections::BTreeMap::new(),
            max_request_body_bytes: 1_048_576,
            kb_query_max_body_bytes: mae_daemon::kb_query::DEFAULT_MAX_BODY_BYTES,
            kb_query_max_scan_nodes: 500,
            kb_query_max_search_results: 20,
            max_connections: 256,
            webview_enabled: false,
            self_issued_tokens_enabled: false,
            self_issued_token_ttl_secs: 3600,
            self_issued_token_max_ttl_secs: 86_400,
            ws_enabled: false,
            ws_accept_bound_idp: false,
            ws_max_idp_session_secs: 900,
            ws_allowed_origins: Vec::new(),
            ws_ticket_ttl_secs: crate::ws::MAX_TICKET_TTL.as_secs(),
            ws_renew_lead_secs: 60,
            ws_ping_secs: 30,
            ws_max_open_docs: 1024,
            ws_max_frame_bytes: None,
            ws_max_outbound_bytes: None,
        }
    }
}

impl OAuthConfig {
    /// The second-factor rule `second_factor_amr`/`second_factor_acr` state.
    pub fn second_factor_rule(&self) -> mae_daemon::sign_in::SecondFactorRule {
        mae_daemon::sign_in::SecondFactorRule {
            amr: self.second_factor_amr.clone(),
            acr: self.second_factor_acr.clone(),
        }
    }
}
