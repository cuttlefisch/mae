# ADR-110: The KB cutover — the store is truth, and a KB has three states

**Status:** Accepted, and **implemented** (PRs #797–#805, #809, #811, #812). Like ADR-104, this is
written *after* the work rather than before it, and that order is a defect rather than a style.
Recorded per principle #17 — amend in the open, with the case named. Writing it retrospectively did
surface one thing the implementation had not stated anywhere: the archives cannot be deleted yet,
and the reason is a loss that already happened (D5).

**Extends:** ADR-029 (KB source of truth = CRDT, Cozo = projection). That ADR settles which
*internal* representation is authoritative; this one settles whether the `.org` files on disk are
authoritative at all. They are different questions and ADR-029 answers only the first — it contains
no mention of ingest policy, detaching, or a lifecycle.
**Relates to:** ADR-092 (one write path for a node — the sole mutator this relies on),
ADR-093 (the node CRDT carries the whole node), ADR-030 (in-text link grammar — D5's loss),
ADR-101 (links as first-class structured edges — the fix D5 waits on),
ADR-104 (system KBs — a class with no org directory at all, and the precedent for this ADR's order),
principle #3 (human and AI call the same primitives), principle #8 (one owner per computation).

**Evidence:** `shared/kb/src/federation.rs` (`IngestPolicy`, `register_native`,
`is_instance_sentinel`), `crates/core/src/editor/kb_ops/{stale_archive,retire,registry,daily,
nodes}.rs`, `crates/ai/src/tool_impls/stale_archive.rs`, `daemon/src/handler.rs`.

## Context

MAE's KBs were built by ingesting `.org` directories: the text was authoritative and the CozoDB
store was a projection rebuilt from it. That is the right default for adopting an existing org-roam
corpus and the wrong one for everything after, because it caps a KB at what a file can express and
makes every store-side edit something the next ingest will overwrite.

The cutover inverts it. What made the work substantial was not the flag — it was that "the files are
truth" had leaked into roughly forty places that did not look like ingest: dailies, capture, file
opening, buffer save, autosave, the file-tree dialogs, link-follow, the audit commands, and the
daemon's store routing.

Three defects are worth recording because they shaped the decisions below, and each was found by
running against a real corpus rather than by reading:

- **`KbInstance.primary` means "first row ever registered"**, not "the primary KB". The editor read
  it as the latter (dailies, edit surface); so did the daemon, which routed a user KB's entire
  content into the daemon's own store while reporting `updated=198 errors=0` on every tick. The
  KB's real store was days stale and every layer reported success.
- **A KB's `org_dir` is frequently a whole project repo.** On a real machine one KB's org dir held
  20 files of which 5 were `.org`. A guard keyed on the directory prefix claimed every `.tf`,
  `.yml` and `ansible.cfg` in those repos.
- **A file with no `:ID:` is skipped by ingest before it is recorded**, so it is absent from
  `source_files`. A whole daily note sat invisible in a real KB because of that skip.

## Decisions

### D1. A KB has three states, and the third is derived rather than stored

| state | meaning | representation |
|---|---|---|
| **Attached** | `.org` is truth; ingest overwrites the store | `IngestPolicy::FromOrgDir` |
| **Migrating** | the store is truth; an archive remains on disk | `StoreIsTruth` + non-empty `org_dir` |
| **Native** | the store is truth; no files are involved | `StoreIsTruth` + **empty** `org_dir` |

Native is not a flag. Every guard already tests `!org_dir.is_empty()`, so clearing `org_dir` makes
them all go quiet at once, and there is no stored state that can disagree with what is on disk. A
separate `is_native` field would be a second thing to keep true.

`:kb-detach` and `:kb-attach` move between the first two; `:kb-retire-archive` completes the third;
`:kb-new` starts there.

### D2. The rule lives where both audiences reach it, and reads stay possible

The guard is in `mae-core` (`kb_ops/stale_archive.rs`); the agent tool layer delegates to it, and
the message names both spellings (`:kb-search` / `kb_search`).

It was originally written in `crates/ai/src/tool_impls/`, which covered the agent and left every
human path — `:e`, the picker, `:w`, autosave, the file-tree dialogs — unguarded. The human got
precisely the silent lost edit the agent was protected from. Principle #3 is the reason: the human
and the AI call the same primitives, so a rule about those primitives cannot live in one caller.

**Writes are refused; reads are not.** An archive opens **read-only with a banner**, because it is
still the only copy of what the store lost at ingest (D5) and must stay readable. Read-only is what
stops an edit being stranded. Refusals happen at the two chokepoints (`save_current_buffer`,
`save_all_modified_buffers`) so autosave cannot silently do what an explicit `:w` is refused.

### D3. A file is a KB's source only if that KB imported it

Membership is `source_files`, not the directory prefix — `get_source_file_hash` is a keyed
single-path lookup, cheap enough for a file-open path.

The prefix test is wrong because an `org_dir` is routinely a whole project repo. Two corollaries:

- A `.org` in the directory that was **never imported** is deliberately not claimed. It genuinely is
  not in the KB, so editing it loses nothing, and D4's gate is what surfaces it.
- A **brand-new** `.org` written into a detached KB's directory needs a *different* rule
  (`kb_orphan_org_target`, keyed on the extension), because a file that does not exist yet can
  never be in `source_files`. It would look exactly like adding a note while being invisible.

### D4. Retirement is gated, all-or-nothing, and moves rather than deletes

`:kb-retire-archive` is a dry run unless given `confirm`. Every `.org` under the origin must pass
three checks: a `source_files` row exists, its content hash still matches, and its node ids still
resolve. Anything failing blocks the whole retirement — not just its own file.

The gate is exact rather than heuristic precisely because of the skip described in Context: a file
ingest never parsed is absent from `source_files`, so it shows up as a blocker instead of being
moved out from under a store that never held it. In practice it refused on a daily note, on four
ADRs written after a detach, and on edits made post-detach that never reached the store — content
that a "detached means the store has it" assumption would have destroyed.

Files are **copied, verified, then unlinked**, into `~/.local/share/mae/retired/<kb>/<date>/`. An
interruption therefore leaves the source in place rather than a half-moved archive, and it works
across filesystems where a rename would not. MAE's own instance sentinel goes with the archive —
the gate skips it (ingest never records it), but once `org_dir` is cleared nothing reads it either.

**MAE never touches git.** A tracked file leaving the tree shows up as an ordinary deletion for the
operator to review; the plan says so when the origin has a remote.

### D5. Retired archives are retained, and this is not caution

`rewrite_links_with_types` flattens every non-`id:` link at ingest to plain text — `display
(target)` — and `export.rs` can only reconstruct `id:` links. Measured on a real corpus: ~1,060 such
links across 354 files, and sampling six of them found the flattened form present in the store and
the original markup absent in all six.

So the loss already happened at ingest; retirement does not cause it. What retirement changes is
that the `.org` files stop being authoritative and become the **only** surviving copy of that
markup. Deleting them makes it permanent. ADR-101 (links as first-class structured edges) is the
fix; until it lands, the holding directory is not a convenience.

### D6. A KB with no directory is identified by its name

`KbRegistry::register` dedupes on `org_dir`. A native KB's is empty, and a retired KB's is cleared to
empty, so reusing that check would make every native KB a duplicate of the first and silently return
its uuid. `register_native` is therefore a sibling rather than a flag on `register`, and it refuses
outright when a name already belongs to a directory-backed KB — adopting it would be a different
claim.

### D7. Never read `primary: bool` to select a store, a policy, or a directory

It means "first row ever registered on this machine". Route by uuid. The machine-global primary's
policy lives on `KbRegistry` (`primary_ingest_policy`), not on any row. This is stated as a decision
because documenting it in `federation.rs` did not stop it being violated twice, in two crates, with
the daemon instance silently misrouting a user's entire KB.

## Consequences

- A migrating KB's archive is read-only in MAE until retired. Other editors are unaffected. That is
  deliberate pressure to finish a migration, and reversible with `:kb-attach`.
- `kb-import-plan` and `kb-import-verify` refuse on a detached KB rather than reporting expected
  post-detach divergence as loss.
- A KB can now exist that no filesystem path describes. Anything keyed on `org_dir` must tolerate an
  empty one; the guards already do, and `system_kb` disambiguates by `db_path`.
- Node buffers needed an explicit image base (`Buffer::image_base_dir`), since a node has no file to
  resolve relative paths against.

## Not decided here

- **Attachments and images as a KB concept.** There is no `NodeKind` for them and no store-side
  representation. Needs its own ADR.
- **Multi-node export.** Ingest extracts many nodes from one file; export writes one flat file per
  node, so org-roam file structure does not survive a round trip.
- **Scheme `(read-file)`** is deliberately unguarded — it is a read, and gating a general-purpose
  language primitive on KB policy is a larger decision than this ADR should make.
