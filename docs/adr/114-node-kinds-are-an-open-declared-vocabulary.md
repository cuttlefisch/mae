# ADR-114: Node kinds are an open, declared, multilingual vocabulary

**Status:** Accepted (design), 2026-11-09. Nothing here is built.
**Relates to:** ADR-093 (the node CRDT carries `kind` as a string; this ADR keeps it that way end to
end), ADR-112 D4 (the parse depends on the text and the KB's declared settings, never on code),
ADR-113 (publication review; the first KB to use it is bilingual), ADR-018 (KB roles: who may edit the
vocabulary), ADR-104 (reserved system namespaces).
**Evidence:** `docs/research/114-kind-vocabulary-prior-art.md`.

## Context

A node's `kind` says what it is: a `task`, a `concept`, a `procedimiento`, a `paso`. The CRDT stores it
as a string (ADR-093), but MAE's `Node` converts it to a fixed enum of 14 values with
`NodeKind::from_str_lossy`, and anything unknown becomes `note`. That conversion is lossy in three
measured ways:

- **Domain kinds vanish.** A team KB measured for this decision uses nine Spanish domain kinds (for
  example `procedimiento` and `paso`) on about one file in seven; all nine become `note`. MAE's own DevPractices corpus has a `procedure` that becomes `note` too.
- **Case is significant by accident.** `from_str_lossy("Task")` is `note`.
- **The enum is MAE's help system, not a general vocabulary.** Six of the 14 (`command`, `key`,
  `scheme_api`, `lesson`, `tutorial`, `meta`) exist for MAE's own manual.

The team writing these KBs works in Spanish and English, so the same kind is reached by two words.

Only a few kinds drive behaviour: the built-in views select `kind = "task"`
(`shared/kb/src/cozo_store/schema.rs`), the graph view roots at an `index` node, the help system reads
`lesson`/`tutorial`, `meta` marks a composite body, and search down-weights `category`/`meta` (or
`:role: hub`). Everything else is display.

The owner decided (2026-11-09) that a kind is **stored as written**, that behaviour is reached through a
**mapping** where needed, and that kinds should support **aliases across languages**. This ADR is how.

## Decision

### D1. The stored kind is the string as written, never rewritten

The node keeps its `kind` byte for byte, in the CRDT and in every reader. `from_str_lossy` stops being a
storage path: nothing writes `note` for a kind it does not recognise. The only operation that rewrites a
node's kind is an explicit, owner-run, revertible bulk edit (D6).

### D2. A KB may declare its kind vocabulary

The vocabulary lives in the KB's declarations, the same per-KB settings ADR-112 D4 already makes
data, replicated with the KB and edited by an Owner (ADR-018). Each entry is one kind:

- **`id`**: a readable slug (`procedimiento`, `task`), immutable once any node resolves to it.
- **`class`**: at most one behaviour class (D4), or none.
- **labels**: one preferred label per language, plus a language-neutral default (Wikidata's `mul`),
  alternative labels and hidden labels (SKOS `prefLabel`/`altLabel`/`hiddenLabel`).

One kind, translated labels: Drupal's "Localize" model. Not one kind per language, which is what
produces `procedure`/`procedimiento` as two kinds.

### D3. Resolution: one match key, one namespace, decided on read

- **Match key.** A raw kind, an id or a label is compared through one function in `mae-kb`:
  NFC, then Unicode default full case folding (locale-independent), then NFC, with surrounding
  whitespace trimmed. Accents are not stripped (`revisión` and `revision` are distinct unless declared
  as aliases). Every consumer (filters, views, graph, search, completion, the browser) uses this one
  function; a property test pins that they agree.
- **One namespace per KB, across all languages.** A stored kind carries no language tag, so every id
  and label of every kind, in every language, must fold to a distinct key. Two kinds claiming one key
  is a declaration error, reported where the vocabulary is edited.
- **Decided on read, deterministically.** Concurrent edits can still merge into a clash that no write
  check saw. A key resolves by precedence: an id beats a label, and a label beats an alias. A tie within a tier
  makes the key **ambiguous**. An ambiguous or undeclared kind resolves to no kind: the node keeps its
  raw string, gets no class, and shows its status. Nothing is ever picked by insertion order or client id.

### D4. Behaviour reads the class, and the class lives in the projection

Behaviour classes are the small set MAE actually branches on: `task`, `index`, `view`, `meta`, `hub`,
`lesson`, `tutorial`. The first implementation step audits every kind-dependent site (the views, the
graph view, search scoring, the help system, the projector) and fixes the set from that audit. In
particular it decides whether `:role: hub` and `category` both become class `hub`.

The class is **joined at query time, never written onto node rows.** The Cozo projection gains
`kind_raw`, `kind_key` and a `kind_vocab{key => kind_id, class, status}` relation. The built-in views
move from `kind = "task"` to joining on class `task`. A vocabulary edit then costs work proportional to
the vocabulary and cannot leave stale rows (SharePoint's list-level copies are the documented failure).
User-written Datalog over `kind` keeps raw-string semantics, as documented, unless it joins the
vocabulary itself.

At most one class per kind, and no inheritance of behaviour between kinds. A query-only `broader` link
("all procedural kinds") is deferred.

Mapping a kind to `view` grants no execution: `kb_view_query` runs the separate `views` relation, not a
node body (verified in the evidence file). If a node body ever becomes executable, assigning a class
becomes a principle #16 control and must be owner-only from then on.

### D5. Built-in kinds are a frozen, versioned vocabulary

MAE's built-in kinds ship as data in the same schema: `mae-builtin/1`. A KB references a version, and a
KB that references none uses `mae-builtin/1` forever. A release that changes the built-ins adds
`mae-builtin/2`; it never edits `/1`. Otherwise two peers on different MAE releases would classify the
same KB differently, which ADR-112 D4 forbids. A declared vocabulary **extends** the built-ins it
references, with an explicit opt-out, so a Spanish KB keeps `task` working without re-declaring it.
Built-in ids live in a reserved namespace, as system KB names do (ADR-104).

### D6. Undeclared kinds, renames and merges

- **Undeclared:** the raw string, no class, status "undeclared", shown as written. A filter on `note`
  does not match it. A KB may opt into **strict** mode, which refuses or flags an undeclared kind on
  write (org's `Xyz_ALL` allowed-values list is the native precedent). The undeclared kinds in a KB are
  a worklist for promoting into the vocabulary.
- **Rename:** changes labels; the id stays.
- **Merge A into B:** A's id and labels become aliases of B, and A's id is tombstoned and never reused
  (Wikidata's merge rule).
- **Split:** rewriting nodes' `:KIND:` is an explicit, owner-run, revertible bulk edit, recorded as one
  edit group. It is the only path that rewrites stored kinds.

### D7. Display, export and movement

- **Display** shows the resolved kind's label in the reader's language, falling back to the default
  label, then the id, then the raw string. The source view (the org text) always shows what is stored.
- **Export** writes the KB's declarations alongside its nodes, so an exported KB keeps its classes.
- **Moving or copying a node** to another KB keeps its raw kind and resolves it against the target KB's
  vocabulary; if the target has no such kind, the node is undeclared there and the operation says so.
  Federated queries resolve each node against its own KB.

## Consequences

- The `Node` type stops holding a lossy enum. The class is derived, and every consumer that matched
  `NodeKind::X` reads the class instead. This touches the help system, the graph view, search scoring,
  the projector and the built-in views.
- Existing KBs that declare nothing behave exactly as today, through `mae-builtin/1`, except that a kind
  outside the built-ins is shown as written instead of as `note`, and `Task` (capital T) now resolves to
  `task`.
- The browser client reads the same declarations for labels, so both surfaces agree by construction.
- Native KBs imported before this decision already lost their domain kinds to `note`; those cannot be
  recovered from the store and need a re-import from their sources where those still exist.

## Rejected

- **A fixed enum with more variants.** Every corpus measured has its own vocabulary; a shipped list
  always lags, and adding Spanish variants makes the enum a translation table.
- **Rewriting kinds to a canonical form on input.** It is the folksonomy "synonym" failure: old nodes
  keep the old form and the corpus stays split. Resolution on read leaves every node as written and
  still groups them.
- **Labels as identifiers.** Renaming a label would reclassify the corpus, the SharePoint
  display-name breakage.
- **Kind inheritance (Logseq/Tana "extends").** Users report it as confusing, and MAE's behaviours are
  exclusive, so a kind has no parent behaviour to fall back to.
- **Per-language uniqueness only (as SKOS requires).** SKOS labels carry a language tag; a stored
  `:KIND:` does not, so uniqueness has to hold across languages.
