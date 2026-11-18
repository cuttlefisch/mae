# Knowledge-base site: behavioural parity specification

Status: draft. Applies to any static-site engine that publishes an org-mode knowledge base
(notes with stable IDs) as a website, and to the replacement of one such engine by another.

Every requirement below describes **observable behaviour only**: what a reader, an
assistive technology, a browser or a build pipeline can see (DOM, accessibility tree, URLs,
keyboard, timing, files, exit codes). How an engine achieves it is out of scope.

## 1. Scope and terms

- **Bundle**: a directory of org-mode files, plus optional build-time data (route history,
  version records), handed to the engine as its only content input.
- **Node**: an org-mode entry that carries an `:ID:` property, either file-level (the
  property drawer before any other content) or on a heading.
- **Page**: the HTML document the engine emits for one file-level node.
- **Section**: a heading inside a page. A section whose heading is a node is *addressable*.
- **Stable ID**: a node's `:ID:` value. It never changes when the file is moved or renamed.
- **Published bundle**: a bundle that the gate has passed; the only thing the engine renders.
- **Gate**: an external, fail-closed selection step that decides what is published, removes
  everything else, and refuses to emit a bundle when in doubt.
- **Reference engine / reference site**: the engine being replaced and the site it builds.
- **Candidate engine / candidate site**: the replacement and the site it builds.
- **Slot**: a named place in the page layout where a site adds its own components (§3.18).
- **Configured locale**: the site's language and locale setting; every user-visible string
  the engine itself writes (control names, notices, dates) comes from it.
- **Mobile / tablet / desktop**: viewport widths below 800 px / 800–1199 px / 1200 px and up.

Conventions for acceptance tests:

- "Browser" means a headless run in **both Chromium and Firefox**, unless a test says
  otherwise (only where one engine cannot emulate a condition, e.g. forced colours or reading
  the clipboard). Pointer profiles: mouse (`hover: hover`, `pointer: fine`) and touch
  (`hover: none`, `pointer: coarse`), each asserted before measuring.
- Pages are served from the build output by request interception or a loopback server;
  any request to another origin is aborted and counted as a failure.
- "No horizontal scroll" means `documentElement.scrollWidth <= innerWidth`.
- "Without reload" means a marker set on `window` before the action is still present after it.
- "Before paint" means the state is already present in the first `requestAnimationFrame`
  callback registered by a document-start init script.
- A contrast ratio is the WCAG 2.x relative-luminance ratio of computed colours, composited
  over what is actually underneath.
- "(new: not guaranteed by today's site)" marks behaviour the reference site lacks or does
  not test; it is required of the candidate but cannot be compared.

## 2. Input contract

- The engine renders **only a published bundle**. It never reads a live knowledge base, a
  version-control history, an editor session or any network resource.
- The gate, not the engine, decides what is published. The engine's invariant is
  **closure**: its output refers only to what the bundle contains, plus web (`http(s):`) and
  `mailto:` links.
- The bundle is read-only to the engine. Build-time data (route history, version records)
  arrives as files at documented paths and is validated in full; invalid data disables only
  the feature that uses it.
- The engine fails closed: when the bundle breaks this contract, or when it cannot render
  every published node faithfully, it publishes nothing (§3.17).

## 3. Requirements

### 3.1 Pages and addresses

| ID | Requirement | Acceptance test |
|---|---|---|
| P-01 | MUST emit exactly one page per file-level node in the bundle. | Count of pages in the output equals count of file-level IDs in the bundle; each ID resolves to one page. |
| P-02 | MUST answer every published ID (page and addressable section) at `/<id>`, leading to the page (and the section's anchor). | For every ID in the address manifest, loading `/<id>` ends on the page URL with the right fragment. |
| P-03 | MUST make `/<id>` redirects static: canonical link, `noindex`, zero-delay refresh, relative target. | Parse each redirect file: `<link rel=canonical>`, `<meta name=robots content=noindex>`, `<meta http-equiv=refresh content="0; url=<relative>">`. |
| P-04 | MUST, when a KB identifier is configured, also answer `/<kb-id>/<id>`; `/<id>` stays. An invalid KB identifier fails the build. | Both paths redirect to the same page; a config with an identifier outside `^[a-z0-9][a-z0-9-]{0,62}$` exits 1. |
| P-05 | MUST keep every URL the site ever served for a page leading to it after a move or rename, from the route history in the bundle. | Build with a history mapping an old path to an ID: the old URL redirects to the current page. |
| P-06 | MUST never overwrite a live page with a redirect; a live page always wins. | A history entry whose old URL equals a live page's URL: the live page has no refresh meta. |
| P-07 | MUST NOT redirect an ID that is no longer published; its old URL is 404. | Remove a node from the bundle: neither `/<id>` nor its old path exists in the output. |
| P-08 | MUST skip and report an ID or history path that cannot be a safe file name (slash, leading dot, `..`, backslash, empty). | Such entries produce no file and appear in the build log's skipped count. |
| P-09 | MUST write an address manifest (ID → URL with anchor, and per page the unpublished IDs it references) next to, not inside, the site. | The manifest exists beside the site directory and lists every published ID. |
| P-10 | MUST resolve links to a section to the section's anchor. | A link to a heading node has `href` ending in that section's fragment; the fragment's element exists. |
| P-11 | MUST render a link to an ID not in the bundle as a non-link placeholder (marked as unresolved, author's description or nothing), never as an `<a>`. | Fixture link to an absent ID: an element with the unresolved marker, no `href`; contrast scenario passes. |
| P-12 | MUST NOT emit the text of an unpublished ID, a local link scheme (`file:`, `id:`, `elisp:` …), or any UUID-shaped reference that is not a published ID, outside code blocks. | Grep the output for the fixture's unpublished IDs and schemes: 0 hits. |
| P-13 | MUST hide references in configured hidden namespaces (`ns:rest`) and show those in other namespaces. | Fixture with one hidden and one shown namespace reference: the first has 0 hits in output, the second is present. |
| P-14 | MUST only make links of: published nodes, `http(s):`, `mailto:`, same-page anchors. | Every `<a href>` in the article matches one of those four forms. |
| P-15 | MUST fail the build when two inputs map to the same URL. | Two files whose names normalise to one URL: exit 1. |
| P-16 | MUST set `<html lang>` to the configured locale and the site title from configuration. | `lang` attribute and `<title>`/site heading match the configuration. |
| P-17 | MUST use the node title as the page title and render body headings one level below it. | Page has one `h1` (the title); a top-level org heading is an `h2`. |
| P-18 | MUST NOT load images from outside the site; a web image becomes a link. | Fixture with a remote image: no `<img>` with a remote `src`; an `<a>` to it exists. |
| P-19 | MUST NOT render property drawers; their anchors remain. | No drawer text in output; the section anchor exists. |
| P-20 | SHOULD emit a not-found page in the site's layout and locale. (new: not guaranteed by today's site) | A `404.html` exists with the site header and `lang`. |
| P-21 | MUST work when served under a configured base path. | Build with a base path; serve under it; every internal link, the graph and redirects resolve. |

### 3.2 Navigation

| ID | Requirement | Acceptance test |
|---|---|---|
| P-22 | MUST navigate between internal pages without a full reload (client-side routing), updating URL and document title. | Browser: click a body link; the `window` marker survives; `location` and `document.title` are the target's. |
| P-23 | MUST restore pages on browser Back/Forward through the router, without reload. | Navigate A→B, `history.back()`: A is shown, marker survives. |
| P-22a | MUST, after each client-side navigation (link, Back, Forward), move focus to the new page's `h1` (`tabindex=-1`) or main region, announce the new page title through a polite live region, and on Back/Forward restore the scroll position the page had when left. | Browser: after a navigation `document.activeElement` is the new `h1` or `main`; a polite live region's text equals the new `document.title`; the window marker survives; repeat after `history.back()`, where `scrollY` also equals its value before leaving. |
| P-24 | SHOULD offer in-page Back and Forward controls that walk the browser's own history, name their destination, and are disabled when there is nothing to go to. | Accessible name contains the destination title; `disabled` at history ends; clicking equals `history.back()`/`forward()`. |
| P-25 | MUST leave exactly one instance of each page-level control after any number of client-side navigations. | After 3 navigations: one explorer tree, one resize handle per panel, one Back/Forward pair, no stray dialogs. |
| P-26 | MUST route links inside inline SVG diagrams like body links (no reload), also from a diagram's dialog, which closes. | Browser: click an SVG `<a>`; marker survives; URL is the target; no open `<dialog>`. |
| P-27 | MUST, for a link to a section: on the same page, scroll it into view without navigation; on another page, navigate without reload and land on it; on direct load with the anchor, land on it. | Browser: section element's top is within the viewport in all three cases. |
| P-28 | MUST mark internal body links to other pages as internal, without `target` or `rel`; same-page anchors and diagram links get no preview behaviour. | DOM: internal links lack `target`/`rel`; hovering a same-page anchor opens no preview. |
| P-29 | SHOULD show breadcrumbs (folder path to the current page) as a navigation landmark with `aria-current` on the last item. (new: not guaranteed by today's site) | Browser: `nav` with accessible name; items link to folder pages; last item has `aria-current=page`. |

### 3.3 Search

| ID | Requirement | Acceptance test |
|---|---|---|
| P-30 | MUST open search with a visible control and with Ctrl+K (Cmd+K), also when the left panel is hidden. | Browser: press Ctrl+K with left panel hidden; the search input is visible and focused. |
| P-31 | MUST index the title, tags and text of every published page, including org pages. | Content index contains each page with title, tags and text; a unique fixture word is found. |
| P-32 | MUST return a known page for a known term and navigate to it by Enter or click, without reload. (new: not guaranteed by today's site) | Browser: type a fixture-unique term; first result is the expected page; Enter navigates; marker survives. |
| P-33 | MUST support tag search (`#tag`). (new: not guaranteed by today's site) | Browser: `#<fixture tag>` lists exactly the tagged pages. |
| P-34 | MUST open folded sections that contain the matched text and scroll to it. | Browser: search a term inside a folded section; after navigation the section is expanded and in view. |
| P-35 | MUST give the search input a 3:1 border and every result a visible 3:1 focus indicator. | Contrast audit scenarios "search open", "result hovered", "result focused" pass in light and dark. |

### 3.4 Backlinks

| ID | Requirement | Acceptance test |
|---|---|---|
| P-36 | MUST list on each page every published page that links to it, including org pages, and nothing unpublished. (new: not guaranteed by today's site, for content) | Build output: backlink list of a fixture page equals the set computed from the bundle's links. |
| P-37 | MUST update backlinks on client-side navigation. | Browser: navigate A→B; the backlink list is B's. |
| P-38 | MUST set backlink text in the body text weight (not bold). | Computed `font-weight` of backlinks equals that of body paragraphs. |

### 3.5 Explorer (file tree)

| ID | Requirement | Acceptance test |
|---|---|---|
| P-39 | MUST mark the current page with `aria-current=page` and a non-colour indicator (bar) at 3:1 against the page; no other entry has either; after navigation, the new page. | Browser, light and dark: exactly one entry with `aria-current`; indicator contrast ≥ 3:1; repeats after a client-side navigation. |
| P-40 | MUST set entries at rest in normal weight, the current page bold, with at least 6 px between consecutive entries. | Computed weights; vertical gap between entry boxes ≥ 6 px. |
| P-41 | MUST wrap long titles to at most two lines, show the current entry and a keyboard-focused entry in full, and give every entry its full title. | Browser: entry heights ≤ 2 line-heights except current/focused; accessible name equals full title. |
| P-42 | MUST clip no entry on touch devices (no reliance on hover tooltips). | Touch profile: every entry's `scrollWidth <= clientWidth`. |
| P-43 | MUST keep the explorer in view while the page scrolls. | Browser: scroll the page 2000 px; the explorer's box stays within the viewport. |
| P-44 | MUST never scroll the window to reveal the current entry; only its own list scrolls; on first load without anchor the window stays at the top. | Browser, all folders open, deep page: `scrollY == 0`; current entry visible inside the list. |
| P-45 | MUST, on navigation, leave the list where it was if the new entry is visible, else scroll the list to it. | Browser: list `scrollTop` unchanged after clicking a visible entry; changed and entry visible otherwise. |
| P-46 | MUST support a configurable descending order per folder, with sub-folders first; other folders keep the default order. | Fixture: configured folder lists newest first with sub-folder first; an unconfigured sub-folder lists oldest first. |
| P-47 | SHOULD put content added after the explorer (e.g. a footer slot) at the bottom of the left panel; with none, the explorer does not move. | Browser: explorer top identical with and without the slot content. |

### 3.6 Link previews and external links

| ID | Requirement | Acceptance test |
|---|---|---|
| P-48 | MUST show a preview of the target page when hovering an internal body link (mouse); for a section link, scrolled to that section. | Browser, mouse: hover; a preview element appears containing the target's title; for a section link its heading is inside the preview's visible area. |
| P-49 | MUST open web links in a new tab without moving the current page, with `rel` that prevents opener access. | Browser: click opens a new page; original URL unchanged; `target=_blank`, `rel` contains `noopener`. |
| P-50 | MUST mark web links with an icon and a visually hidden text saying they open in a new tab, in the configured locale. | Accessible name ends with the locale's "opens in a new tab" string; that text is not visible (clip/size). |
| P-51 | MUST draw the new-tab icon at 3:1 against the page in light and dark, and keep it visible under forced colours. | Contrast ≥ 3:1 both modes; Chromium forced-colours emulation: icon pixels differ from background. |
| P-52 | MUST keep text in previews at WCAG AA, including previews opened from inside each admonition/special-block type. | Contrast audit: one scenario per block type with a preview open; 0 violations. |

### 3.7 Graph

| ID | Requirement | Acceptance test |
|---|---|---|
| P-53 | MUST show a graph of the site in the side panel of each page and offer an expanded full-page view. | Browser: graph present on every content page; an expand control opens the full view. |
| P-54 | MUST place every published page (minus configured exclusions such as tag pages) in the graph, grouped by top-level folder, folders told apart by gaps and labels, not by colour alone. | Graph node count equals published pages minus exclusions; group boundaries present. |
| P-55 | MUST colour by role (all notes, current note and its links, its neighbours, visited band), each role ≥ 3:1 against the page and the visited band ≥ 3:1 against what it sits on, in light and dark. | Computed fills of each role class; contrast checks per mode and per palette. |
| P-56 | MUST lift, name and show the links of a note on hover or focus while keeping the current note's links lit. | Browser: hover a note; its label is visible; its links and the current note's links have the highlight class. |
| P-57 | MUST draw the current page's links above all others, and a hovered note's links above all others while hovered. | DOM order: highlighted link elements come after all others (painted on top). |
| P-57a | MUST change only the hovered (or focused, or search-found) note and its own links; every other link keeps its stroke colour, opacity and width, and is not redrawn, before, during and after. | Browser, both views and modes: for every non-incident link, computed stroke/opacity/stroke-width and element identity are equal before, during and after the hover; incident links do change. |
| P-58 | MUST highlight a note and the link to it from the current page while a body link to that note is hovered; nothing when it is left. | Browser, mouse: hover a body link; exactly that note and those links highlighted; move away; none. |
| P-59 | MUST make the graph one tab stop, with arrow keys, Home and End to move between notes, and Enter to open; hierarchical views also walk folders (Up/Down within a level, Right into, Left out of). | Browser: Tab focuses the graph once; keys move focus as specified; Enter navigates without reload. |
| P-60 | MUST give every note an accessible name and emit no native tooltips (no SVG `<title>`, no `title` attribute) in the graph. | DOM: 0 `title` elements/attributes in the graph; each note has a non-empty `aria-label`. |
| P-61 | MUST open the expanded view over the whole page, opaque, without browser fullscreen and without a history entry. | Browser: element at 25 sampled viewport points belongs to the view; `document.fullscreenElement` null; `history.length` unchanged. |
| P-61a | MUST set the expanded view's camera once when it opens, to its final framing, with no intermediate zoom or pan transforms; the same when it closes. | Browser: record the zoom transform every animation frame for 1.5 s after the expand control is activated; at most one change after the first full-size frame, then constant; equal to the settled framing. Same on close. |
| P-62 | MUST close the expanded view with Escape, and on browser Back return to the previous page through the router, closing the view. | Browser: Escape closes; Back with view open: previous URL, marker survives, no view. |
| P-63 | MUST, in the expanded view, let a search find a note: mark it with a visible label, focus it, colour its links distinctly, and not navigate; a click on it opens its page. | Browser: search + Enter; URL unchanged after 1 s; note has the found marker and visible label; click navigates without reload. |
| P-64 | MUST keep the camera stable: toggling display options (e.g. the "you are here" marker) neither redraws nor reframes; moving in hierarchical views only moves the camera. | Browser: same node element identities and same `viewBox` before and after the toggle. |
| P-65 | SHOULD offer a "you are here" marker, always shown in the side-panel graph, with a toggle in the expanded view (`role=switch`, `aria-checked`) that persists. | Browser: toggle flips `aria-checked` and marker visibility; reload keeps the choice. |
| P-66 | SHOULD mark notes already visited, remembered per browser. | Browser: visit B, return to A; B has the visited band. |
| P-67 | MUST keep the side-panel graph clickable after navigating from a page whose side panel is empty (e.g. tag or folder pages). | Browser: tag page → note page → click a graph note: navigation happens. |
| P-68 | MUST follow the reader's palette and accent choice (§3.12) in the graph. | Browser: change palette; graph role colours equal the new palette's values. |
| P-69 | MUST load nothing from outside the site for the graph. | Request log during graph use: only same-origin requests. |

### 3.8 Tags and folders

| ID | Requirement | Acceptance test |
|---|---|---|
| P-70 | MUST emit a page per tag listing every page with that tag, including org pages. (new: not guaranteed by today's site, for org pages) | Build output: each fixture tag page lists exactly the bundle's tagged nodes. |
| P-71 | MUST emit a tag index listing all tags. | `tags/` index exists and lists every fixture tag. |
| P-72 | MUST emit a listing page per folder with its pages and sub-folders. | Each fixture folder has a listing containing its children. |
| P-73 | MUST take tags only from valid org-mode tags (letters, digits, `_ @ # %`); paths or markup in tag lines are not tags. | Fixture with a malformed tag line: no tag page for it. |
| P-74 | MUST pass the contrast audit on tag and folder listing pages. | Contrast scenarios "tag listing", "folder listing": 0 violations. |

### 3.9 Feeds and indexes

| ID | Requirement | Acceptance test |
|---|---|---|
| P-75 | MUST emit an RSS feed of recent pages, including org pages. (new: not guaranteed by today's site, for org pages) | Feed validates; contains each fixture org page. |
| P-76 | MUST emit a sitemap listing every page URL (no redirects) under the configured base URL. | Sitemap URL set equals the page set. |
| P-77 | MUST emit a content index (title, tags, links, text per page) used by search and graph. | JSON parses; one entry per page; links only to published pages. |
| P-78 | MUST NOT include unpublished IDs or hidden-namespace references in any feed or index. | Grep feeds and index for fixture unpublished IDs: 0 hits. |

### 3.10 Table of contents

| ID | Requirement | Acceptance test |
|---|---|---|
| P-79 | MUST give org pages a table of contents of their headings linking to section anchors. (new: not guaranteed by today's site) | Each fixture page with ≥ 2 headings has a TOC `nav` with one link per heading, each fragment resolving. |
| P-80 | MUST open folded sections when a TOC entry is followed. (new: not guaranteed by today's site) | Browser: click a TOC entry for a folded section; it expands and is in view. |
| P-81 | SHOULD mark the section in view in the TOC with `aria-current`. (new: not guaranteed by today's site) | Browser: scroll to a section; its TOC entry has `aria-current=location`. |

### 3.11 Layout

| ID | Requirement | Acceptance test |
|---|---|---|
| P-82 | MUST show no horizontal page scroll at 320, 390, 1000, 1200 and 1920 px, with panels shown, hidden, or at their maximum stored width. | Browser at each width and state: no horizontal scroll. |
| P-83 | MUST, on wide screens, put the side panels at the viewport edges and the text in a centred column of bounded measure (reference site: 104ch). | Browser 1920 px: panels touch the edges; article width ≤ the configured measure; centred ±2 px. |
| P-84 | MUST align the header row (Back/Forward, slotted controls, graph title) with the panel toggles, without overlap; the second line (site title, page title, graph) at one height. | Browser 1440 px: box tops within 2 px; no two boxes intersect. |
| P-85 | MUST let each side panel be hidden and shown by a control at its top, remembered across pages and applied before paint. | Browser: hide, navigate, reload: hidden state present in the first frame. |
| P-86 | MUST let each panel be resized by a handle: click without drag steps through preset widths (WCAG 2.5.7), arrow keys, Home and End adjust within the limits, drag adjusts. | Browser: click cycles presets; ArrowRight/ArrowLeft change width by a fixed step; Home/End reach the limits; drag changes width. |
| P-87 | MUST cap the left panel at 40 % and the right at 30 % of the viewport, clamping a stored wider width, with the handle stating the current width. | Browser at 801–1920 px with a stored 640 px: width = min(640, 40 %); further ArrowRight changes nothing; handle's value text equals the width. |
| P-88 | MUST draw each panel's inner border at 3:1 against the page. | Light and dark: border contrast ≥ 3:1. |
| P-89 | MUST, on tablet, allow hiding the left panel and move the right panel's content below the text (no right toggle). | Browser 1000 px: left toggle works; no right toggle; graph below the article. |
| P-90 | MUST, on mobile, offer the explorer as a full-width drawer with no resize handle and no panel toggles; a panel hidden at another width is not hidden on mobile. | Browser 390 px with a desktop-hidden state stored: drawer opens full width; no handle or toggles. |

### 3.12 Theming and contrast

| ID | Requirement | Acceptance test |
|---|---|---|
| P-91 | MUST offer light and dark modes, defaulting to the system preference, with the choice persisted and applied before paint. | Browser: `colorScheme` emulation sets the default; toggled mode present in the first frame after reload. |
| P-92 | MUST declare the active palette on the root element and expose its colour tokens exactly as defined. | Root attribute equals the configured palette; each computed token equals the palette definition, both modes. |
| P-93 | SHOULD let the reader choose a palette from a keyboard-operable menu next to the mode toggle; the choice applies at once (page and graph), in both modes, and persists across pages before paint; choosing the site default restores it. | Browser: select via keyboard; tokens change without reload; next page has them in the first frame. |
| P-94 | SHOULD offer accent (emphasis) swatches per palette in a submenu beside it, the palette's default first; palettes without swatches show none. | Browser: submenu items equal the palette's swatch list in order. |
| P-95 | MUST let an accent change only the accent role (accent token, graph accent, "here" marker), never link or notice colours. | Browser: after selecting a swatch, link and notice computed colours are unchanged. |
| P-96 | MUST persist the accent per palette, apply it before paint, clear it on "default", restore it on switching back to that palette, and ignore a stored value not valid for the palette. | Browser: each case checked via reload and storage injection. |
| P-97 | MUST render and remain operable when browser storage throws. | Browser with storage access overridden to throw: no console error; menu works for the session. |
| P-98 | SHOULD map a removed palette stored by a reader to its designated successor and drop its accent. | Inject a removed palette name: the successor is active, no accent attribute. |
| P-99 | MUST make the palette submenu operable by mouse (hover intent; crossing other items does not open them), by click (selected palette opens its submenu; another selects it), by keyboard (→, ←, Escape, Enter, Space) and by touch at 390 px, staying inside the viewport at desktop, 390 and 320 px. | Browser per input mode; submenu box within viewport; no horizontal scroll. |
| P-100 | MUST pass WCAG AA text contrast (4.5:1, 3:1 large) on every audit scenario in light and dark. | Automated WCAG 1.4.3 rule over the scenario set (§4.4): 0 violations. |
| P-101 | MUST give every interactive control's boundary and every focus indicator 3:1 against the page (WCAG 1.4.11, 2.4.7). | Audit: listed control borders and focused outlines ≥ 3:1. |
| P-102 | MUST keep link text ≥ 4.5:1 against the page, and every accent swatch ≥ 4.5:1 against the background, in both modes and every palette. | Computed link colour and each swatch accent vs background ≥ 4.5:1. |
| P-103 | MUST set body links in the surrounding text's weight, without background, underlined. | Computed weight equals paragraph's; `background-color` transparent; `text-decoration-line` includes underline. |
| P-104 | SHOULD support an optional SVG site logo: first in the left column, `role=img` with an accessible name, drawn in the heading colour in every mode and palette, scripts stripped, fitting the column. | Browser: logo first child; `currentColor` fills equal heading colour; no `<script>` in it; width ≤ column. |

### 3.13 Code blocks

| ID | Requirement | Acceptance test |
|---|---|---|
| P-105 | MUST highlight source blocks of supported languages at build time (no client-side highlighter), also inside folded sections. | Output HTML has role-classed spans; disabling JavaScript leaves them; a block in a folded section is highlighted. |
| P-106 | MUST colour each token by its role from the active palette and mode, not by inline styles. | Browser: each role class's computed colour equals the palette token, in two palettes × two modes; no `style` attributes in code. |
| P-107 | MUST keep the block text byte-identical to the source; copying a whole block yields the author's text exactly (tabs, trailing spaces, blank lines, Unicode, CR-LF, markup-like text). | Browser: select a block's contents; `Selection.toString()` and the text of the `copy` event equal the source; build-time comparison per fixture block. |
| P-108 | MUST leave blocks of unsupported or undeclared languages unhighlighted, exact, in the block colour, occupying the same space as highlighted text would; log the languages and pages. Not fatal. | Fixture typo language: plain text; box size equal to a highlighted twin; log line lists it; exit 0. |
| P-109 | SHOULD offer a copy control per block that copies the exact text. (new: not guaranteed by today's site) | Browser (Chromium, clipboard permission): clipboard equals source. |
| P-110 | MUST scroll long lines inside the block, never the page, without wrapping that changes copied text. (new: not guaranteed by today's site) | Browser 390 px: block `scrollWidth > clientWidth`; page has no horizontal scroll; copied text unchanged. |

### 3.14 Diagrams

| ID | Requirement | Acceptance test |
|---|---|---|
| P-111 | MUST render DOT blocks at build time to inline SVG, with no client script and no network. | Output contains `<svg>` for each fixture DOT block; offline check passes. |
| P-112 | MUST name each diagram from its caption (`role=img`, `aria-label`) or a default locale name; diagrams with links are named groups instead. | DOM: `aria-label` equals caption; linked diagram has `role=group`. |
| P-113 | MUST draw default-coloured ink, borders and background with the theme's diagram roles in light and dark, in both browsers. | Computed fill/stroke of default elements equal the role tokens; repeated with roles overridden to distinct test values. |
| P-114 | MUST map author colours that equal a declared palette role to that role, following the theme. | Fixture "role map": each element's computed colour is its role's value in the active palette and mode. |
| P-115 | MUST fail the build when any diagram would fall back to a fixed panel for author colours or lose the theme; the log names the page. | Fixture page with an author colour on the background: exit 1; stderr names the page. |
| P-116 | MUST keep a diagram with a DOT syntax error as code with a visible notice in the locale, without the renderer's message; not fatal. | Fixture broken block: `<pre>` with its source and a notice element; exit 0. |
| P-117 | MUST fail the build when the diagram time budget runs out. | Budget forced to near zero: exit 1. |
| P-118 | MUST self-test diagram rendering before the build and exit with the environment code (3) when a known diagram does not draw. | Renderer disabled in the sandbox: exit 3; nothing published. |
| P-119 | MUST produce deterministic SVG: the same DOT gives the same bytes, also with randomised layouts. | Build twice: identical diagram markup. |
| P-120 | MUST sanitise diagram SVG to an allow-list (no script, `foreignObject`, animation, external URL, author class or native `<title>`); only gradient IDs remain, page-unique. | Grep output SVG for forbidden elements/attributes: 0 hits. |
| P-121 | MUST fit each diagram on the page: no scrollbar, height ≤ 70 % of the viewport, never enlarged beyond natural size. | Browser: diagram box ≤ column width, ≤ 0.7 × `innerHeight`, ≤ natural size. |
| P-122 | MUST open a diagram by click, Enter or Space on its frame (or a separate expand button for linked diagrams) into a modal dialog at natural size, with Fit, 100 % and Close controls named as shown; the active mode is marked. | Browser: dialog open; buttons' accessible names equal visible text; active one `aria-pressed=true`. |
| P-123 | MUST make Fit enlarge a small diagram and shrink a tall one to the dialog. | Browser: after Fit, diagram box fills the dialog's limiting dimension ±2 px. |
| P-124 | MUST close the dialog with Escape, Close or a backdrop click (not a drag), returning the diagram to its place and focus to its opener. | Browser: each way closes; diagram back in the article; `activeElement` is the opener. |
| P-125 | MUST move, not clone, the diagram into the dialog (IDs stay unique); on print or client-side navigation with the dialog open, the diagram returns and no dialog remains. | Browser: duplicate-ID check with dialog open; `print` media emulation and navigation leave 0 open dialogs and the SVG in the article. |
| P-126 | MUST make diagram links focusable, without underline, with a thicker outline on hover and focus and a focus ring. | Browser: link `tabIndex` reachable; computed stroke width increases on hover/focus; no `text-decoration`. |

### 3.15 Long pages

| ID | Requirement | Acceptance test |
|---|---|---|
| P-127 | MUST make each section foldable by a button at its heading (`aria-expanded`, `aria-controls`, named by the heading). | Browser: button per heading with a body; toggling flips `aria-expanded` and visibility. |
| P-128 | MUST open long pages outline-first (≥ 4 foldable sections and ≥ 3000 characters: top level folded; a single top section opens one level down); author-marked folded sections stay folded; others open. | Fixture long page: exactly the expected sections folded on load. |
| P-129 | MUST offer one expand-all/collapse-all control, named for what it will do, with `aria-expanded`, and remember the state for the next page. | Browser: label switches; next page opens as left. |
| P-130 | MUST open folded sections containing a requested fragment (on load, client-side navigation, `hashchange`, Back/Forward, same-page links, footnotes) and scroll to it. | Browser: `#fragment` inside a folded section: section expanded, target in view. |
| P-131 | SHOULD let the browser's find-in-page reveal folded text (`hidden=until-found`), the button reflecting it. | DOM: folded bodies carry `hidden="until-found"` (find-in-page itself is checked by hand). |
| P-132 | MUST show everything without controls when printed and when JavaScript is off. | Print media emulation: no folded content hidden, no fold buttons visible; same with JS disabled. |
| P-133 | MUST scroll a wide table inside its own container, never the page; cells of text at least 14ch wide, short unbroken cells (≤ 24 chars) exempt; header sticky when the table is taller than 80 % of the viewport. | Browser 390 px and 1200 px: container scrolls; page does not; header `position: sticky` stays at top when scrolled. |
| P-134 | MUST offer a full-screen control per table that opens it in a modal dialog; Escape returns it with focus on the control; it returns before print or navigation. | Browser: dialog contains the table; after Escape, table in article, focus on control. |

### 3.16 Unique element IDs and offline output

| ID | Requirement | Acceptance test |
|---|---|---|
| P-135 | MUST emit no duplicate `id` on any page, including pages with explorer, backlinks, TOC, graph and diagrams. | For every built page and after opening each dialog: duplicate `id` count = 0. |
| P-136 | MUST derive element IDs from position or published node IDs only, never from author text (heading text, targets, custom IDs). | Grep the output for fixture heading words inside `id` attributes: 0 hits. |
| P-137 | MUST reference no third-party origin from any HTML page or any stylesheet or script a page loads (fonts included). | Static scan of all loaded assets for absolute URLs to other hosts: 0. |
| P-138 | MUST work fully with no network: every page loads and every behaviour in this spec passes with outbound requests blocked. | The browser suite runs with non-site requests aborted; 0 console errors. |
| P-139 | MUST contain no absolute filesystem path of the build host and no build-environment details. | Grep output for the build directory and user/home path patterns: 0 hits. |
| P-140 | MUST be a self-contained directory, servable read-only from any path, so that swapping directories switches the site. | Serve the output from a read-only copy at a different path: suite passes. |

### 3.17 Build hygiene

| ID | Requirement | Acceptance test |
|---|---|---|
| P-141 | MUST exit 0 only when the output holds exactly the site (and the address manifest); 1 when the content or engine fails; 2 when the input breaks the contract; 3 when the environment cannot render (retryable). | Each negative fixture below yields its code; the clean fixture yields 0 and exactly the expected top-level entries. |
| P-142 | MUST refuse a bundle containing any symbolic link (exit 2), without following it. | Bundle with a link to a system file: exit 2; the target's content is absent. |
| P-143 | MUST refuse a non-empty output directory (exit 2). | Second run into the same output: exit 2. |
| P-144 | MUST NOT write outside the output directory, including through aliases or permalinks with `..`; such an attempt fails (exit 1). | Fixture with an escaping alias: exit 1; nothing beside the site directory. |
| P-145 | MUST fail (exit 1) when any extension, plugin or component fails to load or start, or any emitter fails. | Remove or break one extension module: exit 1. |
| P-146 | MUST fail (exit 1) on any log line outside the documented allow-list of clean-build lines. | Inject an unknown warning: exit 1; the line is echoed to stderr. |
| P-147 | MUST fail (exit 1) when a node is withheld: no file ID, emptied by macros, macro call (while macros are unsupported), undefined footnote, duplicate ID. | One fixture per reason: exit 1; summary line names the count. |
| P-148 | MUST print one summary line per build with counts (published, withheld by reason, links resolved/unresolved, diagrams drawn/as code, code blocks highlighted/plain). | Clean build log matches the documented line format. |
| P-149 | MUST build with no network, a read-only root filesystem and writable space only in temporary storage and the output. | Run in a sandbox with networking off and root read-only: clean fixture exits 0. |
| P-150 | MUST NOT stop a publish for non-content metadata faults (malformed date, missing what's-new target, invalid version data, invalid document state); each yields a log line and the feature is left out or a notice shown. | One fixture per case: exit 0; documented log line present. |
| P-151 | MUST pass all page HTML through a final allow-list sanitiser (elements, attributes per element, URL protocols). | Hostile fixture (script, event handlers, `javascript:` URLs, raw HTML blocks): none survive. |
| P-152 | SHOULD produce byte-identical output from the same bundle and configuration, except declared build timestamps. (new: not guaranteed by today's site) | Build twice; diff is empty or limited to the declared fields. |

### 3.18 Component-hook contract

The engine lets a site add its own components at named slots. The examples below are the
components the reference site uses; the contract must support each of them without changes
to the engine.

| ID | Requirement | Acceptance test |
|---|---|---|
| P-153 | MUST provide at least these slots: document head, header row (after Back/Forward), before the body (after the title and its metadata, above the article), after the body, left-panel footer, right panel, page footer. | A test component per slot renders at the documented position. |
| P-154 | MUST pass each component the page's node data: ID, title, file-level properties, dates, tags, URL, and site settings (locale, base path, KB identifier). | Test component echoes each field into a data attribute; values match the bundle and configuration. |
| P-155 | MUST pass component output through the same sanitiser as page content. | A component emitting a script tag or event handler: stripped in output. |
| P-156 | MUST re-run component client scripts exactly once after each client-side navigation and let them clean up before it. | Browser: counter incremented once per navigation; no duplicated elements. |
| P-157 | MUST fail the build when a configured component fails to load or throws during render. | Broken component module: exit 1. |
| P-158 | MUST keep layout identical when a slot is empty: removing a component moves nothing else. | Browser: built-in element boxes equal with and without the slotted components. |
| P-159 | MUST keep header-row components aligned with and not overlapping built-in controls; at 390 px each wraps to its own line with no horizontal scroll. | Browser 1000 px and 390 px: no box intersections; no horizontal scroll. |
| P-160 | MUST let a component emit an additional page from validated build-time data. | Example: a site-changes page listing versions newest first with dates. |
| P-161 | MUST support, as examples: a document-state badge (from a node property, above the article after the title, absent when the property is absent, its own notice when invalid). | Fixture draft page: badge in position; page without the property: none. |
| P-162 | MUST support, as example: a copy-link control on each page and on each addressable section heading, copying origin + base path + `/<kb-id>/<id>`, current after navigation, announcing success, with a manual fallback when no clipboard is available. | Chromium, secure origin, clipboard permission: clipboard holds the expected URL for page and section, also after navigation. |
| P-163 | MUST support, as examples: a "what's new" link to a configured node (omitted if unpublished), a version footer at the bottom of the left panel linking to the site-changes page (hidden on mobile), and a last-updated line from a date property, with a machine-readable `<time datetime>`, omitted when malformed. | Fixture: each present and positioned; mobile 390 px: footer hidden; malformed date page: no line. |

## 4. Parity procedure

### 4.1 Fixtures

- A **public synthetic fixture bundle**, kept with this spec, exercising every requirement:
  nested folders, a configured descending folder, deep pages, long titles, a long page
  (≥ 4 top sections, > 3000 characters, an author-folded section, a target inside a folded
  section), a 7-column wide table, a link to an absent ID, hidden and shown namespace
  references, web links and a remote image, tags (valid and malformed), source blocks in
  every supported language plus a typo language and an undeclared block, DOT diagrams
  (default colours, author fill under text, role map, linked nodes, tall, small, broken),
  section nodes, a route-history entry, a malformed date, a draft-state page, version data.
- **Negative fixtures**, one per failure in §3.17, each with its expected exit code.
- A site may also compare on a private published bundle of its own; those results stay with
  the site.

### 4.2 Running both sites

1. Build the reference and candidate engines from the **same published bundle and the same
   site configuration** (locale, base path, KB identifier, palette, slots), each in the build
   sandbox of P-149, each into its own empty output directory.
2. Serve each output read-only, on separate origins, with all other origins blocked.
3. Run the browser suite against both, in Chromium and Firefox, at the widths of P-82, with
   both pointer profiles where a requirement depends on input.

### 4.3 Comparing output

- **Addresses**: the set of URLs that resolve (pages and redirects) must be equal, or the
  candidate's a superset where the extra pages are "new" requirements. Every URL in the
  reference's address manifest and route history must resolve on the candidate to the same
  node.
- **Pages**: compare per page a normalised view of the article, not raw markup: title,
  heading outline, text content (whitespace-normalised), link targets (resolved to node IDs),
  anchors, unresolved placeholders, tags, diagram accessible names and default-role use,
  code-block text and language.
- **Feeds and indexes**: same entries by node ID.
- **Every difference** is either a defect, or an accepted change recorded in a parity ledger
  (requirement ID, page, reason, who accepted). An unclassified difference fails parity.

### 4.4 Contrast audit

- Run the automated text-contrast rule plus the 3:1 non-text checks (control borders, focus
  rings) on both sites, light and dark, over a scenario set covering at least: home page,
  folder listing, tag listing, a note, a diagram page, a code page, search open, a hovered
  result, a focused result, tag search, a link preview opened from each admonition/special-
  block type, the expanded graph, its open and close controls hovered and focused, the
  explorer with a folder open, and an unresolved link. The reference audit has 52 scenarios;
  the candidate's must cover at least the same states.
- The audit itself is self-tested against pages with known verdicts (text over siblings,
  over SVG shapes, inside scaled or inner scrollers, clipped, covered, pseudo-elements).

### 4.5 Test harness rules

- Each check exits 1 when it fails, 2 when it could not check (no browser, no site, nothing
  selected); never 0 with no checks run.
- Each check has declared mutants (a change to the candidate that must make it fail),
  committed with it and run by the repository's mutation runner.
- No fixed sleeps: wait until no request is in flight and nothing visible has changed for
  several frames. A check that fails only intermittently is a defect, fixed at its cause.

### 4.6 Retirement criteria

The reference engine may be retired only when, on the candidate:

1. every MUST passes in Chromium and Firefox at every width, three consecutive runs;
2. every requirement that passes on the reference also passes on the candidate;
3. the address comparison shows no reference URL that fails to resolve;
4. the page comparison has no unclassified difference;
5. the contrast audit reports 0 violations in both modes;
6. every negative fixture yields its expected exit code, and the clean build passes in the
   sandbox of P-149 with the offline checks P-137 and P-138;
7. all declared mutants are caught;
8. every performance target in §5 is met, or explicitly waived by the site's owner.

## 5. Performance

Targets are set from measurements of the reference site on the fixture and on a
representative bundle; until then each is a placeholder. Measure each with at least three
runs and report the median and the worst.

| Metric | Measured on | Target |
|---|---|---|
| Full build time | fixture bundle; representative bundle | TARGET: tbd from measurements |
| Incremental build time (one page changed) | representative bundle | TARGET: tbd from measurements |
| Output size (total, and largest page) | representative bundle | TARGET: tbd from measurements |
| Page load (first contentful paint, load event), cold and client-side navigation | headless browser, mobile and desktop profiles | TARGET: tbd from measurements |
| Search latency (keystroke to results shown) | representative bundle | TARGET: tbd from measurements |
| Graph open time (expanded view ready for input) | representative bundle | TARGET: tbd from measurements |
