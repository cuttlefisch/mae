# ADR-114 prior art: open, declared, multilingual node kinds

Date: 2026-11-09. Briefed for refutation. **[S]** = sourced fact (link given), **[C]** = read from MAE
code/ADRs in this checkout, **[I]** = my inference.

---

## 1. Summary verdict

**The core of the proposal holds up.** The three parts are: store the string as written, declare a
per-KB vocabulary, and have behaviour read a derived class. Every mature system surveyed keeps a node's
type separate from the behaviour attached to it, and separate from the name a person sees. The closest
precedent is org-mode's own `#+TODO:` line. Keywords there are an open, per-file vocabulary, and their
position around `|` maps each one onto one of two behaviour classes (open or done) [S]. ADR-112 D4
already commits MAE to "TODO sequences … and the KB's own settings are data" [C]. A kind vocabulary
fits that clause directly.

**Seven points need to change or be pinned down before this is safe:**

1. **The built-in default makes the result depend on MAE's code version.** "A KB that declares nothing
   gets MAE's built-in kinds" means two peers running different MAE releases can derive different
   classes for the same KB. That breaks the "same on every peer" constraint and ADR-112 D4 ("never of
   code"). The default has to be a frozen, versioned vocabulary that the KB records by name (§3g).
2. **Resolution has no language tag to work with.** SKOS gets per-language uniqueness because every
   label literal carries `@es`/`@en` [S]. A stored kind string does not. So uniqueness has to hold
   across the union of all labels and aliases in all languages, after normalisation. "One prefLabel per
   language" is necessary but not sufficient (§3d).
3. **Raw Datalog can't be made to read the class.** The built-in views are stored Cozo queries matching
   `kind = "task"` literally (`shared/kb/src/cozo_store/schema.rs:759-791`) [C], and users can write
   their own through `kb_raw_query` and custom views. The class has to exist inside Cozo, as a joined
   relation, or those queries silently stop matching `tarea` nodes (§3c).
4. **The behaviour-class list is incomplete.** `category` already behaves as a hub: search scoring
   down-weights `Category | Meta` *or* `:role: hub` (`crates/ai/src/tool_impls/kb.rs:1723`) [C]. That
   means a second classification axis (`role`) already exists. The graph view also colours by all 14
   variants through a mirrored canvas enum [C]. Audit every kind-dependent site before fixing the class
   set.
5. **Collisions will happen and need deterministic resolution.** Concurrent CRDT edits can create a
   collision that no write-time check can see. Wikidata's write-time uniqueness check missed exactly
   this case (§2.2). Resolution has to be total and deterministic at read time, and ambiguity has to
   show up as a status, not as a silent pick.
6. **Case and Unicode matching must be specified exactly**, or two peers (or two code paths) will
   disagree. Obsidian shipped "tags are case-insensitive" everywhere except one new feature (§2.7).
   MAE today is case-*sensitive* and lossy: `from_str_lossy("Task")` returns `Note`
   (`shared/kb/src/lib.rs`) [C].
7. **The class is not carried with the node.** DITA writes a node's full ancestry into every instance
   (`@class="- topic/li task/step "`), and SharePoint encodes it in the type ID (`0x0101…`) [S]. MAE's
   class lives only in the KB's vocabulary. A node copied, exported or federated without that
   vocabulary loses its behaviour silently [I]. This needs an explicit decision, not an accident.

---

## 2. Per-source findings

### 2.1 W3C SKOS (Reference and Primer, both 18 Aug 2009)
- Concepts are identified by URIs, and labels are not identifiers. Labels only become addressable
  through SKOS-XL (§1.2, §5.6.3). [S] <https://www.w3.org/TR/skos-reference/>
- S13: `prefLabel`, `altLabel` and `hiddenLabel` are pairwise disjoint. S14: "A resource has no more
  than one value of skos:prefLabel per language tag." [S]
- Cross-concept uniqueness is only a *recommendation*: "no two concepts in the same KOS be given the
  same preferred lexical label for any given language tag" (Primer §2.2.1). The working group
  deliberately did not make it an integrity condition, because classification schemes legitimately
  reuse captions (ISSUE-32). [S] <https://www.w3.org/TR/skos-primer/>,
  <https://www.w3.org/2006/07/SWD/track/issues/32>
- `hiddenLabel` exists so search can match misspellings without displaying them (§5.1). [S]
- `exactMatch` is transitive (S45). `closeMatch` is deliberately *not* transitive. `broadMatch` is
  hierarchical. [S]
- **Lesson:** use the id/label split and the three label tiers as they are. SKOS can leave label
  clashes as a recommendation because it never has to *resolve* a bare string to a concept. MAE does
  (every node's `:KIND:` is a bare string), so MAE needs the stronger, enforced rule [I]. Use a
  non-transitive "close" relation as SKOS does, so that alias chains can't silently expand.

### 2.2 Wikidata (labels, aliases, `mul`, merge)
- Aliases are per language, unlimited in number, and **need not be unique**: "Multiple items can have
  the same alias, so long as they have different descriptions." Don't add alternative capitalisations,
  because "search ignores case". Don't add misspellings either. [S]
  <https://www.wikidata.org/wiki/Help:Aliases>
- Uniqueness is enforced only on label+description per language. The check ran against the pre-edit
  state, so a combined edit could save a forbidden pair: a race between validation and the resulting
  state. [S] <https://phabricator.wikimedia.org/T102148> and related tasks
- The `mul` "default values for all languages" code lets a label shared across languages be written
  once. The display fallback chain is `user-lang → mul → en`. [S]
  <https://phabricator.wikimedia.org/T285156>
- Merge turns the obsolete item into a **redirect**. Bots (KrBot) rewrite referring statements after
  about 24 h in revertible edit groups, and the rule is "Never reuse merged items for other things."
  [S] <https://www.wikidata.org/wiki/Help:Merge>
- **Lesson:** Wikidata can tolerate alias collisions because aliases only feed *search*, where a
  person disambiguates. MAE uses aliases for *classification*, so it can't tolerate them [I]. Adopt
  three things: a `mul`-style "all languages" label, redirect-on-merge, and never reusing a retired id.
  Don't rely on write-time uniqueness checks.

### 2.3 DITA 1.3 specialization (OASIS)
- Every element carries its whole lineage in `@class`, ordered from most general to most specific,
  e.g. `- topic/li task/step `. "All specialization-aware processing can be defined in terms of
  `@class` attribute values." Authors "SHOULD NOT modify the `@class` attribute." [S]
  <https://dita-lang.org/1.3/dita/archspec/base/specialization-class-attribute>
- "Specialized content can be generalized to any ancestor type", and round-tripping is preserved. [S]
  <https://dita-lang.org/1.3/dita/archspec/base/generalization>. Tool-level fallback (styling an
  unknown specialization as its base) is vendor behaviour (PTC Arbortext), not normative. [S]
  <https://support.ptc.com/help/arbortext/r71m060/en/styler/styler/help6501.html>
- **Lesson:** behaviour comes from ancestry, and **the ancestry travels with the instance**, so a
  processor that doesn't know `task/step` still knows it's an `li`. MAE's proposal keeps the mapping
  only in the KB, which is the opposite trade-off (Verdict point 7).

### 2.4 SharePoint / Microsoft 365 content types
- The type ID is recursive and encodes the lineage. Built-in types append two hex digits (`0x01`
  Item, `0x0101` Document). Custom types append `00` plus a GUID. The ID must be unique per site
  collection. [S] <https://learn.microsoft.com/en-us/previous-versions/office/developer/sharepoint-services/aa543822(v=office.12)>
- Names can be translated: `NameResource.SetValueForUICulture("fi-FI", …)` per culture, with the ID
  unchanged (Vesa Juvonen, Mar 2014). [S] <https://learn.microsoft.com/en-us/archive/blogs/vesku/office365-multilingual-content-types-site-columns-and-other-site-elements>.
  From the same post's comments: a translation applied to a site column did not reach lists that
  already used it, and an integrator asked how to get "the english version of the content type"
  because their code had matched on the display name, which localisation broke. [S]
- Translations don't propagate unless another property changes. A list's copy of a content type keeps
  only the translations it had when it was added ("not true inheritance"). [S]
  <https://blog.icefire.ca/blogs/post/translate-sharepoint-online-content-type-gallery>
- A column's **InternalName is fixed at creation**. Renaming changes only the DisplayName. Power
  Automate formulas keyed to a display name break after a rename. [S]
  <https://www.sharepointdiary.com/2011/06/sharepoint-field-display-name-vs-internal-name.html>,
  <https://tachytelic.net/2021/06/encode-sharepoint-column-names-to-internal/>
- **Lesson for SharePoint-trained users:** they expect pick-from-list types with translated names and
  stable ids. The documented failures all come from **copies of the vocabulary** (list-level copies,
  stale translations) and from **code that matched on labels**. MAE's single replicated declarations
  document avoids the first, provided nothing caches a copy. The second is Verdict point 3 [I].

### 2.5 Schema.org / Dublin Core
- `additionalType`: "typically used for adding more specific types from external vocabularies". It
  allows several types per item. [S] <https://schema.org/additionalType>
- `dcterms:type`: "Recommended practice is to use a controlled vocabulary such as the DCMI Type
  Vocabulary." [S] <https://www.dublincore.org/specifications/dublin-core/dcmi-terms/terms/type/>
- **Lesson:** even the most permissive metadata standards treat the type as an open field but
  *recommend* a declared vocabulary. That is the shape MAE proposes.

### 2.6 Drupal (content types, taxonomy translation)
- A content type's machine name is its identity, and maintainers refused to make it freely editable:
  "If you change it, you're telling Drupal that it's now a different content type." Where it was
  editable (D7), it rewrote `{node}.type` and broke Views, code and paths keyed to the old name. [S]
  <https://www.drupal.org/project/drupal/issues/1832724>, <https://www.drupal.org/node/346053>
- i18n_taxonomy offered two modes. **Localize** shares one term across languages and translates only
  its name. **Translate** creates separate terms per language and links them into translation sets.
  In Localize mode, tag-style vocabularies could only be created and edited in the default language.
  [S] <https://www.drupal.org/node/1114016>
- **Lesson:** MAE's proposal is Localize mode (one kind, labels per language), which is the right
  choice. Translate mode creates exactly the `procedure`/`procedimiento` duplicates the proposal is
  trying to avoid. Renaming by rewriting the stored values is what broke Drupal sites.

### 2.7 Notion / Obsidian / Logseq / Tana / org-roam
- **Notion:** select options are `{id, name, color}`. Page values reference an option "by id or
  name", and the API refuses to rename an existing option. [S]
  <https://developers.notion.com/reference/update-property-schema-object>
- **Obsidian:** tags are documented as case-insensitive, but in Bases `file.tags.contains("foo")`
  missed `#Foo` (reported 19 Sep 2025, v1.9.12, fix promised for 1.9.14). One normalisation rule
  re-implemented per feature drifted. [S]
  <https://forum.obsidian.md/t/file-tags-are-case-sensitive-in-bases-but-non-case-sensitive-everywhere-else/105862>
- **Logseq (DB version):** tags are classes with an `Extends` property. Multiple parents are
  allowed, a child gets the union of its parents' properties, and everything extends a root tag. Users
  report confusion about how extension is applied. [S]
  <https://discuss.logseq.com/t/extended-tags-and-inherited-properties/33947>,
  <https://discuss.logseq.com/t/confusion-due-to-different-behavior-of-how-tags-are-extended/35010>
- **Tana:** "extend" works like OOP inheritance, and searching the base tag returns instances of the
  tags that extend it. Users call it "cumbersome" and say "if there are many inheritances … difficult
  for users to gain an overview". [S] <https://outliner.tana.inc/learn/guides/when-to-use-extend-in-supertags>,
  <https://ideas.tana.inc/posts/282-ability-to-define-a-given-supertag-as-a-subclass-of-another-supertages-in-schema-then-exploit-the-class-structure-to-make-inheritance-and-queries-better>
- **org-roam** has no type concept: nodes are file or headline, classified by tags. `ROAM_ALIASES`
  is search-only. [S] <https://www.orgroam.com/manual.html>
- **org-mode itself:** property keys are case-insensitive. `Xyz_ALL` declares the allowed values,
  space-separated, and is inherited. `#+TODO: A B | C D` declares an open vocabulary partitioned into
  behaviour classes. [S] <https://orgmode.org/manual/Property-Syntax.html>,
  <https://orgmode.org/manual/Per_002dfile-keywords.html>. Values are not case-folded by org; I
  believe TODO keywords match case-sensitively, but this is **unverified** [I].

### 2.8 Folksonomy failure evidence
- Stack Overflow (2010): free tags let users "accidentally pollute the tag pool" with "oddball
  variant[s]". Synonyms "automatically and silently remap" new uses to the master tag. [S]
  <https://stackoverflow.blog/2010/08/01/tag-folksonomy-and-tag-synonyms/>. Synonyms don't retag old
  questions, and Meta notes more than 950 synonym pairs with questions in both tags. [S] (meta pages
  cited above). **Lesson:** an alias that only rewrites *new* input leaves the corpus split for good.
  Resolving at read time, as proposed, avoids this.

### 2.9 Unicode matching (W3C charmod-norm, FPWD 16 Jul 2026)
- Case-sensitive matching is "RECOMMENDED … for all new specifications", and "choosing
  case-insensitivity is generally discouraged". If case must be ignored for non-ASCII vocabularies,
  use **Unicode canonical case fold**: normalise, apply full case folding, then optionally NFC. ASCII
  folding only in exceptional cases. Compatibility folding "should not be used". Turkic folding is
  language-sensitive and should be avoided unless the content's language is recorded. [S]
  <https://www.w3.org/TR/charmod-norm/>

---

## 3. Design answers

**(a) Is the stored value an identifier or a label? What are the folding rules?**
Store the **surface string as written**, byte for byte, and never rewrite it (proposal, confirmed by
Drupal and Stack Overflow, §2.6 and §2.8). Treat it as an *occurrence of a name*, not as the
identifier. Resolve it through a **match key** = NFC → Unicode default full case fold
(locale-independent, no Turkic rule) → NFC, plus trimming of surrounding whitespace. This is
charmod-norm's canonical case fold [S]. Going case-insensitive against charmod's default is justified
by user input [I]: org keys are already case-insensitive [S], `Procedimiento`/`procedimiento` must not
split, and today's case-sensitive `from_str_lossy("Task") → Note` is a live defect [C]. **Do not strip
accents.** It conflates real Spanish distinctions, and `revisión`/`revision` should be an explicit
alias, not an implicit match [I]. Specify this function once, in `mae-kb`, and test it
(Obsidian, §2.7).

**(b) One canonical id per kind, or label-as-id?**
**One canonical id per kind**, and it is **not a display label**: SKOS URI, Wikidata Q-id, SharePoint
InternalName, Drupal machine name and Notion option id all work this way [S]. Practically, the id can
be a readable slug (`procedimiento`, `task`) that is never shown when a label exists, and it is
immutable once any node resolves to it (Drupal, SharePoint) [S]. A *raw* string's identity, though,
is its match key, so a node written `:KIND: Procedure` resolves through the vocabulary to id
`procedimiento` [I]. Reject label-as-id: changing a label would then re-classify the corpus (the
SharePoint integrator's breakage in §2.4).

**(c) Behaviour mapping: one class, or inheritance/multiple?**
**At most one behaviour class per kind, with no kind→kind inheritance of behaviour in v1.** MAE's
behaviours are mostly mutually exclusive (task vs index vs meta). Multiple inheritance produces union
semantics that Logseq and Tana users find confusing [S]. DITA inheritance works because a specialised
type can always fall back to its parent's processing, and that doesn't apply here (a `view` can't
fall back to `task`) [I]. If narrower/broader kinds are wanted for *queries* ("all procedural
kinds"), add a SKOS-style `broader` link that **only expands filters** and never confers behaviour
[I]. Fix the class set by auditing every kind-dependent site first (Verdict point 4), and decide
whether `:role: hub` collapses into class `hub`/`category` [C].

**(d) Alias resolution and collision rules**
- Namespace: per KB, **one shared namespace across every language**, because the stored value carries
  no language tag [I]. Every id, prefLabel, altLabel and hiddenLabel, in every language, after
  match-key folding, must map to exactly one kind. Within one kind, keep SKOS S13 (tiers disjoint)
  and S14 (one prefLabel per language) [S].
- Precedence when a key hits more than one kind (this can happen after concurrent merges): id beats
  label, and label beats alias. A tie inside the same tier makes the key **ambiguous**. Ambiguous
  nodes resolve to *no kind*: they keep their raw string and show the error status. They are never
  resolved by insertion order or by CRDT client id [I]. Wikidata shows that write-time checks race
  [S], so validate on write for UX but **resolve on read for correctness**.
- Labels in other languages are display-only and never written back (proposal). Display fallback:
  user language → `mul`-style default → id → raw string (Wikidata `mul`) [S].

**(e) Nodes whose kind isn't declared**
Keep the raw string and give it **class = none, status = undeclared**. Display the raw string, not
`note`. A filter on `note` must not match it. SKOS allows labels without a prefLabel [S]. Lossy
fallback to `note` is the defect being fixed [C]. As an optional per-KB **strict** policy, refuse or
flag undeclared kinds on write. Org's `KIND_ALL` (`Xyz_ALL`) is the native precedent for an
allowed-values list, and it can drive completion [S]. Undeclared kinds also make a natural "promote to
vocabulary" worklist, which fits the KB-as-test-corpus practice [I].

**(f) Renaming and merging kinds without rewriting nodes**
- *Rename* = change labels; the id stays the same (SharePoint, Drupal) [S].
- *Merge A into B* = retire A, turn A's id and labels into aliases of B, and record a redirect.
  Retired ids are **tombstoned and never reused** (Wikidata, "Never reuse merged items") [S].
- *Split* is not expressible without rewriting nodes, so it must be an explicit, owner-run, revertible
  bulk edit with an edit-group id, as KrBot does [S]. That edit is the only path that rewrites
  `:KIND:`.
- After any change the derived classes have to update. Join against the vocabulary at query time
  rather than writing a class onto each node row. A vocabulary edit then costs O(vocabulary) and can't
  go stale the way SharePoint's list-level copies did [I].

**(g) How to express the default (built-in) vocabulary**
As **data in the same schema** as a user vocabulary, shipped as a **named, versioned, frozen** document
(`mae-builtin/1`). A KB *references* a version, and an absent reference means `mae-builtin/1` forever.
Releases add `/2` and never edit `/1` [I]. Without this, a release that adds or renames a built-in
kind changes the classification of undeclared KBs on upgraded peers only, which breaks determinism
across peers [I]. Built-in ids should sit in a reserved namespace. SharePoint keeps short built-in IDs
apart from GUID-suffixed custom ones [S], and MAE already reserves system-KB names (ADR-104) [C]. A KB
vocabulary should **extend** the built-ins by default, with an explicit opt-out, so a Spanish KB keeps
`task` working [I].

---

## 4. Failure modes and mitigations

| # | Failure mode (evidence) | Mitigation |
|---|---|---|
| F1 | Near-duplicates `Procedure`/`procedure`/`procedimiento` (SO folksonomy [S]) | Match-key folding (§3a), with aliases and labels declared on one kind; undeclared-kind worklist |
| F2 | Code matches on labels, and localisation breaks it (SharePoint comments, Power Automate [S]) | Behaviour reads class/id only; labels are never compared in code; lint for string literals of kind names outside `mae-kb` |
| F3 | Raw Datalog bypasses class derivation (built-in views `kind = "task"` [C]) | Project `kind_raw`, `kind_key` and a `kind_vocab{kb, key ⇒ kind_id, class, status}` relation into Cozo; migrate the built-in views to join on class; document that user Datalog on `kind` still sees raw strings |
| F4 | Concurrent vocabulary edits merge into a collision (Wikidata race [S]; yrs LWW per key, ADR-093 [C]) | Read-time deterministic resolution, an ambiguous status, a visible diagnostic; key vocabulary maps by kind id so edits to different kinds merge cleanly |
| F5 | Version-dependent defaults (inference from ADR-112 D4 [C]) | Frozen, versioned built-in vocabulary, referenced by the KB (§3g) |
| F6 | One normalisation rule re-implemented per feature drifts (Obsidian Bases [S]) | One `kind_match_key()` in `mae-kb`; property test that every consumer (filter, view, graph, search, completion) agrees on folding |
| F7 | Copies of the vocabulary go stale (SharePoint list copies, translations [S]) | No denormalised class on node rows; join at query time; never cache a vocabulary copy per buffer or view |
| F8 | Rename by rewriting values breaks references (Drupal [S]) | Ids immutable, labels mutable; merge = alias + tombstone |
| F9 | Class lost when a node leaves its KB (DITA carries lineage, MAE doesn't [S]/[I]) | Decide explicitly: export/copy carries the kind's declaration, or warns; federated queries resolve per source KB |
| F10 | **Reclassification as an escalation** [I]: mapping an existing kind to `view` or `meta` can turn ordinary nodes into stored queries or composite bodies | **Checked [C]:** a `view` node's body is not executed. `kb_view_query` runs the `query` column of the separate `views` relation, looked up by a validated id (`crates/ai/src/tool_impls/kb.rs` `execute_kb_view_query`); the `view` *node* is a help/search copy. So a kind mapped to `view` gains no execution. That stays true only while view execution reads `views`; if a node body ever becomes executable, class assignment becomes a principle #16 control |
| F11 | Cross-language false friends: the same string is a label in `es` for one kind and in `en` for another [I] | The single cross-language namespace (§3d) makes this a declaration error, not a silent pick |
| F12 | Unicode form split: NFC vs NFD `ó` typed on different OSes [S charmod] | NFC is part of the match key; the stored value stays as written |

---

## 5. Open questions for the owner

1. **Case-insensitive resolution:** OK, despite charmod's case-sensitive default? I recommend yes,
   with canonical case fold and no accent folding.
2. **Display:** show the raw string as written, or the resolved kind's label in the reader's
   language? (Showing the label hides what's in the file; showing the raw string hides the
   translation.)
3. **Undeclared kinds:** open by default with a strict mode available, or strict once a KB declares
   any vocabulary?
4. **Does a declared vocabulary extend `mae-builtin/N` or replace it?** If it replaces, `task`
   disappears from a Spanish KB unless it is re-declared.
5. **Class set:** what is the complete set after the audit? In particular, are `category`/hub,
   `command`, `key`, `scheme_api`, `block` and `concept` classes, and does `:role: hub` merge into the
   kind class?
6. **Query-only hierarchy:** is a `broader` link (for "all procedural kinds") wanted now, or deferred?
7. **Owners:** who is "the owner" for vocabulary edits when a KB has several owners or owner devices?
   This decides how often F4 happens.
8. **Moving nodes:** when a node moves to another KB or is exported, does its kind's declaration
   travel with it?
9. **User Datalog:** do custom views over `kind` keep raw semantics (documented), or does MAE rewrite
   stored views to use the class?
