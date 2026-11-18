# ADR-119 prior art: publishing a KB as an offline static site

Method: ~23 web searches/fetches. Most results were search-engine summaries of primary pages, not full reads of them. Claims carry the link they came from. Anything I could not confirm is marked UNVERIFIED. No site-generator source or docs from the excluded project were consulted.

## 1. Incremental builds and dependent pages

**Findings**
- **Zola dropped incremental rebuilds for correctness.** A maintainer explained that the "smart rebuild" produced weird bugs, so the whole site is rebuilt. Users had reported not seeing updated data in `zola serve` ([thread](https://zola.discourse.group/t/z12-performance-of-serve-content-changes/600), [thread](https://zola.discourse.group/t/incremental-rebuilds/2144)). `zola serve --fast` rebuilds only the changed page. The exact version of the removal is UNVERIFIED.
- **Jekyll `--incremental` is documented as experimental.** It uses mtimes plus inter-document dependencies stored in `.jekyll-metadata` ([docs](https://jekyllrb.com/docs/configuration/incremental-regeneration)). The 3.0 announcement says not all dependencies are tracked ([post](https://idratherbewriting.com/2015/11/04/jekyll-30-released-incremental-regeneration-rocks/)). Users report stale output, with `--no-incremental` fixing it ([thread](https://talk.jekyllrb.com/t/includes-and-collections-not-updating-properly/10200), [thread](https://talk.jekyllrb.com/t/jekyll-website-builds-successfully-but-updated-content-is-missing-from-the-generated-production-page/10489)). These are anecdotal, with no confirmed bug IDs.
- **Eleventy `--incremental` starts with a full build.** A config change forces a full build, and so does a change in the includes/layouts dirs. Collection dependencies are declared by hand through `eleventyImport.collections`, and docs list better collection tracking as future work ([docs](https://v2.11ty.dev/docs/usage/incremental/)).
- **Hugo rewrote partial rebuilds around a dependency tracker.** The old rule-based approach had "surprises (e.g. stale content)". The new one tracks nearly all of Hugo's API, including a page's dependence on other pages via `site.RegularPages` ([commit mirror](https://git.feldspaten.org/phoenix/hugo/blame/branch/bep-patch-1/hugolib/rebuild_test.go)). This is in `hugo server`. Some cases (CSS tooling driven by `hugo_stats.json`) still need a full rebuild. This is a mirror of the commit, not Hugo's own docs.
- **Gatsby's page-level incrementality was coarse in practice.** Any change to code or JS/CSS gives a new webpack compilation hash and rebuilds all pages. A changed page referenced by any static query rebuilds all pages ([large-sites post](https://v2.gatsbyjs.com/blog/2019-06-12-performance-improvements-for-large-sites), [engineer's post](https://dev.to/ascorbic/top-6-reasons-gatsby-incremental-builds-can-be-slow-and-how-to-make-them-fast-b75)). Per-page `page-data.json` was deliberately left unhashed, so one data change touches one file.
- **Astro (experimental, per the search summary) caches per page by an author-supplied `cacheKey`.** Pages without a key re-render every build. Config or dependency changes invalidate everything. Middleware changes do NOT invalidate cached pages. A maintainer flagged a stale-after-component-change risk ([docs](https://docs.astro.build/en/reference/experimental-flags/incremental-build/), [review](https://github.com/withastro/roadmap/pull/1404)). The "Astro 7.2" version label comes from the search result and is UNVERIFIED.
- **mdBook and Docusaurus: no evidence found.** Behaviour is UNVERIFIED. I did not research them.
- **Function-level dependency tracking is the designed answer elsewhere.**
  - Salsa (rust-analyzer) uses red-green validation, early cutoff, "backdating" (an unchanged output keeps its old revision) and durability levels. Durability is the min of a query's inputs, and a rust-analyzer session skips about 300 ms of stdlib validation ([blog](https://rust-analyzer.github.io/blog/2023/07/24/durable-incrementality.html), [overview](https://docs.rs/crate/salsa/0.26.1/source/book/src/overview.md)).
  - Turbopack records which value cells each function read and invalidates those tasks. It argues hand-declared dependency graphs are error-prone ([blog](https://nextjs.org/blog/turbopack-incremental-computation)).

**Pattern.** The tools with hand-declared or partial dependency models (Jekyll, Eleventy, Astro middleware) have documented staleness. Zola chose to remove incrementality. Hugo and Turbopack track dependencies automatically at read time.

Implication for our design: Do not hand-list dependencies per feature (backlinks, titles shown elsewhere, tag pages, graph neighbourhoods). Model each output as a pure function of explicitly read inputs, and record reads automatically, as in Salsa. Early cutoff means a body-only edit does not re-render neighbours when their derived inputs (title, summary, link set) are unchanged. Also ship a full-rebuild mode and a CI test asserting incremental output is byte-identical to a full build; the other tools' stale-output bugs are the failure to guard against.

## 2. Client-side full-text search at 1k-10k pages

**Findings**
- **Pagefind** is built at publish time as a post-build step over generated HTML. The site claims a 10,000-page site can be searched with a total network payload under 300 kB, usually nearer 100 kB ([site](https://pagefind.app)). The index is chunked and fetched on demand. A v1.5.2 dependency note mentions chunks about 45% smaller after delta-encoding ([note](https://codeberg.org/Codeberg/Documentation/pulls/840)). I found no per-page index size figure. Spanish (`es`) is stemmed. Language comes from `<html lang>`, one index per language, and a search returns only same-language pages ([multilingual docs](https://pagefind.app/docs/multilingual/)). Its licence is MIT (checked 2026-11-18 in the published crate's metadata, `pagefind` 1.6.0).
- **Stork:** one user reported a 420 KB wasm/js/css bundle and an 8.2 MB index. The maintainer said relevance was prioritised over size ([discussion](https://www.github.com/jameslittle230/stork/discussions/226)). A tinysearch issue states Stork is about 700 KB against about 100 KB for tinysearch on one site ([issue](https://www.github.com/tinysearch/tinysearch/issues/136)). tinysearch's design (xor filters) gives no result snippets or ranking beyond a presence check ([author's write-up](https://endler.dev/2019/tinysearch/)). Stork's maintenance status is UNVERIFIED.
- **Lunr** supports Spanish via the `lunr-languages` plugin, which also has an opt-in accent-fold function ([Lunr guide](https://lunrjs.com/guides/language_support.html), [package](https://npmjs.com/package/lunr-languages)). Lunr, MiniSearch and FlexSearch load the whole index up front, so size grows linearly with content ([comparison](https://healeycodes.com/webassembly-search-tools-for-static-websites)). MiniSearch's docs were not consulted, so its Spanish support is UNVERIFIED. Fuse and tantivy-in-WASM: no evidence gathered.
- **No independent 10k-document comparison exists** in what I found. Index-size and latency comparisons are vendor or blog claims.

Implication for our design: Evaluate Pagefind-style chunked, lazily loaded indexes first, since it is the one option with documented 10k-page behaviour and Spanish stemming, and measure it on our own corpus (raw, gzipped, and bytes transferred per query). If the engine must not depend on a post-processor over HTML, the same chunked-index shape built in Rust at publish time is the model to copy. Any whole-index-up-front library is a poor fit past low thousands of pages.

## 3. Link-graph visualisation at 1k-10k nodes

**Findings**
- A Eurographics-published study built sigma.js/graphology and SVG versions of one tool. Both held over 100 FPS up to 5,000 nodes. SVG fell toward 0 FPS as size grew while WebGL stayed near 100 FPS ([paper](https://diglib.eg.org/bitstreams/e5783eb9-1259-4a5a-9370-2bbf1398455c/download)). A 2018 poster found SVG and Canvas about equal and WebGL faster, with losses starting above 400 nodes when each node was heavy ([poster](https://mt.inf.tu-dresden.de/cnt/uploads/Horak-2018-Graph-Performance-Poster.pdf)).
- Sigma's own performance notes say halving the vertex count shaved only about 6% off frame time, while opacity and antialiasing mattered far more ([docs](https://v4.sigmajs.org/how-to/technical/performance/)).
- Layout is a separate cost from rendering. d3-force's many-body force is O(n log n) per tick ([thread](https://groups.google.com/g/d3-js/c/nwf_Jafk_E8)).
- Obsidian's global graph degrades first: reports of lag around 12k notes and freezes beyond about 25k, while the rest of the app copes ([forum](https://forum.obsidian.md/t/obsidian-graph-view-doesnt-work-for-a-large-vault/106287), [forum](https://forum.obsidian.md/t/maximum-number-of-notes-in-vault/1509/25)). These are forum anecdotes. A third-party plugin claims background layout keeps interaction smooth (vendor claim, [plugin](https://community.obsidian.md/plugins/speed-graph)).
- **Chord diagrams at this scale: no data found.** UNVERIFIED. A chord diagram is O(n^2) in potential ribbons, so grouping by cluster before drawing is likely required (my inference).

Implication for our design: Compute layout at build time and ship positions, so the browser does no simulation. Use canvas or WebGL, not SVG, for the global graph, and keep a bounded local-neighbourhood view (SVG is fine there). Treat chord diagrams as aggregated, cluster-level only until measured.

## 4. SPA-style navigation without a framework

**Findings**
- **Cross-document View Transitions** are declarative (`@view-transition { navigation: auto; }` on both pages). Chrome/Edge 126+ and Safari 18.2 support them, Firefox does not, and unsupported browsers just navigate normally ([Chrome docs](https://developer.chrome.com/docs/web-platform/view-transitions/cross-document), [MDN](https://developer.mozilla.org/docs/Web/API/View_Transition_API)). Real page loads keep native history, scroll restoration and screen-reader page-load announcements. Version numbers differ slightly between sources.
- **Turbo Drive** saves scroll per page and restores it on restoration visits. It keeps a page cache snapshot, and widgets that mutate the DOM (charts) come back empty unless torn down in `turbo:before-cache` ([forum](https://discuss.hotwired.dev/t/disable-turbo-scroll-memory/3312), [post](https://glaucocustodio.github.io/2023/04/03/turbo-drive-plus-echarts-fixing-history-back/)).
- **SVG links:** on SVG `<a>`, `href` and `target` are `SVGAnimatedString` objects, not strings, so handlers that read `a.href` or `a.target` as strings break. Read `href.baseVal` or `getAttribute` ([MDN](https://developer.mozilla.org/en-US/docs/Web/API/SVGAElement/href), [router fix](https://gh.ali360.cloudns.ch/TanStack/router/pull/5590)). swup's default selector reportedly misses SVG `xlink:href` links, with a `svg a[*|href]` workaround from a Q&A mirror. UNVERIFIED against swup's docs. I did not verify htmx boost, barba or the Navigation API.
- **Accessibility:** client-side routing does not move focus or announce the new page. Practitioner guidance is to move focus to the new main heading (`tabindex="-1"`), update the `<title>`, and announce it through a live region ([BBC GEL](https://bbc.github.io/gel/foundations/routing), [Nuxt](https://nuxt.com/docs/4.x/guide/best-practices/accessibility), [user-testing write-up](https://v2.gatsbyjs.com/blog/2019-07-11-user-testing-accessible-client-routing)). These are practitioner sources, not controlled studies.

Implication for our design: Prefer plain multi-page navigation, with cross-document View Transitions as a progressive enhancement, because it keeps history, scroll and announcements correct for free. If a JS router is added, it must handle SVG anchors (inline graph/chord SVG) explicitly, restore scroll, move focus and announce titles, and re-initialise or tear down widgets across swaps.

## 5. Link-preview popovers

**Findings**
- Wikipedia's page previews consume a per-page summary endpoint (`/api/rest_v1/page/summary/{title}`) returning title, extract and thumbnail, not the full page ([tracker task](https://phabricator.wikimedia.org/T113094)). Their tracker records many bugs in the earlier extract path, which motivated a dedicated summary API ([task](https://phabricator.wikimedia.org/T117082)). The endpoint's own documentation was not consulted (UNVERIFIED).
- I found no documented static-site equivalent. Fetching the full HTML and cutting a fragment is the obvious fallback. UNVERIFIED as practice.

Implication for our design: Emit a per-node JSON fragment (title, summary HTML already sanitised, kind) at build time and fetch only that on hover. This keeps hover cost independent of page size and lets the same fragment feed search results and graph tooltips. Stale fragments are exactly the dependency problem from section 1, so they must be derived outputs in the same graph.

## 6. Theming without flash; contrast at build time

**Findings**
- The standard fix is a small synchronous inline script in `<head>`, before the stylesheets, that sets the theme from storage and falls back to `prefers-color-scheme`. Async or deferred scripts run too late. Pure CSS (`prefers-color-scheme`) is flash-free but cannot honour a saved choice ([Next.js guide](https://nextjs.org/docs/app/guides/preventing-flash-before-hydration)). Transitions can animate the initial application. Script-rewriting CDNs can defer the inline script. An offline site has neither the CDN nor a server, but keep the script immune to such rewriting.
- axe-core's `color-contrast` rule works on computed styles, so custom properties resolve before the check. It struggles with gradients, pseudo-element backgrounds, partial transparency and text over images, and can return "incomplete" for manual review ([Deque](https://dequeuniversity.com/rules/axe/4.9/color-contrast)). AA is 4.5:1 normal text, 3:1 large text. pa11y-ci with `WCAG2AA` and the axe runner can gate a CI step (UNVERIFIED: source dropped as low quality). I found no static-site generator that fails its own build on contrast.

Implication for our design: Check the design tokens pairwise per palette and theme inside the build, which is a pure arithmetic check that cannot be "incomplete", and fail the build on a miss. Add a rendered-DOM axe run in CI for what token checks cannot see. Ship the theme script inline as the first thing in `<head>`; for offline pages that is the only mechanism that runs before first paint.

## 7. Org-mode publishing

**Findings**
- **ox-publish and ID links:** ID links to other files may break because they depend on org-id state at export. With broken-link handling set to mark, output lost the link text. Changing publish options does not regenerate files whose sources are unchanged ([list thread](https://mail.gnu.org/archive/html/emacs-orgmode/2022-03/msg00076.html), [list thread](https://list.orgmode.org/5552297.DvuYhMxLoT@pluto/T/)). ox-publish caches anchors in a file so partial updates do not break links.
- **Backlinks are not built in:** people add them with custom publish hooks calling org-roam functions ([forum](https://org-roam.discourse.group/t/export-backlinks-on-org-export/1756?page=2), [forum](https://org-roam.discourse.group/t/publishing-notes-static-site-generation-with-org-roam/3800)). ox-hugo is reported to handle ID links and lets Hugo generate backlinks ([forum](https://org-roam.discourse.group/t/org-roam-to-blog/1581)).
- **Firn:** a secondhand report cites high disk usage and missing ID-link support ([summary](https://honnef.co/notes/20230106075546-braindumping_org_roam_notes/)). The results conflicted on whether Firn is Rust or Clojure ([repo](https://github.com/teesloane/firn)), so treat language and current state as UNVERIFIED.
- **Stable URLs:** I found no source that addresses permalinks surviving a title or file rename for org-roam nodes. UNVERIFIED. Hugo notes pages with colliding URLs are silently overwritten ([thread](https://discourse.gohugo.io/t/url-collision-detection/42953)).
- I did not evaluate Rust org renderers used for sites.

Implication for our design: Derive URLs from the stable node ID (with the title only as a cosmetic slug that redirects), never from the filename or title. Resolve ID links in one pass over the whole bundle and treat an unresolved ID as a build error, not a silently dropped link. Build backlinks as first-class output, and detect URL collisions explicitly.

## 8. Hermetic, fail-closed builds

**Findings**
- Hugo's closest feature is `--panicOnWarning` (with `--printPathWarnings`). Users report it hanging after a warning on one version, being ignored under `serve`, and giving no better location for deprecation warnings ([hang](https://discourse.gohugo.io/t/hugo-paniconwarning-hangs-forever/36548), [ignored](https://discourse.gohugo.io/t/flag-paniconwarning-ignored-with-some-warnings/37307)). A "strict mode" for missing links and unused keys is proposed in a thread and an issue number is cited there ([thread](https://discourse.gohugo.io/t/static-analysis-for-hugo-sites-with-strict-mode-build-time-safety-against-unintentional-errors/56197)). Whether it shipped is UNVERIFIED.
- I found no generator that offers no-network, symlink refusal, or write confinement as built-in switches, and no evidence about reproducible output. Those properties normally come from the surrounding build system. Bazel and Nix were not researched, so this is UNVERIFIED.

Implication for our design: Make fail-closed behaviour a property of the engine, not a flag: no network access in the render path, refuse symlinks in the input bundle, resolve every output path under the output root, and exit non-zero on any warning by default. Add tests that try to violate each rule. The prior art suggests a warning-as-error flag alone is a weak guarantee.

## Open risks
1. No independent benchmark covers search at 10k pages, Spanish stemming quality, or chord diagrams. Our numbers must come from our own measurement.
2. Dependency tracking is the hard part: every tool with hand-declared dependencies has staleness bugs. Byte-identical-to-full-build checks are the only evidence that would fail if tracking is wrong.
3. Early cutoff depends on every derived value having a stable, comparable representation. Non-deterministic ordering would defeat it and also break reproducibility.
4. Cross-document View Transitions are not in Firefox, so the experience must not depend on them.
5. A JS router over inline SVG is a known trap (`SVGAnimatedString`), and unproven for this stack (UNVERIFIED for swup/htmx/barba).
6. Pagefind's fit as a bundled dependency (versus a Rust-built equivalent) is measured in ADR-119's spikes (C4); its licence is MIT.
7. Stable-URL behaviour across renames, and Rust org renderers for sites, have no prior-art evidence found.
8. Several sources are forum threads, mirrors or blog posts rather than maintainers' docs. Hugo's rebuild design in particular is read from a commit mirror.

## Link check (2026-11-18)

Every cited URL was fetched on 2026-11-18. Sources that were dead (404/502), unofficial mirrors,
aggregator pages or low-quality blog posts were removed, with the claims they alone carried dropped or
marked UNVERIFIED. These hosts did not answer from the checking machine and were not re-checked:
codeberg.org (the Pagefind delta-encoding note), discuss.hotwired.dev, the TanStack router pull request
on github.com, and the Hugo commit mirror at git.feldspaten.org (whose claim about Hugo's dependency
tracker should be confirmed against Hugo's own repository before it is relied on). npmjs.com refuses
scripted requests (403); the lunr-languages claim rests on Lunr's own guide.

The engine's own measurements (spikes C1-C12) are in ADR-119's Evidence section, not here.
