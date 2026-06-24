# ADR-017: Asymmetric Peer Authentication for Collab (SSH-style keys + TOFU)

**Status:** Accepted (implemented v0.13.x) — `key` mode uses **mTLS** keyed by
the Ed25519 identities; the JSON `KeyAuth` handshake is retained as the
`collab_tls = false` fallback. Per-KB membership + strict identity binding shipped.
**Date:** 2026-06-15
**Supersedes:** None (extends the PSK auth shipped in v0.11.0)
**Depends on:** ADR-006 (collaborative state engine), ADR-007 (save coordination)
**KB Source:** `concept:adr-asymmetric-auth`

## Context

Collab/daemon authentication today is a **symmetric pre-shared key** (HMAC-SHA256
mutual challenge, `shared/mcp/src/auth.rs::PskAuth`). v1 stored the PSK in
`config.toml`; the trusted-keys keystore (this branch) moved it to a
permission-guarded `trusted_keys` file and let a daemon trust a *set* of named
keys. That is a real improvement, but the symmetric model has structural limits:

1. **Shared-secret distribution.** Every peer must obtain the *same* secret over
   a secure side channel. There is no way to add a peer without handing it a
   secret that also lets it impersonate the daemon to others holding that key.
2. **No per-peer revocation without rotation.** Revoking one peer means rotating
   the shared secret and re-distributing it to everyone else.
3. **No trust-on-first-use (TOFU).** A symmetric secret cannot support "accept
   this peer's identity on first connect" — there is no public identity to pin.
   You either already hold the secret or you do not; an unknown peer's proof is
   unverifiable because the verifier lacks its key.

SSH solved these with asymmetric keys: each host/user has a keypair; servers
trust a list of client *public* keys (`authorized_keys`); clients pin a server's
public key on first sight (`known_hosts`) after a TOFU prompt. We adopt the same
model for MAE collab.

## Decision

Add an **asymmetric auth mode (`key`)** alongside the existing `none` and `psk`
modes. It uses **Ed25519** keypairs (via `ed25519-dalek` v2 — fast, 32-byte keys,
audited, no_std-friendly) and a mutual signed-challenge handshake.

### Identities and trust stores

All under `$XDG_DATA_HOME/mae/collab/` (0700 dir):

| File | Role | Analogue |
|------|------|----------|
| `id_ed25519` | this peer's private key (0600) | `~/.ssh/id_ed25519` |
| `id_ed25519.pub` | this peer's public key + label | `~/.ssh/id_ed25519.pub` |
| `known_hosts` | pinned **daemon** public keys, by address | `~/.ssh/known_hosts` |
| `authorized_keys` | trusted **client** public keys (daemon side) | `~/.ssh/authorized_keys` |

Keypairs auto-generate on first use (like `ssh` clients). Public keys are
stored as `ssh-style` lines: `<algo> <base64-pubkey> <label>`. Fingerprints are
`SHA256:<base64(sha256(pubkey))>`, displayed exactly like OpenSSH.

The existing symmetric `trusted_keys` keystore is retained for `psk` mode; the
two formats are distinct files and never mixed.

### Handshake (mutual, signed-challenge)

Replaces the HMAC exchange when `mode = "key"`. Operates on the raw stream
before JSON-RPC `initialize`, same as PSK. `T` = transcript = the concatenation
of both pubkeys and both nonces (binds signatures to this session, preventing
replay and cross-session reuse).

```
1. Client → Server: hello { v, client_pub, client_nonce }
2. Server → Client: offer { server_pub, server_nonce, sig_s = Sign(server_priv, T) }
3. Client:
     - verify sig_s with server_pub                      (server owns its key)
     - look up server_pub in known_hosts[server_addr]:
         pinned & equal     → continue
         pinned & different → ABORT (host key changed — possible MITM)
         unknown            → TOFU policy (below)
     - Client → Server: auth { sig_c = Sign(client_priv, T) }
4. Server:
     - verify sig_c with client_pub                      (client owns its key)
     - look up client_pub in authorized_keys:
         present → OK, authenticated as that label
         absent  → reject; if pending-approval enabled, record in pending list
5. Server → Client: ok | fail { reason }
```

### TOFU policy (client side)

Controlled by option `collab_host_key_policy` (config key
`collaboration.host_key_policy`):

| Value | Behavior |
|-------|----------|
| `prompt` (interactive default) | MiniDialog: "Daemon at <addr> presented key SHA256:… — accept and pin? [y/N]". Accept → write to `known_hosts`; reject → abort. |
| `accept-new` | Auto-pin unknown hosts (no prompt), but still ABORT on a *changed* key. Mirrors OpenSSH `StrictHostKeyChecking=accept-new`. Default in headless/`--test`/CI. |
| `strict` | Never auto-pin; unknown or changed host key aborts. For locked-down deployments. |

Headless contexts (`mae --test`, no TTY) cannot prompt, so they default to
`accept-new`. A first-run interactive session uses `prompt`.

### Unknown clients (daemon side)

The daemon is a background service and cannot prompt. Unknown client keys are
**rejected**, never auto-trusted. Two knobs:

- `[collab.auth] pending_approval = true` records rejected-but-well-formed client
  keys in a `pending` list (with fingerprint + label + first-seen) and rejects
  with reason `"pending admin approval"`.
- Admin approves out-of-band:
  - `mae-daemon authorized` — list trusted client keys.
  - `mae-daemon pending` — list keys awaiting approval.
  - `mae-daemon authorize <fingerprint|label>` — move pending → authorized.
  - `mae-daemon revoke <label>` — remove from authorized_keys (per-peer
    revocation, no secret rotation).

### Daemon identity CLI

- `mae-daemon identity` — print this daemon's public key + fingerprint (generates
  the keypair if absent). The operator shares the fingerprint out-of-band so
  clients can verify the TOFU prompt.

### Coexistence and selection

`[collab.auth] mode` ∈ `{none, psk, key}`. A daemon runs one mode at a time
(v1). `key` is the recommended mode for multi-user/multi-machine; `psk` remains
for quick shared-secret setups; `none` for trusted-loopback only. Mixing
`psk`+`key` on one listener is a possible future extension, not in this ADR.

## Consequences

**Positive**
- TOFU: peers can establish trust on first connect with fingerprint verification.
- Per-peer revocation without rotating anyone else's credentials.
- No shared-secret distribution; private keys never leave their host.
- Familiar SSH mental model and file layout lowers the learning curve.

**Negative / costs**
- New crypto dependency (`ed25519-dalek`) in `mae-mcp` (both workspaces).
- Larger handshake + protocol surface to test and review (security-sensitive).
- Interactive TOFU needs an editor MiniDialog path *and* a headless policy.
- Daemon gains stateful trust stores (`authorized_keys`, `pending`) and CLI.

## Alternatives considered

- **Symmetric keystore only (this branch's v1).** Simpler, already shipped, but
  no TOFU and no clean per-peer revocation. Kept as `psk` mode for simple cases.
- **mTLS / rustls with client certs.** Heavier (PKI, cert lifecycles), and the
  transport is currently plain framed JSON-RPC. A TLS-terminating proxy
  (`stunnel`) already covers wire encryption for untrusted networks; this ADR is
  about *peer identity*, which Ed25519 covers with far less machinery.
- **OAuth/OIDC.** Enterprise-oriented, requires an IdP; out of scope for a
  local-first editor. Reserved for a future enterprise mode.

## Migration

- `mode = "psk"` and `mode = "none"` continue to work unchanged.
- Switching a deployment to `key`: run `mae-daemon identity` on the host; each
  client connects once (TOFU-pins the host), and the admin authorizes each
  client's key (`pending` → `authorize`). No flag day; PSK and key daemons can
  coexist on different ports during transition.

## Implementation phases

1. `mae-mcp`: Ed25519 keypair type, `known_hosts`/`authorized_keys` parsers,
   fingerprints, `KeyAuth` provider (signed-challenge handshake) + tests.
2. Daemon: `mode = "key"` wiring, trust-store load, pending list, `identity` /
   `authorized` / `pending` / `authorize` / `revoke` CLI, doctor/check-config.
3. Editor: client identity, `known_hosts` pinning, `collab_host_key_policy`
   option, interactive TOFU MiniDialog, headless `accept-new`.
4. Docs + harness + two-machine validation.
