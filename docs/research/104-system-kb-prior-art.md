# ADR-104 Phase 0: prior-art review

**Purpose.** Brief the ADR-104 decisions against published practice *before* ratifying them, per the
standing practice that grounding ADR-084/085 this way reversed one decision outright and corrected
two more. This brief was written **after** the implementation shipped, which is the wrong order and
is noted as such: its job is therefore explicitly **refutation** — what does published practice say
MAE got *wrong*? — not a citation hunt for things already believed.

**Method.** Each decision is stated as a falsifiable claim, then tested against the strongest
contrary source found. A decision that survives is marked *holds*; one that does not is marked
*refuted* or *rationale corrected* — the latter meaning the decision stands but for different
reasons than were originally given, which matters because a decision defended by a wrong reason
will be re-litigated the moment that reason is challenged.

**Verdict up front.** Four decisions hold. **One rationale is corrected and is the most useful
finding in this brief: the case against shipping a pre-built store is not general — it is specific
to sled.** One decision (user-content-wins) is contradicted by the closest prior art and survives
only on a distinction that must be stated explicitly, because it is not self-evident.

---

## D1 — System KBs and user KBs are different classes

**Claim.** An application's own operational corpora need a different lifecycle from content the
user authors, and the distinction should be structural rather than a naming convention.

**Prior art.** Universal, across every ecosystem checked. VS Code separates built-in extensions
(updated with the editor's own monthly release) from marketplace extensions (independently
versioned, user-controlled auto-update, rollback to a pinned version). Emacs separates built-in
packages from ELPA/MELPA. The pattern recurs in OS system vs user fonts, Android system vs user
apps, and distribution vs user-installed packages generally.

The distinguishing property is consistent and worth naming precisely: **it is not "who wrote it"
but "whose release cycle owns it."** Built-in content moves when the application moves; user
content moves when the user says so. That is exactly why the two need different upgrade,
backup and removal semantics — and it is the reason a user KB must survive `make uninstall`
while a system KB is expected to be replaced wholesale by the next release.

**Verdict: holds.** The structural split is the mainstream design, not a MAE invention. The
implementation detail MAE adds — a compile-time catalog rather than a persisted registry — is
addressed separately in D2.

**Sources.** [VS Code Extension Marketplace](https://code.visualstudio.com/docs/configure/extensions/extension-marketplace),
[VS Code enterprise extension management](https://code.visualstudio.com/docs/enterprise/extensions),
[GNU Emacs Manual — Package Installation](https://www.gnu.org/software/emacs/manual/html_node/emacs/Package-Installation.html).

---

## D2 — The system-KB catalog is compile-time, persisting nothing

**Claim.** The set of system KBs belongs in the binary, not in a file on disk, and specifically not
in `kb-registry.toml` alongside the user's own registrations.

**Prior art.** This is the ordinary shape: built-in extensions are not rows in the user's
extension database, and built-in Emacs packages are not entries in `package-selected-packages`.
Nothing found argues for recording immutable, release-owned content in a mutable user-owned
registry.

**The failure mode is documented in MAE's own evidence rather than in the literature**, and is
stronger than anything the search turned up: the pre-change registry on this machine had
`MaePractices` recorded as `UserRegistered` and `DevPractices` as `Guidance` — two different
`kind` values for two rows MAE itself had written. The field intended to mark provenance had
already drifted into meaninglessness, which is why the migration keys on row *shape* rather than
on `kind`. A registry that cannot reliably say what it wrote is the argument for not writing
there at all.

**Verdict: holds**, and note this is *not* the second registry file ADR-058 rejected. That
objection was to a second **persisted** source of truth that could disagree with the first. A
compile-time constant cannot disagree with anything at runtime; it has no on-disk state to drift.

---

## D3 — Corpora are embedded in the binary, with on-disk override

**Claim.** Compiling the `.org` sources into the binary beats shipping them as files next to it.

**Prior art.** Strongly supported, and MAE's exact hybrid is the established pattern rather than
an improvisation. `rust-embed`'s documented behaviour is to embed at compile time in release and
**load from the filesystem during development** — precisely MAE's on-disk-`assets/`-overrides-
embedded semantics, arrived at independently. Go's `go:embed` is described in the same terms:
single-binary deployment, no missing assets, no broken paths, no "works on my machine," and
atomic updates with no version mismatch between code and its data.

**The contrary case is real and applies.** Every source is explicit that the trade is deployment
simplicity *against* runtime flexibility: updating an embedded asset requires a recompile, and
large embedded assets inflate the binary. Both bite MAE. The recompile cost is mitigated — not
eliminated — by the on-disk override, which is why that override is load-bearing rather than a
convenience. The size cost is real and paid deliberately; it is also why the ADR corpus (1.17 MB
of contributor-only decision history) is **not** embedded.

One MAE-specific observation worth recording because it wasted time: the embedded bytes only
reach the binary once something *uses* them. With the statics referenced solely from `#[cfg(test)]`
code the linker dropped them entirely and the "embedded" build measured **smaller** than the one
without. An embedding change is not verified by the code compiling; it is verified by the binary
growing.

**Verdict: holds.**

**Sources.** [rust-embed](https://docs.rs/crate/rust-embed/0.3.5),
[A Quick Tour of Trade-offs Embedding Data in Rust](https://nickb.dev/blog/a-quick-tour-of-trade-offs-embedding-data-in-rust/),
[How to Bundle Static Assets into Go Binaries with go:embed](https://oneuptime.com/blog/post/2026-01-25-bundle-static-assets-go-embed/view),
[Go Static Assets Embedding vs. Traditional Serving](https://leapcell.io/blog/go-static-assets-embedding-vs-traditional-serving).

---

## D4 — Built stores live in the XDG cache dir, not the data dir

**Claim.** A store derived from embedded bytes is cache, not data.

**Prior art.** Settled by the specification itself. `$XDG_CACHE_HOME` is defined for
"user-specific non-essential (cached) data," analogous to `/var/cache`; `$XDG_DATA_HOME` is for
user-specific *data* files, analogous to `/usr/share`. The operative test in the secondary
guidance is exactly the one MAE applied: **if it can be regenerated, it is cache.**

A store rebuilt from bytes inside the binary is regenerable by construction — deleting it costs
the user nothing but a rebuild. Placing it in the data dir would put release-owned derived content
in the same directory as the user's irreplaceable KBs, which is the confusion that made
`backup-kbs.sh` back up the wrong things.

**Verdict: holds**, and the misplacement had a concrete downstream cost, which is the useful part.

**Sources.** [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir/latest/),
[platformdirs — Understanding platformdirs](https://platformdirs.readthedocs.io/en/latest/explanation.html),
[XDG Base Directory — ArchWiki](https://wiki.archlinux.org/title/XDG_Base_Directory).

---

## D5 — A user's same-named KB wins over the bundled default

**Claim (ADR-076's precedent, carried forward).** If a contributor registers their own KB under a
bundled name, theirs wins.

**This is the one decision the closest prior art contradicts.** Emacs resolves the same collision
the *opposite* way: `load-path` order puts built-in directories first, so a built-in package
**shadows** a user package of the same name. The recorded rationale is explicit and is a real
argument — built-in packages "may have dependencies or modifications that are essential to the
distribution's functionality, and allowing user packages to override them could potentially break
compatibility."

**Why MAE still lands the other way, stated so it can be attacked.** The Emacs rationale is about
*code with dependencies*: shadowing `subr.el` breaks things that call it. MAE's system KBs are
**content** — prose corpora consumed by search and by the guidance mechanism. Nothing in MAE
`require`s a node from DevPractices; substituting different guidance text changes what the agent
is advised, which is the entire point of making it configurable at all. The failure mode Emacs
protects against does not exist here.

**But the asymmetry has a limit, and it is principle #16's.** User-content-wins is safe precisely
because the *human* is the one substituting. It would be unsafe if the agent could — which is why
`ai_guidance_kb` is Privileged, why system-KB names are reserved against `kb_register`, and why
`kb_share`/`kb_reimport`/`kb_unregister` refuse them. The decision is "the human's content wins,"
never "whoever writes last wins."

**Verdict: holds, with the rationale sharpened.** ADR-076 asserted this precedent without noticing
that the nearest prior art rejects it. It is defensible, but only on the content-vs-code
distinction, and that distinction must be written down — if a system KB ever becomes load-bearing
for MAE's own behaviour in the way `subr.el` is, this decision has to be revisited.

**Sources.** [GNU Emacs Lisp Reference Manual — Library Search](https://www.gnu.org/software/emacs/manual/html_node/elisp/Library-Search.html),
[GNU Emacs Manual — Package Installation](https://www.gnu.org/software/emacs/manual/html_node/emacs/Package-Installation.html).

---

## D6 — Ship no pre-built stores; build from the embedded corpus on first run

**Claim.** Shipping pre-built `.cozo` stores in release artifacts was wrong and removing them is
right.

**The prior art contradicts the general form of this claim, and the contradiction is worth taking
seriously.** For read-heavy, relatively static reference data — which is exactly what the manual
KB is — the published guidance favours shipping the pre-built database: "you can ship a `.db` file
alongside your application that contains reference data, lookup tables, or cached results. There's
no import step. Just open the file." And directly against MAE's chosen direction: "building up an
entire database on first load is slow, and then invalidating/mutating it when the underlying data
store changes is slow and error-prone."

That is a fair description of a cost MAE now pays. Phase 0 measured it rather than assuming it,
and the numbers are in the ADR.

**Why the decision survives anyway — and this is a correction of its rationale, not a defence of
the original one.** The general advice presumes the shipped artifact is *a stable single file*.
Every property that made shipping worthwhile was absent for sled specifically:

- It is a **directory**, not a file.
- It is **rewritten in place on first open**, so it is not byte-reproducible and an installed
  store can never be checksum-verified against a fixed constant — which is why `install.sh`'s
  "SHA-256 checksum stored (validated at runtime)" was advertising a check that could not exist.
- The **sqlite-only daemon cannot open it at all**, so the shipped artifact was unusable by half
  of MAE.
- It shipped on **some platforms only** — Windows, the Docker image and `cargo install` got
  nothing — so the "no import step" benefit was never universal in the first place.

**The correction that matters for the future:** the objection is to *shipping sled*, not to
shipping a pre-built store. If MAE later wants to eliminate the first-run build cost, the prior
art says the answer is to ship a **single-file sqlite store** — stable, verifiable, openable by
the daemon, and trivially bundled on every platform. That door should be left open in the ADR
rather than closed by a blanket "we don't ship stores" principle. Recording this now is the
difference between a decision that can be revisited on its merits and one that has to be
rediscovered.

**Verdict: holds, rationale corrected.** The original framing ("pre-built stores are the problem")
overstates the finding. The accurate framing is: *sled was the wrong shipping format, and
embedding the sources is the right answer while cozo's disk engines have the properties they
currently do.*

**Sources.** [Why SQLite Is a Better Default Database Than You Think](https://www.mindstudio.ai/blog/why-sqlite-is-a-better-default-database),
[What Is SQLite? The Database That Runs Inside Your App](https://www.mindstudio.ai/blog/what-is-sqlite),
[SQLite for Production](https://daily.dev/blog/sqlite-production-guide-when-how-to-use-beyond-prototyping/).

---

## What this brief did not settle

Named so the gaps are visible rather than implied:

- **No source was found on first-run build budgets for desktop applications.** MAE's ~10s
  watchdog bar is its own, and Phase 0 measured against it; whether that bar matches user
  tolerance is unmeasured.
- **The VS Code search returned little on built-in extension *lifecycle* specifically** — the
  D1 conclusion rests more on Emacs and on the general system/user pattern than on VS Code.
- **Nothing was found on provenance stamping surviving a CRDT sync**, which is the open defect
  behind the system-KB sharing refusal: `NodeSource` is absent from the wire payload, so a shared
  KB arrives re-stamped `Federation` and loses the `Seed` marking that makes shipped content
  read-only. Refusing to share a system KB is a workaround for that, not a designed position, and
  it is tracked separately.
