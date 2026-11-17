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

> **Correction (2026-11-11, ADR-099 Amendment 1 B0):** Hocuspocus now offers in-session token
> refresh (`onTokenSync`, with `connection.requestToken()` / `provider.sendToken()`), so "the gap
> Hocuspocus leaves open" is no longer accurate for it. The decision is unchanged: MAE's server
> still enforces the deadline itself.

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

## Amendment 1 (2026-11-16): native sign-in at the identity provider (P4)

**Status:** Proposed (design). Ordered before ADR-113 PR-4 (ADR-113, Build sequence, O9).

ADR-113 O9 (decided 2026-11-16) keeps native MAE sessions as agents and has native sign-in built
before ADR-113 PR-4, so that a person reaches their review-enabled KBs from native MAE *as a
person*. ADR-098 Amendment 1 already defines what that means on the daemon: a request is a person
only when its credential is an IdP token from an interactive sign-in **and** its binding is flagged
`person` (C3). This amendment designs the native half: how the editor gets such a token, keeps it,
binds it, and keeps it away from the agents running inside the same editor.

It changes nothing about which methods an IdP session may call. Writes over HTTPS still wait for
#727 (D4, ADR-098 C0), and ADR-113 O11 still decides review acts and uploads. Until the native sync
socket exists (P3), the editor's sync edits keep travelling over mutual TLS and stay agent edits; P4
makes the editor's HTTPS requests — reads, and ADR-113's acts once they exist — a person's.

### What exists (2026-11-16)

Built on the daemon: the binding table with `kind` and `bound_by`
(`daemon/src/identity_binding.rs`), the typed principal (`oauth_self_issue.rs`: `Credential`,
`PrincipalKind`), the proof-of-possession routes `POST /identity/challenge` and `/identity/bind`
(`oauth_binding.rs`), the group gate, RFC 9728 resource metadata naming the one accepted issuer, and
on `/ws` the bound-IdP switch (`ws_accept_bound_idp`), the session cap (`ws_max_idp_session_secs`)
and renewal deadlines. The sign-in claims (`SignIn { amr, acr, auth_time, groups }`) are in progress
as the ADR-098 follow-up.

Built in the editor: `mae --sign-identity-challenge` (`crates/mae/src/identity_challenge_cli.rs`,
over `mae_mcp::identity_challenge::sign_challenge`), and the hub read client
(`shared/kb/src/remote_hub.rs`), whose token comes from a keystore entry or a command.

**Not built:** any OAuth client flow in the editor, any native WebSocket client, and any notion in
the editor of *which* actor a hub request is made for. C3's "from an interactive sign-in" is not
checked by the daemon today: any bound IdP token whose binding is flagged `person` resolves to a
person, whatever client obtained it. S2 closes that.

### S1. The flow: system browser, loopback redirect, PKCE

The desktop editor (GUI and TUI) signs in with the authorization code grant as a **public client**
— no client secret, since a secret shipped in a binary is not one (RFC 8252 §8.5):

- It opens the **system browser**, never an embedded view (RFC 8252 §8.12), at the issuer's
  authorization endpoint, found from the issuer's discovery document.
- The redirect is `http://127.0.0.1:<port>/callback` on a port the OS picks when the editor binds
  `127.0.0.1:0` (RFC 8252 §7.3; the literal address, not `localhost`, §8.3). The IdP registration
  allows any loopback port, which RFC 8252 requires of the authorization server; an IdP that matches
  redirect URIs as patterns needs the pattern written for it, which is measured per IdP.
- **PKCE with `S256` only** (RFC 7636); `plain` is never sent. A fresh `state` and OIDC `nonce`,
  each 128 random bits, per attempt.
- The listener accepts one request: the right path, the right `state`, a `code`. Anything else is
  answered `400` and ignored, so a stray request cannot abort the flow. It closes on success, on
  cancel, and after `hub_sign_in_timeout_secs` (default 300).
- The response's `iss` parameter (RFC 9207) is checked when the IdP sends it, and required when the
  discovery document says the IdP sends it. The ID token's `iss`, `aud` and `nonce` are checked. The
  ID token is used only by the editor, to know who signed in; it is never sent to the daemon.
- The access token is requested for the hub (RFC 8707 `resource`, where the IdP supports it) so
  that its `aud` is the listener's canonical URI. Whether the IdP sets that audience is measured at
  setup; the daemon refuses any other audience, as today.

**Where the issuer and client come from.** The issuer is the one the hub names in its resource
metadata (`authorization_servers`), pinned for that hub at the first sign-in and shown to the person
then. A hub that later names a different issuer is refused until the person re-registers it: an
issuer swap is a mix-up attempt until shown otherwise. The client id is a new daemon option,
`oauth.native_client_id`, advertised beside it, so a person configures nothing but the hub's URL.

**Headless** instances (ADR-055) sign in with the **device authorization grant** (RFC 8628) when the
IdP offers it, under a separate client registration, and otherwise keep the operator's service token
(P1), which stays the default. A device-flow session is always an agent (S2), and its subject should
be a service identity at the IdP, not a person: a device code phished to a person (RFC 8628 §5.4)
then yields at most an agent of that person, and an IdP that restricts the headless client to
service identities yields nothing.

### S2. What makes a native session a person, and why the editor's agents stay agents

This is the centre of the design. It has two halves, one on each side of the wire.

**The daemon: a person needs a person client.** C3's first condition, "an IdP token from an
interactive sign-in", becomes checkable. The daemon reads which client the token was issued to
(RFC 9068 `client_id`, or `azp`; the claim name is configuration, measured per IdP) and treats the
request as a person only when that client is in `oauth.person_client_ids` **and** the binding is
flagged `person`. The native client and the browser client go in that list; the headless and
service clients do not. With the list empty, no IdP session is a person (fail closed). This is not a
change to C3. It is the enforcement C3 always described and the code does not yet do
(principle #15).

A refreshed token is still a person's: a refresh is not a new sign-in, and `auth_time` keeps the
original one, which is what C4 freshness reads (S3).

**The editor: the person's token is used only for the person.** An agent working in the editor
signs with the person's key (ADR-098 D1), so the key cannot tell them apart. Only the credential
can, and only if the editor never lends it. So the editor holds two credentials per hub — the
person's IdP session when signed in, and an agent credential — and chooses per request from the
request's **provenance**, carried from where the request began (a down payment on ADR-088's carried
authority):

- A request is the person's only when it starts in a **built-in command dispatched from real human
  input**: a key or mouse event from the terminal or the window. Every other origin is an agent's:
  MCP (including `execute_command` and every command mirror), the embedded and CLI agents,
  `eval_scheme`, and keys fed by `feed-keys`, which carry the provenance of the evaluation that fed
  them, not of a keyboard.
- **Scheme never sends with the person's credential.** In one shared Scheme image an agent can
  redefine any function a person's keybinding later calls (principle #6's stated tension), so a hub
  request made by Scheme code is an agent request whatever started it. The cost is that a person's
  own Scheme customisations reach hubs as an agent (decided, O-b).
- An absent or unknown provenance is an agent's. Falling back the other way would make every new
  call site a person by default.
- Nothing silently downgrades. A person request made while signed out is refused in the editor with
  "sign in", not retried as an agent; a person request refused by the daemon is not retried on the
  agent credential.

**The agent credential is a delegated agent token (ADR-118, drafting).** ADR-118 owns the token
that names "agent A of person P" (its `act` claim, its own issuer and `kid`), the grant table,
granting and renewing under a fresh second factor, revocation, and MCP on the listener. This
amendment designs none of that; it only says how the native editor obtains, holds and uses the
token:

- **Obtaining.** Creating or renewing the grant is a person's act, made through the person's
  session, so in the editor it is a human-input-only command (`hub-agent-grant`, below), never
  something an agent can start. The daemon's step-up refusal for a stale sign-in is handled as for
  any other controlled act (S3).
- **Holding.** In the editor process's memory, apart from the person's tokens, under the same rules
  as they are (S3): no file, argv, environment, child process, transcript or log.
- **Using.** While a person is signed in to a hub, every agent-provenance request to it — MCP tools,
  `mae-agent`, the embedded agent, Scheme — carries the delegated token and nothing else. With no
  live grant, the request is refused in the editor ("no agent grant for this hub"), never sent on
  the person's session. Whether an agent process may hold a token of its own rather than having the
  editor attach it is ADR-118's question.
- **Until ADR-118 lands**, the agent credential is the one the editor uses today (a keystore token,
  or the mutual-TLS session), which C3 already counts as an agent. The person-session boundary does
  not wait for ADR-118; only the token in the agent's hand changes when it lands (P4h).

**The provenance split protects the person from their agent; it does not bind the person.** The
person controls the editor binary and can make it send anything. Nothing the editor asserts — a
provenance, a flag, a header — is evidence to the daemon. So a person's controlled acts still need a
fresh second factor at the IdP (ADR-098 C4, ADR-113 B7 and C4), which the editor cannot manufacture,
and the daemon decides kind from the credential alone.

**The agent cannot start or reuse a sign-in (principle #16).** Signing in creates the credential
that makes a request a person's, so it is a control that bounds the agent:

- `hub-sign-in`, `hub-sign-out` and S3's step-up are a new class of **human-input-only** commands:
  not mirrored to MCP, not offered through `execute_command`, not dispatchable from Scheme, and
  refused when they arrive through `feed-keys`. Not offered beats offered and denied (ADR-085). Scheme
  may move the keybinding; a person still has to press it, and still has to sign in at the IdP.
- No Scheme primitive, option or MCP tool returns, sets or forwards the person's tokens. Options that
  steer the sign-in — the pinned issuer, a client override, the browser command — are Privileged on
  every surface, like `kb-register-hub`.
- The tokens never reach a child process — the shell, babel, `mae-agent`, the MCP shim, a language
  server — and never appear in argv, the environment, transcripts, logs or `*Messages*`.

**What this cannot stop, stated rather than discovered.** An agent allowed to run shell commands as
the same OS user is inside the device boundary. It can open a browser at an authorization URL of its
own, with its own loopback listener and the same public client id, and a person who completes that
sign-in has handed it a person session with a fresh `auth_time`. No client-side measure prevents
that, and the daemon cannot see it, exactly as ADR-098 C3 says of an agent driving a browser. The
defaults bound it — `shell` is asked, not allowed, at the default tier, and the documentation says
that a sign-in page the person did not open is not MAE's — and a deployment that needs the boundary
runs agents as another OS user or in a container (`SECURITY.md`).

### S3. Keeping the session: storage, refresh, renewal, step-up

**Storage (decided, O-a).** The access and ID tokens are held in the editor process's memory. The
refresh token persists across restarts in the operating system's credential store (Secret Service
on Linux, Keychain on macOS, Credential Manager on Windows), the place desktop editors keep theirs,
and only there. Where no credential store is available it stays in memory and the person signs in
once per run. Never in `config.toml`, `init.scm`, the KB registry, argv or the environment, and not
in the collab `trusted_keys` file either: that file is plaintext, readable by every process of the
user, and outside the paths agent file tools refuse. A persisted refresh token is acceptable only
because the IdP must protect it: RFC 9700 §4.14.2 requires an authorization server to rotate a
public client's refresh tokens or bind them to a key the client holds. P4 requires rotation of the
IdP (measured per IdP, refused in the documentation if absent), and binds the token to the member
key where the IdP supports a key-bound grant (DPoP, RFC 9449, or an ID token carrying a `cnf` key).
A credential store is readable by a same-user process with shell, as S2 says; rotation is what
turns a stolen copy into a detected one. ADR-118's delegated agent token is held apart from the
person's, under its own entry.

P4 also makes the agent's file tools refuse reads, not only writes, of credential-bearing paths (the
collab identity key, `trusted_keys`, any token store P4 adds) and refuse writes to the hub
registration that holds the pinned issuer. This narrows what an agent without shell can take; with
shell it changes nothing, as S2 says.

**Refresh.** The editor refreshes when the access token is three-quarters through its lifetime,
measured from `expires_in` on the editor's monotonic clock, not from `exp` against the wall clock,
so a skewed clock neither expires a good token nor keeps a dead one. A rotated refresh token replaces
the old one at once (RFC 9700 §4.14). A refresh that fails ends the person session: the status says
so, person requests are refused until the person signs in again, and nothing opens a browser
unprompted. The agent's delegated token is not refreshed through it: its lifetime and renewal are
ADR-118's.

**On `/ws`, once P3's native socket exists.** At `session/renew_needed` the editor renews with its
current access token, refreshing first if that token would expire before the next deadline. The
`ws_max_idp_session_secs` cap is only a renewal: the same token serves, the daemon re-reads the
binding, and a changed `kind` takes effect from that renewal. A missed deadline closes the socket as
D6 designs; the editor reconnects with the person credential if it still has a live one, and never
reopens a person's socket on the agent credential.

**Step-up without dropping the session.** When a person's act is refused `reauthentication_required`
(ADR-113 C4) or `mfa_required` (B7), the editor shows a native prompt naming the act and its target
— not one Scheme renders, which an agent could restyle — and on confirmation runs S1 again with OIDC
`prompt=login` and `max_age=0`, plus `acr_values` where the operator configures one. The new tokens
are adopted only if their `(iss, sub)` equal the current session's: another account is refused and
the old session kept, as ADR-099 B3 refuses on the socket. An open socket renews with the new token
in place, and the act is retried once. The editor never judges freshness itself; the daemon's window
and leeway decide, so the editor needs no copy of the operator's window.

### S4. Binding on first sign-in

After the first sign-in to a hub the editor asks the daemon what the session resolved to (S5's
`/identity/me`) and, if the subject is unbound, binds it to this editor's member key by ADR-098 C2's
proof of possession, with no further step for the person:

1. `POST /identity/challenge` with the new token and this key's fingerprint.
2. The editor signs through the same `sign_challenge` the CLI uses, with the issuer and subject
   taken from the ID token it has itself just validated and the audience from the hub registration.
   A challenge naming anything else is not signed, and the mismatch is shown. The CLI's rule — sign
   only what the person states — holds, because the editor states only what it verified itself.
3. `POST /identity/bind`. The binding is `agent`, `bound_by = proof`. It becomes a person only when
   the administrator flags it with the local CLI (C2, C3); the status says which it is, and why.

**A second device.** With the same member key (ADR-098 D2, once Phase B exists), the subject is
already bound to this fingerprint and nothing is done. With a key of its own, the subject is bound
to another fingerprint: the editor does not bind, does not try to unbind, and says so. The remedy is
the member key on this device, or an administrator's unbind and a new proof, which moves the
person's identity to this key.

**Key rotation** (ADR-040 `Rebind`; decided, O-f). The binding still names the old fingerprint, so
after a rotation the session resolves to a key that no longer acts. The person re-binds it
themselves, the way a code host adds a signing key after a fresh re-authentication and WebAuthn
registers a new credential from a fresh sign-in: a sign-in with a fresh second factor (C4's window,
the step-up of S3), plus a proof of possession by the new key, plus the old key's signed `Rebind`
when one exists. The daemon moves the binding one to one, keeps its `kind` (a `person` stays a
person), records the act with both fingerprints, and notifies the KB owners and the operator. No
administrator step is needed; an administrator can still unbind. The risk accepted is that a stolen
IdP session *with* a fresh second factor can move a person's identity to another key; the record and
the notice are what surface it.

**A changed subject** (an IdP migration or rebuild). The new `(iss, sub)` is unbound and this key is
bound to the old one, so the proof is refused (one to one). That is D3 working: the administrator
rewrites the table, and no signed log changes.

### S5. Surfaces

- **Commands, human-input-only (S2):** `hub-sign-in`; `hub-sign-out`, which forgets the person's
  tokens and the delegated agent token held for that hub, and revokes the refresh token at the IdP
  where it offers RFC 7009 (revoking the agent grant itself is ADR-118's); `hub-agent-grant`, which
  creates or renews the agent grant through ADR-118's method; and S3's step-up. Default bindings in
  `keymap-leader`.
- **Options (principle #7):** `hub_sign_in_timeout_secs`, ordinary. Privileged on every surface: the
  browser command used for sign-in, and any per-hub issuer or client override, kept with the hub
  registration.
- **Daemon:** `oauth.person_client_ids` and the client claim name (S2); `oauth.native_client_id`
  (S1); and a read-only `GET /identity/me` returning what the bearer resolved to — credential, kind,
  bound fingerprint, `bound_by`, whether the sign-in claims carry `amr` and `auth_time` — and never a
  token. `mae-daemon doctor` reports a configured person client whose tokens lack `auth_time` or
  carry the wrong audience.
- **Status, readable by the agent:** `kb_sharing_status`, `introspect` and the `*KB Sharing*` buffer
  show, per hub, whether a person is signed in, the issuer and subject, the kind the daemon reports,
  the access token's remaining lifetime, the sign-in time, and whether an agent grant is held and
  until when. Reading them grants nothing.

### S6. Failure modes, and the tests each must carry

Each is a test that fails without its mechanism, with mutant entries committed beside it.

- **A local process races the redirect.** It cannot bind the editor's port while the editor holds
  it, so it can only send the listener a forged callback. Tests: a callback with a wrong `state` is
  ignored and the flow still completes with the real one; a callback carrying a code issued against
  another PKCE challenge fails at the token endpoint; a reused code is refused; nothing listens after
  success, cancel or timeout.
- **A stolen code.** Without the verifier, which never leaves memory, it is useless. Test: the code
  redeemed by a second client with no verifier, or a wrong one, gets no token.
- **Mix-up and redirect injection.** Tests: an authorization response whose `iss` differs from the
  pinned issuer is refused; an ID token with another `aud` or `nonce` is refused; a hub advertising a
  new issuer is refused until re-registration; no redirect URI other than the loopback callback is
  ever sent.
- **Refresh token theft.** Bounded, not prevented: the stolen token gives a person session with an
  old `auth_time`, so every C4 act is refused, and group removal ends it at the next request
  (ADR-098 D4). Tests: a refreshed token's `auth_time` equals the original sign-in's, measured
  against a real IdP as well as the fake (an IdP that moves it on refresh voids C4, and `doctor`
  reports it); after group removal the refreshed token is refused.
- **The agent reuses the person session.** Tests, each against a hub fake that records the bearer it
  receives: with a person signed in, `kb_search` through MCP, through the embedded agent, through
  `eval_scheme`, and through a Scheme-defined command bound to a key all present the agent
  credential (the delegated token once P4h lands), never the person's; with no live grant they are
  refused in the editor and nothing is sent; `feed-keys` of the sign-in or grant binding opens no
  listener, no browser and no grant request; `execute_command` of `hub-sign-in` or `hub-agent-grant`,
  and any command mirror of them, are not offered; after `hub-sign-out` an agent request sends
  neither token; a raw-byte scan of every status surface, transcript, log and child-process
  environment finds no token of either kind.
- **The daemon's client gate.** Tests: a token from a client outside `person_client_ids` on a
  `person` binding is an agent; with the list empty every IdP session is an agent; a self-issued
  token is never a person whatever the list says.
- **A sign-in with no `amr`.** Some IdPs issue none for a recovery-link session (Authentik, measured:
  `amr` `["pwd","mfa"]` after a second factor, `acr` constant, no `amr` after a recovery link). It
  is a person without a second factor. Tests: such a token is refused `mfa_required` under an `amr`
  rule, and the editor's prompt says the sign-in had no second factor rather than looping.
- **Clock skew.** Tests: with the editor's wall clock ten minutes off either way, refresh is
  scheduled correctly and sign-in completes; freshness at the daemon tolerates exactly its own
  leeway.
- **Binding.** Tests: the editor refuses to sign a challenge whose issuer, subject or audience differs
  from what it verified; a second device's key is not bound over the first; after a rotation the
  status names the stale binding. Re-binding after rotation is refused without a fresh second
  factor, refused without a proof by the new key, refused when the new key is already bound to
  another subject, keeps `kind`, and writes the record and the notice. A refresh token is never
  written outside the credential store (a test scans the config, data and cache directories after a
  sign-in), and a replayed rotated refresh token ends the session.

### S7. Phases, each one pull request and one reviewer

Every phase touches identity, so each gets an independent review.

- **P4a — daemon: the person client and `/identity/me`.** S2's client gate, `native_client_id` in the
  metadata, `/identity/me`, the `doctor` checks. Before ADR-113 PR-4.
  Its release note calls out `oauth.person_client_ids` as an upgrade step: a deployment whose
  browser client's id is not listed has no person sessions after the upgrade (fail closed).
- **P4b — editor: provenance and the human-input-only class.** Two credentials per hub chosen by
  provenance; Scheme and `feed-keys` as agent; the command class; the file-tool refusals of S3. No
  network flow yet, so it is testable alone against the hub fake. Before PR-4.
- **P4c — editor: the sign-in.** S1's loopback flow against an in-process fake IdP and a real one;
  tokens in memory with the refresh token in the OS credential store (S3), refresh, sign-out,
  status. Before PR-4.
- **P4d — editor: automatic binding.** S4. Before PR-4; with P4a to P4c it is what lets a flagged
  person act from native MAE.
- **P4e — editor: step-up.** S3's prompt and the `prompt=login` retry. After PR-4 and before PR-8,
  which is where the daemon starts refusing for freshness and second factor.
- **P4e2 — self-service re-binding after rotation.** S4's daemon method and the editor's flow; it
  needs P4e's fresh second factor.
- **P4f — native `/ws` renewal.** Lands with P3's native socket, not before.
- **P4g — headless device flow.** Deferred (O-e).
- **P4h — editor: the delegated agent token.** `hub-agent-grant`, holding the token, and attaching
  it to agent-provenance requests in place of today's agent credential (S2). After ADR-118's daemon
  side, and before ADR-113 PR-10, where agent grants start to matter.

ADR-113 PR-1 to PR-3 need no person and can proceed in parallel with P4a to P4d.

### Decided (2026-11-16) and defaults kept

- **O-a. The refresh token across restarts:** the OS credential store, with IdP rotation required and
  key binding where the IdP supports it; memory only where there is no store (S3).
- **O-b. Scheme always as an agent:** yes. Input-origin tracking is attribution, not a control: an
  agent can feed MAE keys, and with shell it can inject OS input or read the editor's memory. What
  bounds a person's controlled acts is the fresh second factor at the IdP (S3's step-up), which an
  agent cannot produce. Running agents as a separate OS user or in a container, reaching MAE only
  through its MCP socket, is the stronger boundary; it is a separate design item, not part of P4.
- **O-c. The person flag stays an administrator's act per binding** (default kept, ADR-098 C3).
- **O-d. TUI over SSH:** not supported in P4. A person signs in from a desktop session. An SSH port
  forward of the loopback port may work, but is not documented or tested.
- **O-e. Headless device flow:** deferred. The operator-issued service token stays the headless path;
  device-flow sessions, when built, are agents.
- **O-f. Re-binding after rotation:** self-service with a fresh second factor (S4).
- **O-g. An empty `person_client_ids` fails closed** (default kept). Bindings already flagged `person`
  act as agents until the operator lists the clients; the upgrade note says so.
