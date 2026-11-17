# ADR-118: Delegated agent tokens, and MCP on the HTTPS listener

**Status:** Proposed (design). Nothing here is built. Phased (G1–G5, below).
**Depends on:** ADR-098 and its Amendment 1 (the typed principal, the binding table, `person` /
`agent`, C3), ADR-052 (the OAuth 2.1 resource server and its RFC 9728 metadata), ADR-113 and its
Amendments 2 and 3 (`agent_forbidden`, the author rule, C4's fresh second factor, C9's per-KB agent
policy, and its build sequence of 2026-11-16), ADR-036 Amendment 1 (the hub verifies every content
op; a daemon never signs for a member), ADR-111 Amendment 1 (native sign-in; `oauth.person_client_ids`;
the editor's provenance split).
**Extends:** ADR-098 C1 (a fourth credential, the delegated agent token), ADR-113 C9 (the grant table
its PR-10 assumes), ADR-053 (the read surface, offered as MCP tools).
**Relates to:** ADR-117 (D3 `self_grant`, D6's one second-factor checker, D7's logs), ADR-116
(history attribution), ADR-099 (`/ws` renewal and its 4403 close), ADR-085 (not offered beats
offered and denied), ADR-050 (tool names shared across MCP surfaces), `docs/MCP_ARCHITECTURE.md`,
CLAUDE.md principle #16.
**Source of the requirements:** a browser client's design for AI assistants reaching MAE over MCP.
The owner's decisions on it are marked *(owner, 2026-11-15)*. It asks MAE for three things: a
principal "agent A acting for person P" with a grant table, the methods to create and revoke it, and
an MCP server on the daemon's HTTPS listener. This ADR is those three.
**Tracking:** issue TBD.

## Context

A person wants an AI assistant to read, and later edit, the KBs they can reach on a hosted daemon.
Today there is no honest way to give it that:

| What exists | Where | Why it is not enough |
|---|---|---|
| The editor's MCP server, over a local socket | `shared/mcp`, `docs/MCP_ARCHITECTURE.md` | No per-client identity: the assistant acts with the editor's own identity, so the daemon cannot tell the person from their assistant. |
| Self-issued tokens (`kb/query.self_token`) and operator tokens (`mae-daemon token mint`) | `oauth_self_issue.rs`, `token_cli.rs` | `sub`, `aud`, `iss`, `iat`, `exp` only. No scope, no notion of acting for someone, no revocation short of rotating the daemon's identity key. |
| The HTTPS listener as an OAuth 2.1 resource server | `daemon/src/oauth.rs`, ADR-052 | Serves `kb/query.*`, `/ws` and the binding routes. It is not an MCP server: there is no `tools/list` or `tools/call`. |
| The typed principal and `person` / `agent` | `identity_binding.rs`, `ValidatedPrincipal.kind`, ADR-098 C1/C3 | Says *whether* a request is an agent, but not *whose* agent or *which* one. |

The alternatives a person reaches for first are both wrong. Handing the assistant their own IdP
session, or an IdP app password, makes the assistant indistinguishable from them: it could approve
its own work. Having it declare itself an agent (a header, a `provenance` field, a permission tier)
is a claim written by the party it is meant to constrain.

What MAE must deliver, restated as MAE's own requirements:

- **R1.** The daemon knows from the credential alone that a request is an agent's, and no agent
  credential is ever a person, whatever flag P's binding carries.
- **R2.** Every agent action records both the agent and the person it acts for.
- **R3.** An agent's reach is at most P's, now, on every request; by default read-only, on KBs named
  one by one, for a limited time. Losing P's access loses the agent's at once.
- **R4.** P and an administrator can revoke it, with immediate effect.
- **R5.** Creating or renewing one needs P present: a fresh sign-in with a second factor.
- **R6.** An agent never performs a person's act: approving, clearing, accepting, granting.

ADR-113 C9 already decided the policy *(owner, 2026-11-15)*: the KB owner sets an agent ceiling
`none | read_only | read_write`; P creates and renews read-only grants on their own authority with a
fresh second factor, 7 days by default and at most an operator cap of 30; an agent never exceeds P;
P never clears or approves their own agent's work; no agent on a review-enabled KB until its owner
raises the ceiling. It left open (O16) whether the daemon can tell P's agents apart. This ADR builds
the mechanism C9 assumes and settles O16.

### Prior art

- **RFC 8693 (Token Exchange), §1.1 and §4.1.** Distinguishes *delegation*, where the actor keeps an
  identity separate from the subject, from *impersonation*, where it is indistinguishable. The `act`
  claim names the current actor; `sub` names the principal acted for. Delegation is exactly R1 and
  R2; impersonation is what the app-password route produces.
- **RFC 9068 (JWT access-token profile)** and **RFC 8725 §3.11 (explicit typing).** An access token
  carries `typ: at+jwt` so it cannot be confused with another JWT kind, and names `iss`, `sub`,
  `aud`, `client_id`, `iat`, `exp`, `jti`. A distinct `typ` per token kind is the standard defence
  against one validator accepting another's tokens.
- **The MCP authorization specification (2025-11-25).** An HTTP MCP server is an OAuth 2.1 resource
  server; it MUST publish RFC 9728 metadata naming at least one authorization server, MUST validate
  that a token was issued for it as audience (RFC 8707), and "MUST NOT accept or transit any other
  tokens". Authorization as a whole is optional, and the full client flow is a SHOULD.
- **OAuth 2.1 §4.3.1 and RFC 9700 §4.14.2.** Refresh tokens for public clients rotate on every use,
  and reuse of a spent one revokes the whole family: the standard way to make a refresh credential
  non-replayable without sender constraint. **RFC 9449 (DPoP)** is the sender-constrained form.
- **RFC 3820 (proxy certificates)** and capability-delegation schemes generally: a principal signs a
  short-lived, scoped statement authorizing another key to act for it, so a verifier checks the
  chain without asking a server. This is the shape D7 uses for agent writes.
- An individual IETF draft (not adopted) applies RFC 8693 to AI agents in the same way: `sub` the
  person, `act` the agent. It is cited as corroboration, not as a dependency.

## Decision

### D1. The principal: "agent A of P"

An agent principal is a pair: an **agent id**, minted by the daemon when a grant is created, and the
**fingerprint of P**, the person who created it. The agent id is random (128 bits, rendered
`agent:<base32>`), one per grant, and never reused.

The validated principal of an agent request is the **agent id**, not P's fingerprint:
`ValidatedPrincipal { principal: "agent:<id>", credential: AgentDelegated, kind: Agent,
on_behalf_of: <P's fingerprint>, grant_id }`. This is deliberate and is the design's main defence.
Any code path that forgets the agent case sees a principal that is a member of nothing and fails
closed, exactly as `idp-unbound:` principals do today (`oauth_self_issue::UNBOUND_PREFIX`). No path
can grant an agent P's rights by forgetting to intersect them. As with unbound principals, the route
answers an agent principal before `kb_access` is consulted, because `kb/add_member` accepts any
string and a member list could hold this very text.

**`act` present ⇒ never a person, at every check.** The kind of an agent credential is fixed to
`Agent` at validation and never read from the binding table, so a `person` flag on P's binding does
not reach it. Every place that asks "is this a person" (ADR-113's acts, ADR-117's administration,
the grant methods below) reads `kind`, and `kind` has one producer. A test pins it for each.

The agent is not P. P's grant attenuates P's own rights and never adds to them (D4), so a grant is
not a self-grant in ADR-117 D3's sense: the beneficiary is `agent:<id>`, a different principal from
P, and its reach is a subset of P's.

### D2. The token

An agent access token is a JWT the daemon issues and only the daemon accepts:

- **Header:** `alg: EdDSA`, `typ: mae-agent+jwt`, `kid: agent-<key id>`.
- **Claims:** `iss` = the daemon's agent issuer (`oauth.agent_issuer`, derived from the listener's
  origin as `<origin>/agent` and refused at startup if it equals `self` or any configured IdP
  issuer); `sub` = P's fingerprint; `act: {"sub": "agent:<id>"}` (RFC 8693 §4.1); `client_id` =
  `agent:<id>` (RFC 9068 §2.2); `grant` = the grant id; `aud` = exactly one resource on this
  listener (D9); `iat`, `exp`, `jti`.
- **No rights.** The token carries no scope and no KB list. Rights are read from the grant table on
  every request (D4), so narrowing or revoking a grant takes effect without waiting for the token to
  expire.
- **Its own signing key.** Agent tokens are signed with a dedicated Ed25519 key held in the daemon's
  state directory, not the daemon's identity key. Self-issued tokens keep the identity key and
  `kid: self`.

The listener chooses a validator by the header before verifying anything, and each validator refuses
the other's tokens:

- `kid: self` → the self-token validator, which now also refuses a token carrying `act`, `grant`, or
  `typ: mae-agent+jwt`;
- `kid: agent-*` or `typ: mae-agent+jwt` → the agent validator only, which requires all of the
  header fields above, a signature by the agent key, `iss` equal to the agent issuer, `aud` equal to
  the resource being called, and a live grant whose agent id equals `act.sub`;
- anything else → the external (IdP) validator, which now refuses any token carrying `act`. An IdP
  may one day issue delegated tokens (RFC 8693 token exchange); accepting them is not designed here
  (Open, O7).

Because the agent key is not the identity key, an agent token relabelled `kid: self` fails the self
validator's signature check, and a daemon older than this ADR sees an unknown `kid`, sends it to the
JWKS path, and refuses it. No validator, old or new, can read an agent token as P.

### D3. The grant table

Grants live in the daemon's SQLite state database, beside the binding table and outside the CRDT,
for the reason ADR-098 D3 gives for bindings: they are hub-local authorization state, not shared
content to converge, and the existing backup covers them.

`agent_grants`: `grant_id` (random); `agent_id`; `label` (P's name for it, at most 64 characters,
shown in every list and record); `person_fp` (P); `person_iss`, `person_sub` (the IdP identity that
created it); `agent_pubkey` (nullable: only write grants have one, D7); `delegation_sig` (nullable,
D7); `created_at`, `created_auth_time`, `created_amr`; `expires_at`; `renewed_at`,
`renewed_auth_time`, `renewed_amr` (latest); `revoked_at`, `revoked_by`, `revoke_reason`;
`granted_by` (`person` or `operator`).

`agent_grant_kbs`: `(grant_id, kb_id, rights)`, `rights` ∈ `read | read_write`. KBs are named one by
one. There is no wildcard, and a KB P joins later is not added to an existing grant.

`agent_refresh`: `(grant_id, credential_hash, issued_at, spent_at)`. Only the SHA-256 of a refresh
credential is stored (D6).

Every create, renew, revoke, ceiling change and refresh-reuse detection is appended to a hash-chained
log signed by the daemon's key, in the shape of ADR-113 D7, so the register of agents (who, for whom,
which KBs, which rights, from when, until when, revoked when and by whom) can be exported and
checked offline.

The KB's ceiling (`none | read_only | read_write`, ADR-113 C9) is stored per KB beside the grants.
On a review-enabled KB it starts at `none` *(owner, 2026-11-15)*. On any other KB this ADR proposes
`read_only` (Open, O1): a read grant can only show the agent what P can already read.

### D4. Every request intersects three things

For an agent request on KB `k`, the effective right is the least of:

1. the grant's right on `k` (absent ⇒ nothing);
2. **P's role on `k` now**, read through the same `kb_access` that serves P (Viewer or a query-only
   member ⇒ `read`; Editor or Owner ⇒ `read_write`). An agent never receives an owner's
   administrative rights, only content rights;
3. **`k`'s ceiling now**.

And the grant must be live: not revoked, not expired, and P's binding the one it was created under,
still bound to the same fingerprint and still flagged `person`. Each is re-read on every request; no
part is cached in the token or the session. So removing P from `k` removes the agent from `k` at the
next request, unbinding P or clearing P's `person` flag ends every grant P made, and an owner
lowering the ceiling to `none` cuts every agent on the KB at once. That last is the owner's
revocation tool; it needs no new method.

A KB the agent cannot read answers exactly as a KB that does not exist (ADR-117 D1's no-oracle rule).
E2E KBs (ADR-037) are refused at grant time with `e2e_unsupported`: the daemon serves only
ciphertext, and reading it would need P's content key, which an agent must not hold.
`kb/query.my_wrapped_key` is never offered to an agent principal.

### D5. Granting, renewing and revoking

Methods on the OAuth listener, alongside `kb/query.*`:

- **`kb/agent.grant {label, kbs: [{kb_id, rights}], ttl_secs?, agent_pubkey?, delegation_sig?}`.**
  Creates a grant for the caller's own agent and returns, once, `{grant_id, agent_id, expires_at,
  access_token, access_expires_at, refresh_credential, hub_url, shim_command, secret_fields}`. The
  response is the only time the refresh credential exists in clear. `secret_fields` names, in the
  result itself, the fields that are shown once and must never be logged, echoed or kept by a
  client that displays them (only the shim's `--enroll` stores the credential) (`["access_token", "refresh_credential"]`), so a client need not hard-code the list. The
  daemon logs none of them, and a test asserts the daemon's log and the client-visible error paths
  carry neither value after a grant.
- **`kb/agent.renew {grant_id, ttl_secs?}`.** Sets a new expiry on the same grant, so the agent id and
  its history stay continuous. Renewing is granting again *(owner, 2026-11-15)*: the same checks.
- **`kb/agent.revoke {grant_id, reason?}`.** Immediate (below).
- **`kb/agent.list`.** Caller-scoped (ADR-117 D1): P sees their own grants; a KB owner sees, for their
  KBs, each grant's label, P, rights and expiry; never a token or credential.
- **`kb/agent.set_ceiling {kb_id, ceiling}`.** Owner only.

Grant, renew, and raising a ceiling require a **person** (D8) whose sign-in meets the second-factor
rule within the freshness window: ADR-117 D6's single checker
(`oauth.second_factor.{accepted_amr, accepted_acr, max_age_secs}`), the same one ADR-113 C4 uses.
Revoking, listing and lowering a ceiling require a person but no fresh second factor: they only
subtract, and making revocation hard would be the wrong trade.

Refusals carry `error.data.kind`: `not_a_person` (an agent, self-issued or unflagged caller);
`reauthentication_required`; `mfa_required`; `second_factor_unconfigured`; `grant_above_ceiling`
(+`kb_id`, `ceiling`); `grant_above_role` (+`kb_id`); `grant_too_long` (+`max_ttl_secs`);
`grant_kbs_required` (an empty list); `grant_limit` (+`limit`, live grants per person);
`e2e_unsupported`; `unknown_grant` (also for another person's grant, so grant ids are not probeable);
`not_owner` (a ceiling change by a non-owner). These fill ADR-113 O12's missing kinds for a ceiling
or grant refusal.

Defaults, each an operator option pinned by a test that states its reason: a grant lasts 7 days
(`agent_grant_default_ttl_secs`) *(owner)*; the cap is `agent_grant_max_ttl_secs`, at most 30 days,
and a request above it is refused, never clamped *(owner)*; at most 10 live grants per person
(`agent_max_live_grants_per_person`), because each is a live credential that someone must review and
a person running more than a handful of distinct agents at once is unusual (Open, O9).

**Revocation is immediate.** A revoked grant fails the next request's liveness check (D4). Live
connections are not left to the next request: every `/ws` socket and every MCP stream opened with an
agent token is registered under its grant id, and revocation closes them, `/ws` with code 4403 and
reason `grant_revoked` (the code ADR-099 uses for `principal_changed`), MCP streams by ending the
stream with a final error. A membership or ceiling change on a KB re-authorizes every agent socket
holding a document of that KB, with the same `reauthorize` a `/ws` renewal runs. A revocation
survives a restart because it is a row, not memory.

**Administrators.** The operator revokes through a local CLI, `mae-daemon agent revoke|list`, like the
binding CLI: never a network method. The same CLI may create a grant for development, tests and
recovery, recorded `granted_by: operator`; a local operator can already mint any token, so this adds
no power. A KB owner revokes agents on their KB by lowering its ceiling.

### D6. Short access tokens, a non-replayable refresh credential

Two shapes were weighed. A token that lives as long as the grant (days) is simple for a client that
can only send a fixed header, but a stolen one works until the grant ends or someone notices. Short
access tokens re-minted against a live grant bound a stolen access token to minutes, and the grant
check on every request means revocation does not depend on either.

**Decision: short access tokens plus a refresh credential.**

- An access token lives 15 minutes by default (`agent_access_token_ttl_secs`), never past the grant's
  expiry. Fifteen minutes is the window a leaked access token stays useful; refreshing costs one
  SQLite read and write per agent per quarter hour.
- The refresh credential is 256 random bits, returned only by `kb/agent.grant`, `kb/agent.renew` and
  the refresh itself. It is redeemed at `POST /agent/token` with RFC 6749 §6's shape
  (`grant_type=refresh_token`, `refresh_token`, `resource`), which returns a new access token **and a
  new refresh credential**; the old one is spent.
- **Presenting a spent credential revokes the grant** (RFC 9700 §4.14.2): it means two parties hold
  it. Live sessions close, the log records `refresh_reuse`, and P must grant again. There is no grace
  window for a client that lost a response: a grace window is a replay window (Open, O4).
- When the grant has an agent key (write grants, D7), the refresh request also carries a proof signed
  by that key over the method, URI, a daemon nonce and the credential's hash, in RFC 9449's shape. A
  stolen refresh credential without the key is then useless. Read-only grants rely on rotation alone.
- The token endpoint takes nothing else: no password, client-credentials, code or token-exchange
  grant. A refresh credential is accepted only at `/agent/token`, never as a bearer token anywhere.

### D7. Attribution, and how an agent writes

**Attribution.** Every agent request, event and record carries the agent id and P's fingerprint:

- the per-document editor record (ADR-113 D3, `doc_editors`) gains an `Agent` attribution kind
  holding both; ADR-113's author rule counts it as P's authorship (C3), so P cannot approve, accept
  or put into force what their agent wrote;
- ADR-116's history `editors` list shows it as "agent A of P", with A's label;
- ADR-113 D7 review events, ADR-117 D7 logs and the grant log (D3) name both;
- read requests are logged with `agent`, `on_behalf_of` and `grant` fields.

**Writes are signed by the agent's own key, bound to P by P's signature.** Three options were
weighed for O16:

- *The agent signs with P's member key.* The daemon cannot tell P's agents from P (ADR-098 C3's
  interim state), a grant can only be per `(KB, P)`, and P's key — the credential every other path
  trusts — leaves P's control. Rejected.
- *The agent writes unsigned and the daemon signs.* ADR-036 Amendment 1 F5 already rules this out: a
  daemon never signs on a member's behalf. Rejected.
- *The agent has its own Ed25519 key, and P signs a delegation to it.* Chosen.

A write grant carries the agent's public key and a **delegation**: P's signature, with P's member
key, over a domain-separated statement (`maedelegation/v1`) naming P's fingerprint, the agent's
public key, the agent id, the KB, the right, and a `not_after` equal to the grant's expiry. An agent
op is signed by the agent key; its `author` is the agent key's fingerprint, and it carries the
delegation. The hub admits it when the op's signature verifies against the agent key, the delegation
verifies against P's key, P is a member with write rights at the op's epoch (ADR-036 A1 F2's source),
the op is before `not_after`, **and** D4's live intersection allows a write. A mesh peer can check
everything but the revocation; it is bounded by `not_after`, at most 30 days.

The agent key is generated where the agent runs and never leaves it: in the native editor's memory
for its in-editor agent, or in the MAE MCP shim for an out-of-process one (D11). P's client signs the
delegation at grant time: native MAE with its identity key, the browser with mae-crypto's WASM
signer (ADR-098 Amendment 1 C4). A renewal that extends `not_after` needs a fresh delegation
signature.

This settles O16: each grant has its own agent id for reads and its own key for writes, so the daemon
tells P's agents apart, and a grant is per `(KB, P, agent)`.

The delegated op is a new op version. A verifier that does not know it refuses the op, so deploying
the hub first is safe. Until mesh peers verify delegations, agent ops stay hub-local (Open, O10).

### D8. Who is a person: a person client, never an app password

This ADR uses ADR-111 Amendment 1's rule rather than adding a second one: a request counts as a
person only when its IdP token was issued to a client listed in **`oauth.person_client_ids`** (the
token's RFC 9068 `client_id` or `azp`, the claim name being configuration measured per IdP) **and**
its binding is flagged `person` (ADR-098 C3). The list has no default; empty means nobody is a person
(fail closed). The browser client and the native client are listed; headless, device-flow and service
clients are not.

What this adds:

- An agent token never reaches that check: its kind is fixed at validation (D1).
- An IdP app password, or a client-credentials token, is not a person: it is issued to a client
  not on the list, or, if an IdP lets it be requested against a listed client, it carries no
  interactive sign-in. The daemon therefore also requires `auth_time` on a person token. The primary
  control is still the IdP's configuration — the listed clients accept neither password nor
  client-credentials grants — and a deployment verifies it with a real app password before relying
  on it.
- **No grant from a grant.** `kb/agent.*` requires a person, so an agent cannot request another
  agent's token: there are no agent chains, and `act` is never nested.

### D9. MCP on the listener

The daemon serves MCP at `/mcp` on the HTTPS listener, over the **Streamable HTTP** transport: one
endpoint taking `POST` for requests and `GET` for a server stream, `Mcp-Session-Id` for sessions, and
`MCP-Protocol-Version` on every request. A request whose `Origin` header is present and not in the
listener's allowed origins is refused, as the transport requires against DNS rebinding.

- **Its own audience.** `/mcp` is a distinct resource, `oauth.mcp_resource_uri` (derived as
  `<canonical_resource_uri>/mcp`, refused at startup if equal to it). A token for `kb/query.*` is
  refused at `/mcp` and the reverse (RFC 8707). An agent asks for the audience it needs with
  `resource` at `/agent/token`.
- **RFC 9728 metadata** at `/.well-known/oauth-protected-resource/mcp`, naming the resource,
  `bearer_methods_supported: ["header"]`, and the daemon's agent issuer as its authorization server.
  That issuer publishes RFC 8414 metadata at `/.well-known/oauth-authorization-server/agent` with
  its token endpoint and `grant_types_supported: ["refresh_token"]`, and no authorization endpoint.
  A 401 from `/mcp` carries `WWW-Authenticate` with `resource_metadata`. An MCP client that tries the
  interactive OAuth flow therefore discovers there is none and stops; it connects with a credential
  P provisioned (D11). Designing that flow is deferred (Open, O7).
- **Agent tokens only.** `/mcp` accepts an agent token whose `aud` is the MCP resource, and nothing
  else: not a person's IdP token, not a self-issued token. A person driving an MCP client with their
  own sign-in is the impersonation this ADR exists to stop, so the MCP surface does not offer it.
- **No token passthrough.** `/mcp` never forwards the bearer it received. A tool call is dispatched
  in process to the same `kb_query` function `kb/query.*` uses, with the agent principal and D4's
  intersection. Nothing new reads a KB.
- **A session belongs to its grant.** An `Mcp-Session-Id` presented with a token for another grant is
  answered as an unknown session, so a session id is never a capability.

**Tools, first phase: the read surface.** Each maps one-to-one onto an existing method and takes the
KB explicitly. Names follow the editor's MCP tools where one exists, so an agent sees one vocabulary
across surfaces (ADR-050):

| Tool | Method |
|---|---|
| `kb_list_kbs` | `kb/query.my_kbs`, filtered to KBs the grant names and the agent can read now |
| `kb_search` | `kb/query.search` |
| `kb_get` | `kb/query.get` |
| `kb_list_nodes` | `kb/query.list` |
| `kb_titles` | `kb/query.titles` |
| `kb_links` | `kb/query.links` |
| `kb_neighborhood` | `kb/query.neighborhood` |
| `kb_graph` | `kb/query.graph` |
| `kb_agenda` | `kb/query.agenda` |

Not offered: `kb/query.my_wrapped_key` (key material), `kb/query.health` (operational, not content).
`kb/query.capabilities` is reported in `initialize`, not as a tool.

**Writes, later (G5).** Write tools are listed only to a grant with `read_write` on at least one KB,
and only once D7's signed path exists. They take an op the client has already signed; the MCP shim
(D11) presents its host with ordinary tools and does the signing. A read-only grant's `tools/list`
has no write tool at all (ADR-085).

### D10. What an agent never reaches (principle #16)

An agent credential reaches an allowlist, not everything minus a denylist: `/mcp`, `kb/query.*`
except `my_wrapped_key`, `/ws` for documents its grant names (reads; writes from G5), `/agent/token`,
and `GET /identity/me` (ADR-111 Amendment 1 S5). Every other method and route answers `not_a_person`
before dispatch, including `kb/agent.*`, `/identity/challenge`, `/identity/bind`, `kb/review.*`,
membership, policy and retention methods. None of the grant, renew, revoke or ceiling methods is an
MCP tool, on this surface or the editor's; they are not offered, not merely refused.

ADR-113's rules hold unchanged on top: `agent_forbidden` for every act, `agents_not_assessed` on
review-enabled KBs until the deployment's assessment act exists (C9).

### D11. How clients hold the token

- **Native MAE's in-editor agent.** As ADR-111 Amendment 1 S2 designs: a human-input-only command
  (`hub-agent-grant`) creates or renews the grant through the person's session; the editor generates
  the agent key in memory and signs any delegation with its identity key; it holds the access token,
  refresh credential and agent key in memory apart from the person's tokens, and attaches them to
  every agent-provenance request. The in-editor agent never sees them.
- **An out-of-process agent** (a third-party MCP host). The host launches `mae-mcp-shim --hub <url>`
  over stdio. The shim is an MCP *client* of `/mcp`: it accepts no token from its host, holds the
  refresh credential (read from stdin at setup, never argv or the environment), refreshes it, and,
  for a write grant, holds the agent key. It keeps the refresh credential and the agent key in the OS
  credential store, or, where there is none, in a file of mode 0600 under the XDG data directory,
  and its status says which (O5). P provisions it once by pasting what `kb/agent.grant` returned; for
  a write grant the shim first prints its public key for P's grant form. A host that can only send a
  fixed header and cannot launch a command is not served in v1 (O3).

**Supported hosts, first.** Claude Code and Claude Desktop, OpenAI Codex, and VS Code with Copilot's
agent mode. Each launches a stdio MCP server from its own configuration, so each is served by the
shim with no change to the host. For each, the person must be able to search, read and, from G5,
edit the hub KBs they can reach. Those are the KBs a browser client edits, because the browser
client and `/mcp` read the same daemon. Other hosts are widened to later (O7). Two things make that
practical for a person who never installs MAE:

- **The shim ships on its own.** Each release publishes `mae-mcp-shim` as a standalone, signed
  artifact per platform (it is already built for every MAE package), with a checksum, so it can be
  installed without the editor.
- **The grant screen hands over a working configuration.** `kb/agent.grant` returns, beside the
  one-time credential, the hub URL and the command line. A client's grant screen shows ready-to-paste
  configuration for each supported host, and `mae-mcp-shim --hub <url> --enroll` reads the
  credential from stdin and stores it. The credential is shown once. A test per host checks the
  documented configuration against that host's published configuration format, and a release
  checklist smoke-tests a real search, read and (from G5) edit in each.

The daemon cannot see inside either. If P gives an agent P's own IdP session, or lets it drive P's
browser, the daemon sees P (ADR-098 C3). What closes that for the acts that matter is that they need
a fresh second factor at the moment of acting (ADR-113 C4, ADR-117 D6, D5 here), which an agent
cannot produce without P present.

## Phases

Each phase is one pull request with one independent review: every one touches identity or
authorization. Ordered against ADR-113's build sequence (2026-11-16) and ADR-111 Amendment 1's
phases.

- **G1 — agent principal, token and grant table, read path.** D1, D2, D3, D4; the operator CLI from
  D5 (grant, list, revoke); `kb/query.*` and `/ws` reads with an agent token; the self and external
  validators refusing `act`. No network grant method yet, so it needs no sign-in claims. After
  ADR-111 Amendment 1 P4a (the person client gate). Independent of ADR-113 PR-1 to PR-7.
- **G2 — network grant methods and refresh.** D5's methods with the fresh second factor, D6's
  refresh endpoint and reuse detection, immediate revocation of live sockets. Needs the sign-in
  claims follow-up to ADR-098 (the same prerequisite as ADR-113 PR-8) and ADR-117 D6's checker.
  Before ADR-111 Amendment 1 P4h, which attaches the token in the editor, and before ADR-113 PR-10,
  which then builds on this grant table rather than its own: PR-10 keeps the review-KB `none`
  default, the assessment act and vocabularies.
- **G3 — MCP on the listener, read-only.** D9's endpoint, audience, metadata and read tools; D10's
  allowlist; the shim's `--hub` and `--enroll` modes for reads, its credential storage, the
  standalone shim artifact, and D11's per-host configuration for the four supported hosts. After G2.
- **G4 — attribution.** D7's `Agent` attribution kind in the editor record, ADR-116 history and
  ADR-113 events, ahead of any agent write, so the first agent write is attributed from its first
  byte. After G1; may land with G2.
- **G5 — agent writes.** D7's delegation and agent-signed ops, write tools in D9 (create and edit a
  node, add a link, at minimum), the shim's signing,
  the DPoP-shaped refresh proof. After ADR-036 Amendment 1's switch is on (#727), ADR-113 PR-5 (the
  author rule) and PR-10 (ceilings and the assessment gate).

### Tests each phase must carry (principle #14)

Each must fail without its mechanism, and each phase commits mutant entries for what it adds.

- **G1.**
  - An agent token for P, with P's binding flagged `person`, has kind `agent` at every check; it is
    refused an ADR-113 act with `agent_forbidden`. Mutant: compute kind from the binding for agent
    credentials.
  - A self-issued token signed with the daemon's identity key and carrying `act` is refused, never
    read as P. A token signed with the agent key and relabelled `kid: self` is refused. An IdP token
    carrying `act` is refused. Mutant: one key for both token kinds.
  - A grant for K1 only: reading K2 answers as a non-existent KB.
  - P removed from K1: the agent's next request to K1 is refused, with no wait for expiry. P's binding
    unbound, or its flag cleared: every grant P made is dead at the next request. Mutant: cache P's
    role at grant time.
  - Ceiling lowered to `none`: the next request is refused. A grant row above the ceiling (written by
    the operator CLI) still yields only the ceiling.
  - An expired grant and a revoked grant are refused, and a revocation survives a daemon restart.
  - A grant on an E2E KB is refused; `my_wrapped_key` is refused to an agent.
  - A route scan: an agent token on every listener method outside D10's allowlist is refused
    `not_a_person`.
- **G2.**
  - Grant with a stale `auth_time` → `reauthentication_required`; without a second factor →
    `mfa_required`; with none configured → `second_factor_unconfigured`; from an agent or
    self-issued token → `not_a_person`; from a person token whose `client_id`/`azp` is not in
    `person_client_ids` → `not_a_person`.
  - A grant above the ceiling → `grant_above_ceiling`; above P's role → `grant_above_role`; longer
    than the cap → `grant_too_long`, never clamped; with no KBs → `grant_kbs_required`.
  - An agent calling `kb/agent.grant`, `renew`, `revoke` or `set_ceiling` → `not_a_person`, and none
    is listed in any MCP `tools/list`.
  - Revoking a grant closes its open `/ws` socket with 4403 `grant_revoked`; renewing or revoking
    another person's grant → `unknown_grant`.
  - A refresh credential used twice: the second use is refused and revokes the grant; the rotated
    credential issued by the first use then fails too. A refresh credential presented as a bearer
    token is refused.
- **G3.**
  - `/mcp` refuses a person's IdP token, a self-issued token, and an agent token whose `aud` is the
    `kb/query` resource; `kb/query.*` refuses an agent token for the MCP resource.
  - A request with a foreign `Origin` is refused. A session id replayed with another grant's token is
    unknown.
  - A read-only grant's `tools/list` contains no write tool and no grant method. A tool call for a KB
    outside the grant answers as a non-existent KB.
  - The bearer received at `/mcp` appears in no outbound request and no log line.
- **G4.**
  - An agent read and an agent write record both the agent id and P's fingerprint in the editor
    record, the history version and the review event.
- **G5.**
  - An agent op with a valid delegation is admitted; with a delegation signed by a non-member, for
    another KB, past `not_after`, or for a revoked grant at the hub, it is refused.
  - An agent edit, then P's approval of the revision containing it → `author_cannot_approve`, also
    when P is on the approver list; another person's approval is accepted.
  - A refresh without the agent key's proof, on a write grant, is refused.
  - A write through a read-only grant is refused, and an agent write to a review-enabled KB before
    the assessment act → `agents_not_assessed`.

## Consequences

- An assistant gets its own identity, scoped below P's and expiring; every record shows "agent A of
  P". C9's grants exist, and ADR-113 O16 is answered: grants are per `(KB, P, agent)`.
- The daemon becomes an issuer of a second token kind and runs a token endpoint. It stays a resource
  server for every other token.
- The daemon holds a second signing key. Rotating it costs every agent one refresh; it touches
  nothing else.
- MCP clients that only send a fixed header are not served in v1; they go through the shim.
- Every request by an agent costs a grant read, a role read and a ceiling read. They are indexed
  point reads on the hot path, measured in G1 against ADR-109's budget.
- The op format gains a delegated variant (G5). Mesh peers refuse it until they verify it, so agent
  edits are hub-local until then.

## Rejected

- **An IdP app password or IdP session in the agent's hands.** Impersonation (RFC 8693 §1.1): the
  daemon sees P, and P's agent could approve its own work.
- **A separate IdP client for MCP hosts, with every token from it classed as an agent.** It names no
  individual agent (one client id for every host), cannot scope by KB without inventing IdP scopes,
  and its separation depends on configuring another system correctly. It can be added later, on top
  of this, with the same rule (O7).
- **An IdP service account per agent.** Strong identity, but an administrator creates each one, P
  cannot revoke it, and nothing keeps it below P. It remains the right shape for an autonomous agent
  that acts for nobody, which is ADR-098's existing `agent` binding.
- **The agent principal equal to P's fingerprint, with a flag.** Every path that forgot the flag
  would give the agent all of P's rights. D1's distinct principal fails closed instead.
- **Rights in the token.** A narrowed or revoked grant would keep working until expiry.
- **A token as long as the grant.** A stolen token would work for days; see D6.
- **The browser client as an MCP server in front of the daemon.** It would either pass the client's
  token through, which the MCP specification forbids, or act with its own credential for someone
  else, the confused deputy.
- **The daemon signing agent ops, or the agent using P's key.** See D7.

## Decided (2026-11-16) and defaults kept

- **O1. The ceiling on KBs without review.** Recommended `read_only` by default (D3): a read grant
  shows the agent only what P can read. The alternative, `none` everywhere, makes every owner opt in.
- **O2. Write custody for MCP hosts.** Recommended: the agent key lives in the shim or the editor,
  never the daemon (D7, D11). The alternative, the daemon holding a per-grant key for hosts that
  cannot run the shim, reads against ADR-036 Amendment 1 F5 and would need it amended.
- **O3. Decided: shim only in v1**, for Claude Code and Claude Desktop, OpenAI Codex, and VS Code with
  Copilot (D11), which must search, read and edit. A fixed-header token is reconsidered only if a
  needed host cannot launch a command.
- **O4. Refresh reuse with no grace window.** Recommended (D6). The cost: a client that loses a
  refresh response must be granted again.
- **O5. Decided: the OS credential store, else a 0600 file** under the XDG data directory, with the
  status naming which (D11). A same-user process with a shell can read either; D6's reuse detection
  is what exposes a stolen copy.
- **O6. Administrators.** Recommended: the operator's local CLI, and owners through the ceiling. The
  alternative is a network administrator role, which would need ADR-117's administration design.
- **O7. Deferred, not designed: the MCP OAuth flow for third-party clients** (the browser client's
  fourth question, answered "not now"), and with it an IdP issuing delegated tokens by RFC 8693
  token exchange. Revisit if a needed MCP host cannot use the shim.
- **O8. Revoking without a fresh second factor.** Recommended (D5): revocation only subtracts.
- **O9. The live-grant limit per person.** Recommended 10.
- **O10. Agent ops on the mesh.** Hub-local until mesh peers verify delegations (recommended), or
  relayed once they do, with revocation bounded only by `not_after`.
- **O11. An owner granting their own agent `read_write` on their own KB.** Raising the ceiling and
  granting are both the owner's acts; recommended allowed, because the agent stays within the
  owner's rights and its writes stay under ADR-113 review where review is on. The alternative
  requires a second person to raise a ceiling.

Every other item above is the recommendation, kept as the default until the owner changes it.
