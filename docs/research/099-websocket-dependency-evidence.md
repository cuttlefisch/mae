# ADR-099 D5: the WebSocket framing dependency, confirmed at adoption

D5 requires the implementer to confirm, when the dependency is adopted, that the WebSocket
library on the security-critical HTTPS listener is maintained and has more than one maintainer,
and to fall back to framing RFC 6455 by hand only if no candidate passes. This records what was
checked on 2026-11-11 and what was chosen.

## Choice: `tokio-websockets` 0.13, not `tokio-tungstenite`

D5 named `tokio-tungstenite` (or `hyper-tungstenite`) as the likely candidate. `tokio-websockets`
was chosen instead, for a reason D5 did not anticipate: **it is already in the daemon binary.**
`iroh-relay` (via `iroh`, ADR-025's mesh transport) depends on it, and `cargo tree -i
tokio-websockets -e normal -f '{p} {f}'` on the daemon workspace before this change already reports
the features `client, getrandom, rand, ring, rustls-bring-your-own-connector, server, sha1_smol`.
Adopting it directly with `features = ["server"]` adds **no new crate and no new feature** to the
build: the `daemon/Cargo.lock` diff is two lines, both edges from `mae-daemon` to crates already
locked (`tokio-websockets`, `futures-util`). Its code was already shipped in every daemon binary;
adopting it adds no new trust, which is the property D5's hand-rolled fallback was meant to protect.
`tokio-tungstenite` would have added `tungstenite` and its dependency tree.

Only framing is delegated. hyper performs the HTTP upgrade, and the daemon authenticates the
request, checks its origin and negotiates the version before the upgrade, on the listener's own
tested path. The `Sec-WebSocket-Accept` hash is computed in the daemon (`ws/upgrade.rs`, pinned
against RFC 6455's worked example).

## Maintenance evidence

| | `tokio-websockets` (chosen) | `tokio-tungstenite` (considered) |
|---|---|---|
| crates.io owners | 3: `Gelbpunkt`, `vilgotf`, `Erk-` | 2: `daniel-abramov`, `agalakhov` |
| Latest release | 0.13.3, 2026-06-25 (0.13.2 in our lock, 2026-03-17) | 0.30.0, 2026-07-11 |
| Release cadence | 6 releases 2025-09 → 2026-06 | 6 releases 2024-12 → 2026-07 |
| Recent commit authors | last 40 commits (2026-01 → 2026-07): `Gelbpunkt` 17, `vilgotf` 15, `Erk-` 3, `paolobarbolini` 2, dependabot 3 | last 30 commits (2023-12 → 2026-10): `daniel-abramov` 18, 11 others 1–2 each; 21 of 30 committed by one person |
| Repository | `github.com/Gelbpunkt/tokio-websockets`, not archived, last push 2026-11-07, 3 open issues | `github.com/snapview/tokio-tungstenite`, not archived, last push 2026-10-24, 20 open issues |
| Downloads | ~20 M total, ~9 M recent | ~355 M total, ~113 M recent |
| Notable dependents | `iroh-relay` (n0), `twilight` (Discord library) | very broad |

Both pass "maintained and multi-maintainer" on publisher count. `tokio-tungstenite` has far wider
use; `tokio-websockets` has the broader set of *active* committers (two people each authoring a
third or more of recent commits, against one person committing most of `tokio-tungstenite`'s), and
it is the one already compiled into this binary. Sources: the crates.io API (`/api/v1/crates/<name>`
and `/owners`) and the GitHub API (`/repos/<repo>` and `/commits`), queried 2026-11-11.

## `cargo deny`

`cargo deny --manifest-path daemon/Cargo.toml --config deny.toml check` (the daemon leg of CI's
cargo-deny matrix) passes with this change: `advisories ok, bans ok, licenses ok, sources ok`. Its
warnings (duplicate versions, yanked `chacha20`/`der`/`spin`) all predate the change, which adds no
crate.

## Revisit when

- `iroh` drops `tokio-websockets` — then this dependency is ours alone, and the comparison above
  should be re-run rather than inherited.
- An advisory lands against it — the daemon's exposure is the server framing path only.
