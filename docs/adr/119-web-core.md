# ADR-119: The MAE web core: one core for the live web app and the static export

**Status:** Proposed (design), being built in phase order. Built: S0 (mae-org 0.10.0, #973), and L1a's
first slices: the core (`shared/site`: resolver, derivation, sanitiser, #974) and the live route
`GET /kb/<kb-id>/<id>` (#976), and D6's first step, a derivation cache per KB (Amendment 5). Amendment 1 (2026-11-22) records what building them decided;
Amendment 2 (2026-11-23) what the live app does not serve (folders, feeds, the sitemap, redirects),
where its home page comes from, and the retirement of ADR-073's view; Amendment 3 (2026-11-24) the
live explorer and who may list a KB; Amendment 4 (2026-11-24) where the live app's slot components and
their data come from. Spikes
C1–C6, C5b and C10–C12 are measured (results: `docs/research/119-spike-results.md`). C4b, C7, C8 and
C13 are still to spike. D14 (search) stays open until C4b reports. D15 (graph) waits on the owner's
decisions that C5b raised (its "Open" entry below). Phased (S0, L1–L3,
X1, below).

**Depends on:**
- ADR-112 (mae-org: one lossless parser, its semantics and renderers, and the `Resolver` it renders
  against);
- ADR-030 (the KB link grammar);
- ADR-110 (the store is the truth);
- ADR-105 (KB identity and node-document addressing);
- ADR-052 and ADR-111 (the daemon's HTTPS listener and how remote clients prove identity);
- ADR-098 and its Amendment 1 (the typed principal, bindings);
- ADR-053 (the `kb/query.*` read surface and `kb_access`);
- ADR-074 Amendment 1 (the per-KB change index), for L1;
- ADR-099 (WebSocket sync), ADR-100 (the browser edit surface) and ADR-036 Amendment 1 (the hub
  verifies content ops, #727), for L2;
- ADR-113 (publication review), ADR-116 (version history), ADR-117 (access administration) and
  ADR-115 (attachments), for L3.

**Amends:**
- ADR-073 D2 and D3: the live page is rendered by the daemon, not assembled in the browser from
  `kb/query.*`.
- ADR-097 D1: names this core as the browser KB surface it scopes.

**Relates to:**
- ADR-118 (agents use `/mcp`, not this app);
- ADR-104 (MAE's own corpora, the first public KBs the export can publish);
- CLAUDE.md principles #7, #12 (its browser amendment), #13, #14 and #16.

**Source of the requirements:**
- The owner *(2026-11-18)*: this is "the core for how [MAE's web surface] with auth and editing with all the
  change records etc all built out", architected to go beyond the static KB site it replaces.
- A browser client's behavioural parity specification for that site,
  `docs/verification/kb-site-parity.md`: 166 requirements, each with a mechanical test.

Decisions the owner took are marked *(owner, 2026-11-18)*, and only those.

**Prior art:** `docs/research/119-kb-site-engine-prior-art.md`.

**Tracking:** issue TBD.

## Context

MAE's web surface today has three parts, and none of them is a product:

- **A live page** at `GET /kb/{kb_id}/view` (ADR-073). It is a client-side shell that polls
  `kb/query.*`: a node list and a content pane, with no search, history or previews.
- **A WebSocket sync** (ADR-099), built for clients that hold their own CRDT state.
- **A browser client's own site:** a general-purpose static-site generator, heavily adapted. It
  renders KBs that a fail-closed gate has published.

The client measured that site on its real published bundle (1,511 org files, 5.5 MB; 8,747 IDs; 16,810
note links), in the publish job's limits (2 CPUs, 1.5 GB), median of 3, on 2026-11-18:

- **Full build: 65 s, and one changed file still costs about 60 s.**
- **39 s of the 65 come from one design mistake:** the 8,747-entry link-target map is copied into a
  WASM renderer for every page.
- **Diagrams: 16.6 s for 49.**
- **Every cold page load fetched about 5.5 MB,** 90 % of it one index rebuilt on every page.
- **A navigation took about 0.5 s to settle.**

It has no authentication and no editing, and it shows no change records.

The owner wants one MAE web surface that does all of it *(owner, 2026-11-18)*:

- read and navigate KBs at least as well as that site;
- sign in;
- edit;
- see history and review;

and still produce a static site where one is required. MAE knows the KB's structure (stable node IDs,
the typed link graph, mae-org's lossless tree), so it can derive pages incrementally instead of
rebuilding everything.

**Clean-room.** The behaviour to match is the parity specification, which describes observable
behaviour only. No one writing this core reads the source of the generator being replaced, or of its
plugins.

Disclosure: this ADR's author once read a design note that summarised that generator's build pipeline
and plugin interface. It covered architecture, no code.

The client wrote two components itself and owns them: a chord-graph library and a palette package.
Both come across relicensed GPL-3.0-or-later *(owner, 2026-11-18)*, minus the files the client
identified as derived from the replaced generator.

## Decision

### D1. One core, two front-ends *(owner, 2026-11-18)*

The **core** is a library. From a KB's nodes it derives everything a reader sees:

- pages;
- addresses;
- backlinks;
- tag and folder listings;
- search shards;
- graph data;
- feeds.

It also provides the browser runtime (router, layout, graph, search client, theming).

Two front-ends use it:

- **The live app, served by mae-daemon** on its HTTPS listener *(owner, 2026-11-18)*. It covers
  signed-in reading first (L1), then editing (L2), then change records and review (L3). It reads the
  daemon's store, which is the truth (ADR-110).
- **The static export.** A published bundle goes in and a static offline site comes out (X1). It is
  for sites that must be static, such as a public site or one behind a fail-closed publication gate.
  It reads only the bundle.

Both pass the same parity tests wherever a requirement applies. Requirements specific to the static
export (P-140, the build exit codes P-141–P-149) apply to X1. Their live equivalents are in D19.

### D2. Where the code lives

- **The core:** a library crate, `shared/site`, used by the daemon. Like the other shared crates, it
  is a member of the editor workspace.
- **The browser runtime:** TypeScript in `shared/site/web/`, built in CI into versioned assets the
  daemon serves and the export copies.
- **The static export:** a binary, `mae-site`, built in its own small workspace, so the editor
  doesn't link a WASM runtime or a search indexer.
- **Heavy optional parts** (the Graphviz runtime, the search indexer) sit behind cargo features, so a
  daemon built without the web app pays nothing for them.

### D3. Inputs and who controls them

**Content**
- **Live:** the daemon's store, through the KB access check `kb_access` (ADR-053). KB authors, and any
  agent that can write to a KB, control it.
- **Export:** the bundle, which holds only `.org` files and declared data files.

**Site configuration, templates and client behaviours** come from the operator, never from content:
- live: the daemon's configuration;
- export: a site directory beside the bundle.

Configuration covers locale, base path, hidden namespaces, palette and component slots.

Nothing a KB contains can supply configuration, a template or browser code (principle #16). An agent
that can edit a KB therefore cannot change what the app runs.

### D4. Addresses come from IDs only

- **Live:** pages are `/kb/<kb-id>/<id>`.
- **Export:** pages are `/<id>/`. Every published ID also answers `/<id>` and `/<kb-id>/<id>`
  (P-02–P-04).

A title or a folder path is never part of an address, so a retitle or a move leaves every URL valid.
Two inputs mapping to one URL fail the derivation (P-15). References in configured hidden namespaces
are removed (P-13).

### D5. One resolver per KB, built once

Every node's ID, title and anchor go into one map per KB. A heading's UUID `CUSTOM_ID` is an ID too,
mapped to its page and the anchor `cid-<uuid>`. Each page is rendered against only the entries it
references. mae-org's `Resolver` is plain data (ADR-112 D3), and rendering in-process removes the copy
that cost 39 s.

**Links between KBs.** ADR-030 has no KB-qualified link form. An `id:X` that is not in the page's own KB
therefore renders as the unresolved placeholder, in both front-ends. Nothing probes other KBs for X, so
there is no cross-tenant probe and no oracle, and every derived output stays independent of the reader
(D7).

Resolving links into another KB needs a KB-qualified link form (ADR-105 addressing: `kb_id` plus id),
an ADR-030 amendment, and an identical answer for "missing" and "forbidden". That is future work, not
L1.

**Prerequisite (S0, built in mae-org 0.10.0):** mae-org honours Org's search option
`id:X::#CUSTOM_ID` (P-10). The rule: use the anchor only when that `CUSTOM_ID` is on the page X resolves
to; otherwise link to X's page. It is never a placeholder and never a failure. A page's render reports
where each of its CUSTOM_IDs landed (`Rendered::custom_ids`; not always `cid-…`, since a headline that is
also a node is anchored at its `id-…`), and the resolver carries that map for each target
(`Resolved::custom_ids`), so the resolver (D5) is built from the pages' own renders.

The core renders with mae-org. KB ingest in the editor still uses mae-kb's older parser, because
ADR-112's switchover (O4) is not done. Until it is, the two can disagree on edge cases. The core's
tests pin mae-org's behaviour.

### D6. Incremental derivation that is correct by construction

Tools that make authors declare dependencies by hand produce stale output, and one dropped incremental
builds for that reason (prior art §1). So derivation works like this.

**Fingerprints**
- **A context hash covers every global input:**
  - the core version, its web assets and its grammar versions;
  - the configuration;
  - the templates.

  When it changes, everything is re-derived.
- **Every output's fingerprint is a hash of what deriving it reads:**
  - for a page: its node's source, the resolver entry (or the absence of one) for every ID it
    references, its backlink list, the **view** it is derived for (D8), and the context hash;
  - for a listing page: its member list.

  An output is re-derived only when its fingerprint changes.
- **Site-wide chrome is not embedded in pages.** The explorer, the folder tree and other per-KB data
  are separate, content-hashed assets that each page loads once. Anything a page does embed from
  elsewhere joins its read set; P-163's "what's new" link, for example, depends on whether its target
  is published.
- **Determinism:** pass-1 summaries are cached by source hash. Every list written is sorted, so no
  hash-map order reaches the output.

**What changes come from**
- **Live:** from the per-KB change index (ADR-074 Amendment 1, C1), which says which nodes changed
  since a cursor. The daemon re-derives the affected outputs and publishes a thin notice (D9).
  - The cursor a derivation used is taken **before** it reads, so a write that lands during the
    derivation is still delivered afterwards.
  - Publication events (approve, enter into force, derogate) are changes to the published view, so they
    get change-index rows too (D8).
- **Export:** from comparing the bundle with the last build. Each build writes a complete new output
  directory, copying unchanged outputs from a cache kept **outside** the site, then swaps it in with
  one rename (P-140). A failed build leaves the previous site untouched.

**The proof is a property test.** Random edit sequences must leave the incremental output
byte-identical to a full derivation after **every** step.
- The spike's edit set: retitle a page, edit a body, add or remove a link, add, delete or move a page,
  change tags.
- It grows to cover:
  - heading retitles and IDs;
  - `CUSTOM_ID` changes;
  - duplicate IDs, which must fail;
  - configuration and template changes;
  - folder moves;
  - every new derived output.

It proves consistency, not correctness: a bug shared by both paths cancels out. Correctness is the
parity tests' job (P-xx) and their declared mutants'.

### D7. Authorization, and what may be shared between readers

Access is decided per KB. No ADR defines a per-node access rule (ADR-053, ADR-105 and ADR-115 all go
through `kb_access`). So:

- **Every request is authorized, with nothing cached.** Each page, asset, search shard, graph
  payload and notice the live app serves passes `kb_access(kb_id, principal)` and the checks around it:
  - the typed principal, where an unbound IdP subject reaches nothing (ADR-098 C1);
  - a query-only policy (ADR-067);
  - blocklists.
- **Derived outputs are shared between readers of a KB and a view, and never mix KBs or views.** They
  are keyed by `(kb_id, view, revision or change sequence, context hash)`. Rendering never depends on
  the reader (D5), so one derivation serves every reader the KB admits to that view.
- **Query-only members (ADR-067)** may read pages one at a time, as `kb/query.get` already lets them.
  They are refused the bulk derived assets (search shards, the graph edge list, the explorer payload),
  which would hand them the whole KB that ADR-067 withholds. They search through `kb/query.search` and
  its caps (ADR-053).
- **No oracle:** a KB a principal cannot read answers exactly as a KB that does not exist (ADR-118 D4,
  ADR-117 D1).
- **Browser caching:** assets keyed by content hash are served `Cache-Control: private, immutable`,
  never `public`. A copy already in a browser's cache can outlive a later revocation. That is accepted
  and stated: revocation stops new reads, not what a reader already saw.
- **E2E KBs** (ADR-037) are refused by the live app with `e2e_unsupported`. The daemon holds no
  plaintext to render, as ADR-113, ADR-116 and ADR-118 already say.

  The core can run in WASM, so rendering an E2E KB in the browser after decryption is a later option.
  It needs its own ADR.

### D8. Which version a reader sees (ADR-113)

With publication review off for a KB, there is one version: the working copy.

With it on, ADR-113 gives two views, `working` and `published`, and does not say which a reader gets.
The rule *(owner, 2026-11-18: "published unless edit, if user has edit perms they should be able to
toggle between the two")*:

- a principal **without** Edit on the KB always gets the `published` view, and cannot request
  `working`;
- a principal **with** Edit can toggle between the two, and each is labelled. Its default is
  `working`.

The daemon enforces this, not the client.

**The `published` view is derived from published revisions only** (ADR-113 V9), not filtered from the
working one:
- its resolver, titles, backlinks, tag and folder listings, search shards and graph edges all come
  from published revisions;
- a working node that links to a published one is not its backlink;
- a link to an unpublished node is the unresolved placeholder, never the node's working title.

Notices, cursors and ETags for a view are computed from that view's own changes only, so nothing about
draft activity reaches a published-only reader. A test seeds drafts beside published revisions and
asserts that no published-view output, notice or header changes when only the drafts do.

**Until this exists (L3), the live app serves only KBs with review off.** A review-enabled KB answers
every reader with `review_view_unavailable`. Only members reach that answer (`kb_access` comes first),
and they already know the KB uses review, so it is no oracle.

The static export renders only approved, in-force revisions.

### D9. Live updates

The live app learns of changes from the thin, content-free, per-KB notices of ADR-074 Amendment 1. They
arrive over the WebSocket (`kb/changes.subscribe`) or SSE.
- Each notice is authorized when it is delivered, and scoped to the reader's view (D8).
- The browser re-fetches only what a notice names. Each re-fetch is a request authorized on its own.
- On `reset`, the browser drops its pages, explorer and search shards for that KB.
- On `access_lost`, it drops them too, and leaves the KB.

L1 needs the change index (ADR-074 Am.1 C1) built. Until it is, a reader reloads to see changes.

### D10. Editing (L2)

Editing uses ADR-100's surface: structured chrome bound to the node's CRDT schema, plus a live preview.
The preview is the core's own renderer in WASM, so a draft looks exactly as it will when published. It
travels over ADR-099's WebSocket.

**Prerequisite:** the hub verifies every content op (ADR-036 Amendment 1, #727). Until then, ADR-099 E3
limits writes to operator-minted credentials.

### D11. Change records and review (L3)

- **History:** ADR-116's history (`kb/history.*`, partly built) becomes per-page history and diff
  views.
- **Review:** ADR-113's revisions, review requests and approvals become the review views.
- **Administration:** ADR-117's caller-scoped reads drive "my pending" and access administration.
- **Attachments:** ADR-115's attachments resolve through the core, with placeholders for every
  state other than resolved.

These are views over daemon tables the core does not own. The core renders them and does not decide
them.

### D12. Code highlighting: tree-sitter, shared with the editor

Highlighting uses `tree-sitter-highlight` with the editor's grammars, plus PowerShell, INI, SQL and
nginx (all MIT or Apache-2.0).

- Tokens are role classes, never inline styles.
- A block whose highlighted text is not byte-identical to its source falls back to plain text.
- An unknown language is plain and logged (P-105–P-110).

The daemon carries every grammar the editor has, plus the four above, from one shared registry
(`mae-syntax`, its `site-grammars` feature), so a language the editor highlights also highlights on the
site and there is no second list to drift *(owner, 2026-11-24)*. Measured on the release build, that is
21 grammars and +28.7 MB (69.6 MB to 98.4 MB). C12's 14.5 MB counted six of the editor's grammars, not
all of them.

### D13. Diagrams: a self-built Graphviz WASM module, after a licence opinion

*(owner, 2026-11-18: self-built WASM, after a licence opinion.)* DOT renders to inline SVG through
Graphviz compiled to WASI, run in a WASM runtime.

- **Instances:** one compiled module per process, and a fresh instance per diagram.
- **Determinism:** the variance found came from the guest's own C library random state. A fresh
  instance plus a fixed `start=` seed make output byte-stable (P-119).
- **Limits:** a time budget by fuel or epoch (P-117), and a memory cap through the runtime's resource
  limiter. The memory cap is unmeasured.
- **No build details in the output:** the version comment is stripped (P-139).
- **Theme and SVG rules:** theme-role mapping and the fail-on-fallback rule are P-113–P-115, and the
  P-120 allow-list applies to every SVG.

Two conditions come first:

1. **A licence opinion.** Graphviz is EPL-1.0, which the FSF lists as GPL-incompatible. The prebuilt
   WASI module found is packaged EPL-2.0, but it is still Graphviz. Whether GPL code may load it at run
   time, as a separate sandboxed file, is a legal question.
2. **Our own reproducible build** of the module, with HTML-label support (expat), its sha256 and
   recipe recorded. The only prebuilt module drops HTML-like labels silently, and its provenance is
   unverified.

Until both are done, diagrams sit behind a narrow renderer interface and show as source. Nothing else
waits.

### D14. Search: an index derived per KB and loaded in pieces (open until C4b)

Each KB has its own index, re-derived from changed nodes (D6). It is chunked, fetched on demand, never
rebuilt on navigation, and supports Spanish stemming and tag search. Two candidates were measured, and
neither meets every target.

- **Pagefind (MIT), as a library.**
  - Biggest cold fetch 59 KB.
  - Cold first result 326–329 ms on a 10 Mbps / 40 ms link, against a 300 ms target.
  - Warm 4–6 ms, but 17–19 ms at 5k pages of real-density text, against 16 ms.
  - Build RSS 1.57 GB at 5k real-density pages, measured through its command-line build.
  - Incremental updates of a single page are not shown.
- **MAE's own chunked index.**
  - Shards up to 38 KB, cold 295–363 ms on the same link, build RSS under 100 MB.
  - A shard can be rebuilt alone, which suits a live KB.
  - Its costs: a format we own, and a browser stemmer that must match the Rust one.

Some readers are on slow VPN links *(owner, 2026-11-18)*. On 1.6 Mbps / 150 ms, both candidates take
1.15–1.9 s, because a first result needs two serial round trips.

C4b measures both candidates, on real-density text, for:
- library-path time and memory;
- updating one page incrementally;
- an inlined first shard or an equivalent, weighed against the 300 KB-per-download target.

The slow link then gets its own bound. The daemon's existing `kb/query.search` (a scan) stays for API
clients.

### D15. The graph: a derived edge layer plus a live overlay (open until C5b)

Drawing every chord as its own SVG element does not scale. At 10k dense nodes that is 122k DOM nodes,
about 1 s to a first stable frame, and 160–550 ms per hover.

What met every target on desktop Chromium and Firefox, without throttling:
- the chord layer derived once per KB revision into one image (11–34 KB);
- only the current, hovered and neighbouring notes drawn live, from a compact edge list (94 KB
  compressed at 10k dense nodes).

Hover touches only that note and its own links, so P-57a holds by construction.

**Open, spike C5b:**
- **Palette and accent changes** (P-55, P-68, P-93, P-95). The candidate is an edge mask tinted in CSS.
- **Zoom** (P-61a, P-64). The candidates are tiles per zoom level, or drawing the visible chords live.
- **The hierarchical views** (P-59). They take 100–460 ms in the browser, and their scene data is too
  large to precompute as is.
- **Labels, the "here" marker and the visited band:** the spike omitted them.
- **Phones:** C5b repeats the runs at 4× and 6× CPU throttling.

### D16. The browser runtime: an in-page router

Navigation never reloads the page *(owner, 2026-11-18)*. The router owns, each with a parity test:

- links inside inline SVG (P-26);
- focus and a live-region announcement (P-22a);
- scroll restoration (P-23);
- tearing down and re-running behaviours exactly once per navigation (P-25, P-156).

Spike C6: about 190 lines, 2.4 KB compressed, every listed requirement passing in Chromium and Firefox.

Its limits:
- a page's own inline scripts do not run after a swap;
- scroll state does not survive a full reload;
- `file://` serving is untested;
- its timings are loopback only.

The same runtime serves both front-ends. In the live app it also listens for D9's notices.

**How the live app authenticates a page load.** A deep link or a reload is a plain navigation, which
cannot carry a bearer header, and ADR-111 refuses tokens in the query string. So:
- `GET /kb/<kb-id>/<id>` returns the app shell, which holds no KB content and needs no authentication;
- the router then fetches the page's content, with the bearer token in the `Authorization` header;
- the token is held as ADR-111 and ADR-118 prescribe for browser clients, never in `localStorage`.

There are no cookies, so there is no CSRF.

KB-authored HTML runs in the same origin as the token and the `kb/query.*` API, so an escape from the
sanitiser could reach every KB the user can read. Three defences guard against that:
- D19's sanitiser;
- a strict CSP with no inline script;
- Trusted Types (`require-trusted-types-for 'script'`), so only the router's sanitising path can write
  HTML.

The alternative is serving content from a separate origin inside an iframe. It isolates more, at the
cost of the router's single-document model. It is recorded in Rejected, with its trade-off.

### D17. Theming and contrast checked when derived

The palette package's tokens set light and dark themes. The reader's choice is applied before first
paint by a small, render-blocking, same-origin script in `<head>`, not an inline one (D19)
(P-91–P-99). Every token pair the theme uses is checked for contrast when the theme is derived, and a
miss is an error. The client's rendered-page audit (axe plus 3:1 non-text checks over 52 scenarios)
runs in CI (spike C8).

### D18. Components through named slots

A site adds components at named slots (parity §3.18), from the operator's configuration (D3).

- **Templates:** build-time components are minijinja templates (Apache-2.0), passed a node's data.
  - A template cannot include or extend arbitrary paths.
  - It runs under strict undefined values, a recursion limit, capped ranges and a fuel limit (enabled,
    unmeasured).
  - The core supplies an ISO date parser and `fail()`, and removes `debug()`.
- **Template failures:**
  - export: a failing template fails the export (P-157);
  - live: the slot is left empty, and the failure is logged, rate-limited per template, and reported
    to the operator.
- **One sanitiser for content and components:** ammonia (MIT or Apache-2.0), keeping `class`, `role`,
  `time[datetime]`, `hidden="until-found"` and P-120's inline-SVG allow-list.
- **Client behaviours** register with the router: `mae.site.register({id, init(ctx) -> cleanup?})`.
  `ctx` holds `root`, `page`, `site` and an `AbortSignal`. The router runs each behaviour exactly once
  per navigation (P-156).
- **MAE's embedded Scheme is not offered yet.** As shipped it can read files and the environment, its
  `load` is not gated, and it has no step limit. It becomes an option once mae-scheme has a pure
  profile: an allow-list standard library, no `load`/`include`/`eval`, a step budget, and a crate
  split.

### D19. Fail-closed in both front-ends

**Both**
- No network in the derivation path.
- The final allow-list sanitiser (P-151) runs over everything from content or components. The core's
  fixed chrome (layout, router, theme script, asset references) is added after sanitising.
- Pages carry a Content-Security-Policy allowing scripts and styles from the serving origin only. There
  are no inline scripts, and the live app also requires Trusted Types (D16).

**Live**
- A node that cannot be rendered faithfully is shown as a labelled placeholder, with the reason
  logged. It is never a half-rendered page, and never an error that names unpublished data.
- Request handling never exposes a host path or a build detail (P-139).

**Export**
- Refuse symlinks, with `O_NOFOLLOW` and a regular-file check after open.
- Write only under the new output directory.
- Exit 0, 1, 2 or 3 as P-141–P-152 define.
- Withhold a node that uses `#+INCLUDE` or a local image (ADR-115's store is not part of a bundle
  yet).

Each rule gets a test that tries to break it (spike C7).

**Platforms (principle #13):** Linux and macOS in CI, including paths, symlink refusal and the WASM
runtime. Windows hosting is not a target for now.

### D20. Performance targets, not gates

Until the owner tries test instances, these are targets, not gates *(owner's standing decision)*.

**Export**, at today's size (scaling linearly to 5k pages), within 2 CPUs:
- full build ≤ 15 s (≤ 45 s at 5k), peak ≤ 512 MB;
- one-page change ≤ 2 s.

**Live** *(owner, 2026-11-18: accepted)*:
- a change visible to a reader ≤ 2 s after its commit;
- a cached page served in ≤ 50 ms at p95 on the server;
- re-deriving after one node's change ≤ 200 ms at today's size.

**Both front-ends, in the browser:**
- no single download > 300 KB compressed;
- search ≤ 300 ms cold on ≥ 10 Mbps / ≤ 40 ms, ≤ 16 ms warm, and a bound for 1.6 Mbps / 150 ms set by
  C4b;
- no long task > 200 ms on a typical page (≤ 500 ms on the largest);
- navigation settled ≤ 150 ms, and the graph adds no long task > 100 ms per navigation;
- expanded graph stable ≤ 300 ms;
- Graphviz ≤ 50 ms per diagram amortised.

**Where the evidence stands:**
- **The render core is measured:** on one core, 1,536 pages take 0.38 s and 5,040 about 1.2 s.
- **A complete derivation** (sanitising, highlighting, diagrams, search, graph, redirects) is not.
- **The live path** (re-derivation driven by the change index, under concurrent reads) is not
  (spike C13).
- **Search** misses its targets as noted in D14.

### D21. Resource bounds (ADR-109)

The daemon derives pages for every KB it hosts, so one KB must not starve the rest.
- **Per node:** mae-org's depth bound (`RENDER_MAX_DEPTH`) applies, and a cap on a node's source size
  and link count withholds an oversized node with a labelled placeholder.
- **Per block and diagram:** highlighting stops at a per-block size cap and falls back to plain text.
  Diagrams run under D13's fuel and memory limits.
- **A bounded worker pool:** derivation runs on a fixed pool, separate from request handling, with a
  per-KB queue. A burst of changes in one KB waits its turn and cannot occupy every worker.
- **A capped cache:** the derivation cache has a byte cap per KB and a global one, with least-recently
  used eviction. An evicted output is re-derived on demand.
- **ADR-109's budget:** these caps are set inside ADR-109's resource budget and its tenant model, and
  C13 measures them.

## Evidence

Every number above comes from `docs/research/119-spike-results.md`, or from the client's measurements
of its own site, as stated in Context. The spikes are throwaway measurement code.

They ran on synthetic bundles scaled from a 48-file sample, with fresh IDs. Synthetic density is lower
than live (3.2 IDs and 2.5 links per page, against 5.8 and 11), which can flatter every result. Most
spikes ran single runs on one shared Linux machine. The spikes exercised the export path, and the live
path is C13.

| Claim | Result |
|---|---|
| C1 render core | **Holds.** 1,536 pages: 0.38 s on 1 core, 0.11 s on 4. 5,040: about 1.2 s and 0.33 s. 10,032: 2.4–3.0 s and 0.72 s. RSS ≤ 25 MB. Today's equivalent stages take about 52 s. Excludes sanitising, highlighting, search, graph and redirects. |
| C2 one resolver | **Holds**, with F1 found and specified (D5). Built in ≤ 7 ms at 10k. On the sample, the same 121 resolved links as the reference renderer. Unresolved links become a placeholder with no `<a>`. |
| C3 incremental | **Holds at spike scope:** pages, backlinks and tag pages, with no folders or chrome, so "move: 0" is vacuous. 300 random edits × 2 seeds: incremental equals full, byte for byte, after every step. Two deliberately broken dependency sets diverged at steps 9 and 17. |
| C4 search | **Open (D14).** |
| C5 graph | **Side-panel ring on desktop holds.** The rest is open (D15). |
| C6 router | **Holds.** P-22, P-22a, P-23–P-27, P-28 (its marking half), P-37, P-130 and P-156 pass in Chromium and Firefox: 34/34, and the same on 2 re-runs. Click to settled: 15 ms in Chromium and 9 ms in Firefox, median on loopback. 18 of 19 deliberately broken routers caught; the survivor is equivalent. Weak spot: P-26's test needs strengthening for Firefox. |
| C10 diagrams | **Decided, conditions in D13.** WASI Graphviz: 49 diagrams in 34 ms, bounded by fuel or epoch, identical to the reference on 4/5 blocks (the 5th lost its HTML labels in the prebuilt module). Separate `dot` process: 56–113 ms. Pure-Rust layout: rejected. |
| C11 components | **Holds.** 4 components in 24 template lines; 1,536 pages × 4 rendered and sanitised in 86 ms. Embedded Scheme: unsafe as shipped (D18). |
| C12 highlighting | **Holds.** All 10 languages; 28/28 blocks byte-exact; the check caught 7/7 corrupted outputs. 430 shell blocks in 23 ms, 430 mixed-language blocks in 112 ms. |
| C9 parity | The client runs its checks against the live app (L1) and the export (X1). |

## Phases

Each phase carries its parity requirements' tests and their declared mutants (principle #14), and it
grows D6's property test.

- **S0.** mae-org: `id:X::#CUSTOM_ID` (D5), with tests and mutants. *Done: mae-org 0.10.0.*
- **L1 (first deliverable *(owner, 2026-11-18)*): the read-only live app in the daemon.** It covers
  plaintext KBs with review off (D8), and comes in three steps.
  - **L1a.** The core library: resolver, derivation, cache, sanitiser, CSP, Trusted Types, per-request
    authorization and resource bounds (D5–D7, D19, D21).
    - Prerequisite: ADR-074 Amendment 1's change index, built for D6 and D9.
    - Pages at their addresses, backlinks, tag and folder listings, breadcrumbs, table of contents and
      highlighting.
    - The browser runtime: router and page-load auth (D16), layout and panels, explorer, previews,
      folding, tables, theming, copy controls and components (D18).
  - **L1b.** Search, once C4b decides D14.
  - **L1c.** The graph, once C5b decides D15.
- **X1. The static export:** the `mae-site` binary on the same core. This retires the client's
  generator wherever a static site is required (parity §4.6).
- **L2. Editing:** ADR-100 over ADR-099, once #727 lands.
- **L3. Change records and review:** ADR-116, ADR-113, ADR-117 and ADR-115 as views (D11). Also the
  published-view derivation and its leak test (D8), which open review-enabled KBs to the live app.
- **Diagrams** go live in whichever phase is current once D13's conditions are met.

## Amendment 1 (2026-11-22): decisions made while building L1a

Building S0 and L1a's first slices decided the following. Two are the owner's (A1, A2); the rest
follow from the decisions above and the parity specification (`docs/verification/kb-site-parity.md`).

### A1. The browser runtime's build output is committed; the daemon embeds it *(owner, 2026-11-22)*

D2 said CI builds the runtime into versioned assets that the daemon serves, not how the daemon gets
them. The packages' generated output (first: the palette's `dist/palette.css` and `dist/tokens.json`)
is **committed**. The generators are byte-deterministic, and CI's `site-web` job rebuilds the output
and fails on any difference, untracked files included. `mae-site` embeds it at compile time
(`mae_site::assets`), so neither a daemon build nor the export needs node. Rejected: serving a
release-shipped directory (more deployment parts), and a `build.rs` that runs npm (every Rust build
needs node).

Assets are served at `/site-assets/<name>` under content-addressed names (`palette-<fnv64>.css`),
immutable for a year, so a change always reaches a browser under a new name. They are public: they hold
no KB content, and a `<link>` cannot carry a bearer token. Any other name under the prefix is the
site's not-found.

### A2. One component library for the reader and the browser editor *(owner, 2026-11-22)*

The reader's chrome components (explorer, search, graph, TOC, backlinks, panels, menus, dialogs,
badges, footer) and the browser editor's UI are one library, not two that drift. Each component is:

- a documented markup contract (elements, ARIA roles, names and states, `data-` attributes), which the
  engine's templates and the editor's TypeScript both emit;
- CSS over the palette's tokens only;
- a framework-free TypeScript behaviour module registered through `mae.site.register` (D18), run once
  per navigation and cleaned up before the next (P-156).

Golden fixtures in this repository (inputs and the expected normalised accessibility tree) are the
contract test both repositories run. The library's built assets reach the browser editor as versioned
files with a canonical hash, vendored there with a fetch-and-verify step, as mae-org's and
mae-crypto's WASM are. The inventory, states and accessibility criteria come from the browser client's
design-system specification, which is this amendment's input. Two rules from it are fixed here:

- **Core chrome vs slot components.** The core's chrome is added after sanitising (D19). Its ids carry
  the reserved prefix `mae-`, which no content anchor can spell (`mae_site::sanitize`). Operator slot
  components are sanitised like content, under an allow-list extended to exactly the ARIA states and
  `data-` keys their contract needs.
- **Meaningful without JavaScript.** The markup works without scripts. The export needs none; the live
  app's pages arrive through the browser client's backend (A6), and the router adds no-reload
  navigation.

### A3. Addresses of generated pages; IDs that cannot be addresses

D4 extends to the pages the engine generates. Tag pages sit below the tag index:
`/kb/<kb-id>/tags` and `/kb/<kb-id>/tags/<tag>` live, `<base>/tags/` and `<base>/tags/<tag>/` in the
export. A generated address is never shadowed (P-166):

- **Reserved IDs.** A page whose file ID is one the engine generates an address for, compared as the
  export compares directories (so `Tags` too), is withheld (`Withheld::Reserved`): `tags`,
  `index.json` (P-77), `explorer.json` (Amendment 3) and `site` (Amendment 4 H2). Later generated
  namespaces join the list.
- **IDs with no address.** An empty ID would give `//` (protocol-relative), and `.` or `..` are dot
  segments a browser collapses. Percent-encoding cannot defuse those: the URL standard reads `%2e` as
  `.`. So an ID that is empty or all dots is withheld (`Withheld::InvalidId`).
- **Export directory collisions.** Two file IDs whose encoded directory names are equal case-folded,
  or equal once trailing dots are dropped (Windows drops them), are withheld (`Withheld::FsCollision`,
  P-15). The live app, addressing pages by URL, does not need this rule and does not apply it.
- **One holder per node ID.** A node ID held twice anywhere in the KB, file-level or not, withholds
  every page holding it. A link is never decided by page order, and no page can take another's address.

### A4. Every engine-written string comes from a catalogue (P-164)

The chrome takes its words from `mae_site::strings` by the configured locale, never from a literal:
`en-US`, and `fr-FR` as the parity suite's second locale. A locale with no catalogue falls back to
`en-US`. Proper names the engine never translates (palette and product names) are declared as such
and are not in the catalogue.

*(owner, 2026-11-23)* The reader ships four catalogues: `en-US`, `fr-FR`, `es-ES` and `eo`; `es-ES`
and `eo` are complete, and `eo` awaits a speaker's review. A catalogue is complete by construction (a
missing string does not compile), so a locale either has every string or falls back whole. A
configured locale is looked up as BCP 47 tags are (RFC 4647): case-insensitively, the exact tag first,
then a catalogue of the same language (`es-MX` reads `es-ES`'s, as the reader's own language); only
another language falls back.
Translations are written fresh for the catalogue, never copied from another site generator's locale
files (the Context's clean-room rule). Locale data (dates, numbers, plural rules, language names) will come from
Unicode CLDR, through ICU4X in Rust and `Intl` in the browser, when the reader first renders any;
nothing in it does today, so no dependency is added yet. A locale with no catalogue renders the
chrome in `en-US`, and `mae_site::strings::resolve` reports that it fell back: every chrome element
written from the fallback carries `lang="en-US"`, so a screen reader voices it as English, while
`<html lang>` stays the configured locale and content inside such an element carries that locale
back. The one exception is a listing's `<title>`, plain text that cannot mark part of itself.

### A5. The live route's answers

`GET /kb/<kb-id>/<id>` (and the tag views) answer with the same not-found for every refusal and
absence: no KB, no read access, an end-to-end-encrypted KB (L1 serves plaintext KBs only), no such
node or tag, an invalid identifier, a withheld page. The read gate is `kb/query.*`'s own
(`kb_query::load_gated`). Until D6's cache, every request derives the KB, bounded by the node count and
the per-node byte size (`kb_query_max_scan_nodes`, `kb_query_max_body_bytes`), charged to the tenant
quota as a scan, and rendered off the async workers. The route is behind `oauth.site_enabled`,
default off; `oauth.site_locale` sets the locale.

**Known collision (architecture debt).** With `webview_enabled`, ADR-073's view answers
`/kb/<kb-id>/view` before authentication, so a node whose ID is `view` cannot be reached at its own
address. The live app succeeds that view. The resolution (retire its path, or reserve `view` under A3)
is due with L1a's router.

### A6. The reader authenticates through the browser client's backend *(owner, 2026-11-22)*

This supersedes D16's "How the live app authenticates a page load". The browser client uses a
backend-for-frontend (RFC 10017) for the reader and the editor alike. Its server is a confidential
client that holds the tokens behind a cookie session and calls the daemon; the browser never holds a
token. So:

- the daemon's `GET /kb/<kb-id>/<id>` and its tag views stay bearer-only, as built (A5), and are called
  server-side by that backend, one daemon session per person;
- the daemon builds no token-free shell, and D16's "there are no cookies, so there is no CSRF" no
  longer describes the browser: the backend's cookie session carries its own Origin and CSRF checks;
- the in-page router keeps everything else D16 gives it (no-reload navigation, focus, scroll,
  behaviours once per navigation), and fetches pages through the backend;
- D19's sanitiser, the strict CSP and Trusted Types still guard the page the browser renders.

The daemon's own refusal answers (A5) are unchanged, and the backend passes the daemon's CSP and
security headers through with each page.

## Amendment 2 (2026-11-23): what the live app does not serve, and its home page

The parity specification was written for a static-site engine. Three of its groups assume things a
live, signed-in app over a store-is-truth KB (ADR-110) does not have. The owner decided each
*(owner, 2026-11-23)*.

### B1. Folders are an export feature

A native KB has no folders. After the cutover its nodes have no source file (`source_file` is `None`),
so there is no path to list or to show as breadcrumbs. Folders exist only where a bundle on disk does,
in the static export (X1). So:

- **Export only:** folder listing pages (P-72) and the folder half of P-74, breadcrumbs (P-29), and a
  configured descending order per folder (P-46).
- **The live explorer and graph** cannot group by folder (P-39–P-44, P-54, P-59, P-67 as they concern
  folders). Their live grouping is decided with the explorer slice of L1a and with L1c. The parity rows
  apply to the live app in every respect except folders.
- **Rejected:** a folder as a new node property (a schema change under ADR-093 to rebuild something
  the cutover removed on purpose), and hierarchical tags read as folders (it overloads tags, and P-70's
  tag rules would then decide navigation).

D1's "tag and folder listings" and L1a's "tag and folder listings, breadcrumbs" read, for the live
app, as tag listings only.

### B2. Feeds, the sitemap and redirects are export features

Every live route needs a bearer token or the browser client's session (A5, A6). Feed readers and
crawlers have neither, so a live feed or sitemap would serve nobody. The RSS feed (P-75), the sitemap
(P-76), the static redirects and the two address forms they serve (P-03, P-04), the route-history
redirects (P-05, P-06) and the safe-file-name rule for them (P-08) are produced by the export only. The
live app has one address per page, `/kb/<kb-id>/<id>` (P-01, P-02 for page IDs; P-07's 404 for an
unpublished ID), and A3's ID rule is its counterpart to P-08. P-78 still applies to the live app's
content index (`/kb/<kb-id>/index.json`). The live app's "what's new" (P-163) is a page, not a feed.

### B3. The home page is operator configuration

`/kb/<kb-id>/` serves the KB's home page: a node the **operator** names, per KB, in the daemon's site
configuration, as the export's site directory names its what's-new node. Under D3, configuration comes
from the operator, never from content, so no tag, property or reserved ID in a KB can make a node the
home page. With no home node declared, or one that is unpublished or unreadable to the reader,
`/kb/<kb-id>/` serves the tag index (A3). That makes the home address answer the same for a reader who
cannot see the declared node as for one on a KB that declares none, so it says nothing about a node
the reader cannot read (A5).

### B4. ADR-073's live view is retired *(owner, 2026-11-23)*

The live app supersedes ADR-073's view. Its `GET /kb/<kb-id>/view`, gated by `webview_enabled`, answers
before authentication and before the live app, so a node whose ID is `view` could not be reached at its
own address (A5's debt). The route is removed, and with it the reason to reserve `view`. `webview_enabled`
is deprecated: accepted and ignored, with a startup warning, for one minor release, then removed.
Rejected: reserving `view` under A3 (two renderers of one KB, and an ID withheld for an old path), and
moving the view to another path (a breaking change that keeps the second renderer).

## Amendment 3 (2026-11-24): the live explorer, and who may list a KB

B1 left the live explorer's grouping to its slice of L1a. Building it decided the following; G1 and G2
are the owner's.

### G1. The explorer groups pages by tag *(owner, 2026-11-24)*

- One group per tag, in the tag index's order, each listing its pages A to Z (G3). A page with several
  tags is in each of their groups; a page with none is in an "untagged" group, once. Every published page
  is listed at least once.
- Its data is a reserved address, `/kb/<kb-id>/explorer.json` (A3, P-166), derived by the core
  (`mae_site::explorer`). Each page names it in an empty, `hidden` box between the header and `main`,
  with the version it was derived with (the first 16 hex digits of the payload's SHA-256). The router
  keeps the box across navigations and fetches again only when a page names another version, so the
  list loads once per change, as D6 asks.
- It is served `private, no-store` like every other live answer, not D7's `private, immutable`: the
  version in the page already saves the refetch, and nothing outlives a revocation in the browser's
  cache.
- Rejected: a flat A-to-Z list (one list of hundreds on a large KB); grouping by node kind (ADR-114;
  in most KBs nearly every page is a `note`, so it would be one group); an operator setting per KB (a
  mode per KB to build and test, for a choice nobody has asked to vary).

### G2. A query-only member is refused the KB's content in bulk, not its titles *(owner, 2026-11-24)*

- ADR-067 withholds a durable bulk copy of a KB's **content** (its precedents are Confluence's "Export
  Space" and Drive's "disable download"), not its titles. The content index (`index.json`) holds every
  page's text, so a query-only member is refused it, with the one not-found (A5). Before this it was
  served to them; that is fixed here.
- The explorer, the tag index and tag pages hold titles only, and a query-only member gets them as any
  reader does. `kb/query.titles`, `kb/query.list` and `kb/query.graph` already give every reader all
  titles, deliberately (they give query-only members backlinks and a graph view), so refusing them on
  the site would withhold nothing.
- **This reverses the owner's first choice the same morning** (no explorer for a query-only member,
  following D7's list, which named the explorer payload as a bulk asset). An independent review found
  the RPC path above; with that evidence the owner chose to refuse content only. D7's list is read
  accordingly: search shards hold content and stay refused; the explorer and the graph's edge list hold
  titles and links, which the query surface already serves.
- The gate is `kb_query::may_copy_whole_kb`: a member whose replication policy is `Full`, read from
  `member_role`, the lookup `kb_access` decides on, on every request. A restriction applied while a
  member reads takes the content index away at their next request; lifting it gives it back.
- **Residual, accepted:** a query-only member can still read every page they can reach, one request
  at a time, by following links and the explorer. That is ADR-067's own stated limit: it stops the
  default path to a bulk copy, not a client that reads everything it is allowed to read live.

### G3. A to Z means a reader's order

Every list of titles or tags (tag index, tag pages, explorer) sorts by the text decomposed (NFD), with
its combining marks dropped, in lower case, then by the text itself (`mae_site::order`). Bytes had put
every capital before every lower-case letter and every accented initial after "z". This is close to
CLDR's root collation at its first level for Latin script, not a language's tailoring: es-ES orders "ñ"
as a letter after "n", and this orders it with "n". A tailored collation is left for when a reader
asks for it.

The two new catalogue strings (the explorer's name and the untagged group) join A4's catalogues in all
four locales; `eo`'s await the same speaker's review as the rest of its strings.

## Amendment 4 (2026-11-24): slot components in the live app

D18 decided how a component is written (a minijinja template, sanitised like content, with an optional
behaviour registered through `mae.site.register`). It did not say where the live app finds the
components or what a "version" is without a build. Both are the owner's (H1, H2); the rest follows.

### H1. Components come from an operator directory *(owner, 2026-11-24)*

- **Configuration, never content (D3).** `daemon.toml` names a directory per KB,
  `[oauth.site_components] "<kb-id>" = "<dir>"`, beside `site_home` and `site_title`. Nothing in a KB
  can add, change or select a component.
- **A manifest, not a naming convention.** The directory holds `components.toml`, which lists each
  component:
  - its slot (P-153's seven: document head, header row, before the body, after the body, left-panel
    footer, right panel, page footer);
  - its template file;
  - its optional behaviour script;
  - the exact `data-` keys and ARIA states its markup needs, which is all A2's sanitiser extension
    admits for it. Its classes must start with `site-`, so it can wear neither the chrome's (`mae-`) nor
    content's (`org-`, `language-`);
  - an optional stylesheet, served like a behaviour;
  - for a page-emitting component, its page name and its data file (H2).
- **Loaded once, at startup.** Every template is compiled, every script and data file is read and
  validated, and every declared attribute is checked against the sanitiser's rules before the daemon
  serves. Any failure stops the daemon with the component and the reason, before any store opens, as an
  invalid `[[tenant]]` entry does (an invalid TLS path only disables the HTTPS listener, which would
  leave a site serving pages without the components the operator declared).
  It is the live counterpart of P-157's "the build fails". A change takes a restart; there is no hot
  reload. One check cannot refuse: whether a configured KB ID names a KB the daemon has. KBs are
  created at run time, so an operator may configure one first; a well-formed ID that names no KB is
  warned about at startup instead (`site_components::unmatched_kb_ids`).
- **A failure while rendering** leaves that slot empty for that page, and is logged, rate-limited per
  template (D18).
- **What a template sees (P-154):**
  - the page's own data: ID, title, file-level properties, dates, tags and URL;
  - the site settings: locale, base path and KB ID;
  - its own validated data file (H2);
  - for any other node ID, only whether it is published in this view, and its address. That is what
    P-163's "what's-new link, omitted if unpublished" needs. It tells the reader nothing they could not
    learn by following the link.

  Never another page's content. A component cannot read the KB beyond the page it renders, so a
  template cannot widen what a reader sees.
- **Behaviours** are served like the runtime's own assets, under content-addressed names, as
  `script-src 'self'` requires. Each runs once per navigation and is cleaned up before the next (P-156),
  under the router's single Trusted Types policy: a behaviour builds DOM, and cannot inject HTML.
- **Empty slots add nothing** (P-158). A slot with no component writes no element, so removing a
  component moves nothing else.
- **The five examples** (state badge P-161, copy link P-162, what's-new link, version footer and
  last-updated line P-163) ship as a fixture component directory under `docs/verification/fixtures/`,
  with the tests that render each one. They are not engine code: the contract is that they need no
  engine change.
- **Rejected:** shipping the examples as built-in engine features (the contract would not be met until
  later, and each example would be a second implementation to retire); serving no components until the
  export exists (the live app is L1's first deliverable).

### H2. A version is an entry in an operator data file *(owner, 2026-11-24)*

- **The data.** The site-changes page and the version footer (P-160, P-163) read a data file the
  manifest names. It is validated at startup against the component's declared shape, as the export's
  build-time data is. A malformed entry stops the daemon (H1). A malformed date in a page's own
  properties only omits that line (P-150, P-163).
- **The address.** A component-emitted page lives under one reserved segment, `/kb/<kb-id>/site/<name>`
  (`<base>/site/<name>/` in the export). `site` joins A3's reserved IDs, so no operator-declared name
  reserves an ordinary word per KB, and no node can shadow a generated page.
- **Rejected:** deriving versions from the KB's history (it needs L3's change records, ADR-116 and
  ADR-113, which are not built), and keeping the page export-only (the version footer would then link
  nowhere live).

### H3. One registration API for every behaviour *(owner, 2026-11-24)*

The router implemented `register(name, {mount(doc)})` while D18 specified
`mae.site.register({id, init(ctx) -> cleanup?})`. That drift is resolved toward D18, with one API, not
two: the router's own behaviours (theme controls, explorer, source-block copy) and every operator
component register the same way.

- `ctx` holds `root` (the document), `page` (`id`, `url`, `kb`), `site` (`kb`, `base`, `locale`) and
  an `AbortSignal`. The signal is aborted when the page is left, before the behaviour's own cleanup
  runs.
- The shell writes `page` and `site` as attributes on `<main>` only for a KB with components, so a
  site without them stays byte-identical (P-158). Without them `page` and `site` are `null`, which
  only the router's own behaviours can meet; they do not read them.
- A component's script is a module loaded after the router's, so `window.mae.site.register` exists
  when it runs (module scripts run in document order). One registered after the router started runs
  on the page on screen at once.
- Each behaviour runs once per navigation and is cleaned up before the next (P-156). One that throws
  is reported and skipped; it never stops another from running. A second registration under the same
  `id` replaces the first.

## Amendment 5 (2026-11-26): D6's first step, and a path the daemon never routes

### E1. The live app derives a KB once per change

A5's "until D6's cache, every request derives the KB" ends here. It cost about 2 s a page at 1,560
nodes on one CPU (#1036). The live app now holds one derived site per KB (`daemon/src/site_cache.rs`)
and answers later requests from it.

- **The key.** A held site is keyed on everything deriving it reads:
  - the KB's content generation, which the store moves after every committed write to the KB's
    collection or node documents, and after a deletion of one;
  - the attachment store's change count;
  - the options (hidden namespaces, highlighting) and D21's bounds.

  The request takes the generation before it reads anything, so a racing write can make a held site
  newer than its key, never older. A held site serves any request whose key it is at least as new as.
- **Authorization is not cached.** The read gate and the whole-KB copy check run on every request,
  before the cache is asked. Only plaintext, review-off KBs reach it (D8), so the derived site is the
  same for every reader the gate admits.
- **Concurrency.** One KB is derived once at a time: concurrent misses wait for the first.
- **Failure.** A failed derivation is not held.
- **Bound.** At most `oauth.site_cache_kbs` KBs are held (default 8), least recently used first out;
  0 derives per request.

This is the coarse half of D6. Per-node invalidation, with the property test that incremental output
equals a full derivation, is still to build. C13 still measures the cache's memory per KB.

### E1a. D8 enforced in the live app (2026-11-27)

The live route's gate did not read the review policy: a KB with publication review on would have been
served from its working copy, drafts included. D8 keeps such a KB off the live app until the
published view (L3). The gate now answers it with the same not-found as every refusal, on every
request and before the cache, so switching review on hides a cached KB at once. A policy that cannot
be read hides it too.

No KB on the test deployment had review on when this was found, so nothing was exposed. The
published view on the site (in-force revisions only, with its leak test) is the next step for a
review-enabled KB to be readable there.

### E2. `/edit/` belongs to the browser client

The browser client's editor (ADR-100) mirrors the reader's addresses under `/edit/`. Its node pages
are at `/edit/kb/<kb-id>/<id>`, and its own views are below `/edit/kb/<kb-id>/-/`, so they never
meet a node ID. Node IDs are encoded as D4's segments are.

MAE's daemon, the live app, the static export and any later feature route **nothing** under the
top-level `/edit/`. A future MAE route that wants a top-level path checks this list first. The list
is: `/edit/` (the browser client).

## Amendments this needs

- **ADR-073 D2 and D3:** the live page is rendered by the daemon from the core. Amendment 2 B4 retires
  ADR-073's view and deprecates `webview_enabled`.
- **ADR-074 Amendment 1:** its change index becomes a prerequisite for L1, not an optimisation.
- **ADR-097 D1:** names this core.
- **ADR-113:** D8's rule for which view a reader gets. Also that the
  published view is derived from published revisions only (V9), and that publication events write
  change-index rows.
- **ADR-074 Amendment 1, C2 and C4:** notices, cursors and ETags are scoped per view.
- **ADR-067:** query-only members are refused the bulk derived assets that hold content (D7, read as
  Amendment 3 G2 reads it), not those that hold titles.
- **ADR-053:** its search caps stay the only search surface for query-only members. Full members get
  search shards, which hold nothing they could not already read.
- **ADR-111 and ADR-052:** how the live app authenticates page loads (D16): an unauthenticated shell,
  then bearer-authenticated fetches.
- **ADR-030 (later):** a KB-qualified link form, before links between KBs can resolve (D5).

## Consequences

- **One web surface instead of three.** The static site, the polling page and a future editor become
  one core with two front-ends.
- **Derivation becomes incremental:** an edit re-derives a few pages, not all of them.
- **New dependencies:** a WASM runtime (about 21 MB and about 1 min of build), a search indexer or our
  own, four tree-sitter grammars, minijinja, ammonia, and a TypeScript runtime with its own toolchain
  and tests. The toolchain's licences are checked when L1 starts.
- **The daemon becomes a web application server.** Its HTTPS listener, its resource budget (ADR-109)
  and its tests grow accordingly.
- **Every derived output must join D6's scheme.** The property test is the guard, and it must grow
  with each one.

## Rejected

- **Three surfaces kept separate:** a static generator, a polling page and an editor would triple
  the work and drift apart.
- **Rendering in the browser from `kb/query.*` (ADR-073's model) for the whole app:** it repeats the
  per-page work the client measured as the bottleneck. Kept only for E2E KBs, later (D7).
- **Site code or configuration inside KB content:** KB authors and agents would control what the app
  runs (D3).
- **Rendering per reader:** it would multiply cost by readers. Access is per KB, so it isn't needed
  (D7).
- **syntect:** covers 5 of 10 languages, and its grammars diverge from the editor's.
- **A pure-Rust Graphviz layout:** drops URL links, fails real diagrams, takes 13.7 s at 500 nodes.
- **Linking Graphviz into the process:** a crash in it would take the daemon down.
- **Whole-index client search libraries:** they pass 300 KB at about 500 real pages and re-parse on
  every navigation.
- **Plain page loads with View Transitions:** the owner chose the router (D16).
- **Hand-declared dependencies per feature:** the documented cause of stale incremental output (D6).
- **Inline scripts:** they would need a weaker CSP (D19).
- **Serving KB content from a separate origin inside an iframe (for now):** stronger isolation, but it
  breaks the router's single-document model (D16). It is reconsidered if an escape from the sanitiser
  or Trusted Types is ever found.
- **Probing other KBs to resolve an `id:` link:** a probe across tenants, and an oracle (D5).

## Open

- **The licence opinion** and the self-built Graphviz module (D13).
- **Spikes:**
  - C4b: search, incremental and on slow links;
  - C5b is measured (2026-11-27). Its three owner decisions:
    - **The bulk chord layer at one pixel per CSS pixel** *(owner, 2026-11-27, after a side-by-side
      at 1,560 and 10,000 notes)*: yes. It is 10–30× cheaper (40 ms at 1,560 notes, about 125 ms at
      10,000, against 0.4–4.5 s), keeps zoom usable, and is visibly softer only as background
      texture: the current page's and the hovered note's links, the labels, the markers and the
      visited band stay at device resolution on the overlay. A sharper layer swapped in later by a
      worker, for small KBs, stays possible as an addition.

      With this, D15 is decided: the chord layer baked at 1× in a worker and tinted for palette and
      accent, the overlay at device resolution, notes as elements, and the hierarchical views
      redesigned before the graph ships.
    - **The hierarchical views** *(owner, 2026-11-27)*: redesigned **before** the graph ships, so
      L1c's first release has the chord, sunburst and balloon views. As built, they take 0.3–2 s and
      0.35–1.9 GB.

      **What the redesign changes**, measured with `buildScene` on the same synthetic graphs:
      - The layout itself is cheap. With the links removed, a scene takes 8–12 ms at 1,560 notes and
        46–53 ms at 10,000, so positions stay in the browser.
      - The cost is one scene object per link: an SVG path string and 24 culling boxes each. At
        1,560 notes that is 22 MB of link data, against 1.1 MB for every note and folder, and
        160–190 ms of the build. At 10,000 notes it is 1.3–1.4 s of the build.
      - The redesign keeps the layout and makes no per-link objects. The worker draws the bundled
        links straight into the 1× tinted layer, as the chord view does. Only the links of the
        current, hovered and found notes get paths, for the overlay. With the links on a canvas,
        nothing needs culling boxes.
      - Still to measure, when it is built: the worker's bake of the bundled paths, which have more
        segments than a chord.
    - **The graph's parity rows** *(owner, 2026-11-27)*: P-55, P-57, P-57a and P-64 assert on the
      graph model the mounted graph exposes, plus sampled pixels, instead of on chord elements. The
      model's shape is in the parity spec, §3.7.

    Real-device GPU runs are still owed.
  - C7: fail-closed rules;
  - C8: contrast;
  - C13: the live path, measuring change index → re-derivation → notice under concurrent readers, the
    derivation cache's memory per KB, and D21's caps under a hostile KB.
- **minijinja's fuel limit:** measure it.
- **mae-scheme's pure profile:** a separate issue.
