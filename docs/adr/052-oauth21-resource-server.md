# ADR-052: OAuth 2.1 resource-server design for MAE

**Status:** Proposed.
**Extends:** ADR-017 (asymmetric peer auth — adds OAuth as a peer-identity mechanism
alongside PSK and Ed25519 mTLS, not a replacement for either), ADR-018 (identity-anchored
KB access control — feeds, never duplicates, the `kb_access(kb_id, principal, op)`
chokepoint).
**Peer to:** ADR-037 (E2E content encryption — an OAuth-authenticated session remains
subject to the daemon's key-blindness for encrypted KBs; OAuth authenticates identity, it
does not grant plaintext access to content the daemon itself cannot read).
**Closes documented roadmap debt:** `docs/MCP_ARCHITECTURE.md:196` already lists
"OAuth/OIDC (via `initialize` params extension)" as the next unbuilt rung of the auth
roadmap after PSK → Ed25519 mTLS — this ADR is that rung.
**Tracking:** issue #375 (epic tracker); phase issue #381.

## Context

The Model Context Protocol's authorization spec (2025-06-18 revision) requires OAuth 2.1
for any **HTTP-based** MCP transport, but explicitly states **stdio transports should NOT**
implement it (credentials come from the environment instead). An MCP server's role is
purely an OAuth 2.1 **resource server** — it validates bearer tokens; the authorization
server issuing them can be a separate entity. Servers MUST implement RFC 9728 Protected
Resource Metadata (`/.well-known/oauth-protected-resource`, `WWW-Authenticate` on 401),
MUST validate token audience per RFC 8707 Resource Indicators (confused-deputy
protection), and MUST NOT accept or pass through mis-scoped tokens. PKCE is required
client-side; all AS endpoints must be HTTPS.

MAE has no HTTP/SSE transport anywhere today — confirmed by direct code search across
`shared/mcp` and `daemon`. Its existing auth mechanisms are PSK (HMAC-SHA256 mutual
challenge-response, narrowly scoped to AI-residency provider trust per ADR-048) and real
Ed25519-identity mTLS (ADR-017, TLS 1.3 via `rustls`, SSH-style TOFU `known_hosts`/
`authorized_keys`) — the latter is genuinely reusable cryptographic identity
infrastructure, but wired only into the daemon's TCP collab listener, not any
network-facing tool-call surface, and there is no OAuth/OIDC support anywhere.

Rust tooling survey: the official `modelcontextprotocol/rust-sdk` (`rmcp` crate) has OAuth
2.0 support (auth code + PKCE, RFC 8414/7591 metadata/DCR). A third-party
`rmcp-server-kit` already ships OAuth 2.1 Bearer JWT validation against a cached JWKS
endpoint, plus mTLS and Argon2 API-key auth, as ready building blocks. MAE's own
`mae-mcp` crate is hand-rolled JSON-RPC, not built on `rmcp`.

This work is required because ADR-053's live remote KB query surface needs a real
network-authenticated identity for thin/ephemeral clients (VS Code sessions that haven't
joined/replicated a KB) — mTLS alone doesn't fit that use case well, since it requires
pre-provisioning a keypair + `authorized_keys` entry per client rather than the
interactive, per-user consent flow OAuth provides and that VS Code's own MCP client
already natively supports (`oauth` config object in `.vscode/mcp.json`, including
`enterpriseManaged` SSO).

## Decision

1. **A new, dedicated HTTPS listener on `mae-daemon`** — not a retrofit of the existing
   TCP collab listener (which stays mTLS/PSK, unauthenticated-JSON-RPC-over-TCP) and not
   the editor's local Unix socket (stays trust-by-filesystem-permission per
   `SECURITY.md`). This matches the MCP spec's own transport split: stdio should not do
   OAuth at all, so MAE's existing stdio/Unix-socket path is correctly untouched by this
   ADR.
2. **Resource-server posture only.** MAE validates bearer tokens; it does not, by default,
   issue them. RFC 9728 Protected Resource Metadata is served at the well-known path;
   `WWW-Authenticate` on 401 points clients to it; RFC 8707 audience validation is
   mandatory before any tool/query call proceeds.
3. **Adopt `rmcp`/`rmcp-server-kit`** for the PKCE and JWKS-validation machinery rather
   than hand-rolling it. OAuth's security surface (token validation, replay protection,
   audience binding) is exactly the class of code CLAUDE.md principle #14 says should not
   be reinvented without strong reason, and spec-compliant, actively maintained crates
   already exist. `rmcp-server-kit`'s existing OAuth 2.1 Bearer JWT + cached-JWKS support
   is evaluated first; hand-rolling is only justified if its dependency footprint or async
   runtime assumptions conflict with `mae-daemon`'s existing `tokio` setup.
4. **Identity mapping.** A validated OAuth subject (a config-selectable claim — never
   hardcoded to one IdP's claim shape, per principle #7) maps to a **new principal type**
   parallel to `PeerIdentity`'s Ed25519-fingerprint principal, feeding the *same*
   `kb_access`/epoch-fence chokepoints (ADR-018/023) as every other principal kind — OAuth
   is an additional identity **source**, never a parallel authorization system. First-touch
   principal creation follows the same TOFU-adjacent discipline as `authorized_keys`: an
   owner/admin must explicitly grant a role before the mapped principal can do anything
   beyond metadata discovery.
5. **MAE does not host a general-purpose authorization server.** Two supported modes:
   (1) **delegate to an external IdP** (GitHub OAuth App / enterprise IdP — a natural fit
   given VS Code's own `oauth.enterpriseManaged` support) — MAE only ever validates,
   shipped first as lower-risk and matching how most target users already authenticate;
   (2) a **minimal, embedded, single-tenant AS** (RFC 8414 metadata + RFC 7591 dynamic
   client registration, PKCE-only, no password grant) for self-hosted/no-IdP users — an
   explicit fast-follow, gated behind its own adversarial test suite, not bundled into the
   initial cut.
6. **An OAuth-authenticated session's effective permission ceiling is the lowest of** its
   mapped KB role (ADR-018) and any process-configured cap (ADR-051) — never silently
   escalated by the mere fact of successful authentication.

## Consequences

**Positive.** Closes the one gap in MAE's auth roadmap that's a hard prerequisite for any
non-MAE, network-remote client to authenticate at all (mTLS's pre-provisioned-keypair model
doesn't fit an ad hoc VS Code user well). Reuses mature, audited OAuth tooling instead of
hand-rolling a security-critical protocol. Keeps identity mapping strictly additive to the
existing RBAC chokepoint rather than forking authorization logic.

**Costs (honest).** This is genuinely new infrastructure — MAE's first HTTP listener, first
TLS-terminated public-facing endpoint beyond mTLS, and first dependency on an external
crate for security-critical logic. It expands the daemon's attack surface and requires
real operational trust decisions (which IdP, how principal mapping is configured) that
don't have a "correct default" — these need to be config-driven and clearly documented as
security-relevant, not defaulted silently.

## Alternatives rejected

- **Hand-roll OAuth 2.1 token validation to keep `mae-mcp` dependency-pure and stylistically
  consistent.** Rejected — the security surface (replay, audience confusion, PKCE
  downgrade) is exactly what principle #14 says shouldn't be reinvented without strong
  reason, and doing so here would mean MAE's first OAuth implementation is also its least
  battle-tested.
- **Retrofit OAuth onto the existing TCP collab listener instead of a new HTTPS
  listener.** Rejected — that listener's wire format is MAE's own Content-Length JSON-RPC
  framing, not HTTP; grafting bearer-token semantics onto a non-HTTP transport contradicts
  the MCP spec's explicit stdio/non-HTTP exemption and would produce a nonstandard hybrid
  neither generic MCP clients nor MAE's own tooling could reason about cleanly.
- **MAE hosts a full general-purpose authorization server from day one.** Rejected — a
  general AS is a large, security-critical subsystem; delegating to an external IdP first
  gets real usage with far less new attack surface, with a minimal self-hosted AS as a
  scoped fast-follow once the delegation path is proven.

## Verification

- A real OAuth 2.1 authorization-code+PKCE flow against a real external IdP successfully
  authenticates a client and maps to a KB role end-to-end.
- `/.well-known/oauth-protected-resource` is discoverable and correct; a 401 response
  carries a correct `WWW-Authenticate` header.
- **Adversarial tests, all required in default CI (none exist today):** wrong-audience
  token rejected; expired/revoked token rejected; PKCE-downgrade attempt rejected;
  cross-resource-indicator token replay rejected; a token issued for a *different* MCP
  server presented to MAE is rejected (the confused-deputy case the MCP spec names
  explicitly).
