# ADR-111: Remote clients reach the daemon over HTTPS, and prove identity at the application layer

**Status:** Accepted (design); phased. P1 is partly built — see "What exists" — and nothing below
claims otherwise.
**Relates to:** ADR-052 (OAuth 2.1 resource server — the listener this makes primary), ADR-098
(durable identity; its OIDC ↔ key binding and group gate become P4 here), ADR-099 (WebSocket sync —
scope **extended** from browsers to every remote client), ADR-025 (P2P mesh — keeps raw transport
identity), ADR-037 (E2E content — where confidentiality from the proxy comes from), ADR-060
(multi-tenant daemon), ADR-097 (browser surface), ADR-105 (KB addressing), ADR-109 (hosted budget).
**Evidence:** `docs/research/111-daemon-exposure-and-client-identity.md`.

## Context

A real deployment put the daemon on a small organisation's container platform: one reverse proxy
fronts every service and terminates TLS with an internal CA; containers may not publish ports; an
OIDC identity provider is becoming the single source of users and groups; tens of users, browsers
later. The daemon's collaboration surface is raw TCP authenticated by **mutual TLS with pinned
Ed25519 keys**, and the key fingerprint *is* the authorization principal. That cannot cross a
TLS-terminating proxy, has no single sign-on, and cannot serve a browser.

The easy answers were each a stopgap: publish the port as an exception, add an L4 passthrough that
seizes :443 for every other service, or run an overlay network. The operator asked for the long-term
shape instead. The research brief tested six options against the strongest evidence against each.

## Decision

### D1. Remote clients use the HTTPS listener; raw-TCP collab is not a remote-client surface

Native editors, headless instances serving AI agents, and browsers reach a daemon through its HTTPS
listener (ADR-052), which may sit behind a TLS-terminating proxy. Raw-TCP mTLS collab and iroh remain
for what they fit: the P2P daemon mesh (ADR-025), same-host or private-network publishers, and an
operator's own tunnel. They are not exposed to end users.

### D2. No security property may depend on the transport being end-to-end

TLS may terminate at a proxy; the upstream to the daemon stays TLS (re-encrypted with a CA-issued
certificate, which the listener already accepts). Anything that must survive a compromised or curious
proxy is secured at the application layer — D3, D4, D5.

### D3. Identity is an OIDC session plus a device key bound to it

A session authenticates with a bearer token from the organisation's IdP (or, without one, a token the
daemon issues itself). Each device keeps its Ed25519 key for signing, and binds it to the OIDC subject
once, by proof of possession — ADR-098 D3's binding table, the Fulcio / Matrix cross-signing shape.
Membership stays owner-authored and signed (ADR-018/026); group claims gate the *session*, never write
membership (ADR-098 D4). Offboarding is group removal → token denial → disconnect → binding revoked.

### D4. Integrity comes from signatures the hub verifies, not from the channel

The hub verifies content-op and membership signatures on every write it accepts. Today it does not
(#727: unsigned content ops are counted and accepted on the hub). Enforcing it — with an exit for
existing unsigned history — is a **prerequisite** for D1 carrying writes, because it is what makes a
proxy unable to forge an edit.

### D5. Confidentiality from the proxy, where required, is end-to-end per KB

A non-E2E KB's content is plaintext to the proxy, which is operated by the same organisation. A KB
that must not be is marked E2E (ADR-037). Transport choice does not change this and is not asked to.

### D6. One sync transport for remote clients: WebSocket on the HTTPS listener

ADR-099's WebSocket design is extended from browsers to every remote client, carrying MAE's existing
doc-scoped envelope. Because tokens expire and connections do not, the server enforces an **in-band
refresh deadline** and closes a connection that misses it — the gap Hocuspocus and Liveblocks leave
open — and sends keepalive pings under proxy idle timeouts.

### D7. Phasing — each phase is part of the end state

- **P1 — read path.** Registering a hub; including it in federated search; a token without collab
  mTLS (an operator-issued, scoped, expiring token over the local socket, which is also the headless
  service-account path); `jwks_url` optional for self-issued-only; truncation reported; the webview's
  `?access_token=` fallback removed.
- **P2 — integrity.** D4; JWKS accepts ES256/EdDSA as well as RS256; a typed principal (ADR-052 D4).
- **P3 — one sync transport.** D6, native and headless clients included.
- **P4 — SSO.** D3: IdP as issuer; PKCE + loopback for desktops, device flow or service token for
  headless; ADR-098 binding and group gate (after #176).
- **P5 — browser and confidentiality.** ADR-100's surface; E2E for sensitive KBs.

## What exists (2026-09)

Built: the HTTPS listener (CA-issued certs, audience from configuration), 10 read-only `kb/query`
methods, self-issued tokens, E2E per KB, the P2P mesh. **Not** built: everything listed under P1–P5.
In particular an MCP client cannot yet search a hub at all — `kb_search` does not query one and the
client feature is off by default. An earlier draft of this ADR's plan said the read path was "mostly
built"; verification said otherwise, and this section exists so that claim is not re-made.

## Consequences

- The platform's single proxy, its certificate authority and its identity provider apply to MAE
  unchanged; MAE needs no port exception and no second ingress.
- Browsers, native editors and headless agents converge on one surface and one identity model.
- The proxy can read non-E2E KB content. That is an explicit trust in the organisation's own
  infrastructure, bounded by D5.
- Long-lived connections carry a refresh protocol and keepalives — real complexity, bought
  deliberately (D6).
- Until P2 lands, the HTTPS listener must stay read-only for untrusted callers: without D4, a
  channel-authenticated write is the only integrity MAE has.

## Rejected

- **L4 passthrough keeping mTLS end-to-end.** Makes one service own :443 for every service; cannot
  route MAE by hostname (fixed SNI); no SSO; no browsers. Syncthing is the cautionary precedent.
- **Proxy-verified client certificates.** Identity becomes a forgeable header behind the proxy.
- **Overlay network for clients.** Excludes browsers and routes around the platform instead of
  through it.
- **P2P relay for clients.** No SSO; browsers only through a relay.
- **mTLS tunnelled inside WebSocket.** Teleport's workaround: triple encryption, extra round trips,
  still no browsers. Kept only as a documented fallback for non-browser clients.
