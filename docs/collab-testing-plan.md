# Trusted-Peer Collaboration — Testing Plan

Step-by-step validation of the trusted-peer collaboration & KB-replication
feature (ADR-017): Ed25519 mTLS, TOFU pinning, strict identity binding, per-KB
membership. Three tiers — automated, single-host manual, and the **two-machine
live run** (the real multi-machine goal).

> Branch: `feat/crdt-collab-validation`. Both machines must build from it.
> Reference topology used below: **D** = daemon host `framework` (`192.168.1.137`),
> **E** = a second editor machine on the same LAN. Substitute your own IPs.

---

## Tier 0 — Automated (run on one machine; also runs in CI)

```bash
# Unit tests (both workspaces)
cargo test --workspace --exclude mae-gui     # editor + shared crates
cargo test -p mae --bins                     # collab_bridge, TOFU verifier, dispatch
cd daemon && cargo test && cd ..             # daemon: strict-binding + membership

# End-to-end (real daemon + editors over mTLS, headless, self-cleaning)
make test-collab-mtls-e2e          # single-host mTLS: connect, share, peer authed
make test-collab-membership-e2e    # two-editor: non-member denied → add → allowed
make test-collab-e2e-all           # both
```

**Pass:** all green; the e2e scripts print `PASS:`. (CI runs the same two scripts
against release artifacts in the `e2e` job.)

---

## Tier 1 — Single-host manual smoke (~5 min)

Confirms the binaries + CLI on one machine before involving a second.

```bash
DD=/tmp/mae-smoke; rm -rf $DD; mkdir -p $DD/{srv/.config/mae,srv/.local/share,cli/.config/mae,cli/.local/share}
srv(){ HOME=$DD/srv XDG_CONFIG_HOME=$DD/srv/.config XDG_DATA_HOME=$DD/srv/.local/share "$@"; }
cli(){ HOME=$DD/cli XDG_CONFIG_HOME=$DD/cli/.config XDG_DATA_HOME=$DD/cli/.local/share "$@"; }

printf '[collab]\nbind="127.0.0.1:9490"\n[collab.auth]\nmode="key"\n' > $DD/srv/.config/mae/daemon.toml

srv mae-daemon identity                       # → daemon fingerprint + pubkey
LINE=$(cli mae --collab-identity | sed -n 's/.*public key:  //p')
srv mae-daemon authorize $LINE alice          # authorize the editor as "alice"
srv mae-daemon --check-config                 # → auth.mode=key, tls=true, 1 key
```

**Pass:** `identity` prints a `SHA256:` fingerprint; `authorize` succeeds;
`--check-config` ends with `Config OK` and shows `auth.tls: true` + 1 authorized key.

---

## Tier 2 — Two-machine live run (the multi-machine validation)

**D = daemon host + editor "alice"; E = editor "bob".** Both connect to D's
daemon over the LAN. Use your real config dirs (not isolated temp dirs).

### Step 1 — Prereqs (both machines)
- [ ] Both built from `feat/crdt-collab-validation` (`mae --version`, `mae-daemon --version` match).
- [ ] On the same LAN; D's IP known (`ip -4 addr` → e.g. `192.168.1.137`).
- [ ] Port `9473` open on D (firewall): `sudo firewall-cmd --add-port=9473/tcp` (Fedora) / `sudo ufw allow 9473/tcp`.

### Step 2 — Start the daemon on D (key + mTLS, all interfaces)
`~/.config/mae/daemon.toml` on **D**:
```toml
[collab]
bind = "0.0.0.0:9473"
[collab.auth]
mode = "key"
```
```bash
# D:
mae-daemon identity            # note D's fingerprint — you'll verify it on E
mae-daemon                     # (or: systemctl --user start mae-daemon)
ss -tlnp | grep 9473           # confirm listening on 0.0.0.0:9473
```
- [ ] D listens on `0.0.0.0:9473`; daemon log says `collab authentication configured (mTLS)`.

### Step 3 — Exchange + authorize identities
```bash
# E: print bob's identity line
mae --collab-identity          # → mae-ed25519 <b64> <hostname>

# D: authorize bob (relabel as "bob"), and alice (D's own editor)
mae-daemon authorize mae-ed25519 <bob-b64> bob
mae --collab-identity          # alice's line (on D)
mae-daemon authorize mae-ed25519 <alice-b64> alice
mae-daemon authorized          # → lists alice + bob with fingerprints
```
- [ ] `mae-daemon authorized` lists both `alice` and `bob` with distinct fingerprints.
- [ ] Reachability: on **E**, `nc -zv 192.168.1.137 9473` succeeds.

### Step 4 — Connect both editors (TOFU)
On **both** D (alice) and E (bob), in `init.scm`:
```scheme
(set-option! "collab-auth-mode" "key")
(set-option! "collab-server-address" "192.168.1.137:9473")  ; loopback ok on D
(set-option! "collab-auto-connect" "true")
;; collab-host-key-policy defaults to "prompt"
```
Launch `mae`; on first connect each editor shows **"Trust Daemon Key? SHA256:…
[y/N]"**.
- [ ] The fingerprint shown matches `mae-daemon identity` from Step 2. Press `y`.
- [ ] `:collab-status` shows Connected. Daemon log: `mTLS client authenticated peer=alice` / `peer=bob`.

### Step 5 — Buffer collaboration converges
- [ ] On **D (alice)**: open/create a file, `:collab-share`.
- [ ] On **E (bob)**: `:collab-join <name>` (or `SPC C j` picker). The buffer appears with alice's content.
- [ ] Type on **both** simultaneously → edits converge on both (CRDT). Remote cursor shows the **authenticated** label (`alice`/`bob`), even if `collab-user-name` is set to something else (strict binding).

### Step 6 — Shared KB membership
- [ ] On **D (alice, owner)**: `:kb-share`.
- [ ] On **E (bob)**: `:kb-join default` → **denied** ("not a member"). Daemon log: `kb/join denied`.
- [ ] On **D**: `:kb-member-add default bob`. Daemon log: `kb membership change member=bob add=true`.
- [ ] On **E**: `:kb-join default` → **succeeds**; bob sees the KB.
- [ ] On **D**: `:kb-member-remove default bob` → bob's next KB node edit is rejected.

### Step 7 — Security / negative checks
- [ ] **Unauthorized peer:** a 3rd machine NOT in `authorized_keys` → connect fails (daemon log: `verify_client_cert` rejection / TLS refused).
- [ ] **Changed host key:** on D, delete `~/.local/share/mae/collab/id_ed25519` and restart the daemon (new identity). On E, reconnect → editor **aborts** with a host-key-changed error (MITM defense). Restore by re-pinning (delete E's `known_hosts` entry).
- [ ] **Confidentiality:** on D, `sudo tcpdump -A -i any port 9473` during a key-mode session → shows TLS records, **not** plaintext JSON-RPC. (Contrast: a `psk`-mode session is plaintext.)

---

## Results checklist

| # | Check | Pass? |
|---|-------|-------|
| T0 | `make test-collab-e2e-all` green | ☐ |
| T1 | Single-host CLI smoke (identity/authorize/check-config) | ☐ |
| 2 | Daemon listens `0.0.0.0:9473`, mTLS configured | ☐ |
| 3 | Both peers authorized; E reaches D:9473 | ☐ |
| 4 | TOFU prompt fingerprint matches; both connect | ☐ |
| 5 | Buffer edits converge; cursor labels = authenticated identity | ☐ |
| 6 | KB join denied → owner adds → allowed → remove denies | ☐ |
| 7a | Unauthorized peer rejected | ☐ |
| 7b | Changed host key aborts | ☐ |
| 7c | Traffic is TLS-encrypted (tcpdump) | ☐ |

---

## Troubleshooting
- **TLS handshake EOF / connection refused:** wrong daemon (check `ss -tlnp | grep 9473`), or daemon not in `key`+`tls` mode (`mae-daemon --check-config`).
- **"client key not authorized":** the editor's pubkey isn't in `authorized_keys` — re-run `mae-daemon authorize`.
- **TOFU never appears / auto-connects:** `collab-host-key-policy` is `accept-new`, or the host is already pinned in `~/.local/share/mae/collab/known_hosts`.
- **KB join always allowed (no denial):** both peers share the same authorized-keys **label** → give them distinct labels in `mae-daemon authorize`.
- **Logs:** daemon `MAE_LOG=info mae-daemon`; editor `MAE_LOG="mae::collab_bridge=debug,info"`.
