# End-to-End Encrypted KB Sharing — User Guide

> **Audience:** anyone sharing a MAE knowledge base and wanting its *content* hidden from the
> daemon/hub/relay that stores or forwards it. This is the practical runbook: enable E2E, manage
> members, rotate and recover your identity, and understand — honestly — what the encryption does
> and does **not** protect.
>
> For the cryptographic design-of-record (primitives, threat model, security review) see
> [`E2E_ENCRYPTION.md`](E2E_ENCRYPTION.md). For running the daemon that hosts a share see
> [`DAEMON_ADMIN.md`](DAEMON_ADMIN.md). For the sharing/membership model see
> [`KB_SHARING.md`](KB_SHARING.md).

Every action below is available identically to the three peer actors (principle #3):

| Action | Human command | Scheme primitive | AI / MCP tool |
|---|---|---|---|
| Enable E2E | `:kb-set-encryption <kb> e2e` | `(kb-set-encryption "<kb>" "e2e")` | `kb_set_encryption` |
| Share a KB | `:kb-share <kb>` | `(kb-share "<kb>")` | `kb_share` |
| Join a KB | `:kb-join <kb>` | `(kb-join "<kb>")` | `kb_join` |
| Add a member | `:kb-add-member <kb> <fp> <role>` | `(kb-add-member "<kb>" "<fp>" "<role>")` | `kb_add_member` |
| Remove a member | `:kb-remove-member <kb> <fp>` | `(kb-remove-member "<kb>" "<fp>")` | `kb_remove_member` |
| Rotate identity | `:collab-rotate-identity` | `(run-command "collab-rotate-identity")` | `collab_rotate_identity` |
| Register recovery key | `:collab-register-recovery-key` | `(run-command "collab-register-recovery-key")` | `collab_register_recovery_key` |
| Recover identity | `:collab-recover-identity <path> <old-fp>` | `(execute-ex "collab-recover-identity <path> <old-fp>")` | `collab_recover_identity` |

---

## 1. Your keys — where they live and why you must back them up

MAE derives your entire collaborative identity from a single 32-byte Ed25519 seed. It lives in
your **collab directory**:

```
$XDG_DATA_HOME/mae/collab/          (default: ~/.local/share/mae/collab/)
├── id_ed25519        # your PRIVATE identity seed (0600, hex) — the root of everything
├── id_ed25519.pub    # your public key line: `mae-ed25519 <b64> <label>`
└── recovery/
    └── id_ed25519    # your offline RECOVERY key (only after :collab-register-recovery-key)
```

> [!CAUTION]
> **`id_ed25519` is the single root of your access to every KB you share or join.** There is no
> server-side copy and no password reset. If you lose it *and* have no recovery key registered,
> your seats in every shared KB are unrecoverable and any content sealed only to you is lost.
>
> **Back it up now** — copy `id_ed25519` to an offline medium (an encrypted volume, a hardware
> token, a password manager). The keys are stored as **plaintext protected by file permissions
> only** (`0600` in a `0700` dir); at-rest passphrase/keychain encryption is a tracked future item
> (I3), not shipped in v0.15. Treat the collab directory like `~/.ssh`.

**Two independent safety nets** — use both:

1. **Back up `id_ed25519`** (above) — restores the *same* identity on a new machine.
2. **Register a recovery key** (§5) — lets a *fresh* key inherit your seats if the primary is lost
   or compromised, without ever exposing the primary.

---

## 2. Enabling E2E on a KB

E2E is **owner-only** and set per-KB. Share the KB first, then enable encryption:

```
:kb-share my-notes
:kb-set-encryption my-notes e2e
```

What happens:

- A per-KB symmetric **content key** is generated and sealed (wrapped) to each current member's
  published X25519 wrap key (ADR-041). Only members can unwrap it.
- From this point node **content** rides the CRDT as ciphertext (XChaCha20-Poly1305). A daemon,
  hub, or relay that stores or forwards the KB but is **not a member** holds only ciphertext.
- The change is recorded as a signed `SetEncryption` op in the KB's membership op-log, so every
  member converges on "this KB is E2e" and the setting cannot be silently downgraded by a
  non-owner (enforced daemon-side — see the append-only op-log gate, ADR-039).

> [!NOTE]
> E2E today is validated and gated on the **mTLS hub** (production-ready). The **P2P mesh** ships
> as **beta**; enabling E2E on a mesh-only share is not yet anchored end-to-end (see §7). Share
> over the hub when you need the encryption guarantee.

To confirm state at any time: `:kb-sharing-status` (or the `*KB Sharing*` buffer, `SPC C K m`).

---

## 3. Managing members

Membership is identity-anchored: members are named by their key **fingerprint** (`SHA256:…`), not
by a username. Get a peer's fingerprint from their `id_ed25519.pub` or their `:collab-status`.

```
:kb-add-member my-notes SHA256:abc… editor      # roles: viewer | editor | owner
:kb-remove-member my-notes SHA256:abc…
:kb-set-policy my-notes invite                  # restrictive | invite | permissive
:kb-approve my-notes SHA256:def… viewer         # approve a pending join request
```

- **Adding** a member re-seals the current content key to their wrap key so they can read from
  their join forward.
- **Removing** a member **rotates the content key** (a new epoch): subsequent ops are sealed under
  a key the removed member never had. See §7 for the honest limit — removal protects *future*
  content, not content the removed member could already read.
- Roles are hierarchical: `owner ⊇ editor ⊇ viewer`. Only the owner can add/remove members, set
  policy, or enable encryption.

---

## 4. Rotating your identity (ADR-040)

Rotate when you want to move to a fresh key — routine hygiene, a new device, or a suspected
compromise. Rotation cross-signs a successor: your **old key endorses the new key**, and across
every KB you own or belong to the rotation is recorded so peers accept the new key as *you*.

```
:collab-rotate-identity
```

What it does, in order:

1. Generates a fresh keypair.
2. For every KB you belong to, authors a `Rebind` op **signed by your old key** naming the new key
   as your successor (same role, no self-elevation). For KBs you **own**, it also re-wraps the
   content key to your new wrap key.
3. Saves the new key to your collab directory (replacing `id_ed25519`) and switches to it.

> [!IMPORTANT]
> Your **node-id changes** when you rotate (it is derived from your key). The daemon/peers must
> **authorize the new key out-of-band** before you reconnect — on the daemon:
> `mae-daemon authorize mae-ed25519 <b64> <label>` (the line from your new `id_ed25519.pub`; see
> [`DAEMON_ADMIN.md`](DAEMON_ADMIN.md)). This is a deliberate manual step: rotation is not a live
> re-handshake.
>
> **First edit after rotation:** because your per-node op author-id is derived from your key, the
> first edit to an existing node under the new key trips the ADR-023 epoch fence once ("rebase
> required"). With `collab-fence-resolution=auto` (the default advisory) it re-authors transparently;
> otherwise re-issue the edit.

Rotations compose: chained rotations, an owner rotating, and a member rotating **after** the owner
has itself rotated all converge — the owner reactively re-wraps the content key to a rotated
member's new key regardless of whether the owner has rotated first. CI-gated (`MAE_E2E_ROTATE`).

---

## 5. Recovery — surviving a lost or compromised primary key

Recovery uses a **pre-registered offline key**. You register it *while healthy*; if the primary is
later lost or compromised, the recovery key authorizes a rebind onto a fresh primary — the primary
itself is never needed for recovery.

### 5a. Register a recovery key (do this now, while healthy)

```
:collab-register-recovery-key
```

- Generates a fresh offline recovery key, registers its public half (a signed `RegisterRecoveryKey`
  op) across every KB you belong to, and saves the **secret** to
  `~/.local/share/mae/collab/recovery/id_ed25519` — a path distinct from your primary so it is not
  clobbered.
- The status report prints the recovery fingerprint and the save path.

> [!CAUTION]
> **Move the recovery key OFFLINE and remove it from this machine.** Anyone who holds it can rotate
> your identity. Keeping it next to your primary defeats its purpose — store it separately (a
> different medium than your `id_ed25519` backup). The **latest** registration wins, so you can
> re-register to rotate the recovery key itself.

### 5b. Recover after a key loss

On a machine with the restored recovery key available and a fresh primary already generated and
**authorized out-of-band**:

```
:collab-recover-identity <path-to-recovery-dir> <old-fingerprint>
```

- `<path-to-recovery-dir>` holds the restored `id_ed25519` recovery key.
- `<old-fingerprint>` is the lost key's `SHA256:…`.
- This authors a **recovery-signed rebind** so your new primary inherits the lost key's seats in
  every KB where the recovery key was registered. Validated end-to-end and CI-gated
  (`MAE_E2E_RECOVER`).

### 5c. Owner-mediated recovery (no recovery key registered)

If a member lost their key and never registered a recovery key, the **owner** can restore access
manually: remove the lost member, rotate the KB's content key, then re-add the member's new
fingerprint and have them re-join. This is the fallback, not the happy path — §5a is why you
register a recovery key in advance.

---

## 6. Quick end-to-end walkthrough

```
# Owner
:kb-share team-kb
:kb-set-encryption team-kb e2e
:collab-register-recovery-key            # ← back the printed recovery key up OFFLINE
:kb-add-member team-kb SHA256:member… editor

# Member
:kb-join team-kb                         # pulls the sealed content key + node content
# …edit collaboratively; the hub sees only ciphertext…

# Later: member rotates to a new laptop
:collab-rotate-identity                  # then authorize the new id_ed25519.pub on the daemon

# Owner off-boards someone
:kb-remove-member team-kb SHA256:leaver… # content key rotates; future ops sealed anew
```

---

## 7. What E2E protects — and what it does **not** (read this)

E2E in MAE delivers exactly one property: **confidentiality of node *content* from non-members**
(the daemon/hub/relay/mesh host, and a removed member — for content authored after their removal).
It is deliberately *not* more than that. Be honest with yourself about the boundaries:

**Not hidden (metadata):**
- **Membership, roles, and authorship** — the signed op-log is not encrypted; a relay sees who is
  in the KB and who authored each op.
- **Op sizes, counts, and timing** — traffic analysis can infer activity and rough content size.
- **The node/link graph shape** — node ids and manifest entries are visible to the host.

**Not protected against:**
- **A member you admitted.** Any current member holds the content key and can read everything and
  leak it. E2E guards against non-members, not insiders.
- **Forward secrecy / post-compromise security (FS/PCS).** There is no ratchet. If a content key
  leaks, all history sealed under it is exposed. Removal re-keys **forward only** — a removed member
  who kept the ciphertext *and* the old key can still read pre-removal content. (BeeKEM/TreeKEM-style
  PCS is tracked for a future release, ADR-037 §D4.)
- **At-rest key theft.** Keys on disk are plaintext under file permissions only (§1). Anyone who
  reads your collab directory is you. At-rest passphrase/keychain protection (I3) is deferred.
- **Key loss without a backup or recovery key** — unrecoverable (§1, §5).

**v0.15 scope limits (tracked, being tightened):**
- **E2E on the P2P mesh** — content now **seals over the mesh** and the relaying daemons are
  key-blind (gated: ADR-043 + the `MAE_E2E_MESH` CI gate). But a member **joining over the mesh
  cannot decrypt yet** — their wrap key doesn't reach the owner through the mesh join path, so they
  are admitted keyless (tracked, #255). So: **enable E2E on the hub** for now; mesh E2E is
  seal-capable but not member-readable end-to-end. (Also: the **permissive** join policy is
  incompatible with E2E — it admits keyless members; use **invite** with E2E.)
- **Joining after a removal + rotation** cannot decrypt ops sealed before you had access.

If your threat model needs metadata privacy, insider protection, or forward secrecy, MAE's E2E is
**not sufficient today** — track the ADR-037 §D4 roadmap. For the full cryptographic review and
prior-art positioning (vs. Signal/MLS/Matrix/Keyhive) see [`E2E_ENCRYPTION.md`](E2E_ENCRYPTION.md)
§7 and [`SECURITY_REVIEW.md`](SECURITY_REVIEW.md).
