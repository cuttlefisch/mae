# Collab Test Notes — bob (E, macOS)

Running log from the **machine-E ("bob")** side of the two-machine ADR-017 collab
validation (`feat/crdt-collab-validation`). Issues encountered, status, and fixes.
D keeps its own view; this is bob's. **Update + commit as we go** so D sees our findings.

See also: [collab-testing-plan.md](collab-testing-plan.md) (the plan + coordination board).

## Environment

- **E = bob:** macOS (`Marthas-MacBook-Pro`), `192.168.1.132`, dev **GUI** build (`make build`), 0.13.12.
- **D = alice + daemon:** `framework`, daemon `192.168.1.137:9480`, key-mode mTLS.
- **D daemon fingerprint (pinned, verify out-of-band):** `SHA256:07aWfiNGm690ZcPzxEWvCSTYgkIz+Dw7Db0RPOKK7Ls`
- Policy: `collab_host_key_policy = accept-new` (workaround for #66).

## Issue log

### ✅ Resolved
- **(fixed `a8ac842`) Tier 0 e2e was Linux-only.** Daemon ignored XDG on macOS
  (`dirs` crate → `~/Library/Application Support`); e2e scripts used `ss`/`timeout`
  (absent on macOS). Fixed: daemon dirs XDG-first + portable `port_listening`/timeout
  shims. Tier 0 now green on macOS (mTLS 7/7, membership 7/7+7/7, mae-mcp 121, daemon 9,
  mae --bins collab 94). Codified as CLAUDE.md principle #13.

### 🔧 Open — filed
- **[#66] Interactive TOFU `prompt` policy deadlocks the TUI / `HostKeyPrompt` unwired.**
  Verifier blocks on a reply that no UI sends → 120s freeze (TUI hard-freeze; GUI silent
  fail). Workaround: `accept-new` (both editors). Fix deferred. https://github.com/cuttlefisch/mae/issues/66

### 🐛 Open — needs investigation
- **[HIGH, D-side] alice rope panic crash.** A rope-related panic crashed alice's
  editor during the live run (≥2×). Suspect the yrs↔ropey bridge (`shared/sync`) or
  rope reconciliation on remote update. **TODO:** capture the panic message + backtrace
  from D (`MAE_LOG=debug`, stderr, or `~/Library/Logs/DiagnosticReports` on mac /
  `coredumpctl`/stderr on Linux). Shared code → fix benefits both. **Blocks clean validation.**
- **bob local edits to a joined buffer not visible.** `buffer-insert` (via MCP
  `eval_scheme`) on the joined doc `/tmp/mae-collab-run/collab-demo.txt` did **not**
  appear in the buffer on read-back — tried **twice** (once before alice's crash, once
  after). Candidate causes, unconfirmed: (a) local edit lost on reconnect/resync
  rebuilding the rope from daemon state; (b) joined-buffer local-edit path issue;
  (c) MCP `eval_scheme` insert not targeting the joined buffer. Note: `(buffer-name)`
  is undefined in the runtime — used `get-buffer-by-name`/`buffer-string` — so the
  diagnostic was incomplete. **Re-test in a clean run; may be coupled to the rope panic.**
- **Connection flapping.** Repeated `Collab disconnected: connection lost: peer closed
  connection without sending TLS close_notify` → auto-reconnect `Connected (0 peers)`.
  Strongly correlated with alice crashing/restarting; unknown whether independently
  reproducible. **Watch in a clean run** (if it persists without alice crashing → bug).

## Convergence results

| Direction | Result |
|-----------|--------|
| alice → bob | ✅ joined doc rendered alice's content (`collab demo — line from alice (D)`) |
| bob → alice | ❓ unconfirmed — bob's edit didn't render locally + alice crashed |

So far: **one-way receive confirmed; round-trip not yet validated.**

## Next run (from scratch)

1. D fixes the rope panic → pushes.
2. Both: `git pull --rebase` → rebuild both binaries.
3. Restart D's daemon (key mode, `0.0.0.0:9480`, authorize bob) + alice (accept-new).
4. bob: relaunch dev GUI, reconnect MCP, re-verify fingerprint, re-run Steps 5–7.
5. Re-test the "bob local edit not visible" path early — confirm or clear it.
