# ADR-104: System KBs are a distinct class, delivered with the binary

**Status:** Accepted, and **implemented** — unusually for this repo, this ADR is written *after* the
work rather than before it (PRs #699, #701, #702, #704, #705, #706, #707, #708). That order is a
defect, not a style: the prior-art review it should have been gated on was run retrospectively and
still corrected one decision's rationale (D6) and sharpened another that the nearest prior art
actually contradicts (D5). Recorded per principle #17 — amend in the open, with the case named.

**Supersedes:** **ADR-076** (the system of bundled KBs). ADR-076's structure survives; its delivery
mechanism does not. Rather than amend four of its five decisions in place and leave a reader to diff
two documents, 076 is superseded and the parts that still hold are restated here (D4, D5).

**Extends:** ADR-058 (per-project KB provisioning — the "no second registry" objection, which D2
addresses head-on), ADR-059 (ADR-as-KB — why the ADR corpus stays opt-in and unembedded),
ADR-063 (guidance-delivery uniformity).
**Relates to:** ADR-004/102 (KB engine — D6 depends directly on cozo's disk-engine properties),
ADR-035 (editor↔daemon boundary — the daemon's inability to open sled is load-bearing in D6),
ADR-084/085/090 (permission tiers — the controls in D5), principle #16 (asymmetry as a feature),
principle #12 (local-first — a corpus in the binary needs no network at all).

**Evidence:** `shared/kb/src/{system_kb.rs,kb_build.rs,federation.rs}`,
`crates/mae/src/{system_corpus.rs,guidance_kb_engine.rs,bootstrap/mod.rs,manual_kb.rs}`,
`crates/core/src/editor/kb_state.rs`, `crates/ai/src/{guidance.rs,tools/authorization.rs}`,
`crates/mae/src/kb_provisioning_cost.rs` (the Phase 0 harness), and the prior-art brief
`docs/research/104-system-kb-prior-art.md`.

## Context

MAE ships four corpora of its own — the manual/help KB, MaePractices, DevPractices and the ADR KB —
and also manages knowledge bases the user authors. Before this work the two were the same thing:
both were rows in `kb-registry.toml`, both were CozoDB stores in the data dir, both were reached by
the same lookup paths.

That conflation was not a tidiness problem. It produced concrete, repeated breakage:

- **The registry could not say what it had written.** On a real machine, `MaePractices` was recorded
  `UserRegistered` and `DevPractices` `Guidance` — two different `kind` values for two rows MAE
  itself wrote. The migration therefore keys on row *shape*, not on `kind`, because `kind` had
  already drifted into meaninglessness.
- **Half of MAE could not open its own shipped content.** The stores were sled; the daemon is
  sqlite-only.
- **A security control was a workaround.** Sharing a system KB strips its `Seed` provenance,
  because `NodeSource` is absent from the CRDT wire payload — so shipped read-only content arrives
  at the peer as ordinary editable `Federation` content.
- **Most users got none of it.** ADR-076's own Context records that MaePractices and the ADR KB
  were never wired into `release.yml`/`install.sh`. Windows, the Docker image and `cargo install`
  shipped zero corpora throughout.
- **A backup script protected the wrong things.** `backup-kbs.sh` archived the four regenerable
  system stores and ignored the user's own KBs entirely.

Every one of those is the same root cause: content whose truth, mutability, upgrade semantics and
backup needs are *the application's* was being handled by machinery designed for content that is
*the user's*.

## Decisions

### D1 — System KBs and user KBs are distinct classes, structurally

A **system KB** is MAE's own operational corpus: its truth lives in MAE's sources, it is replaced
wholesale by the next release, it is never the user's to lose, and it exists to drive MAE's and the
AI peer's behaviour. A **user KB** is content the user authors or federates: its truth lives on
their machine, MAE only manages it, and losing it is data loss.

The distinction is enforced in the type system and the tool surface, not by naming convention.
`mae_kb::system_kb` is the catalog; system names are reserved against `kb_register`; `kb_unregister`,
`kb_reimport`, `kb_share` and `kb_share_p2p` refuse them.

*Prior art:* the mainstream design — VS Code built-in vs marketplace extensions, Emacs built-in vs
ELPA, system vs user fonts. The distinguishing property is not authorship but **whose release cycle
owns the content**.

### D2 — The catalog is compile-time and persists nothing

`SYSTEM_KBS` is a `&'static [SystemKb]` in the binary. It is not written to `kb-registry.toml`, and
existing system rows are evicted from that file on startup.

**This is not the second registry ADR-058 rejected.** That objection was to a second *persisted*
source of truth that could disagree with the first. A compile-time constant has no on-disk state, so
it cannot drift, cannot be edited into disagreement, and cannot be half-migrated. The registry
remains the single persisted registry — of user KBs, which is all it was ever able to describe
correctly.

### D3 — Corpora are embedded in the binary; on-disk `assets/` overrides

The `.org` sources are compiled in via `include_dir!`. An on-disk `assets/<corpus>` found by walking
up from the executable wins, preserving the edit-and-rebuild loop without a recompile.

This is the established hybrid, not an improvisation: `rust-embed` embeds in release and loads from
the filesystem in development; `go:embed` is justified in the same terms (single-binary deployment,
no missing assets, no broken paths, atomic code/data updates). The documented counter-cost —
recompile-to-update, and binary growth — is real, and is why the override is load-bearing rather
than a convenience, and why the ADR corpus is excluded (D4).

**Verification note, learned the hard way:** embedded bytes reach the binary only when something
*uses* them. Referenced solely from `#[cfg(test)]` code, the linker dropped them and the "embedded"
build measured *smaller*. An embedding change is verified by the binary growing, not by compiling.

### D4 — The bundled set, and what is deliberately not bundled *(carried from ADR-076)*

| KB | Role | Embedded | Auto-enabled |
|---|---|---|---|
| manual | the help system | yes | yes |
| DevPractices | generic, vendor-neutral practices — the shipped `ai_guidance_kb` default | yes | yes |
| MaePractices | MAE-contributor conventions | yes | yes |
| ADR | MAE's own decision history | **no** | **no** |

The ADR corpus stays opt-in per ADR-059 — injecting dozens of ADR summaries into every session is
noise, not signal — and unembedded because 1.17 MB of contributor-only history has no business in
every user's binary. It ships as its own `mae-adr.cozo.tar.gz` release asset.

Built stores live under **`$XDG_CACHE_HOME`**, not the data dir. The spec's own test decides it: a
store rebuilt from bytes inside the binary is regenerable by construction, and `$XDG_CACHE_HOME` is
defined for exactly that. Putting it in the data dir is what let `backup-kbs.sh` confuse
release-owned derived content with the user's irreplaceable KBs.

### D5 — A user's same-named KB wins over the bundled default *(carried from ADR-076, rationale sharpened)*

If a contributor registers their own KB under a bundled name, theirs wins.

**The nearest prior art rejects this**, and ADR-076 asserted it without noticing. Emacs resolves the
identical collision the opposite way: `load-path` order makes a built-in package *shadow* a
same-named user package, on the stated grounds that built-ins "may have dependencies or
modifications that are essential to the distribution's functionality."

MAE lands the other way because that rationale is about **code with dependencies**. Nothing in MAE
`require`s a node from DevPractices; system KBs are *prose consumed by search and guidance*.
Substituting different guidance text changes what the agent is advised, which is the entire point of
making it configurable. The failure mode Emacs protects against does not exist here.

**The limit is principle #16's, and it is what makes this safe:** user-content-wins holds because the
*human* is substituting. It would be unsafe if the agent could, which is why `ai_guidance_kb` is
Privileged, system names are reserved against `kb_register`, and the sharing/lifecycle tools refuse
them. The rule is "the human's content wins," never "whoever writes last wins."

**If a system KB ever becomes load-bearing for MAE's own behaviour the way `subr.el` is, this
decision must be revisited.** It is defensible only on the content-vs-code distinction.

### D6 — Ship no pre-built stores; build from the embedded corpus on first run

No `.cozo` store ships in any release artifact, package, AppImage or `.app` bundle. The ADR KB's
standalone asset is unaffected.

**Phase 0 measured the cost rather than assuming it** (`kb_provisioning_cost.rs`; one fast idle
Linux machine, 2026-08 — figures are machine-specific and belong here, dated, not in prose
elsewhere). Manual KB, full pipeline of 1187 code-generated seed nodes plus 237 org nodes:

| engine | total | seed | persist | org | store |
|---|---|---|---|---|---|
| mem | 2.183s | 0.020s | 1.408s | 0.755s | — |
| sqlite | 6.365s | 0.009s | 4.970s | 1.385s | 14 MB |
| sled | 9.272s | 0.009s | 6.774s | 2.488s | 38 MB |

Guidance corpora (org only, sqlite): MaePractices 0.021s, DevPractices 0.198s. The mechanism this
replaced cost 0.044s for 1198 nodes.

Three findings shaped the design. `persist_nodes` is 65–75% of every column (~1.2 ms/node) — the
cost is store-write overhead, not parsing. `seed_kb` itself is free at 0.020s. And **sled, the
shipped format, lands at 9.3s against a ~10s watchdog on a fast idle machine** — independent support
for ADR-102 D4's sled deprecation.

Phase 0 also **refuted the original plan for the manual**: "no durable store, ingest into
`open_mem()`" trades a 0.044s per-startup load for a 2.183s per-startup build, because an in-memory
store cannot be cached across runs. The manual therefore keeps an immediate in-memory store — the
invariant being that *the query layer has MAE's manual content in every mode from the first tick* —
with the durable version-keyed projection as an upgrade built in the background, not the only source.

**The rationale is narrower than "pre-built stores are bad," and the prior art forced the
correction.** Published guidance favours shipping a pre-built database for read-heavy static
reference data — which the manual is — and warns that building on first load "is slow, and then
invalidating/mutating it when the underlying data store changes is slow and error-prone." That is a
fair description of a cost MAE now pays.

The advice survives contact with MAE only because it presumes *a stable single file*, and every such
property was absent for sled: it is a directory, it is rewritten in place on first open (so it is not
byte-reproducible and an installed store can never be checksum-verified — which is why `install.sh`
advertised "SHA-256 checksum stored (validated at runtime)" for a check that could not exist), the
sqlite-only daemon cannot open it, and it shipped on some platforms only.

**So the objection is to shipping sled, not to shipping a pre-built store.** If MAE later wants to
eliminate the first-run build cost, the answer the evidence points to is a **single-file sqlite
store** — stable, verifiable, daemon-openable, trivially bundled everywhere. That door is left open
deliberately; do not restate D6 as a blanket principle that closes it.

## Consequences

**Gained.** Windows, the Docker image and `cargo install` have MAE's documentation and practices for
the first time, with no per-platform packaging step to forget. Release artifacts lose the store
weight. The daemon can open every system store MAE builds. The registry describes only what it can
describe correctly. `make uninstall` now backs up the user's KBs instead of four regenerable ones.

**Paid.** A first-run build, once per version, off the main thread — measured above. Updating an
embedded corpus needs a recompile, mitigated by the on-disk override. Binary growth (~600 KB).

**Still open.** `NodeSource` is absent from the CRDT wire payload, so a shared KB loses `Seed`
provenance on the receiving peer. The system-KB sharing refusal is a workaround for that defect, not
a designed position; tracked separately, and D1's refusals should be revisited once provenance
survives sync.

**Not settled by the prior-art review**, named so the gaps are visible: no source was found on
first-run build budgets for desktop applications (MAE's ~10s watchdog bar is its own, and whether it
matches user tolerance is unmeasured), and the VS Code evidence for D1 was thin enough that the
conclusion rests mainly on Emacs and the general system/user pattern.
