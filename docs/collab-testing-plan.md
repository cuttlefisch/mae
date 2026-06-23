# Trusted-Peer Collaboration — Testing Plan

Step-by-step validation of the trusted-peer collaboration & KB-replication
feature (ADR-017): Ed25519 mTLS, TOFU pinning, strict identity binding, per-KB
membership. Three tiers — automated, single-host manual, and the **two-machine
live run** (the real multi-machine goal).

> Branch: `feat/crdt-collab-validation`. Both machines must build from it.
> Reference topology used below: **D** = daemon host `framework` (`192.168.1.137`),
> **E** = a second editor machine on the same LAN. Substitute your own IPs.

---

## Live-run coordination board (this session)

> Shared scratchpad — **each machine fills in its own row, commits, and pushes**.
> Both connect by explicit `host:port` (mDNS came back empty on this LAN, so don't
> rely on discovery). Min build: commit `b947a52` or later.

> [!IMPORTANT]
> **Known bug — use `accept-new`, not the `prompt` default ([issue #66]).** The
> interactive TOFU `prompt` policy (what `mae setup-collab` writes) is unwired and
> **freezes the editor** (~120s, then fails) — hard-freezes the **TUI**, silently
> fails the **GUI**. This bites **every** editor, including **D's own "alice"** (it
> connects to D's daemon too). Until #66 is fixed, **each editor must add**
> `(set-option! "collab_host_key_policy" "accept-new")` to `init.scm` (non-blocking,
> auto-pins on first sight). Verify the daemon fingerprint **out-of-band**: after
> connecting, check the pinned `~/.local/share/mae/collab/known_hosts` entry equals
> D's `mae-daemon identity` SHA256.
>
> [issue #66]: https://github.com/cuttlefisch/mae/issues/66

| Role | Machine | LAN IP | Built ≥ `b947a52` | `accept-new` set | Identity fingerprint | Status |
|------|---------|--------|-------------------|------------------|----------------------|--------|
| **D** = daemon host + editor "alice" | _framework (driver)_ | `192.168.1.137`? | ☐ | ☐ | _paste `mae-daemon identity` SHA256 here_ | daemon listening :9480 |
| **E** = editor "bob" | `Marthas-MacBook-Pro` (mac) | `192.168.1.132` | ✅ | ✅ | `SHA256:9xLh0DWeeAi3hl2W7yudaE05aTHtYQpNUUyMWO+2CrI` | **dev GUI build up, connecting** |

**Chosen test port:** `9480` (avoids the personal-daemon `:9473` collision).

### D — to do (driver)
1. `git pull --rebase` to ≥ `b947a52`; **rebuild both binaries** — `make build-daemon`
   plus `make build` (GUI) or `make build-tui`. The branch moved since the harness was
   first built (cross-platform e2e fix `a8ac842`), so a rebuild is required.
2. `~/.config/mae/daemon.toml`: `[collab] bind = "0.0.0.0:9480"` + `[collab.auth] mode = "key"`.
3. Authorize bob (E's key is already generated):
   ```
   mae-daemon authorize mae-ed25519 aBjMkdzHH9YVUxfP5NxHJo7fcu5qGC75pUl1SWdAvnM= bob
   ```
4. `mae-daemon identity` → paste D's fingerprint into the board above.
5. `mae-daemon`; open firewall: `sudo firewall-cmd --add-port=9480/tcp` (or `ufw allow 9480/tcp`).
6. **Before launching alice's editor:** `mae setup-collab --server 127.0.0.1:9480`, then add
   `(set-option! "collab_host_key_policy" "accept-new")` to `init.scm` (see #66 box) or it freezes.
7. Fill in / confirm D's row (IP, fingerprint, both checkboxes, status → "listening").

### E — ready state (mac, done)
- Built from `b947a52` (`mae`/`mae-daemon` 0.13.12, **dev GUI** via `make build`); local personal daemon **stopped** (port `9473` clear).
- Identity at `~/.local/share/mae/collab/id_ed25519` (line + fingerprint in the board).
- `init.scm`: key mode, `server 192.168.1.137:9480`, auto-connect, **`accept-new`**.
- Editor up, MCP-driven; on connect, verify the pinned `known_hosts` fingerprint == D's row, then run Steps 5–7.

---

## Setup — build + dependencies (do this first, on every machine)

The repo is **two Cargo workspaces** that must both be built: the editor (repo
root) and the daemon (`daemon/`).

### Toolchain + system packages
- **Rust ≥ 1.95** (MSRV): `rustup default stable` (or `rustup toolchain install 1.95`).
- **iproute2** (the e2e scripts use `ss` for port readiness):
  - Fedora: `sudo dnf install iproute` · Debian/Ubuntu: `sudo apt install iproute2`
- **GUI build only** (skip if using the TUI build, which is enough for collab tests):
  - Fedora: `sudo dnf install clang fontconfig-devel freetype-devel`
  - Debian/Ubuntu: `sudo apt install clang libfontconfig1-dev libfreetype6-dev`

### Build both binaries
```bash
git fetch && git checkout feat/crdt-collab-validation && git pull

make build-tui        # editor (release, terminal) → target/release/mae
make build-daemon     # daemon (release)           → daemon/target/release/mae-daemon
# (or `make build` for the GUI editor; or debug: `cargo build` + `cd daemon && cargo build`)

./target/release/mae --version
./daemon/target/release/mae-daemon --version       # both should match
```

> **Tip:** the Makefile e2e targets (`make test-collab-e2e-all`) build both binaries
> for you, so for Tier 0 you can skip the manual build. For the **two-machine run**
> you need the binaries on `PATH` — `make install` / `make install-daemon`, or
> copy `target/release/mae` and `daemon/target/release/mae-daemon` into `~/.local/bin`.

### Key setup — who generates what
| Tier | Keys | How |
|------|------|-----|
| **0 automated** | generated + authorized **by the scripts** | nothing to do — `collab-mtls-e2e.sh` / `collab-membership-e2e.sh` create isolated identities, authorize them, and clean up |
| **1 / 2 manual** | **you** generate + authorize | identities auto-generate on first `mae --collab-identity` / `mae-daemon identity`; you exchange the public-key lines and run `mae-daemon authorize` (Tier 2 Step 3) |

Identities live under `$XDG_DATA_HOME/mae/collab/` (`~/.local/share/mae/collab/`):
`id_ed25519` (private, 0600), `id_ed25519.pub`, `known_hosts` (pinned daemons),
`authorized_keys` (daemon's trusted clients). To reset a peer's identity, delete
`id_ed25519*` and re-run — then re-authorize it.

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

> **Port choice — avoid colliding with an already-running daemon.** The default
> collab port is `9473`. If you already run a personal `mae-daemon` on D (it binds
> `127.0.0.1:9473`), a **test** daemon binding `0.0.0.0:9473` will fail to start —
> `0.0.0.0` includes loopback, so the two overlap ("address already in use →
> collab disabled"). This plan uses **`9480`** for the test daemon to sidestep that.
> Check first: `ss -tlnp | grep -E ':(9473|9480)'` should show nothing for the port
> you pick. Substitute any free port; just keep `bind`, the firewall rule, and
> `--server` consistent.
>
> **Bind vs. connect — `0.0.0.0` is NOT a connect target.** The daemon *binds*
> `0.0.0.0:<port>` (listen on all interfaces). Editors *connect to* D's **reachable
> IP** — D's LAN IP from E, or `127.0.0.1` from D's own editor — never `0.0.0.0`.
> `mae setup-collab --server 0.0.0.0:…` is rejected for this reason.

### Step 1 — Prereqs (both machines)
- [ ] Both built from `feat/crdt-collab-validation` (`mae --version`, `mae-daemon --version` match).
- [ ] On the same LAN; D's IP known (`ip -4 addr` → e.g. `192.168.1.137`).
- [ ] Chosen test port (`9480` here) free on D: `ss -tlnp | grep 9480` shows nothing.
- [ ] Port `9480` open on D (firewall): `sudo firewall-cmd --add-port=9480/tcp` (Fedora) / `sudo ufw allow 9480/tcp`.

### Step 2 — Start the daemon on D (key + mTLS, all interfaces)
`~/.config/mae/daemon.toml` on **D**:
```toml
[collab]
bind = "0.0.0.0:9480"
[collab.auth]
mode = "key"
```
```bash
# D:
mae-daemon identity            # note D's fingerprint — you'll verify it on E
mae-daemon                     # (or: systemctl --user start mae-daemon)
ss -tlnp | grep 9480           # confirm listening on 0.0.0.0:9480
```
- [ ] D listens on `0.0.0.0:9480`; daemon log says `collab authentication configured (mTLS)`.

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
- [ ] Reachability: on **E**, `nc -zv 192.168.1.137 9480` succeeds.

### Step 4 — Connect both editors (TOFU)
On **both** D (alice) and E (bob), the one-command setup (idempotent — generates
the identity + writes the options to `init.scm`):
```bash
# E (bob) connects to D's LAN IP; D (alice) uses 127.0.0.1 (its own daemon).
mae setup-collab --server 192.168.1.137:9480   # on E
mae setup-collab --server 127.0.0.1:9480        # on D (alice's editor)
# (add --ssh-key ~/.ssh/id_ed25519 to reuse an existing SSH key as the identity)
```
Equivalent manual `init.scm`:
```scheme
(set-option! "collab-auth-mode" "key")
(set-option! "collab-server-address" "192.168.1.137:9480")  ; 127.0.0.1:9480 on D
(set-option! "collab-auto-connect" "true")
(set-option! "collab-host-key-policy" "accept-new")  ; NOT "prompt" — see #66 box
```
> If you used `--ssh-key`, authorize each peer in Step 3 with
> `mae-daemon authorize --from-ssh-pub <peer>.pub <label>` instead of the
> `mae-ed25519` line.

> [!WARNING]
> `setup-collab` leaves `collab-host-key-policy = "prompt"`, which freezes the editor
> ([#66]). **Set it to `accept-new` before launching** (line above). The interactive
> "Trust Daemon Key? [y/N]" prompt below is the *intended* UX once #66 is fixed — for
> now `accept-new` auto-pins silently and you verify the fingerprint out-of-band.
>
> [#66]: https://github.com/cuttlefisch/mae/issues/66

Launch `mae`. With `accept-new`, it connects + auto-pins (no prompt). Verify the pin:
- [ ] `:collab-status` shows Connected. Daemon log: `mTLS client authenticated peer=alice` / `peer=bob`.
- [ ] **Out-of-band key check:** the pinned line in `~/.local/share/mae/collab/known_hosts`
      for D's address matches D's `mae-daemon identity` SHA256 (catches MITM that silent
      auto-pin would otherwise miss).
- [ ] *(Deferred to #66)* with `prompt` policy, the editor shows **"Trust Daemon Key? SHA256:… [y/N]"**
      and the fingerprint matches Step 2 — re-test this path once #66 lands.

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
- [ ] **Confidentiality:** on D, `sudo tcpdump -A -i any port 9480` during a key-mode session → shows TLS records, **not** plaintext JSON-RPC. (Contrast: a `psk`-mode session is plaintext.)

### Step 8 — B-19: viewer-era edits must NOT cascade on grant (ADR-023 epoch fence)

> **What we're proving.** A member who edits a node while a **viewer** (denied at the
> daemon) must NOT have those pre-grant edits silently cascade to everyone once they
> are later promoted to **editor**. The daemon's **epoch fence** rejects the pre-grant
> lineage (`rebase required`); only fresh, current-epoch edits are accepted. The client
> is assumed hostile, so this is enforced **daemon-side** — see ADR-023.
>
> **Min build:** commit `fac00959` or later (daemon fence + editor rotation). This is a
> NEW build for both machines.

> [!IMPORTANT]
> **Verify the running binary == the new build before testing** (the deploy gotcha that
> burned us on B-18): `sha256sum ./target/release/mae` vs the binary the running PID
> actually exec'd — `sha256sum /proc/$(pgrep -n mae)/exe` (Linux) or re-`install` then
> relaunch. A stale `~/.local/bin/mae` will silently "fail" the fix.

**Roles:** D = **alice** (owner), E = **bob** (the promoted member). `<bob-fp>` is bob's
authorized key fingerprint (`mae-daemon identity` on E, or `:collab-status`).

> **This session's concrete values** (substitute if yours differ):
> - **KB:** `collabtest` · **edit target node:** `collabtest:beta` (nodes: `alpha`/`beta`/`overview`).
> - **`<bob-fp>` = `SHA256:9xLh0DWeeAi3hl2W7yudaE05aTHtYQpNUUyMWO+2CrI`** (alice = `SHA256:+jBin…`).
> - **bob connects MANUALLY** (autoconnect disabled) — after launch bob runs `:collab-connect`,
>   then `:kb-join collabtest`. This is deliberate, for granular control over the timing below.

> [!NOTE]
> **Pre-step (alice) — reset bob to viewer.** From prior T7 runs bob is a **leftover editor**
> on `collabtest`. Start the exploit from a clean viewer state: `:kb-member-add collabtest <bob-fp> viewer`.
> (Downgrade editor→viewer is itself a role change ⇒ the daemon bumps bob's epoch — expected.)

1. **alice (D):** KB `collabtest` is already shared; pick the target node `collabtest:beta`.
   Ensure bob is connected (he triggers `:collab-connect` manually) and has done the Pre-step reset.
2. **alice (D):** confirm bob is a **viewer** — daemon log `kb membership change … role="viewer"`.
3. **bob (E):** `:collab-connect` then `:kb-join collabtest` → succeeds (read-only). Open
   `collabtest:beta` and **edit it** to something unmistakable, e.g. `VIEWER-ERA-HIJACK`.
   - [ ] The edit shows in **bob's local** buffer, but the daemon **denies** the write —
         daemon log `kb/node_update denied` (viewer, least privilege). bob's status shows
         the rejection. The divergent edit now lives **only** in bob's local copy.
   - [ ] **alice (D)** does **NOT** see `VIEWER-ERA-HIJACK` (it never reached the daemon).
4. **alice (D):** promote bob to **editor** — `:kb-member-add collabtest <bob-fp> editor`
   (a role *change* ⇒ the daemon bumps bob's authorization epoch again).
5. **bob (E):** trigger a sync of the pre-grant edit — either make **one more edit** to
   `collabtest:beta`, or reconnect (`:collab-disconnect` then `:collab-connect`) so the
   ADR-022 reconcile pushes bob's local-ahead. Observe the fence fire:
   - [ ] **Daemon log:** `kb/node_update: REBASE REQUIRED (stale-epoch op fenced — B-19)`
         (with `stale_client` ≠ `current_client`). The pre-grant op is **rejected**.
   - [ ] **bob's status line:** `your earlier edit to collabtest:beta … was NOT synced —
         reconnect and re-apply it`.
   - [ ] **THE no-cascade assertion — alice (D)'s `collabtest:beta` still does NOT contain
         `VIEWER-ERA-HIJACK`.** The viewer-era lineage did not launder through the grant.
6. **bob (E):** reconnect (`:collab-connect`) + `:kb-join collabtest` (relearns the new epoch),
   then **re-apply** the edit fresh (type it again, e.g. `POST-GRANT-EDIT`).
   - [ ] This edit **is accepted** and **converges on alice** — a legitimately-granted
         editor can edit going forward (authored under the current-epoch client_id).
   - [ ] *(Known limitation, by design)* bob's original pre-grant edit is **not**
         auto-recovered — he re-makes it. Graceful auto-adopt+re-author is a tracked
         follow-up; the security guarantee (no cascade) holds regardless. Confirm the
         honest status message appeared rather than a silent drop.

> **Report (bob):** for step 5, paste the daemon `REBASE REQUIRED` log line + bob's
> status line, and confirm step 5's no-cascade check on alice's side. For step 6, confirm
> the fresh edit converged. Flag anything where a pre-grant edit *did* appear on alice.

### Step 9 — B-19 resolution UX: the notification/attention bus (ADR-024)

> **What we're proving.** Step 8 showed a fenced edit is *safe* but the signal was a buried
> log line and the granted editor was *stuck* (the 8d "known limitation"). ADR-024 fixes both:
> a fenced edit now raises a **non-clobberable mode-line badge** (`⚑ N`) + a row in the
> **`*Notifications*`** buffer with at-point actions that **adopt the authoritative version
> and re-author your edit** — so a granted editor converges instead of being stuck. This is
> the graceful close of 8d.
>
> **Min build:** commit `03d5e5a5` or later (R1–R5). NEW build for daemon + both editors.

> [!IMPORTANT]
> **Binary-hash deploy check first** (the recurring gotcha): `sha256sum /proc/$(pgrep -n mae)/exe`
> vs `sha256sum ./target/release/mae`; same for the daemon. A stale binary will "fail" the test.

Run **Step 8 steps 1–4 first** to get bob's edit fenced (alice resets bob→viewer, bob edits
`collabtest:beta` to `VIEWER-ERA-HIJACK` denied, alice promotes bob→editor, bob's push is
fenced). Then, instead of Step 8's manual re-apply, resolve it via the bus:

1. **bob (E):** confirm the fence now surfaces as a **notification**, not just a log line:
   - [ ] The mode-line shows an attention **badge `⚑ 1`** (worst-severity glyph + count).
   - [ ] `SPC n n` (or `:notifications-open`) opens **`*Notifications*`** with a `collab`
         category row: *"KB 'collabtest': edit to collabtest:beta fenced — not synced"* and three
         indented actions: **→ Accept-remote / → Keep-mine (re-author) / → Stash externally**.
2. **bob (E):** move the cursor onto **`→ Keep-mine (re-author)`** and press **Enter**.
   - [ ] Daemon log shows **`kb/node_fetch`** for `collabtest:beta` (the adopt fetch).
   - [ ] bob's editor adopts the authoritative node and re-applies his edit under the current
         epoch — status/notification like *"Re-applied your edit to collabtest:beta…"*. No new
         `REBASE REQUIRED` this time (it's authored under the current-epoch client_id).
   - [ ] **alice (D):** `collabtest:beta` now **shows bob's re-authored content** — it
         **converged** (the granted editor is no longer stuck — 8d closed gracefully).
   - [ ] The `*Notifications*` row moves to **resolved** and the badge clears (`⚑` gone).
3. **(Accept-remote path)** Re-fence a fresh edit (bob edits again pre-relearn, or repeat 8.3–8.5),
   then in `*Notifications*` choose **→ Accept-remote**.
   - [ ] bob's local copy is **discarded** and replaced by alice's authoritative version
         (`kb/node_fetch` → adopt, no re-author). Both sides match.
4. **(TOFU regression — R4)** Confirm the host-key trust prompt still works through the new bus:
   on a fresh pin (clear bob's `~/.local/share/mae/collab/known_hosts` entry, set
   `collab_host_key_policy = "prompt"`, reconnect) → a modal titled **"Action Required"** asks
   *"Trust daemon at … ? [y/N]"*; **y** connects + pins, **n** aborts. Same behavior, new plumbing.

> **Report (bob):** paste the `kb/node_fetch` daemon line + bob's re-author status for step 2,
> and confirm alice saw the converged content. Note whether the badge + `*Notifications*` row
> rendered correctly (TUI/GUI). Flag if Keep-mine got re-fenced, or if any action silently no-ops.

---

## Results checklist

| # | Check | Pass? |
|---|-------|-------|
| T0 | `make test-collab-e2e-all` green | ✅ (macOS, b947a52) |
| T1 | Single-host CLI smoke (identity/authorize/check-config) | ☐ |
| 2 | Daemon listens `0.0.0.0:9480`, mTLS configured | ☐ |
| 3 | Both peers authorized; E reaches D:9480 | ☐ |
| 4 | `accept-new` connects + pins; pinned fingerprint == D's (out-of-band) | ☐ |
| 4b | *(deferred #66)* interactive `prompt` TOFU shows + matches | ☐ |
| 5 | Buffer edits converge; cursor labels = authenticated identity | ☐ |
| 6 | KB join denied → owner adds → allowed → remove denies | ☐ |
| 7a | Unauthorized peer rejected | ☐ |
| 7b | Changed host key aborts | ☐ |
| 7c | Traffic is TLS-encrypted (tcpdump) | ☐ |
| 8a | Viewer edit denied at daemon; never reaches alice | ☐ |
| 8b | After promotion, pre-grant op **fenced** (`REBASE REQUIRED`) — no cascade | ☐ |
| 8c | bob's honest "not synced — re-apply" status shows (no silent drop) | ☐ |
| 8d | Fresh post-grant edit accepted + converges on alice | ☐ |
| 9a | Fenced edit raises a mode-line badge `⚑` + a `*Notifications*` row (ADR-024) | ☐ |
| 9b | **Keep-mine** → `kb/node_fetch` → adopt + re-author → **converges on alice** (8d closed) | ☐ |
| 9c | **Accept-remote** → local discarded, alice's version adopted | ☐ |
| 9d | TOFU host-key prompt still works through the bus ("Action Required" modal, y/N) | ☐ |

---

## Troubleshooting
- **TLS handshake EOF / connection refused:** wrong daemon (check `ss -tlnp | grep 9480`), or daemon not in `key`+`tls` mode (`mae-daemon --check-config`).
- **"address already in use → collab disabled":** the port is taken — most often an already-running personal daemon on `9473`, or a stale test daemon. `ss -tlnp | grep <port>` to find it; pick a free port and keep `bind` / firewall / `--server` consistent.
- **"client key not authorized":** the editor's pubkey isn't in `authorized_keys` — re-run `mae-daemon authorize`.
- **TOFU never appears / auto-connects:** `collab-host-key-policy` is `accept-new`, or the host is already pinned in `~/.local/share/mae/collab/known_hosts`.
- **KB join always allowed (no denial):** both peers share the same authorized-keys **label** → give them distinct labels in `mae-daemon authorize`.
- **Logs:** daemon `MAE_LOG=info mae-daemon`; editor `MAE_LOG="mae::collab_bridge=debug,info"`.
