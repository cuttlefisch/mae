# Verification checklist: behaviours guaranteed by the previous site engine's tests

## What this is

The previous static-site engine (and its companion graph, palette, organisation-notes and mutation-runner components) was verified by roughly 700 *mutants*: each one a deliberate, small breakage of the code that a named test had to catch. A mutant that survives means a behaviour is not really tested. Every mutant that the old tests caught therefore documents a behaviour that mattered enough to be guarded.

This file distils those mutants into **130 behavioural intents**, deduplicated and grouped by behaviour rather than by file. Each line states what an engine must do, as something observable (DOM, accessibility tree, URLs, keyboard, files, exit codes, logs). No line describes how the old engine achieved it. The number after each line, `(xN)`, is how many original mutants merged into that intent; a high number means the old suite attacked that behaviour from many angles.

## How to use it

Treat each line as a behaviour the new engine should guarantee, and write a test for it that would **fail if the behaviour broke**: check it by mutating the new engine and watching the test go red, not just by seeing it pass. Keep the mutation for each test in the repository's mutation list so it can be re-run. Where an intent maps to a requirement in the parity specification (P-xx), the acceptance test there is the starting point; add the edge cases the line names. Intents with no P-xx are listed again at the end as candidates for new requirements.

Notes on reading it: some old intents describe edge cases of a specific design (for example, exact step sizes or label geometry); keep the behaviour, adapt the numbers. Counts are of source mutants, not of tests.

Totals: 130 intents from 706 source mutants; 101 map to a P-xx requirement, 29 are candidates for new requirements.

## 1. Pages, addresses and redirects  (10 intents, 40 mutants)

- [ ] **1.1** MUST write a static redirect at /<id> for every published node, including sections, landing on the section's anchor. (x5) — P-02, P-03
- [ ] **1.2** MUST keep every old URL from the route history leading to its page, and MUST only do so when history is supplied and enabled. (x6) — P-05
- [ ] **1.3** MUST never overwrite a live page, or an earlier redirect, with a redirect; a page's current URL never redirects to itself. (x5) — P-06
- [ ] **1.4** MUST skip and report IDs and history paths that cannot be safe file names (slash, `..`, backslash, empty, leading dot), writing nothing for them. (x5) — P-08
- [ ] **1.5** MUST make the redirect's target relative and correctly escaped, never climbing out of the site folder. (x3) — P-03
- [ ] **1.6** MUST fail the build when two published nodes share one ID. (x1) — P-147
- [ ] **1.7** MUST write the address manifest next to the site, never inside it, and list for each page the unpublished IDs it references. (x5) — P-09
- [ ] **1.8** MUST compute a page's URL in the manifest exactly as the page is emitted. (x2) — P-09
- [ ] **1.9** MUST validate the optional KB identifier (type, length limit, allowed first and last character) and fail the build on an invalid one. (x7) — P-04
- [ ] **1.10** MUST, when a KB identifier is configured, also answer /<kb>/<id> (without displacing /<id>), and record the KB in the manifest. (x1) — P-04

## 2. Links, anchors and resolution  (4 intents, 14 mutants)

- [ ] **2.1** MUST turn a link into a link only when its target is a published node; any other node reference never becomes an anchor. (x4) — P-10, P-11, P-14
- [ ] **2.2** MUST pass all page HTML through a final allow-list pass that refuses off-site and protocol-relative hrefs and does not trust a link's class. (x3) — P-14, P-151
- [ ] **2.3** MUST point a link to a section at the section's anchor, and keep section anchors stable and numbered by position, never by author text. (x5) — P-10, P-136
- [ ] **2.4** MUST, for a link to a section on the same page, scroll it into view without navigating; from another page, navigate without reload and land on it. (x2) — P-27

## 3. Copy-link control (site component example)  (7 intents, 33 mutants)

- [ ] **3.1** MUST offer a copy-link control on each page and each addressable section heading, and nowhere else (not on other headings, not in preview popups). (x4) — P-162
- [ ] **3.2** MUST copy origin + base path + /<kb>/<id>, with the base's trailing slash removed and a malformed address refused. (x6) — P-162
- [ ] **3.3** MUST mark only addressable section headings with a validated ID, clear stale marks, and let the mark survive the sanitiser. (x7) — P-162
- [ ] **3.4** MUST leave exactly one copy-link control per heading after repeated client-side navigation, and keep it working after navigating. (x2) — P-162, P-156
- [ ] **3.5** MUST offer a manual-copy fallback (field pre-selected) when the clipboard is unavailable or refuses, closable by Escape and Close, returning focus to the control. (x6) — P-162
- [ ] **3.6** MUST announce a successful copy through a live region. (x2) — P-162
- [ ] **3.7** MUST place the copy-link control in the header row beside the history controls, wrapping to its own line without horizontal overflow. (x6) — P-159, P-162

## 4. Page metadata: dates, versions, site-changes page  (10 intents, 104 mutants)

- [ ] **4.1** MUST accept a last-updated date only in strict calendar form (full anchored match; month and day ranges; no zero year, month or day; correct month lengths; leap-year rules for the 100 and 400 cases) and judge it without time-zone drift. (x12) — P-150
- [ ] **4.2** MUST read the updated date from sections as well as pages, and escape it in the build record. (x2) — P-150
- [ ] **4.3** MUST show a valid updated date as a machine-readable time element on the page and in the search text, and MUST NOT count malformed dates as dated. (x13) — P-150, P-163
- [ ] **4.4** MUST validate the version record strictly (schema version, required keys, list and object shapes, version-label format, commit-hash form and length, date calendar, ordering, entry type and scope) and report an invalid record in the log without failing the build. (x26) — P-150, P-160
- [ ] **4.5** MUST emit a site-changes page listing versions newest first, grouping entries by type under headings, omitting empty sections, and showing unpublished versions as such. (x11) — P-160
- [ ] **4.6** MUST re-validate version data in the component that shows it, and link to the changes page with a correctly encoded relative address. (x9) — P-163
- [ ] **4.7** MUST show the site version in the left-panel footer, linking to the changes page, hidden on mobile, in the body font and weight, and accept only a valid version label. (x13) — P-163
- [ ] **4.8** MUST show a "what's new" link in the header, omitted when its target is unpublished, aligned and sized like the other header controls. (x7) — P-163
- [ ] **4.9** MUST show the document-state badge after the title and above the article, absent without the property, with its own notice when invalid, and never before the title. (x5) — P-161
- [ ] **4.10** MUST summarise in the build log the state notices and counts (valid, invalid with reason) without failing the build. (x6) — P-148, P-150

## 5. Navigation, history and client-side routing  (5 intents, 20 mutants)

- [ ] **5.1** MUST navigate between internal pages without a full reload, and a full reload MUST NOT be silently substituted. (x3) — P-22
- [ ] **5.2** MUST walk the browser's own history with in-page Back and Forward controls that read the position, tolerate out-of-range indices, bound the stored list, are disabled when there is nowhere to go, and survive reloads. (x10) — P-24
- [ ] **5.3** MUST route links inside diagrams like body links, also from a diagram dialog. (x1) — P-26
- [ ] **5.4** MUST mark internal body links to other pages (including same-page) as internal, with no target or rel. (x3) — P-28
- [ ] **5.5** MUST keep a single instance of each page-level control (tab, handle, lists) after repeated navigation, with unique IDs derived from position. (x3) — P-25, P-135

## 6. Layout, panels and header row  (11 intents, 60 mutants)

- [ ] **6.1** MUST cap each panel's width (right narrower than left), clamp a stored wider width, and keep the grid in step with the cap. (x4) — P-87
- [ ] **6.2** MUST hide and show each panel with a control at its top, applying and remembering the state before first paint, with the correct expanded state and focus kept on the control. (x9) — P-85
- [ ] **6.3** MUST place the hidden-panel control at a consistent height, at the window edge, the same size as when open, outside the panel when open, and without a floating shadow. (x7) — P-84, P-85
- [ ] **6.4** MUST let a panel be resized by a handle: a click steps through preset widths (skipping those that do not fit), arrows and Home/End adjust within limits, and drag follows the pointer in the right direction. (x8) — P-86
- [ ] **6.5** MUST, on tablet, allow hiding the left panel only (no right toggle, right content below the text); on mobile, show no toggles and never apply a width hidden at another size. (x4) — P-89, P-90
- [ ] **6.6** MUST centre the page text in a column of bounded measure, with the side panels at the viewport edges. (x3) — P-83
- [ ] **6.7** MUST draw each panel's inner border visibly (3:1 against the page). (x3) — P-88
- [ ] **6.8** MUST align the history controls, header components and graph title on one row with consistent height, spacing and centring, without overlap. (x11) — P-84, P-159
- [ ] **6.9** MUST keep layout identical when a footer or header slot is empty, and put slotted footer content below the explorer, counting the explorer's handle, from 1200 px exactly. (x6) — P-47, P-158
- [ ] **6.10** MUST place the site logo in the flow, first in the left column, centred with the panel button, to the panel edge, also at exactly 1200 px. (x4) — P-104
- [ ] **6.11** MUST keep the reader view control to the right of the mode toggle and palette picker. (x1) — P-84

## 7. Explorer (file tree)  (6 intents, 43 mutants)

- [ ] **7.1** MUST mark the current page with aria-current and a non-colour bar, at rest in normal weight and the current page bold, with spacing between entries. (x6) — P-39, P-40
- [ ] **7.2** MUST never scroll the window to reveal the current entry; only the list scrolls, and on first load without an anchor the window stays at the top. (x3) — P-44
- [ ] **7.3** MUST, on navigation, leave the list where it is if the current entry is visible, else scroll the list so the entry sits at the top edge, and bring it into view after first paint. (x6) — P-45
- [ ] **7.4** MUST accept a configured descending order per folder only as a well-formed list of unique, normalised folder paths (not a non-list, empty, backslash, leading or trailing slash, `//`, `..`, duplicate or non-text entry), and fail the build on a bad one. (x16) — P-46
- [ ] **7.5** MUST embed the order list safely (escaped against script-breaking characters, not taken from the environment). (x2) — P-46, P-151
- [ ] **7.6** MUST list a descending folder newest first with sub-folders before notes, folders matched exactly by path prefix, text compared accent- and case-sensitively, numbers as text, sorted at build time, in the browser as well. (x10) — P-46

## 8. Link styling and external links  (3 intents, 13 mutants)

- [ ] **8.1** MUST set body links and backlinks in the surrounding text's weight (not bold), underlined, without background. (x4) — P-38, P-103
- [ ] **8.2** MUST open web links in a new tab with an opener-blocking rel, mark them with an icon plus visually hidden text in the configured locale, and keep the icon visible at 3:1 in both modes and under forced colours. (x8) — P-49, P-50, P-51
- [ ] **8.3** MUST show a preview only for internal body links to other pages, never for same-page anchors or diagram links. (x1) — P-48, P-28

## 9. Theming, palettes and accents  (10 intents, 69 mutants)

- [ ] **9.1** MUST declare the active palette on the root element from a validated list, falling back to the default for unknown, differently-cased or incomplete palettes, and load the palette styles. (x7) — P-92
- [ ] **9.2** MUST apply the reader's palette before first paint, and persist the choice; the graph follows the chosen palette. (x3) — P-91, P-93, P-68
- [ ] **9.3** MUST map a stored palette that has been removed to its designated successor (never via prototype keys), drop its accent, and migrate on client-side navigation. (x5) — P-98
- [ ] **9.4** MUST offer the palette picker aligned with the mode toggle, within the window even with many palettes, opening on the marked entry, with Down/Escape/outside-click behaviour and a mark that follows the choice. (x8) — P-93, P-99
- [ ] **9.5** MUST apply the stored accent before first paint (any valid ID, a broken one removing the palette), persist it per palette, keep the default accent first, clear on default, never carry an accent across palettes, and tolerate storage that throws. (x11) — P-94, P-96, P-97
- [ ] **9.6** MUST show accent swatches only for palettes that have them (a single swatch shows no row), each with its mode colour and state, and every swatch at least 4.5:1 against the background. (x3) — P-94, P-102
- [ ] **9.7** MUST let an accent follow into the graph even without a reader palette. (x1) — P-68, P-95
- [ ] **9.8** MUST open the accent submenu by mouse with hover intent (crossing other items does not open it; entering it cancels the change; leaving closes it), by click, and by keyboard (Right opens, Left closes, Escape closes the whole menu), with correct expanded state and focus on the marked item. (x13) — P-99
- [ ] **9.9** MUST place the submenu beside its item where it fits (right or left), clamped vertically, over the list when there is no room, and following a scrolled list; a palette without swatches has no submenu; a swatch of another palette is not applied to the current one. (x8) — P-94, P-99
- [ ] **9.10** MUST support an optional SVG site logo: shown first in the left column, drawn in the heading colour in every palette, scripts, clip-paths and unsafe content stripped, IDs prefixed to stay page-unique, name escaped, file confined to the site folder, no fill treated as black, size bounded. (x10) — P-104

## 10. Contrast audit  (2 intents, 9 mutants)

- [ ] **10.1** MUST audit text contrast in light and dark over a fixed scenario set and treat a hidden or unresolved-link element as excused only when it really is hidden or unresolved. (x2) — P-100, P-101
- [ ] **10.2** MUST keep code-block and diagram ink at required contrast in every palette and mode; a palette variable that is unreadable, or not separated from the background, fails. (x7) — P-100, P-102

## 11. Code highlighting  (7 intents, 37 mutants)

- [ ] **11.1** MUST colour each token by its role from the active palette, on the block's own background, with the inline code colour distinct from the block's, and each role (keyword etc.) styled. (x9) — P-106
- [ ] **11.2** MUST extract palette roles from the palette definition, including comment roles. (x1) — P-106
- [ ] **11.3** MUST highlight supported languages at build time, including those whose grammar is missing in the common set (shell, PowerShell, JSON) and snippets in inline code, counting dot blocks separately. (x6) — P-105
- [ ] **11.4** MUST keep block text byte-identical to the source: tabs, line endings and surrounding text unchanged, and highlight the whole tree, not only its first level. (x5) — P-107
- [ ] **11.5** MUST assign a role from the most specific scope matching, with prefix and variable scopes, scopes held in objects, comments and JSON keys treated correctly. (x6) — P-106
- [ ] **11.6** MUST sanitise highlighted code to an allow-list of role classes only (no element inside a role span, no free classes, no cleaning inside pre, role classes by exact prefix). (x8) — P-120, P-151
- [ ] **11.7** MUST report per language the blocks highlighted and left plain, with the reason. (x2) — P-108, P-148

## 12. Performance and time budgets  (2 intents, 11 mutants)

- [ ] **12.1** MUST split highlighting work into bounded chunks (line separator, merge of equal roles, last chunk within the end) with limits per line, per block and per time unit; a limit of zero means unlimited. (x9) — candidate (no matching P-xx)
- [ ] **12.2** MUST bound the whole build's highlighting time with a budget that sums block times. (x2) — candidate (no matching P-xx)

## 13. Offline output and third-party content  (1 intents, 5 mutants)

- [ ] **13.1** MUST ship third-party grammars only with their licence text, provenance and a checksum that is verified; nested packages attributed correctly. (x5) — candidate (no matching P-xx)

## 14. Diagrams  (4 intents, 9 mutants)

- [ ] **14.1** MUST detect a fixed-size diagram attribute even when the source is split across lines or strings. (x1) — P-115
- [ ] **14.2** MUST keep a diagram with links a named group, still presented as an image, with an expand button, and open from a click anywhere on its frame. (x3) — P-112, P-122
- [ ] **14.3** MUST make diagram links focusable, underlined like text links, in the text weight, with a thicker outline on hover and focus and a visible focus ring. (x4) — P-126
- [ ] **14.4** MUST NOT count a missing theme as a failure in diagram drawing statistics. (x1) — P-148

## 15. Folding, long pages and tables  (7 intents, 37 mutants)

- [ ] **15.1** MUST honour author-marked folded sections (property key matched exactly, any value but the marker folds not), strip property drawers from output and still read the marker after rendering. (x9) — P-19, P-128
- [ ] **15.2** MUST show everything, no fold controls, when printed. (x1) — P-132
- [ ] **15.3** MUST put each fold button inside the section heading, with the heading as the section's first child, and carry the search term when navigating. (x3) — P-127
- [ ] **15.4** MUST open folded sections containing a requested fragment, including a fragment naming a folded heading itself. (x2) — P-130
- [ ] **15.5** MUST keep the expand-all control accurate: label, expanded state, chevron and next-page state follow every individual fold, find-in-page reveal and preparation; it persists state, and works without storage. (x13) — P-129, P-131
- [ ] **15.6** MUST size fold text buttons to the minimum target (24 px) with an outline style, as before. (x2) — P-127
- [ ] **15.7** MUST scroll a wide table inside its own container (a labelled region), with cell minimum widths, padding and a sticky header, never moving the page; the table returns from its dialog. (x7) — P-133, P-134

## 16. Search text and index  (1 intents, 6 mutants)

- [ ] **16.1** MUST separate adjacent text of cells, header cells, list items, inline elements and diagram labels with whitespace when extracting page text for the index. (x6) — P-31, P-77

## 17. Unique element IDs  (1 intents, 5 mutants)

- [ ] **17.1** MUST give every registered component its own element IDs, renamed inside the component and its script, with a distinct suffix per instance, in tests and in the browser. (x5) — P-135, P-136

## 18. Build hygiene (exit codes, fail-closed, sandbox, logging)  (1 intents, 3 mutants)

- [ ] **18.1** MUST print the KB-address summary line only when a KB identifier is configured, and always then. (x3) — P-148

## 19. Graph: layout and geometry  (6 intents, 23 mutants)

- [ ] **19.1** MUST leave an angular margin for a child's rings, pack subtrees without pushing them out when they fit, and end a chord in a ring on the ring's outer edge. (x3) — candidate (no matching P-xx)
- [ ] **19.2** MUST allocate sunburst shares without overshooting the parent, respect the minimum angle, place leaf-only folders at the right ring and keep the single page of a one-page site. (x4) — candidate (no matching P-xx)
- [ ] **19.3** MUST count a folder's size to include its own page, treat structure as direct-parent only, and list notes before folders. (x3) — candidate (no matching P-xx)
- [ ] **19.4** MUST offer a layout switch only as configured, offer orientation flip only with the radial hierarchy layout (always including the default orientation), ignore a stored flip the site has withdrawn, and keep the default layout offered. (x5) — P-65
- [ ] **19.5** MUST place group labels with the correct offset direction and break label groups at the widest gap. (x2) — candidate (no matching P-xx)
- [ ] **19.6** MUST keep label arcs readable: at least a half circle and at most nearly a whole one, no longer than their segment, room capped at the arc, bottom-half labels not upside down, positions rounded consistently. (x6) — candidate (no matching P-xx)

## 20. Graph: camera, culling and focus framing  (3 intents, 14 mutants)

- [ ] **20.1** MUST zoom about the pointer, fit with padding (clamped), and compose smooth zoom steps without cancelling. (x4) — P-61a, P-64
- [ ] **20.2** MUST cull off-screen elements with margin on both axes around the true visible area (touching counts as visible) and include a label's full reach (width and height). (x8) — candidate (no matching P-xx)
- [ ] **20.3** MUST frame a focused note's sector across all axes it crosses and include its neighbours. (x2) — P-61a, P-64

## 21. Graph: interaction, search, keyboard and accessibility  (7 intents, 17 mutants)

- [ ] **21.1** MUST include a hierarchy's own pages in the keyboard walk, so every note is reachable. (x1) — P-59
- [ ] **21.2** MUST open the expanded view over the whole page, without a history entry (neither opening nor Back creating one). (x3) — P-61, P-62
- [ ] **21.3** MUST offer the "you are here" toggle as a switch with its state exposed, redrawing nothing when toggled, and always show the marker in the side-panel graph. (x5) — P-65, P-64
- [ ] **21.4** MUST, in the expanded view, make a search find a note without navigating: label it, focus it, and colour its links distinctly from the others. (x3) — P-63
- [ ] **21.5** MUST draw the current page's links above the others, and highlight nothing in the graph merely from body links being hovered other than that note and its link. (x2) — P-57, P-58
- [ ] **21.6** MUST give every note an accessible name and emit no native tooltips. (x2) — P-60
- [ ] **21.7** MUST use the site's base path for the graph and its links. (x1) — P-21

## 22. Test harness integrity  (12 intents, 56 mutants)

- [ ] **22.1** MUST run browser checks headless regardless of the caller's display environment (no display variables leaking in, the headless flag forced, no unrelated executable or browser-cache path accepted). (x8) — P-138
- [ ] **22.2** MUST launch every browser check (each pointer profile and browser, the logo check, the contrast check) through the one shared launch configuration. (x5) — P-138
- [ ] **22.3** MUST exit 1 on any failed check, on an uncaught exception, on a crashed or non-launching browser (counted as a page failure), and when a selection runs no check; and exit 2 when it could not check (no site built, missing input). (x10) — candidate (no matching P-xx)
- [ ] **22.4** MUST count a failed test-image build, failed mount or a container-runtime error as a failed check, never as a pass. (x3) — candidate (no matching P-xx)
- [ ] **22.5** MUST choose the number of parallel shards from the CPU quota and memory the process is actually allowed (scheduler variable, container quota, its own control group but not parents'), and never more shards than check groups. (x5) — candidate (no matching P-xx)
- [ ] **22.6** MUST distribute check groups to shards by weight (heaviest first, to the least loaded shard), keep file order within a shard, count a repeated group once, give an unknown check weight 0 and a missing table the median. (x6) — candidate (no matching P-xx)
- [ ] **22.7** MUST let each child run its own shard and print its shard number on its lines. (x2) — candidate (no matching P-xx)
- [ ] **22.8** MUST run every check exactly once across shards, flagging repeated and missing ones. (x2) — candidate (no matching P-xx)
- [ ] **22.9** MUST print one result line per check naming the browsers it ran in, and start all browsers up front when none is requested. (x3) — candidate (no matching P-xx)
- [ ] **22.10** MUST prepare fixtures per check, at the moment a check is registered, not once and not at selection time. (x2) — candidate (no matching P-xx)
- [ ] **22.11** MUST support re-running only failed checks (matched by contained name, never returning "not checked", reading the newest run and not the previous). (x4) — candidate (no matching P-xx)
- [ ] **22.12** MUST keep failure artefacts reliable: none for a passing check, the page stays open until capture, closing twice really closes, the number closed without capture is reported, the console is captured. (x6) — candidate (no matching P-xx)

## 23. Mutation runner itself  (10 intents, 78 mutants)

- [ ] **23.1** MUST restore each mutated file byte-for-byte, verifying its checksum, including on interrupt, and exit 2 when a restoration fails; it never overwrites edits made while mutated or after a cut. (x10) — candidate (no matching P-xx)
- [ ] **23.2** MUST write a journal before mutating and recover it at the next start, ignoring stale journals and detecting changes by checking tracked and untracked files when no journal exists. (x7) — candidate (no matching P-xx)
- [ ] **23.3** MUST allow one runner per tree: an exclusive lock that checks whether its owner is alive. (x3) — candidate (no matching P-xx)
- [ ] **23.4** MUST require the unmutated baseline to pass (with at least one test run, a failing preparation step failing the round), before judging any mutant. (x4) — candidate (no matching P-xx)
- [ ] **23.5** MUST count a mutant as caught on a test failure, load failure, resource-cap failure, hang or timeout, any non-zero command exit, and a failing preparation; but a dead worker, signal-killed file, partly-run bail, or foreign signal is NOT a verdict (no result), and a foreign signal does not abort the round. (x13) — candidate (no matching P-xx)
- [ ] **23.6** MUST count only tests the mutant's filter selects (not the whole file, not another file, not an empty filter), run the named test alone, and stop at first failure. (x9) — candidate (no matching P-xx)
- [ ] **23.7** MUST reject malformed mutant entries (text occurring more than once, missing file, unknown runner type, comma-containing names, symlinked target, nonexistent test or anchor) and not rely on retries. (x11) — candidate (no matching P-xx)
- [ ] **23.8** MUST keep the mutated tests from inheriting the runner's mode variable, and kill the whole test process group on signal or timeout, leaving nothing alive. (x6) — candidate (no matching P-xx)
- [ ] **23.9** MUST support running only mutants for what changed since a ref (tracked, untracked, renamed, test files, list changes force all, package prefix stripped, runner's own changes and lock changes handled), never silently running none or all by accident. (x12) — candidate (no matching P-xx)
- [ ] **23.10** MUST support resuming a run only when the sources are unchanged (comparing sources, re-running "no result" entries), and write one line per mutant. (x3) — candidate (no matching P-xx)

## Candidates for new requirements

Intents with no matching P-xx in the parity specification (several fall under its test-harness section rather than a numbered requirement).

- **12.1** MUST split highlighting work into bounded chunks (line separator, merge of equal roles, last chunk within the end) with limits per line, per block and per time unit; a limit of zero means unlimited.
- **12.2** MUST bound the whole build's highlighting time with a budget that sums block times.
- **13.1** MUST ship third-party grammars only with their licence text, provenance and a checksum that is verified; nested packages attributed correctly.
- **19.1** MUST leave an angular margin for a child's rings, pack subtrees without pushing them out when they fit, and end a chord in a ring on the ring's outer edge.
- **19.2** MUST allocate sunburst shares without overshooting the parent, respect the minimum angle, place leaf-only folders at the right ring and keep the single page of a one-page site.
- **19.3** MUST count a folder's size to include its own page, treat structure as direct-parent only, and list notes before folders.
- **19.5** MUST place group labels with the correct offset direction and break label groups at the widest gap.
- **19.6** MUST keep label arcs readable: at least a half circle and at most nearly a whole one, no longer than their segment, room capped at the arc, bottom-half labels not upside down, positions rounded consistently.
- **20.2** MUST cull off-screen elements with margin on both axes around the true visible area (touching counts as visible) and include a label's full reach (width and height).
- **22.3** MUST exit 1 on any failed check, on an uncaught exception, on a crashed or non-launching browser (counted as a page failure), and when a selection runs no check; and exit 2 when it could not check (no site built, missing input).
- **22.4** MUST count a failed test-image build, failed mount or a container-runtime error as a failed check, never as a pass.
- **22.5** MUST choose the number of parallel shards from the CPU quota and memory the process is actually allowed (scheduler variable, container quota, its own control group but not parents'), and never more shards than check groups.
- **22.6** MUST distribute check groups to shards by weight (heaviest first, to the least loaded shard), keep file order within a shard, count a repeated group once, give an unknown check weight 0 and a missing table the median.
- **22.7** MUST let each child run its own shard and print its shard number on its lines.
- **22.8** MUST run every check exactly once across shards, flagging repeated and missing ones.
- **22.9** MUST print one result line per check naming the browsers it ran in, and start all browsers up front when none is requested.
- **22.10** MUST prepare fixtures per check, at the moment a check is registered, not once and not at selection time.
- **22.11** MUST support re-running only failed checks (matched by contained name, never returning "not checked", reading the newest run and not the previous).
- **22.12** MUST keep failure artefacts reliable: none for a passing check, the page stays open until capture, closing twice really closes, the number closed without capture is reported, the console is captured.
- **23.1** MUST restore each mutated file byte-for-byte, verifying its checksum, including on interrupt, and exit 2 when a restoration fails; it never overwrites edits made while mutated or after a cut.
- **23.2** MUST write a journal before mutating and recover it at the next start, ignoring stale journals and detecting changes by checking tracked and untracked files when no journal exists.
- **23.3** MUST allow one runner per tree: an exclusive lock that checks whether its owner is alive.
- **23.4** MUST require the unmutated baseline to pass (with at least one test run, a failing preparation step failing the round), before judging any mutant.
- **23.5** MUST count a mutant as caught on a test failure, load failure, resource-cap failure, hang or timeout, any non-zero command exit, and a failing preparation; but a dead worker, signal-killed file, partly-run bail, or foreign signal is NOT a verdict (no result), and a foreign signal does not abort the round.
- **23.6** MUST count only tests the mutant's filter selects (not the whole file, not another file, not an empty filter), run the named test alone, and stop at first failure.
- **23.7** MUST reject malformed mutant entries (text occurring more than once, missing file, unknown runner type, comma-containing names, symlinked target, nonexistent test or anchor) and not rely on retries.
- **23.8** MUST keep the mutated tests from inheriting the runner's mode variable, and kill the whole test process group on signal or timeout, leaving nothing alive.
- **23.9** MUST support running only mutants for what changed since a ref (tracked, untracked, renamed, test files, list changes force all, package prefix stripped, runner's own changes and lock changes handled), never silently running none or all by accident.
- **23.10** MUST support resuming a run only when the sources are unchanged (comparing sources, re-running "no result" entries), and write one line per mutant.
