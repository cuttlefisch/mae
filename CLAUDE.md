# AI Development Guide (CLAUDE.md) — Modern AI Editor (MAE)

> [!CAUTION]
> **MAE is in early Alpha.** AI features and cost guardrails are experimental and may fail. Always monitor your API usage and costs directly in your provider dashboards.

## What This Project Is

An AI-native lisp machine editor — a successor to GNU Emacs where the human user and an AI agent are **peer actors** calling the same Lisp primitives. The editor is built on a Rust core with an embedded Scheme (R7RS-small) runtime. LSP and DAP are first-class protocols exposed to both the Scheme extension layer and the AI agent's tool-calling interface.

The project README (`README.md`) contains the architecture spec and stack rationale. **Read it before starting any work.**

## Stack

- **Language:** Rust (core) + Scheme R7RS-small (extensions)
- **License:** GPL-3.0-or-later
- **Build:** `make check` / `make build` / `make test` / `make ci` from workspace root
  - `make build` now builds with GUI by default (`--features gui`)
  - `make build-tui` for terminal-only build
  - `make ci` still excludes GUI (skia system deps)
  - `make check-config` validates init.scm + config.toml without launching the editor
  - **Daemon** (separate workspace): `cd daemon && cargo build`, `cd daemon && cargo test`, `cd daemon && cargo clippy -- -D warnings`
  - **Container workflow** (no local toolchain required):
    - `make docker-ci` — full CI in container (mirrors GitHub CI exactly)
    - `make docker-new-user` — validate first-run flow in pristine environment
    - `make docker-dev` — interactive dev shell with Rust toolchain
    - `make docker-smoke` — quick binary smoke test
    - `make docker-clean` — remove Docker images and cache
  - Dockerfile: multi-stage (base -> builder -> ci -> runtime), TUI-only (no Skia in container)
  - `docker compose run --rm --build <service>` is the canonical invocation
- **Self-test:** Call the `self_test_suite` MCP tool to get the structured JSON test plan, then execute each test by calling the listed MCP tools and checking assertions. If MCP is unavailable, fall back to `make self-test` (headless). Categories: `introspection`, `editing`, `git`, `help`, `project`, `lsp`, `dap`, `babel`, `guidance`, `performance`, `scrolling`.

## Repository Layout

Two workspaces + shared crates (ADR-014):

```
mae/                              (repo root)
├── Cargo.toml                    (editor workspace — cozo+sled)
├── Cargo.lock                    (editor lock)
├── crates/                       (editor-only crates — 18 crates)
├── daemon/                       (daemon workspace — cozo+sqlite, no rusqlite)
│   ├── Cargo.toml                (daemon workspace + own Cargo.lock)
│   └── src/                      (mae-daemon binary)
└── shared/                       (shared crates — members of editor workspace)
    ├── kb/                       (mae-kb: CozoDB store, org parser, federation)
    ├── sync/                     (mae-sync: yrs CRDT, ropey bridge)
    └── mcp/                      (mae-mcp: JSON-RPC protocol, shim)
```

## Crate Layout

### Editor Workspace (`Cargo.toml`)

Dependency column lists the defining external crates, or — where a crate has intra-repo edges — those.
Verified against the real `Cargo.toml` graph 2026-08; the column previously read "(planned)" and had the
direction **backwards** for five leaf crates.

| Crate | Purpose | Key dependencies |
|---|---|---|
| `mae-core` | Buffer management (rope), event loop, core primitives | `ropey`, `crossbeam`; also depends on 10 intra-repo crates incl. `mae-canvas`/`mae-kb`/`mae-export` |
| `mae-renderer` | Display/rendering — `Renderer` trait + terminal backend | `ratatui`, `crossterm` |
| `mae-gui` | GUI rendering backend — winit window + Skia 2D + native SVG | `winit`, `skia-safe` (features: `svg`) |
| `mae-scheme` | Embedded Scheme runtime for configuration and packages | purpose-built R7RS-small |
| `mae-lsp` | LSP client — types, references, diagnostics exposed to Scheme + AI | `tower-lsp` or `lsp-types` |
| `mae-dap` | DAP client — breakpoints, call stacks, variables exposed to Scheme + AI | `dap-types` |
| `mae-ai` | AI agent integration — tool-calling transport (Claude/OpenAI/Gemini/DeepSeek/Ollama) | `reqwest`, `serde_json` |
| `mae-agent-cli` | Terminal AI-agent harness (ADR-046) — the default `SPC a a`/`SPC a p` surface, binary `mae-agent` | `mae-ai`, `mae-mcp`, `ratatui` |
| `mae-shell` | Embedded terminal emulator (alacritty_terminal) | `alacritty_terminal` |
| `mae-babel` | Org-mode code block execution (12 languages) | *(none)* |
| `mae-export` | Org/Markdown → HTML/Markdown export | `mae-babel` |
| `mae-canvas` | Visual buffer (diagrams, drawings) | *(none — `mae-core` depends on it)* |
| `mae-snippets` | Snippet expansion engine | *(none — `mae-core` depends on it)* |
| `mae-format` | Buffer formatting (external formatters) | *(none — `mae-core` depends on it)* |
| `mae-make` | Build system integration (make, cargo, npm) | *(none — `mae-core` depends on it)* |
| `mae-lookup` | Online lookup (dictionary, docs) | `reqwest` |
| `mae-spell` | Spell checking integration | *(none — `mae-core` depends on it)* |
| `mae-scheme-extra` | Extension point for out-of-tree Scheme kernel primitives (#521) — ships as a no-op; downstream forks add their own crates here | `mae-scheme` |
| `mae` | Binary crate — CLI entry point, config loading, event loops | `clap`, `tokio`; depends on 13 intra-repo crates |

### Shared Crates (`shared/` — editor workspace members, also used by daemon)

| Crate | Purpose | Key Dependencies |
|---|---|---|
| `mae-kb` | Knowledge base — CozoDB graph store, typed relationships, org parser, federation | `cozo`, `tree-sitter`, `tree-sitter-org` |
| `mae-sync` | Collaborative state — yrs CRDT, ropey bridge, encoding helpers | `yrs`, `serde`, `base64` |
| `mae-mcp` | MCP server — Unix/TCP, JSON-RPC, multi-client, stdio shim, transport-generic I/O | `tokio`, `serde_json` |

### Daemon Workspace (`daemon/Cargo.toml` — separate Cargo.lock)

| Crate | Purpose | Key Dependencies |
|---|---|---|
| `mae-daemon` | Background service — KB persistence, collaborative editing (TCP sync + WAL), maintenance scheduler, JSON-RPC API | `cozo` (sqlite), `sqlite`, `mae-kb`, `mae-mcp`, `mae-sync`, `tokio` |

## Architecture Principles

These are derived from analysis of 35 years of Emacs git history. They are non-negotiable design constraints:

1. **Concurrency from day one — and honestly about where that is today.** Emacs spent 23,901 commits across 3 branches trying to retrofit a concurrent GC and still hasn't merged it. The core uses Rust's ownership model, and the design intent is that the Scheme runtime never needs a global interpreter lock.

   **What is actually built (verified 2026-08, amended per principle #17):** the Scheme heap is **`Rc<T>` refcounted with no tracing collector at all** (`crates/scheme/src/value.rs` — "Stage 1"). There is a `Trace` trait as groundwork, but nothing walks it. Consequences a reader must not be misled about:
   - `(gc-collect!)` is a **no-op**, and `(gc-stats)`'s `collections` is **always 0** — `collections_count` is declared, copied and reported, but never incremented anywhere.
   - `Rc` is `!Send`, so the runtime is **single-threaded by construction**. "No GIL, ever" is therefore true but *vacuous* at Stage 1: there is no shared multi-threaded interpreter for a lock to guard.
   - Refcounting **leaks reference cycles** (a closure capturing its own environment). Nothing reclaims them, and `code_pool` is append-only and never cleared.

   This entry claimed a "purpose-designed concurrent GC" until 2026-08. It was aspiration written in the present tense, in the file that primes every AI session — the exact drift principle #17 exists to catch. State the Stage-2 plan as a plan; do not restore a present-tense claim until a collector exists and its counter moves.

2. **Modular display layer.** Emacs's `xdisp.c` is 38,605 lines and the most bug-prone file in the codebase. Our renderer is a separate crate with a clean trait-based HAL. Platform-specific code lives in the rendering backend library (crossterm/Skia), not in our codebase.

3. **The AI is a peer, not a plugin.** The AI agent calls the same Scheme functions as the user's keybindings. `(buffer-insert ...)`, `(lsp-references ...)`, `(dap-inspect-variable ...)` — same API surface for human and AI. No separate "AI mode" or simulated keystrokes. **Exception: the controls that bound the agent — see principle #16.**

4. **LSP and DAP are first-class.** Not bolted-on packages. The AI gets structured semantic knowledge (types, references, diagnostics from LSP) and runtime debug state (call stacks, variables from DAP) as part of its reasoning context.

5. **Module boundaries enable distributed ownership.** Each crate has a clear responsibility. No 10k+ line files. This is a direct response to Emacs's bus-factor problem (top 5 contributors = 50.8% of all commits, critical subsystems maintained by single individuals).

6. **Runtime redefinability is sacred.** Users must be able to redefine any function while the editor is running. This is the property that makes Emacs irreplaceable. The Scheme layer provides `defadvice`-equivalent, live REPL, and hot reload.

    **Known tension, unresolved:** in a single shared Scheme image, redefinability *is* an escalation primitive — code at a lower tier can redefine a function that privileged code later calls. PostgreSQL solves the equivalent problem by running trusted and untrusted PL in **separate interpreter instances**, and still ships a warning that the mechanism may not hold. MAE does not separate them. Until it does, per-primitive tiers (ADR-084 D3) bound *direct* calls only. Do not treat redefinability and lower-tier eval in one image as independently safe; see the `@ai-caution` at the VM.

7. **No hardcoding — Scheme-first configurability.** Every user-visible behavior that could reasonably differ between users MUST be exposed as a configurable option via the OptionRegistry. **Exception: values that bound the agent's own authority are not freely settable — see principle #16.** This means:
   - Register in `options.rs` with a `config_key` (enables `:set-save` persistence)
   - Automatically accessible via `(set-option!)` / `(get-option)` in Scheme
   - Automatically accessible via `:set` command at runtime
   - Default values live in the option definition, never as magic constants in rendering code
   - Constants that are truly fixed (buffer sizes, protocol limits) belong in the relevant module, documented with rationale

   **Corollary: No ad-hoc solutions.** Never add a hardcoded workaround for a problem that should be solved architecturally. If you find yourself duplicating logic between TUI and GUI, extract to `render_common` or `text_utils`. If you find a magic number, make it an option. If you find a one-off fix for one backend, fix it properly for both.

8. **Shared computation, backend-specific drawing.** All layout math, content formatting, span computation, and data preparation lives in `mae-core` (specifically `render_common/` and `text_utils`). Backend crates (`mae-renderer`, `mae-gui`) contain ONLY the code that touches platform APIs (ratatui widgets, Skia paint calls). If two renderers compute the same thing, extract it.

9. **Every change must consider downstream impact.** Before implementing any change, assess:
   - **Bug risk**: What existing behavior could break? What edge cases does this touch?
   - **Performance impact**: Does this add work to a hot path? Is it O(1), O(n), or O(n²)?
   - **Type safety at boundaries**: When extracting shared code, verify that type conversions (e.g., `usize` ↔ `u16`) don't silently truncate.
   - **Regression guard**: If the change touches rendering or input handling, verify both TUI and GUI backends. If it touches options, verify the Scheme API + `:set` + `:set-save` persistence (which writes `init.scm` — the primary config surface; `config.toml` is legacy bootstrap for AI provider + theme only) all work.
   - **Adversarial test (security/auth/crypto/sync)**: any change to these subsystems MUST add or extend a test that exercises the *failure mode* (forged signature, wrong/rotated key, stale epoch, removed member, hostile key-blind relay, out-of-order/concurrent ops) — not just the success path. See principle #14.

10. **Multi-client safety by design.** Any state mutation must be safe for concurrent observation. The MCP server may have N connected clients. Editor state changes emit events to a broadcast channel. Clients that can't keep up are dropped (bounded queues, write timeouts). File writes use content-hash verification + advisory locks. No operation assumes single-client.

11. **CRDT-first sync (yrs/YATA).** All collaborative state flows through yrs (Yjs Rust port). Text buffers use `YText`, visual documents use `YMap`/`YArray`, KB nodes are yrs documents. The ropey rope is a read-only rendering mirror rebuilt from yrs on remote changes. Local edits generate yrs transactions (attributed, undoable via per-user `UndoManager`). This is the universal substrate — no separate sync mechanism for different content types. See ADR-002, ADR-005, ADR-006. Local undo/redo uses `reconcile_to()` (character-level LCS diff) to generate CRDT-safe deltas instead of full-state replacements.

12. **Local-first by design.** MAE satisfies 5 of 7 Ink & Switch local-first ideals today (no spinners, multi-device, network optional, collaboration without conflict, user ownership). P2P collaboration and E2E encryption will complete the remaining two. The daemon is an optimization for persistence and discovery, not a requirement for collaboration. **The daemon is configurable (`daemon_mode` = `off` / `on-demand` / `shared`) with the in-process embedded KB as the *floor* (the default, not a fallback); it earns placement only by an objective value category — SHARED across frontends, OUTLIVES editor sessions, COORDINATES peers, or DURABILITY. Features that genuinely require it (P2P sharing, continuous shared-KB sync) are gated + surfaced as such. See ADR-035 for the editor↔daemon boundary.**

13. **Cross-platform parity (macOS + Linux) is a development constraint, not an afterthought.** MAE is developed and run across macOS and Linux *simultaneously* (often on the same branch, same day). Every script, path-resolution, and tool invocation MUST behave identically on both — or fail loudly with a portable fallback, never silently no-op on one platform. A "fix" that only works on one developer's machine is not a fix; it manufactures the stop-and-go cross-machine debugging this principle exists to prevent. Concretely:
    - **Directory resolution is XDG-first on ALL platforms.** Honor `XDG_CONFIG_HOME` / `XDG_DATA_HOME` when set, then fall back to the platform default. The bare `dirs` / `directories` crate follows Apple conventions on macOS (`~/Library/Application Support`) and *ignores* XDG — so calling `dirs::config_dir()` / `dirs::data_dir()` directly breaks env-var test isolation and contradicts the documented `~/.config/mae` + `~/.local/share/mae` contract. Use the XDG-first helpers (`mae-mcp::identity::default_collab_dir`, `mae-mcp::keystore`, editor `pkg/paths.rs::{dirs_candidate,data_dir_candidate}`), never raw `dirs::*` for primary config/data paths.
    - **Shell scripts use portable tooling.** No Linux-only commands without a fallback: `ss` → `lsof` → `netstat`; `timeout` → `gtimeout` → optional/omitted; avoid GNU-only behavior (`sed -i` arg differences, `readlink -f`, `mktemp` templates, `date` flags). Prefer POSIX; gate platform branches on capability (`command -v`), not `uname`. Keep the Linux path first so CI/driver behavior is unchanged.
    - **CI must exercise both OSes** for anything touching paths, sockets, or scripts — the collab e2e (`scripts/collab-*-e2e.sh`) especially, since that's where this bites.
    This is the cross-machine corollary to principles #8 (shared computation) and #9 (downstream impact): verify the change on *both* platforms, not just the one in front of you.

14. **Adversarial testing, not confirmation.** A test exists to *falsify* the implementation, not to congratulate it. We hold the line against three failure modes that let bugs ship green:
    - **No fragile linear tests.** A test that only walks the happy path in one fixed order — setup → do → assert-it-worked — proves the code works *on that one path*, nothing more. Prefer **property/round-trip** tests (encode↔decode, seal↔open, the same result under shuffled apply order), **N-way convergence** (≥3 peers/writers, not 2), and **state-machine** coverage (the transitions, not one trace).
    - **No cherry-picked "unicorn" values.** Inputs chosen because they make the test pass — a magic constant, a single hand-picked identity, a value that dodges the edge — hide the bug they were chosen around. Use **real inputs** (freshly generated identities/keys, varied/boundary/random-but-seeded values, multiple distinct cases) and **selective oracles** that pin the *meaningful* outcome (the decrypted title equals the edit; the forged op is *not* a member), not an incidental one.
    - **Favor the attacker's test.** For anything security-, auth-, crypto-, or sync-relevant, the primary test encodes the **attacker model**: wrong key opens nothing, a tampered signature is rejected, a stale-epoch op is fenced, a removed member can't read post-rotation, a malicious key-blind relay sees only ciphertext, concurrent/out-of-order ops still converge. A negative/adversarial case that *must fail* is worth more than ten that pass.
    - **Per-phase adversarial review.** Every lettered phase / PR gets a review pass that asks "what did these tests *not* try to break?" — and the gap becomes the next test. The goal is software that is correct *because we tried hard to falsify it and couldn't*. (This is the standing testing discipline; it supersedes any habit of writing only confirmation tests.)

15. **Bugs are drift signals, not just defects.** Before fixing a bug, check whether its root cause traces to a place where implementation fell behind an already-decided ADR or a tracked epic issue. If it does, fix the drift for that whole feature area — or explicitly scope a bounded down payment and cross-link the owning epic issue so the remainder stays visibly tracked — rather than patching the local symptom and leaving the same drift to regenerate similar bugs later. If no relevant ADR/epic applies but the bug reveals duplicated logic or an ad-hoc workaround, resolve it by consolidation (principle #8), not by adding a third parallel implementation. Concretely: before writing a fix plan, check `docs/adr/` for a governing ADR and `gh issue list`/the KB for a tracked epic in that feature area.

16. **Controls that bound the agent are not part of the peer surface.** Principle #3 makes the AI a peer for *doing work*. It does not extend to the controls that decide **what the agent is allowed to do** — permission tiers, workspace trust, KB membership and residency. Those must be reachable by the human and **not** by the agent, because a control the agent can change is not a control. Concretely, and each of these is a deliberate, evidenced exception rather than an oversight:
    - **Workspace trust has no Scheme primitive, no command, and no MCP tool.** Trust is granted only by editing `~/.config/mae/trusted-projects`. An agent able to grant trust could then write `.mae/init.scm` and escalate across a restart — the shape of CVE-2025-53773, where GitHub Copilot was induced to write `chat.tools.autoApprove` into `.vscode/settings.json`.
    - **The permission-tier option is not settable at the tier it governs**, on either surface — `set_option`, `set-option!` and `set-option-save!` all require the privileged tier for it specifically, while staying ordinary for every other option.
    - **MAE's own configuration is not agent-writable.** `~/.config/mae/**` and any `.mae/**` are refused across `create_file`, `rename_file` and AI-originated buffer saves; the human's own editing, including `:set-save`, is untouched.
    - **A tool may be withheld from a surface it cannot serve.** `ask_user`, `propose_changes` and `delegate` are filtered from external MCP discovery rather than advertised and then refused (ADR-085's shape: *not offered* beats *offered and denied*).

    The general rule when this bites: a security control is the one place where **asymmetry between human and AI is the feature**. Prefer removing the capability from the agent's surface over adding a check the agent could reach.

17. **These principles are amendable, and drift in them is a bug like any other.** When following a principle produces a worse outcome, the correct response is to change the principle here — with the evidence — not to violate it silently or to follow it off a cliff. Two rules make that safe:
    - **Amend in the open.** A principle changes by editing this file in the same PR as the work that motivated it, with the concrete case named. Principle #16 exists because three separate security fixes each quietly contradicted #3 and #7; the contradictions were right and unwritten, which is the worst of both.
    - **Evidence over taste.** Prefer published prior art to intuition when a principle is under revision (see the *Prior-Art Review Before Deciding* practice). Grounding ADR-084/085 that way reversed one decision outright, corrected two more, and surfaced two defects the codebase audit had missed — including a live RCE.

    A principle that has never been revised is not proven; it is untested.


### Rendering Pipeline
The GUI renderer uses a three-phase pipeline: `compute_layout()` produces
a `FrameLayout`, `render_buffer_content()` draws text, and `render_cursor()`
positions the cursor. All three MUST consume the same `HighlightSpan` set.
See `crates/gui/src/RENDERING.md` for detailed rules.

### Debt/Invariant Tagging

MAE uses two distinct in-code comment conventions — don't confuse them:

- **`@ai-caution: [category] <explanation>`** — a landmine/invariant warning for a specific
  function, field, or block that future editors (human or AI) must not casually violate (e.g.
  `// @ai-caution: [window-split] Agent shells MUST use display_buffer_for_agent() +
  split_root(), NOT display_buffer_and_focus() — the latter steals conversation windows.`). Place
  it directly above the guarded code, or as a file-header `//!` line when the whole file carries
  one invariant. `[category]` is a short bracketed tag grouping related warnings (`[rendering]`,
  `[dispatch]`, `[window-split]`, `[architecture-debt]`, etc.) so they're greppable together. This
  is also the convention for flagging tracked architectural debt in-code (see below) — use
  `[architecture-debt]` and cross-link to `ROADMAP.md`'s "Architecture Debt" section so the debt is
  discoverable by grepping the source, not only by reading a separate tracking doc.
- **`@stability: stable|experimental`** — a crate/module-level maturity marker (one per crate's
  `lib.rs`, or a module's `autoloads.scm` header per `docs/module-template/README.md`). This is
  about API maturity, not a warning about a specific invariant — don't use it where `@ai-caution`
  is meant, and vice versa.

Architectural debt is tracked in three places that should cross-reference each other: this file's
principles, `ROADMAP.md`'s "Architecture Debt" checklist, and — for size-ceiling debt specifically —
`docs/AUDIT_BASELINE.json`, the machine-checked accepted-exceptions set. When you add a new tracked
exception in one place, add an `@ai-caution: [architecture-debt]` marker at the file in question and a
pointer in the other two, so a reader landing in any one of the three finds the others.

**Never write a line count (or any other measured number) into that prose.** Size-ceiling debt is
enforced by `tools/audit-metrics` and ratcheted in CI (`make audit-metrics-check`): a new
over-ceiling file fails, an accepted file that grows past 10% fails, a file that shrinks never fails.
The baseline holds the numbers; `make audit-metrics-bless` re-accepts them deliberately. This
replaced a hand-maintained list in `.claude/commands/mae-audit.md` that had drifted badly by 2026-08
— 14 of 15 tracked sizes stale, one file +96% past its recorded figure, and an untracked backlog
roughly twice what the prose claimed. The cross-reference *placement* discipline held; the
*number-freshness* discipline could not, because a moving number cannot live in prose.
`tools/audit-metrics` also verifies the cross-reference itself, reporting orphaned markers, tracked
entries with no in-code marker, `@ai-caution`s missing their `[category]`, and crate roots missing
`@stability`.

## Development Priorities

Start terminal-only. Skip GUI until the model works.
Granular milestone tracking lives in **ROADMAP.md**.

All phases below are COMPLETE. See ROADMAP.md for granular milestone details.

| Phase | Summary | Tests |
|-------|---------|-------|
| 1. Core + Renderer | ropey buffer, event loop, ratatui/crossterm, vi-modal editing | — |
| 2. Scheme Runtime | R7RS-small, `init.scm`, `(define-key ...)`, REPL | — |
| 3. AI Integration | Claude/OpenAI/Gemini/DeepSeek, tool-calling, permission tiers | 1,148 |
| 3d–3h. Hardening | Full vim, multi-file AI, agent reliability, context compaction | 1,673 |
| 4. LSP + DAP + Syntax | LSP nav/completion, DAP debugging, tree-sitter (17 langs), KB | — |
| 5. Knowledge Base | CozoDB graph (Datalog), federated queries, org parser, HNSW vectors | — |
| 6. Embedded Shell | alacritty_terminal, MCP server, file auto-reload | — |
| 7. Documentation | Help system (1,300+ KB nodes), tutorials, `:describe-configuration` | — |
| 8. GUI Backend | winit + Skia, inline images, multi-cursor, magit-style git | 2,629 |

**Current:** actively released on the 0.14.x line — collaborative **KB sharing** is user-ready: trusted-peer mTLS auth, per-KB
membership/roles/policy (Owner/Editor/Viewer, ADR-018), epoch-fenced write access (ADR-023), the ADR-024
attention bus, a magit-style `*KB Sharing*` management buffer (`SPC C K m`), and full introspection +
lifecycle parity across the human (buffer + Scheme `(kb-…)` primitives) and the AI peer (`kb_sharing_status`
+ lifecycle MCP tools). See `docs/COLLABORATION.md`.

**Also shipped — `DrivenWindow` + native KB graph view** (v0.14.x): `DrivenWindow`
(`crates/core/src/driven_window.rs`) is a new first-class "window this actor is driving" primitive
(`resolve_persistent` / `follow_focus_away_from`) that fixes AI/MCP agent actions — including
external Claude Code via the MCP shim — cascading into repeated new window splits;
`AiState.work_window` now uses it, and `display_buffer_for_agent()` (renamed from
`switch_to_buffer_non_conversation()`) is the generalized agent-display entry point.
Companion-window protection is now a structurally enforced *default* for all MCP-driven
dispatch (issue #372) — `Editor::with_ai_dispatch_scope` proactively establishes the driven
window before an MCP-originated command runs (not only after a call site that happens to
invoke `display_buffer_for_agent` itself), wrapping both `execute_tool_with_requester` and
the Scheme-command bridge in `crates/mae/src/ai_event_handler.rs` — the two MCP/AI mutation
entry points — so a single external agent can never silently steal the sole visible window. A native
org-roam-ui-style KB graph view (`BufferKind::Graph`, `crates/core/src/graph_view.rs`) is built on
the previously-orphaned `mae-canvas` crate — background-threaded force layout
(`crates/mae/src/graph_layout_bridge.rs`), click-to-navigate via `DrivenWindow`'s companion-window
strategy, follow-current-node, opt-in physics animation, full Scheme+MCP parity
(`kb-graph-view-*`). A shared idle-dispatch mechanism (`Editor::on_idle_tick`,
`crates/core/src/editor/idle_ops.rs`) closes ROADMAP #83 (which-key idle delay) and now also
drives a new KB-link hover preview popup. A freshly opened graph window now computes an initial
zoom-to-fit level (`graph_view::zoom_to_fit`, applied once in `Editor::graph_view_reflatten_window`
only when a window's `Viewport` is first created) instead of always defaulting to a fixed `zoom:
1.0` regardless of diagram size — previously a dense chord/force diagram opened way too zoomed in
to see anything. Deliberately one-directional (only ever zooms OUT, via `kb_graph_zoom_to_fit_margin`,
default 0.85) — a sparse graph's tiny node extent is left at the natural 1.0 scale rather than
zoomed artificially far IN to fill the viewport. Deferred: full per-MCP-session window isolation (two
simultaneous MCP clients still share one driven window — now designed in **ADR-051**, tracked under
issue #375's Phase C, #378), GPU-accelerated
rendering (still out of scope, confirmed 100% CPU-rasterized). See ROADMAP.md's "Completed
Features" and "Architecture Debt" for the full breakdown.

**Also shipped — dev-practices KB dogfooding** (issue #370): `ai_guidance_kb`
(`crates/ai/src/guidance.rs`) already shipped as an opt-in mechanism to surface a registered
KB's standing practices to every AI session. Auto-registered as a federated instance at every
startup whenever the pre-built KB is found — additive-only, never overwrites a contributor's own
customized entry of the same name, and a silent no-op if nothing is found (e.g. a terminal-only
build that skipped `make practices-kb`/`make devpractices-kb`). `ai_guidance_kb`'s validation was
relaxed to accept any name unconditionally at set time (previously it hard-rejected an
unregistered name, which broke the moment a shipped default was added: `init.scm` evaluates
before KB federation populates the registry) — resolution is deferred to read time, matching
`read_guidance_kb_context`'s already-existing best-effort design.

**Also shipped — the system of bundled KBs** (issue #514, **ADR-076**): a fourth bundled KB,
**DevPractices** (`assets/devpractices/*.org`, forked from the independent `dev-practices-kb`
project, generic and vendor-neutral — commit conventions, adversarial testing, ADR process, code
annotation, all written for *any* software project, not MAE-specific), joins the manual/help KB,
**MaePractices** (`assets/practices/*.org` — MAE-contributor-specific conventions, this file's own
design principles distilled), and the **ADR KB** (`docs/adr/*.md`, ADR-059 — MAE's own decision
history). MAE's shipped `init.scm` template (`crates/mae/src/config.rs::default_init_template`)
now defaults `ai_guidance_kb` to `"DevPractices"` — the right default for the overwhelming
majority of users, who are building other software with MAE, not contributing to MAE itself.
Working on MAE? Switch to MAE's own conventions with one command:
`:set-save ai-guidance-kb MaePractices`. The ADR KB stays deliberately **opt-in**, not
auto-registered (per ADR-059 — injecting dozens of ADR summaries into every AI session by default
would be noise, not signal); `kb_register` it manually to query MAE's own architecture decisions.
All four are built via a shared pipeline (`shared/kb/src/kb_build.rs`) and bundled into every
release artifact except Windows (TUI-only, ADR-066 Phase B) — see ADR-076 for the full taxonomy,
the shared auto-registration engine (`crates/mae/src/guidance_kb_engine.rs`), and the "a
contributor's own same-named registration always wins over the bundled default" precedent that
makes this whole system safely overridable.

**Next — P2P decentralized KB sync** (multi-session/multi-machine initiative): a **daemon mesh** so global
peers maintain shared KBs with **no central server**. Design = **ADR-025** (iroh QUIC transport, Ed25519
node IDs reuse trusted-peer fingerprints, + config/install/activation), **ADR-026** (peer-verifiable
signed, hash-chained membership + signed ops + peer-enforced epoch fence), **ADR-027** (observability built
alongside). **Tracker: issue #96**; ADR PR **#95**; phased epics #88–#94. Pre-work (crypto-deps #87/#51,
epoch hardening #72, TOFU deadlock #66, split oversized collab files #70, authorized_keys resolver #73)
is **done**, as are Phases 1/3/4 (#88/#90/#91). **Current bottleneck: Phase 2 / #89** (daemon-as-peer
mesh transport — dial + gossip + anti-entropy), with no other open prerequisites. E2E content encryption
+ leaderless auth-DAG are deferred. Also still pending: hosted-edit (ADR-020 D1).

**Also next — Ollama/local-model parity** (AI-integration initiative): bring self-hosted models to parity
with hosted providers for agentic MAE work — tool-calling reliability harness, KB-enrichment lifecycle for
local models, safety rails for unpriced models, and a scoped orchestrator-worker multi-agent path for bulk
KB batch work. Design = **ADR-045** (provider parity + local-model harness), **ADR-046** (CLI/MCP-shim vs
embedded-GUI agent surface — embedded window frozen at current feature set, new work targets a CLI harness
on `mae-mcp-shim`), **ADR-047** (multi-agent orchestration scoped to KB batch work only). Phased epics
A–G tracked under the epic issue cross-linked from those ADRs.

**Also next — external-editor MCP pairing** (v0.15 initiative): make MAE a general-purpose, headless
MCP backend for **any** editor's AI agent — starting with VS Code + GitHub Copilot's agent mode — so
external tool calls are steered by MAE's KB search/CRUD and dev-guidance-KB mechanism, with the paired
editor as the human's GUI and MAE running invisibly underneath. Design = **ADR-050** (VS Code/Copilot +
cross-editor MCP compatibility: MCP-standard tool `annotations`, flat-schema extension, guidance-delivery
fallback), **ADR-051** (per-session permission policy + per-session `DrivenWindow` isolation — closes the
gap noted above), **ADR-052** (OAuth 2.1 resource-server design — a new HTTPS listener on `mae-daemon`,
hand-rolled directly against `jsonwebtoken` after evaluating and explicitly rejecting `rmcp-server-kit`
as a single-maintainer third-party dependency for a security-critical listener), **ADR-053** (live scoped read-through KB
query surface — search/read a hub KB without full local replication; capped server-side search for
unencrypted KBs, capped lazy-fetch-and-decrypt for E2E-encrypted KBs, never naive plaintext search of
encrypted content), **ADR-054** (daemon concurrency hardening — replaces the KB-query path's single
global `Mutex<DaemonState>` with per-KB-instance locking, adds connection caps to the previously-unbounded
KB socket + P2P listener, and supersedes-with-evidence ADR-004's unbenchmarked "5-10 concurrent editors"
claim), **ADR-055** (headless MAE as a first-class release/service target — new `mae --headless` entry
point on the existing TUI binary, reusing the proven `--self-test` bootstrap shape, with systemd/launchd
units and dedicated soak/idle-CPU testing). **Tracker: issue #375**; phased issues #376–385 (Phases
A–J); every phase carries explicit success criteria and required adversarial tests, not just a merged-PR
definition of done.

## Key Design Decisions Already Made

- **Scheme over other Lisps:** R7RS-small is close enough to elisp for a compatibility shim, has hygienic macros (superior to elisp's `defmacro`), proper tail calls, and first-class continuations. Janet was too limited on macros. Racket has the best language but worst embedding story. Fennel/LuaJIT is proven (Neovim) but fragile upstream.

- **Rust over other cores:** Eliminates the GC problem entirely. Zig was considered (simpler FFI, comptime) but has a smaller ecosystem and less mature async story. C/C++ would repeat Emacs's mistakes.

- **GPL-3.0-or-later:** Copyleft ensures the project stays open. No FSF copyright assignment — contributions are owned by their authors.

- **Terminal-first:** ratatui/crossterm for initial development. GPU rendering (Skia) is now the primary target.

## Keybinding Architecture

- **Kernel keymaps** (`keymaps.rs`): vi-modal primitives ONLY (hjkl, operators, text objects, Escape, `:`, `C-w` window + resize, `C-c` capture) + the empty `leader`/`command`/etc. keymaps. The kernel defines **no** SPC leader bindings — enforced by `kernel_keymap_has_no_leader_bindings`.
- **Shared leader tree** (`modules/keymap-leader/`, embedded): the single source of truth for the mae which-key menu, bound into the kernel-created **`leader` keymap** WITHOUT an `SPC` prefix (`(define-key "leader" "b s" "save")`). Every flavor depends on it.
- **Keymap flavor modules** (`modules/keymap-doom/` = modal default, `modules/keymap-nonmodal/` = non-modal/CUA; both embedded). A flavor depends on `keymap-leader` and only wires its ENTRY into the transient keypad + its default mode: doom binds `SPC` (normal/visual) → `leader-dispatch` (Normal default); nonmodal sets `default_mode=insert`, binds `C-;` (insert) → `leader-dispatch`, + CUA chords. Selected via `keymap_flavor` option (default "doom"); switch live with `:keymap-set-flavor <name>` (resets keymaps to kernel + reloads — no stale bindings).
- **Transient keypad** (`leader_active` overlay, `leader-dispatch` command): a God-Mode/Meow-Keypad layer that does NOT mutate the base mode. While active, keys resolve against the shared `leader` keymap (which-key renders, N levels deep via pending-key accumulation); resolving one command or cancelling (`Esc`/`C-g`/unbound) pops the overlay, restoring the base mode (Normal for doom, Insert for nonmodal). Traversal is flavor-independent; restoration is flavor-specific by construction.
- **Extensibility** (user-facing, no kernel patches):
  - *New flavor*: drop `modules/keymap-<name>/` (`[dependencies] keymap-leader = "*"`), set `default_mode` + an entry binding to `leader-dispatch`. Ships embedded if in repo `modules/`; users add flavors via `~/.local/share/mae/modules` or `MAE_MODULES_PATH`.
  - *New which-key command*: `(define-key "leader" "x y" "cmd")` + `(set-group-name "leader" "x" "+label")` in any module or `config.scm` — appears in EVERY flavor's keypad and survives flavor switches.
  - *Hooks*: `leader-open` / `leader-execute` (keypad-resolved command) / `leader-cancel`, `keymap-flavor-changed`, plus generic `command-pre`/`command-post` + per-command `:before`/`:after` advice.
- **Feature modules** (dailies, git-status, etc.): bind leader entries into the `leader` keymap (not `normal`/`SPC`), so they appear in the keypad regardless of flavor.
- **Scheme API**: `(define-key MAP KEY CMD)`, `(set-group-name MAP PREFIX LABEL)`, `(define-keymap NAME PARENT)`, `(undefine-key! MAP KEY)` — work at init + REPL (runtime redefinable). `:reload-modules` (alias `mae-reload`) re-runs module loading live.
- **`(mae!)` block**: Declarative module selection in `init.scm`. Keymap flavor + its dep closure + language modules auto-enable. Belongs in `init.scm` (read before module loading; `config.scm` is too late for `keymap_flavor`/`default_mode`).
- **Never duplicate** leader bindings between kernel and modules. The `leader` keymap is the sole owner. New leader bindings go in `keymap-leader` (or a feature module / user config), never `keymaps.rs`.
- **Never add ad-hoc solutions**: Prefer proper architectural solutions over hardcoded workarounds. When you find yourself duplicating logic between TUI and GUI renderers, extract shared code.
- **Every option must be Scheme-accessible**: If a behavior is configurable, it goes through OptionRegistry. No config.toml-only settings, no env-var-only settings, no compile-time-only flags for user-facing behavior.

## Emacs Lessons (Reference Data)

These findings from analyzing the Emacs git repo (clone of emacs-mirror/emacs) motivated our architecture:

- **Fix ratio climbed from 15% to 32%** over 35 years — a complexity ceiling from C + untyped Lisp. Rust's type system structurally prevents this.
- **`xdisp.c`: 38,605 lines, 20k+ commits/decade** — the display engine is a monolithic maintenance black hole. We use a modular renderer crate.
- **IGC/MPS: 23,901 commits across `feature/igc`, `igc2`, `igc3`** — still unmerged after 3 iterations. GC retrofit is intractable. We avoid needing one.
- **Bus factor ~4 people** — top 5 = 50.8% of commits. Single-person dependencies on native-comp (Corallo), tree-sitter (Yuan Fu), Android (Po Lu), Tramp (Albinus). We enforce module boundaries.
- **~10% of all commits are platform support** — separate `*term.c` files per platform. We delegate to crossterm/Skia.
- **Emacs 31 direction:** VC/git (1,048 commits = 16%), completions, TTY child frames, newcomer presets, `elisp-scope.el` (static analysis). QoL is the frontier.
- **Development velocity peaked in 2022 (9,647 commits) and declined to ~3,356 in 2024.** The 2025 pace is even lower. Whether this is stabilization or contributor burnout is unclear.

## Development Dependencies

Required for full self-test coverage (DAP and LSP categories):

| Package | Purpose | Install |
|---------|---------|---------|
| `lldb` | DAP adapter for C/C++/Rust (provides `lldb-dap`) | `sudo dnf install lldb` (Fedora), `sudo apt install lldb` (Debian/Ubuntu), `brew install llvm` (macOS) |
| `rust-analyzer` | LSP server for Rust | `rustup component add rust-analyzer` |
| `clangd` | LSP server for C/C++ | `sudo dnf install clang-tools-extra` (Fedora), `sudo apt install clangd` (Debian/Ubuntu), `brew install llvm` (macOS) |
| `debugpy` | DAP adapter for Python | `pip install debugpy` |
| C/C++ compiler (`g++`/`clang++`) | org-babel `c`/`c++`/`cpp` block execution | `sudo dnf install gcc-c++` (Fedora), `sudo apt install g++` (Debian/Ubuntu), Xcode CLT (macOS) |

Quick setup: `make setup-dev` (auto-detects package manager).

Environment variable overrides for adapter/server paths:
- **DAP:** `MAE_DAP_LLDB`, `MAE_DAP_CODELLDB`, `MAE_DAP_DEBUGPY`
- **LSP:** `MAE_LSP_RUST`, `MAE_LSP_PYTHON`, `MAE_LSP_TYPESCRIPT`, `MAE_LSP_GO`, `MAE_LSP_CPP`, `MAE_LSP_C`, `MAE_LSP_RUBY`, `MAE_LSP_YAML`, `MAE_LSP_JSON`, `MAE_LSP_TOML`, `MAE_LSP_BASH`, `MAE_LSP_TERRAFORM`, `MAE_LSP_DOCKERFILE`, `MAE_LSP_ANSIBLE`, `MAE_LSP_HELM` — see `crates/mae/src/bootstrap.rs::setup_lsp()`'s `defaults` table for the full, authoritative list (this line is a convenience summary, not the source of truth)
- **LSP `initializationOptions` passthrough (ADR-075):** `config.toml`'s `[lsp.<lang>]` sections accept an optional `init_options` table, sent verbatim as the server's `initialize` request `initializationOptions` field — e.g. `yaml-language-server`'s Kubernetes-manifest association, which has no per-file client-side detection in MAE (the server does its own glob matching once configured):
  ```toml
  [lsp.yaml]
  command = "yaml-language-server"
  args = ["--stdio"]
  [lsp.yaml.init_options.yaml.schemas]
  kubernetes = "k8s/*.yaml"
  ```
- **YAML dialect routing (ADR-075):** a `.yaml`/`.yml` file whose path looks like an Ansible playbook (`site.yml`, a `*playbook*` filename, an ancestor path component exactly `playbooks`, or a `.ansible.yml`/`.ansible.yaml` double extension) or a Helm chart template (an ancestor path component exactly `templates`, checked only when the Ansible heuristic didn't already claim it) is routed to `ansible-language-server`/`helm-ls` instead of the generic `yaml-language-server` — LSP completions/diagnostics only, tree-sitter highlighting stays plain YAML in both cases (Helm's Go-template-aware highlighting needs a tree-sitter injection-callback resolver MAE doesn't have yet). Both heuristics are pure client-side path matching (no `Chart.yaml`/project-marker verification — that would need filesystem I/O this hot path can't afford) and can false-positive on non-Ansible/non-Helm projects using the same directory names for unrelated reasons; override `[lsp.yaml]` (or the dialect-specific `[lsp.ansible]`/`[lsp.helm]`) in `config.toml` if that's disruptive for a given project.
- **Babel compilers:** `MAE_BABEL_CXX` (C++), `MAE_BABEL_CC` (C) — or the `babel_cxx_compiler` / `babel_c_compiler` / `babel_cxx_std` options, or a per-block `:cmd`
- **Browser:** `MAE_BROWSER` — command used to open external `http(s)://` links (default `xdg-open`). Automated tests set this to a harmless no-op (`true`) so `cargo test` never pops a real GUI browser window; for interactive manual verification of link-opening without a GUI window, `export MAE_BROWSER=lynx` (or `w3m`/`elinks`) — those are optional and not required by any test.

## Scheme Testing Framework

MAE has a headless test runner inspired by Emacs ERT/Buttercup and Neovim Plenary. Tests boot a real editor (no mocks) and exercise the same Scheme API surface available to users.

### Running Tests
```bash
mae --test tests/crdt/              # CRDT sync tests
mae --test tests/editor/            # Editor feature tests
mae --test tests/collab-e2e/test_smoke.scm  # Single file
make test-scheme-crdt               # CRDT tests (builds first)
make test-scheme-editor             # Editor tests
make test-scheme-all                # All local tests
```

### Architecture (3 layers)
1. **`scheme/lib/mae-test.scm`** — BDD library: `describe-group`/`it-test`/`should`/TAP output
2. **`crates/mae/src/test_runner.rs`** — Rust orchestrator: iterates tests, syncs state between steps
3. **`crates/scheme/src/runtime.rs`** — Scheme primitives for buffer mutation + state inspection

### Writing Tests
```scheme
(describe-group "Feature name"
  (lambda ()
    (it-test "setup"
      (lambda ()
        (create-buffer "*test-feature*")))
    (it-test "do something"
      (lambda ()
        (buffer-insert "hello")))
    (it-test "verify result"
      (lambda ()
        (should-equal (buffer-string) "hello")))))
;; No (run-tests) — Rust-side iteration handles state refresh
```

### Design Principles
- **Real editor, not mocks.** Tests boot headless with full event loop. Same API for tests and users.
- **Real event loops for event-loop behavior.** When behavior depends on the event loop (hooks firing, async yields, mode transitions with side effects), tests MUST exercise the actual event loop — not synthetic flushes or manual drain calls. A test that manually calls `drain_hook_evals` is testing the drain function, not the hook system. If behavior is tied to the event loop, spawn a real editor instance (PTY or MCP) and test through it. Never create synthetic event triggers to avoid using the event loop.
- **One pending op per test step.** Each `it-test` is one eval→apply cycle. `buffer-insert` + `goto-char` in the same step may execute in unexpected order. Split into separate steps.
- **SharedState pattern for cross-test reads.** Functions like `buffer-string`, `buffer-sync-enabled?`, `current-mode`, and `get-buffer-by-name` read from `Arc<Mutex<SharedState>>` (not closure-captured snapshots) so they see fresh state after `sync_scheme_state`.
- **Assertions signal errors.** `should`/`should-equal`/`should-contain` signal Scheme errors caught by the runner. Use `should-mode` for mode checks.
- **File-boundary state isolation.** The runner snapshots global editor state (mode, keymap_flavor, default_mode, line_numbers, word_wrap) before each test file and auto-restores after. Cross-file pollution is caught and warned: `# warning: test_foo.scm leaked global state (auto-restored): mode: Normal → Insert`. Tests that change flavor/mode/options should still restore them (the snapshot is a safety net, not a substitute for proper cleanup).
- **TAP v14 output.** Machine-parseable, CI-friendly.
- **Rust-side iteration preferred.** Don't add `(run-tests)` at end of test files. The runner calls `run-nth-test` with `apply_to_editor` + `sync_scheme_state` between each step.
- **Clean environment for e2e tests.** The e2e tests run in CI with no user config (`init.scm`, `config.toml`) and no on-disk modules. When testing locally, use a clean HOME: `HOME=/tmp/mae-test XDG_CONFIG_HOME=/tmp/mae-test/.config XDG_DATA_HOME=/tmp/mae-test/.local/share ./target/release/mae --test tests/editor/`
- **Adversarial, not confirmation (principle #14).** Don't write happy-path-only linear tests, and don't pick "unicorn" inputs that make the test pass. Use real/varied inputs + selective oracles + round-trip/property + N-way convergence + the negative case that must fail (wrong key, forged sig, stale epoch, removed member, hostile relay). Test isolation: per-test fixtures (e.g. a per-test tmp dir/name), never a shared/address-derived path that breaks under parallelism.

### Adding New Test Primitives
- **Read-only state**: Add to `SharedState`, register Rust function in `new()` that reads from SharedState, update SharedState in `inject_editor_state`.
- **Mutations**: Add pending field to `SharedState`, register Scheme function that sets it, process in `apply_to_editor`.

## Developing MAE Inside MAE (MCP Tools)

All 135+ MAE editor tools are exposed via MCP with full parity — the same tools the built-in AI agent uses. When developing MAE with Claude Code connected via the MCP shim (`mae-mcp-shim`), prefer these tools over raw file reads for structured editor operations.

### Connection

Socket path: `/tmp/mae-{PID}.sock` (per-process, stale sockets cleaned on startup).
Shim: `mae-mcp-shim` — translates MCP JSON-RPC over stdio to the Unix socket.

### Code Navigation (LSP)

| Tool | Purpose |
|------|---------|
| `lsp_definition` | Go to definition (structured file + position) |
| `lsp_references` | Find all references to symbol at point |
| `lsp_hover` | Type info / docs for symbol |
| `lsp_workspace_symbol` | Search symbols across workspace |
| `lsp_document_symbols` | List all symbols in current buffer |
| `lsp_diagnostics` | Current errors/warnings from LSP |

### Debugging (DAP)

| Tool | Purpose |
|------|---------|
| `dap_start` | Launch or attach debug session |
| `dap_set_breakpoint` | Set breakpoint (conditional/logpoint) |
| `dap_continue` / `dap_step` | Control execution |
| `debug_state` | Inspect stack frames, variables |

### Knowledge Base

| Tool | Purpose |
|------|---------|
| `kb_search` | Full-text search across all KB nodes |
| `kb_get` | Fetch a specific node by ID (supports block-level: `concept:buffer#3`) |
| `kb_links_from` / `kb_links_to` | Navigate the typed link graph |
| `kb_graph` | Neighborhood subgraph around a node |
| `kb_search_context` | RAG-style ranked excerpts for architecture questions |
| `kb_agenda` | Agenda queries: todo, priority, tag, stale, orphan, dead-end, custom Datalog |
| `kb_health` | Structured health report (node/link counts, orphans, broken links, hubs) |
| `kb_history` | Node version history (snapshots on each update) |
| `kb_restore` | Restore a node to a previous version |
| `kb_view_query` | Execute a stored Datalog view (kanban, backlog, sprint, agenda) |
| `kb_raw_query` | Execute arbitrary CozoDB Datalog against the KB |
| `kb_vector_search` | Vector similarity search (HNSW index, requires embeddings) |

Node ID namespaces: `cmd:*` (commands), `concept:*` (architecture), `lesson:*` (tutorial), `scheme:*` (Scheme API), `option:*` (editor options), `category:*` (categories), `task:*` (tasks), `view:*` (views), `meta:*` (meta-nodes).

### Collaboration / KB Sharing

| Tool | Purpose |
|------|---------|
| `collab_status` | Connection state, peer count, synced docs |
| `collab_connect` | Connect to a daemon for collab |
| `collab_share` | Share a buffer for collaborative editing |
| `collab_doctor` | Run connectivity diagnostics |
| `collab_list` | List shared documents on the server |
| `collab_discover` | Discover MAE peers via mDNS |
| `kb_sharing_status` | Introspect KBs + members/roles/policy/pending/my-role (call before managing) |
| `kb_share` | Share a KB for collaborative editing |
| `kb_join` | Join a shared KB from the server |
| `kb_leave` | Leave a shared KB (local copy preserved) |
| `kb_add_member` / `kb_remove_member` | Add/remove a member by fingerprint (owner-only) |
| `kb_approve` | Approve a pending join request as a role (owner-only) |
| `kb_set_policy` | Set join policy: restrictive\|invite\|permissive (owner-only) |

KB-sharing lifecycle is also first-class in Scheme: `(kb-share)`, `(kb-join)`,
`(kb-leave)`, `(kb-add-member)`, `(kb-remove-member)`, `(kb-approve)`,
`(kb-set-policy)`, `(kb-sharing-status)`. The `*KB Sharing*` buffer (`SPC C K m`),
the Scheme primitive, and the MCP tool all read the same introspection snapshot.

### Buffer / Editor

| Tool | Purpose |
|------|---------|
| `buffer_read` / `buffer_write` | Read/edit buffer contents |
| `project_search` | Ripgrep across project files |
| `command_list` | List all registered commands |
| `execute_command` | Dispatch any editor command |
| `eval_scheme` | Evaluate Scheme expression |
| `audit_configuration` | Structured config health report |
| `introspect` | Diagnostic snapshot of editor state |

### Model Exam

| Tool | Purpose |
|------|---------|
| `model_exam` | Run deterministic tool-calling exam (`action=plan` / `action=grade`) |

### Validation

`self_test_suite` returns the structured JSON test plan. Execute each test by calling the listed tools and checking assertions. Categories: `introspection`, `editing`, `git`, `help`, `project`, `lsp`, `dap`, `babel`, `guidance`, `performance`, `scrolling`.

`model_exam` provides a 12-test deterministic exam (6 categories) for validating model tool-calling capabilities. Results auto-save to `~/.local/share/mae/exam-results/`. See [MODEL_SUPPORT.md](docs/MODEL_SUPPORT.md).

### When to Use

- **Navigating MAE's own code**: `lsp_definition` / `lsp_references` over raw grep — structured results, no false positives.
- **Understanding architecture**: `kb_search "window group"` or `kb_get "concept:window"` — curated docs, not raw source.
- **Debugging MAE**: `dap_start` with `lldb-dap` for Rust, `debug_state` for stack inspection.
- **Testing changes**: `execute_command` to trigger commands, `self_test_suite` for structured E2E.

### Tool Selection: LSP vs Grep

When developing **inside MAE** (connected via `mae-mcp-shim`):
- **Prefer LSP tools** (`lsp_definition`, `lsp_references`, `lsp_hover`, `lsp_workspace_symbol`) for navigating Rust code — they give precise file+line+column with no false positives
- **Use `project_search`** (ripgrep) for cross-language text patterns, string literals, config values
- **Use `kb_search`/`kb_get`** for architectural concepts and documented workflows

When developing **outside MAE** (Claude Code directly on filesystem):
- Use built-in Grep/Glob/Read tools (faster, no event loop round-trip)
- LSP tools require the editor to be running with rust-analyzer connected

## Security

See `SECURITY.md` for the full security posture. Key points for development:

- **Permission tiers are not a security boundary.** They are enforced *at the effect*
  (ADR-084), not merely "before tool execution", and the pre-v0.15 audit found real bypasses —
  the embedded agent reached `sh -c` regardless of tier, `write` tier reached shell via
  `eval_scheme`, and the `knowledge` category allowlist granted arbitrary code execution. Those
  are fixed (ADR-084/085/090), but per `SECURITY.md`: *"Do not rely on any tier as a boundary
  against an adversarial or prompt-injected model; run MAE in a container for genuinely untrusted
  input."* **Never restore a "no bypass vectors exist" claim here** — that exact sentence stood in
  this file while the audit was refuting it, priming every AI session with a false invariant.
- **The default tier is `readonly`** as of v0.15 (ADR-090 D5) — reads auto-approved, writes and
  shell *asked*. A non-interactive surface (external MCP, `--prompt`, `--self-test`) **denies**
  rather than asks, so those deployments need an explicit `auto_approve_tier`. Breaking change.
- **Project-local `.mae/init.scm` requires workspace trust** (ADR-089). Opening a cloned repo used
  to be arbitrary code execution, with no AI agent or prompt injection involved.
- Use `api_key_command` with a password manager, not plaintext `api_key` in config
- MCP socket (`/tmp/mae-{PID}.sock`) — Unix permissions plus PSK pairing
  (`/tmp/mae-{pid}.psk`); Windows uses named pipes (ADR-066)
- Transcripts in `~/.local/share/mae/transcripts/` contain raw tool output (no secret scrubbing)
- Shell blocklist is substring-based and bypassable — defense in depth, not a sandbox

## Server-Client Architecture

MAE's MCP server supports multiple concurrent clients over Unix domain sockets.
Each client gets its own session with capability negotiation and state subscriptions.

### Protocol
- JSON-RPC 2.0 with Content-Length framing (LSP-compatible)
- Session lifecycle: `initialize` → `notifications/initialized` → ready → `shutdown`
- Heartbeat: `$/ping` returns `"pong"`, idle detection via `last_activity`
- Backpressure: per-client bounded queues (100 events), write timeout (5s)

### State Notifications
Clients subscribe to event types via `notifications/subscribe`: `buffer_edit`,
`cursor_move`, `diagnostics`, `mode_change`, `buffer_open`, `buffer_close`,
`sync_update`, `peer_joined`, `peer_left`, `save_committed`.
Events carry version numbers for ordering. Slow clients are dropped, not blocked.

### File Safety
- Content-hash verification on save (SHA-256, catches mtime failures)
- Advisory file locks (`.{name}.mae.lock` with PID/hostname)
- inotify-based external change detection (existing `notify` infrastructure)
- Git worktree isolation for multi-AI workflows

### Architecture Decision Records
ADRs live in `docs/adr/` and as KB concept nodes (`concept:adr-*`).
See ADR-001 (protocol), ADR-002 (text sync — accepted: yrs), ADR-003 (file safety), ADR-004 (KB scaling), ADR-005 (KB CRDT), ADR-006 (collaborative state engine), ADR-007 (save coordination), ADR-008 (CRDT target metrics), ADR-014 (binary architecture — editor + daemon workspaces), ADR-017/018 (asymmetric peer auth + identity-anchored access control), ADR-019/020/022/023 (durable / replicated / crash-safe sync + epoch-fenced write access), ADR-024 (notification attention bus), the **P2P daemon-mesh trio ADR-025/026/027** (iroh transport / peer-verifiable signed-hash-chained integrity / collaboration observability), the **KB-architecture set ADR-028–034** (data lifecycle / CRDT-as-truth + cozo-as-projection / in-text link grammar / derived intelligence / durable CRDT store / operation coordination / cross-peer artifact sharing), **ADR-035** (editor↔daemon boundary + `daemon_mode`), the **content-integrity + confidentiality pair ADR-036/037** (signed content ops / E2E content encryption), the **E2E KB-sharing pair ADR-038/039** (editor-authored key-blind membership / identity + authorization hardening), the **identity-arc pair ADR-040/041** (key rotation/rebind — cross-signed, history-preserving / key separation — a published X25519 wrap key), **ADR-042** (membership-derivation cache + O(n) deterministic causal order), **ADR-043** (P2P share integrity — a fresh mesh share seeds the signed owner-genesis so it anchors membership + E2E identically to the hub), **ADR-044** (e2e daemon-lifecycle safety), **ADR-048** (AI residency policy for sensitive KBs), **ADR-049** (`mae-agent` as the default AI-interaction surface, `ai_chat_enabled` gates the legacy embedded chat — supersedes ADR-046's rejected-deprecation call in part), the **external-editor MCP pairing set ADR-050–056** (VS Code/Copilot + cross-editor MCP compatibility / per-session permission+DrivenWindow isolation / OAuth 2.1 resource server / live scoped read-through KB query surface / daemon concurrency hardening / headless service mode / session-scoped tool-category dispatch enforcement for a KB+guidance-only engine instance), and the **MAE long-term architecture set ADR-057–066** (5-layer architecture vision ratification + confirmed gap analysis / per-project KB provisioning + scoped search / ADR-as-KB-node generalization for molecularly-structured decision records / `mae-daemon` as a trusted-org-scale multi-tenant server / KB enrichment as a background daemon capability / federation registry scaling + unified local/remote-hub search / guidance-delivery uniformity across MAE-embedded and external-MCP sessions / a second native MAE frontend for visual-design workflows sharing the same KB/CRDT core / KB+daemon drift corrections / native Windows support for MAE clients), and **ADR-067** (admin-enforced live-query-only KB access — a signed-op-log replication-policy axis, orthogonal to ADR-018's role table, letting a KB owner restrict an authorized member to ADR-053's live query surface instead of full `kb_join` replication; extends ADR-018/026, closes a gap ADR-053 left explicitly out of scope for non-members-only), the
**KB-visualization / export arc ADR-068–074 + 077–082** (full-corpus multi-KB retrieval with
degree-of-interest LOD / edge bundling / `mae-canvas` substrate hardening / chord-diagram wedge
redesign / KB read mode / a live network-shareable HTML KB view on the daemon + its SSE push
transport / `kb-export-subgraph-html` as a real module rather than a Scheme reimplementation /
untranslated-node fallback signalling / guidance-as-colophon / configurable layout constants
(**080 superseded** by 081's real JSON injection) / `required_tag` hard filtering) — note 069–074
are **accepted design only, not built**, **ADR-075** (language/LSP registry consolidation +
Terraform/Dockerfile/Ansible/Helm support), **ADR-076** (the system of bundled KBs), **ADR-083**
(`kb_agenda` becomes federation-aware), and the **pre-v0.15 audit set ADR-084–091** (permission
tiers enforced *at the effect* against a compiler-proven allow-list / `ToolCategory` describes
subject matter, not blast radius / a tool result states whether the caller's requested
postcondition holds / four text-index domains with one owner each / agent effects authorised by
carried provenance rather than ambient session tier — *design only* / project-local init files
require explicit workspace trust / permission decisions are three-state allow-ask-deny / MCP tool
dispatch carries a session handle), and **ADR-092** (one write path for a KB node — `kb_update_node_with`
is the sole content mutator, CRDT text is updated by character-level diff rather than wholesale
replace, and the human edit surface is the node's normalized org source text rather than its rendered
view or a file; ADR-029's **write** side, *proposed, phased*), and **ADR-093** (the node CRDT carries
the whole node — `kind`/`todo_state`/`priority`/`aliases`/`properties`/`source_version` join the
schema behind a `schema_v` key, with tolerant readers and **no upcast-on-read**, so a v1 document
opens unchanged and two peers can never author clashing migration ops; supersedes ADR-092 D4's
"editable is bounded by what syncs", and is the prerequisite that makes a lossless text-KB migration
possible at all). The holistic sharing story + security audits live in `docs/KB_SHARING.md`,
`docs/E2E_ENCRYPTION.md`, and `docs/SECURITY_REVIEW.md`.

> **This index goes stale silently and has done so before** — ADR-068 through 091 were missing
> from it entirely until 2026-08-04, i.e. every ADR from the KB-visualization arc and the whole
> pre-v0.15 audit set, so agents rediscovered decisions that already existed. `docs/adr/` is the
> source of truth; when adding an ADR, add it here too.

### Sync Engine (yrs — Accepted)
Collaborative state uses **yrs** (Yjs Rust port, YATA algorithm). Decision rationale:
- Handles text (`YText`), visual documents (`YMap`/`YArray`), and KB nodes
- Built-in `UndoManager` with per-user stacks
- Proven at scale: Notion (200M+ users), Excalidraw, TLDraw
- Dual structure: yrs is source of truth, ropey is rendering mirror

Transport: JSON-RPC 2.0 with Content-Length framing over TCP (port 9473) and Unix sockets.
Planned upgrade path: msgpack wire format (Content-Type negotiation).

`mae-sync` wraps yrs with MAE-specific document schemas and provides the
ropey bridge. See ADR-006 for full architecture.

### Daemon (`mae-daemon`)

Unified background service: KB persistence (Unix socket) + collaborative editing (TCP). Replaces the former `mae-state-server` (merged in v0.13.2).

**Usage:**
```bash
mae-daemon                          # KB (Unix socket) + collab (TCP 9473)
mae-daemon --check-config           # validate configuration
mae-daemon doctor                   # run diagnostics
```

**Architecture:**
- Dual listener: Unix socket for KB queries, TCP for collab sync
- Per-document locking, WAL-first SQLite persistence, background compaction
- PSK mutual authentication (HMAC-SHA256, `mae_mcp::auth`)
- Transport-generic I/O: `mae_mcp::{read_message, write_framed, handle_request}`

**Config:** `~/.config/mae/daemon.toml` (TOML, XDG-compliant). Legacy: auto-reads `state-server.toml` if `daemon.toml` not found.

**Editor commands (SPC C prefix, doom keymap):**
- `collab-start` (SPC C s), `collab-connect` (SPC C c), `collab-disconnect` (SPC C d)
- `collab-status` (SPC C i), `collab-share` (SPC C S), `collab-sync` (SPC C y), `collab-doctor` (SPC C D)

**Systemd:** `assets/mae-daemon.service` (user unit)

## API Stability

These APIs are intended to remain stable through v1.0:

Counts below are approximate by design — `docs/CODE_MAP.json` (regenerated by `make code-map`, CI-gated)
is the authoritative source for the Scheme and command surfaces. Do not hand-maintain exact figures here.

- **Scheme API:** ~210 editor-facing primitives + ~18 variables (see `:help concept:scheme-api`).
  This counts `crates/scheme/src/runtime/**` only — the ~310 further `register_fn` calls under
  `crates/scheme/src/stdlib/**` are R7RS-standard library functions (`string-append`, `vector-ref`, …),
  specified by the language, not by MAE.
- **Hooks:** 26 hook points (see `:help concept:hooks`)
- **MCP tools:** ~770 tools (~210 hand-authored + one generated per registered command), categorized
  (core/lsp/dap/kb/execution/shell/ai/commands/git/web/visual/debug/collab) — most are 1:1 command mirrors; see
  `docs/MODEL_SUPPORT.md` for the exam methodology this scale is validated against
- **Commands:** ~560 registered builtins
- **Config options:** ~230 registered, persistable via `:set-save`

## Related Resources

- **Full architecture spec:** `README.md`
- **Emacs source for reference:** the Emacs source tree (clone of emacs-mirror/emacs, `emacs-30` branch)
- **Declarative project config:** `.project` in repo root (for declarative-project-mode in Emacs)
- **ropey:** https://github.com/cessen/ropey — rope data structure for buffer management
- **ratatui:** https://github.com/ratatui/ratatui — terminal UI framework
- **tree-sitter-org:** org-mode grammar for tree-sitter
