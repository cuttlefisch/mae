# ADR-111 Phase 0: how remote clients should reach the daemon — prior art and code facts

**Purpose.** Ground ADR-111 in published practice and in what the code actually does, *before*
building on it. Briefed for **refutation**: for every option, look for the strongest evidence that it
fails, not for citations that it works.

**The question.** A MAE daemon deployed on a small organisation's container platform — one reverse
proxy fronting every service and terminating TLS with an internal CA, no directly published container
ports, an OIDC identity provider becoming the single source of users and groups, tens of users,
browsers later. How should native editors, headless instances serving AI agents over MCP, and
browsers reach the daemon, and how do they prove who they are?

**Verdict up front.** Authenticate at the **application layer** (OIDC session + device keys bound to
that identity, with the hub verifying op signatures), carried over **HTTPS/WebSocket through the
proxy**. Transport-bound mutual TLS on a raw TCP port — what MAE's collab listener does today — is the
one shape the environment is built to exclude, and the prior art that most closely matches MAE's
situation abandoned it for exactly that reason.

---

## 1. Code facts (verified in the tree, 2026-09; ADR status lines were not trusted)

| Surface | State | Client identity |
|---|---|---|
| Collab TCP (`collab.bind`) | built | mTLS; server cert self-signed from the Ed25519 identity; **the client key fingerprint is the authorization principal** |
| OAuth HTTPS listener (ADR-052) | built; TLS mandatory; any PEM chain (a CA-issued cert works) | bearer JWT (external JWKS, **RSA only**) or a self-issued EdDSA token; audience checked against the configured resource URI, not the request `Host` |
| `kb/query.*` on that listener | built, **read-only**, 10 methods | as above; serves KBs the daemon holds as collab-shared collections |
| Webview (ADR-073) | built | bearer header **or `?access_token=` in the URL** (lands in proxy access logs) |
| SSE push (ADR-074) | design only | — |
| WebSocket sync (ADR-099) | design only; browser-scoped | bearer on upgrade |
| P2P / iroh (ADR-025) | built | node id = Ed25519 identity |

- The collab client sends a **fixed SNI** (`mae-daemon`) and verifies the server by a TOFU-pinned
  key, keyed by address. It has no proxy support. So even L4 passthrough cannot route by hostname.
- **Content-op signing is not enforced on the hub** (`require_signed_content_ops` defaults to false;
  unsigned ops are counted and accepted — #727). Membership ops are signed by the daemon.
- E2E content encryption (ADR-037) is built but **opt-in per KB**; for a non-E2E KB a TLS-terminating
  proxy sees plaintext.
- ADR-098's OIDC-principal ↔ key binding and group-claim session gate are **design only**, blocked on
  #176.

**What the read path from an MCP client to a hub is missing** (measured, not assumed — an earlier
draft called it "mostly built" and was wrong): `kb_search` never queries a remote hub; the
`remote-hub` feature is off by default; the daemon's own query layer shadows a registered hub; there
is no command to register a hub URL; **no token can be obtained without the collab mTLS connection**;
`jwks_url` is required even when only self-issued tokens are used; hub search scans at most 500 nodes
and does not say when it truncated.

## 2. Options, and the strongest evidence against each

| | A. L4 passthrough (keep mTLS) | B. HTTPS/WebSocket + OIDC + signed ops | C. Proxy verifies client certs | D. Overlay network | E. P2P + relay | F. mTLS tunnelled in WebSocket |
|---|---|---|---|---|---|---|
| Crosses a TLS-terminating proxy | only if the proxy's `stream` layer owns :443 | yes | yes | bypasses it | relay's HTTP side only | yes |
| SSO | none | native | only via OIDC-issued certs | overlay login, not app login | none | only via OIDC-issued certs |
| Offboarding | edit key file | token denial + disconnect | cert lifetime | node-key expiry | edit key list | cert lifetime |
| Integrity if the proxy is compromised | strong | strong **iff the hub verifies every op signature** | weak (trusts proxy headers) | strong | strong | strong |
| Browsers | no | yes | poor | no | relay only | no |

- **A.** `ssl_preread` sees only the ClientHello, so sharing :443 means the `stream` block takes the
  port for *every* service and HTTP servers move behind PROXY protocol just to keep client IPs
  ([nginx ssl_preread](https://nginx.org/en/docs/stream/ngx_stream_ssl_preread_module.html),
  [PROXY protocol](https://docs.nginx.com/nginx/admin-guide/load-balancer/using-proxy-protocol/)).
  Teleport found most TLS-terminating load balancers drop custom ALPN, SNI and mTLS
  ([Teleport TLS routing](https://goteleport.com/docs/reference/architecture/tls-routing/)).
  Syncthing's certificate-fingerprint device IDs are why its *sync* cannot go through a
  TLS-terminating proxy while its GUI can
  ([Syncthing security](https://docs.syncthing.net/users/security.html),
  [reverse proxy](https://docs.syncthing.net/users/reverseproxy.html)) — the same shape as MAE's
  collab port. And in MAE specifically the fixed SNI rules out hostname routing.
- **B.** Tokens expire and WebSocket connections do not
  ([websocket.org](https://websocket.org/guides/authentication/)). Hocuspocus authenticates only at
  connect — periodic re-auth is an open request because a revoked user "can continue to edit the
  document until they reconnect" ([#752](https://github.com/ueberdosis/hocuspocus/issues/752));
  Liveblocks had an already-connected client keep writing after access was removed
  ([#3720](https://github.com/liveblocks/liveblocks/issues/3720)); nginx closes an idle WebSocket at
  60 s by default ([nginx WebSocket](https://websocket.org/guides/infrastructure/nginx/)); the proxy
  sees plaintext. **These are design requirements, not disqualifiers** — see ADR-111 D6.
- **C.** Identity becomes a header the backend must trust; anyone reaching the backend around the
  proxy forges it ([NGINX upstream mTLS](https://docs.nginx.com/nginx/admin-guide/security-controls/securing-http-traffic-upstream/)).
  Browser client-cert UX is poor ([Pinterest employee mTLS](https://medium.com/pinterest-engineering/employee-facing-mutual-tls-8643fe0cc0f9)).
- **D.** An overlay client on every device excludes browsers; Headscale has had node-key re-auth bugs
  whose fix was deleting the node ([#2693](https://github.com/juanfont/headscale/issues/2693)); and it
  routes around the "everything through the proxy" policy rather than satisfying it.
- **E.** Browsers reach iroh only through a relay, without UDP or hole-punching
  ([iroh in the browser](https://docs.iroh.computer/languages/wasm-browser)); no SSO story.
- **F.** Teleport's own workaround for L7 load balancers — upgrade to WebSocket, run mTLS inside —
  costs "triple encryption", ~13 extra round trips per upgrade, keepalive pings against idle
  timeouts, and still no browsers ([RFD 0123](https://github.com/gravitational/teleport/blob/master/rfd/0123-tls-routing-behind-layer7-lb.md)).

## 3. The closest precedent

Matrix federation authenticates server-to-server requests at the HTTP layer, and says why in the
spec: *"Requests are authenticated at the HTTP layer rather than at the TLS layer because HTTP
services like Matrix are often deployed behind load balancers that handle the TLS and these load
balancers make it difficult to check TLS client certificates."* It signs each request with Ed25519
in an `X-Matrix` header instead ([server-server API](https://spec.matrix.org/latest/server-server-api/)).
That is MAE's situation, and the same resolution: keep the keys, move the proof off the transport.

## 4. Binding a person's OIDC identity to a device key is established practice

- Sigstore Fulcio: an OIDC token plus proof of possession of a key yields a 10-minute certificate
  ([Fulcio](https://docs.sigstore.dev/certificate_authority/oidc-in-fulcio/)).
- step-ca issues SSH certificates from an OIDC login, 16 h by default
  ([Smallstep](https://smallstep.com/blog/diy-single-sign-on-for-ssh/)); revocation is passive by
  default, so offboarding lag equals certificate lifetime
  ([step-ca revocation](https://smallstep.com/docs/step-ca/revocation/)).
- Matrix cross-signing: a user-level key signs each device key
  ([MSC1756](https://github.com/matrix-org/matrix-doc/blob/master/proposals/1756-cross-signing.md)).
- DPoP binds access tokens to a client key at the application layer, so it survives a
  TLS-terminating proxy ([RFC 9449](https://datatracker.ietf.org/doc/html/rfc9449)).

## 5. Native and headless clients

- Desktop editors: authorization-code flow with PKCE and a loopback redirect
  ([RFC 8252](https://www.rfc-editor.org/rfc/rfc8252.html)).
- Headless instances: device authorization grant (open to remote phishing,
  [RFC 8628 §5.4](https://www.rfc-editor.org/rfc/rfc8628.html)) or an operator-issued service token.
  When Matrix tokens began expiring, headless bots without refresh support dropped every few hours —
  refresh is not optional for a service.
- Re-auth on long-lived connections: Matrix sidesteps it (`/sync` is a sequence of requests;
  `M_UNKNOWN_TOKEN` with `soft_logout` drives a refresh). A persistent WebSocket must instead enforce
  an in-band refresh deadline server-side — the gap Hocuspocus and Liveblocks leave open.
