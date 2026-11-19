// SPDX-License-Identifier: GPL-3.0-or-later
// Client-side drawing for the chord graph. All layout comes from
// ../geometry (unit-tested); this file only turns it into SVG and wires up
// interaction. Text goes through textContent and setAttribute only, never
// innerHTML, so note titles cannot inject markup.
import { chooseNames, NAMES_KEY } from "./names";
import {
  buildRing,
  chordPath,
  neighbours,
  normaliseSlug,
  polar,
  wedgeCorner,
  wedgePath,
  wedgeThickness,
  VISITED_BAND,
  type Ring,
} from "../geometry";
import { chooseLayout, chooseOrientation, resolveLayouts, type LayoutName } from "../layouts";
import type { Orientation } from "../sunburst";
import { drawHierarchical } from "./hier";
import { FIND_EVENT, mountFind } from "./find";
import { collectNotes } from "../hierarchy";
import { sectorBox } from "../scene";
import { FIT_FLOOR, fitWith, union, viewBox, type Box } from "../viewport";
import { overlayBoxes } from "./controls";
import { hereFloor, HERE_KEY, reserve, showHere, type HereOptions } from "../here";
import { mountHere } from "./here";
import { handleOf, peekAll, register } from "./views";
import { slugOfHref } from "../link";
import { raise } from "./order";
import { onCleanup } from "./runtime";
import type { GraphData } from "../types";

const SVG_NS = "http://www.w3.org/2000/svg";
const VIEWBOX = 200; // the ring is drawn in a 200 × 200 box centred on 0,0
const OUTER = 92; // leaves room for a hovered wedge to grow outward
// Growth is a fraction of the wedge's thickness, so it is the same on screen in
// the sidebar and in the dialog (0.3 for the hovered note, as an earlier graph export had it).
const LIFT_HOVER = 0.3;
const LIFT_NEIGHBOUR = 0.15;
// Wedge thickness on screen: 24px in the dialog, 16px in the narrow sidebar,
// capped at 16 viewBox units (8% of the box): below a 200px drawing the wedge
// is thinner than 16px, so the ring never fills in.
const THICKNESS_PX = { dialog: 24, sidebar: 16 };
const VISITED_KEY = "chord-graph-visited";
// The reader's choice of view, kept across pages and visits.
const LAYOUT_KEY = "chord-graph-layout";
const ORIENTATION_KEY = "chord-graph-orientation";

interface Options {
  layout?: string;
  layouts?: string[];
  orientation?: string;
  orientations?: string[];
  /** «You are here», already checked when the graph was mounted (mount.ts, resolveHere). */
  here?: Partial<HereOptions>;
  excludePrefixes: string[];
  chordPull: number;
  markVisited: boolean;
  groupGap: number;
  denseFrom: number;
}

function svgEl<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string> = {}) {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
  return el;
}

function readVisited(): Set<string> {
  try {
    return new Set(JSON.parse(localStorage.getItem(VISITED_KEY) ?? "[]") as string[]);
  } catch {
    return new Set();
  }
}

export function markVisited(slug: string) {
  try {
    const visited = readVisited();
    visited.add(slug);
    localStorage.setItem(VISITED_KEY, JSON.stringify([...visited]));
  } catch {
    // Storage may be unavailable (private mode, quota): the marker is a nicety.
  }
}

// The reader's choices (view, sunburst orientation). Unavailable storage
// (private mode, quota): a choice lasts for this page only.
function readChoice(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function storeChoice(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // See readChoice.
  }
}

let idCounter = 0;
// The scale each view's ring was last framed at, for its box size and marker
// (shown or hidden). Per box size and marker: a box that alternates between two sizes (a scrollbar
// coming and going) finds both and is not drawn again and again.
const ringScale = new WeakMap<HTMLElement, Map<string, number>>();
// One AbortController per drawn view and per container: a redraw (opening the
// dialog again, navigation) cancels the previous listeners instead of stacking.
const listeners = new WeakMap<HTMLElement, AbortController>();

/**
 * What the engine gives a mounted graph (mount.ts, mountChordGraph), per
 * container: the data to draw, the site's base path, and how to open a page in place.
 */
export interface GraphContext {
  data: Promise<GraphData>;
  /** "" or "/something": the path the site is served under. */
  basePath: string;
  /** Opens a page in place; absent, a click on a note is the browser's. */
  navigate?: (url: URL) => void;
}
const contexts = new WeakMap<HTMLElement, GraphContext>();

/** `container` draws with `ctx` from now on. */
export function attach(container: HTMLElement, ctx: GraphContext): void {
  contexts.set(container, ctx);
}

/** `container` draws nothing more: a draw still waiting for its data stops there. */
export function detach(container: HTMLElement): void {
  contexts.delete(container);
}

// Draws one view (the sidebar box or the dialog) of a chord-graph container.
async function drawView(view: HTMLElement, container: HTMLElement, current: string) {
  const opts = JSON.parse(container.dataset.cfg ?? "{}") as Options;
  const svg = view.querySelector<SVGSVGElement>(".chord-graph-svg");
  const caption = view.querySelector<HTMLElement>(".chord-graph-caption");
  const hint = container.querySelector<HTMLElement>(".chord-graph-hint");
  const ctx = contexts.get(container);
  if (!svg || !caption || !ctx) return;
  listeners.get(view)?.abort();
  const ac = new AbortController();
  listeners.set(view, ac);
  const signal = ac.signal;
  onCleanup(view, () => ac.abort());

  if (hint) {
    hint.id ||= `chord-graph-hint-${++idCounter}`;
    svg.setAttribute("aria-describedby", hint.id);
  }

  const data = await ctx.data;
  // Redrawn (or the page left) while the data was on its way: this draw is stale.
  if (signal.aborted) return;
  const prefixes = opts.excludePrefixes ?? [];
  const exclude = (s: string) =>
    prefixes.some((p) => s === p.replace(/\/$/, "") || s.startsWith(p));
  const locale = document.documentElement.lang || undefined;

  // Which view: the reader's choice if the site offers it, else the site's.
  const layouts = resolveLayouts(opts);
  const layout = chooseLayout(readChoice(LAYOUT_KEY), layouts);
  const orientation = chooseOrientation(readChoice(ORIENTATION_KEY), layouts);
  // Both views (sidebar and dialog) follow a choice of the reader at once.
  const redrawAll = () => {
    for (const v of container.querySelectorAll<HTMLElement>(".chord-graph-view")) {
      void drawView(v, container, current);
    }
  };
  for (const b of view.querySelectorAll<HTMLButtonElement>(".chord-graph-layouts button")) {
    b.setAttribute("aria-pressed", String(b.dataset.layout === layout));
    b.addEventListener(
      "click",
      () => {
        const chosen = b.dataset.layout as LayoutName;
        if (chosen === layout) return;
        storeChoice(LAYOUT_KEY, chosen);
        redrawAll();
      },
      { signal },
    );
  }
  // Which drawing this view holds: for styles and tests, which otherwise had to
  // guess it from the switch (absent on a site that offers one view).
  view.dataset.layout = layout;
  // Which pages the expanded view names (names.ts); read by hier.ts.
  const names = chooseNames(readChoice(NAMES_KEY));
  view.dataset.names = names;
  for (const b of view.querySelectorAll<HTMLButtonElement>(".chord-graph-names button")) {
    b.setAttribute("aria-pressed", String(b.dataset.names === names));
    b.addEventListener(
      "click",
      () => {
        if (b.dataset.names === names) return;
        storeChoice(NAMES_KEY, b.dataset.names ?? "");
        redrawAll();
      },
      { signal },
    );
  }
  // «Names» only changes something in the hierarchical views: the chords draw no names
  // at rest (they show on hover), so it is not offered there.
  // «You are here» (../here.ts): the pin, unless the site set hereMarker "none".
  // With hereToggle the reader shows or hides it in the dialog, and the choice is
  // kept like the view. Like «Names», the choice is the dialog's: the sidebar box
  // always marks the current page (it has no switch to bring the pin back).
  // The switch changes the drawn view in place (views.ts, setHere): no redraw, so
  // no new framing and no camera move (owner, 2026-11-17: a redraw sent the graph
  // off screen and back). A view not drawn yet is drawn with the new choice.
  let hereShown = showHere(readChoice(HERE_KEY), opts.here, view.closest("dialog") !== null);
  view.dataset.here = hereShown ? "shown" : "hidden";
  for (const b of view.querySelectorAll<HTMLButtonElement>(".chord-graph-here button")) {
    b.setAttribute("aria-checked", String(hereShown));
    b.addEventListener(
      "click",
      () => {
        const next = view.dataset.here !== "shown";
        storeChoice(HERE_KEY, next ? "shown" : "hidden");
        view.dataset.here = next ? "shown" : "hidden";
        b.setAttribute("aria-checked", String(next));
        const drawn = handleOf(view);
        if (drawn) drawn.setHere(next);
        else void drawView(view, container, current);
      },
      { signal },
    );
  }
  const namesBox = view.querySelector<HTMLElement>(".chord-graph-names");
  if (namesBox) namesBox.hidden = layout === "chord";
  const zoomBox = view.querySelector<HTMLElement>(".chord-graph-zoom");
  if (zoomBox) zoomBox.hidden = layout === "chord";
  const flip = view.querySelector<HTMLButtonElement>(".chord-graph-flip");
  if (flip) {
    flip.hidden = layout !== "sunburst";
    flip.setAttribute("aria-pressed", String(orientation === "leaves-inside"));
    flip.addEventListener(
      "click",
      () => {
        const next: Orientation = orientation === "leaves-inside" ? "top-inside" : "leaves-inside";
        storeChoice(ORIENTATION_KEY, next);
        redrawAll();
      },
      { signal },
    );
  }
  const hierHint = container.querySelector<HTMLElement>(".chord-graph-hint-hier");
  if (layout !== "chord") {
    if (hierHint) {
      hierHint.id ||= `chord-graph-hint-${++idCounter}`;
      svg.setAttribute("aria-describedby", hierHint.id);
    }
    drawHierarchical({
      view,
      container,
      svg,
      caption,
      data,
      current,
      layout,
      orientation,
      exclude,
      locale,
      visited: opts.markVisited ? readVisited() : new Set<string>(),
      denseFrom: opts.denseFrom ?? 150,
      hereShown,
      hereLabel: container.dataset.labelHere ?? "",
      basePath: ctx.basePath,
      navigate: ctx.navigate,
      signal,
    });
    return;
  }
  // Back to the ring: what the hierarchical views set on the shared SVG goes.
  view.classList.remove("hier");
  svg.style.removeProperty("--k");
  // The ring: every note of the site, one ring, all its links (the subgraph
  // folding of 0.2.3 is gone, owner 2026-11-06: the ring shows linkage; the
  // hierarchical views are for moving around).
  const ring: Ring = buildRing(data, { exclude, locale, groupGap: opts.groupGap ?? 2 });
  // The dialog is large enough for folder labels around the ring; the
  // sidebar box is not, so it keeps the ring at full size instead.
  const large = view.closest("dialog") !== null;
  view.classList.toggle("dense", ring.nodes.length >= (opts.denseFrom ?? 150));

  svg.replaceChildren();
  svg.setAttribute("viewBox", `${-VIEWBOX / 2} ${-VIEWBOX / 2} ${VIEWBOX} ${VIEWBOX}`);
  const currentNode = ring.nodes.find((n) => n.slug === current);
  const here = mountHere({
    svg,
    shown: hereShown,
    title: currentNode?.title ?? "",
    label: container.dataset.labelHere ?? "",
    large: view.closest("dialog") !== null,
    signal,
  });
  if (ring.nodes.length === 0) {
    caption.textContent = container.dataset.labelEmpty ?? "";
    return;
  }

  // The wedge's thickness is set in screen px at the scale the ring is framed at.
  // With room for the «You are here» label that scale is smaller than the box's
  // (fitRing): the scale found last time for this box, marker or none, if any; when it
  // turns out different, the ring is drawn once more at it.
  const frame0 = svg.getBoundingClientRect();
  const rendered = frame0.width || 300;
  const drawKey = `${frame0.width}x${frame0.height}|${hereShown}`;
  const scaled = ringScale.get(view)?.get(drawKey);
  const hinted = scaled !== undefined ? scaled * VIEWBOX : null;
  const drawnAt = hinted ?? rendered;
  const thickness = wedgeThickness(
    drawnAt,
    VIEWBOX,
    large ? THICKNESS_PX.dialog : THICKNESS_PX.sidebar,
    16,
  );
  const growth = LIFT_HOVER * thickness;
  // Labels need room outside the ring: up to ~20 units of text either side.
  const outer = large ? OUTER - 24 : OUTER;
  const rest = outer - growth; // a wedge's outer edge at rest
  const inner = rest - thickness;
  const chordR = inner - 0.5;
  const corner = wedgeCorner(thickness);
  const byslug = new Map(ring.nodes.map((n) => [n.slug, n]));
  const visited = opts.markVisited ? readVisited() : new Set<string>();
  const base = ctx.basePath;

  const edgeLayer = svgEl("g", { class: "chord-graph-edges" });
  const nodeLayer = svgEl("g", { class: "chord-graph-nodes" });
  svg.append(edgeLayer, nodeLayer);

  if (large) {
    // Folder arcs and labels, for folders big enough to label legibly.
    const labelLayer = svgEl("g", { class: "chord-graph-groups", "aria-hidden": "true" });
    svg.append(labelLayer);
    const minSize = Math.max(3, Math.ceil(ring.nodes.length * 0.015));
    for (const g of ring.groups) {
      labelLayer.append(
        svgEl("path", {
          class: "chord-graph-group-arc",
          d: wedgePath(0, 0, outer + 1, outer + 1.6, g.start, g.end, 0),
        }),
      );
    }
    // Largest folders are labelled first; a label too close to one already
    // placed is skipped rather than drawn on top of it. Its arc still shows,
    // and hovering its notes still names them.
    const placed: Array<[number, number]> = [];
    const minGapY = 4.5; // a line of label text, in viewBox units
    for (const g of [...ring.groups].sort((a, b) => b.size - a.size)) {
      if (g.size < minSize) continue;
      const mid = (g.start + g.end) / 2;
      const [x, y] = polar(0, 0, outer + 4, mid);
      const side = Math.cos(mid) >= 0 ? 1 : -1;
      if (placed.some(([px, py]) => Math.sign(px) === side && Math.abs(py - y) < minGapY)) continue;
      placed.push([x, y]);
      const text = svgEl("text", {
        class: "chord-graph-group-label",
        x: x.toFixed(2),
        y: y.toFixed(2),
        "text-anchor": side > 0 ? "start" : "end",
        "dominant-baseline": "middle",
      });
      text.textContent =
        g.name === "" ? (container.dataset.labelRoot ?? "") : g.name.replace(/[-_]+/g, " ");
      labelLayer.append(text);
    }
  }

  const edgesBySlug = new Map<string, SVGPathElement[]>();
  for (const e of ring.edges) {
    const s = byslug.get(e.source)!;
    const t = byslug.get(e.target)!;
    const path = svgEl("path", {
      class: "chord-graph-edge",
      d: chordPath(0, 0, chordR, s.angle, t.angle, opts.chordPull),
    });
    edgeLayer.append(path);
    for (const slug of [e.source, e.target]) {
      if (!edgesBySlug.has(slug)) edgesBySlug.set(slug, []);
      edgesBySlug.get(slug)!.push(path);
    }
  }

  // The current note's context is standing state: its chords in the accent and
  // its neighbours lifted a little, whatever the pointer does.
  const currentNeighbours = byslug.has(current) ? neighbours(ring, current) : new Set<string>();
  for (const path of edgesBySlug.get(current) ?? []) path.classList.add("current");
  const half = ring.slot / 2;
  const shapeD = (slug: string, lift: number) => {
    const n = byslug.get(slug)!;
    return wedgePath(0, 0, inner, rest + lift, n.angle - half, n.angle + half, corner);
  };
  const restingD = (slug: string) =>
    shapeD(slug, currentNeighbours.has(slug) ? LIFT_NEIGHBOUR * thickness : 0);

  const wedges: SVGAElement[] = [];
  const shapes = new Map<string, SVGPathElement>();
  for (const n of ring.nodes) {
    const link = svgEl("a", {
      href: `${base}/${n.href}`,
      class: "chord-graph-wedge",
      "data-slug": n.slug,
      tabindex: "-1",
      "aria-label": n.title,
    });
    if (n.slug === current) {
      link.classList.add("current");
      link.setAttribute("aria-current", "page");
    }
    if (currentNeighbours.has(n.slug)) link.classList.add("neighbour");
    // No <title>: the browser showed it as its own tooltip over the name the graph
    // draws (owner, 2026-11-17). The accessible name is the link's aria-label.
    const shape = svgEl("path", { class: "chord-graph-shape", d: restingD(n.slug) });
    link.append(shape);
    if (visited.has(n.slug) && n.slug !== current) {
      // Inner band: rounded on the inside exactly like its wedge (wedgeCorner
      // keeps the corner within what the band can carry), square outside.
      link.classList.add("visited");
      link.append(
        svgEl("path", {
          class: "chord-graph-visited",
          d: wedgePath(
            0,
            0,
            inner,
            inner + thickness * VISITED_BAND,
            n.angle - half,
            n.angle + half,
            corner,
            0,
          ),
        }),
      );
    }
    nodeLayer.append(link);
    wedges.push(link);
    shapes.set(n.slug, shape);
  }

  const restingIndex = Math.max(
    0,
    ring.nodes.findIndex((n) => n.slug === current),
  );
  wedges[restingIndex]?.setAttribute("tabindex", "0");

  // ---- framing: nothing drawn under a control (controls.ts) ----
  // The same rule as the hierarchical views (viewport.ts, fitClear): the ring's
  // design box (VIEWBOX, with its margin) and, in the dialog, the folder
  // names that reach outside it; and as parts, each wedge grown to its largest (on
  // hover it reaches `outer`), each folder arc and each name. A
  // control that covers no part (the expand button, in the empty corner of a
  // ring's box) does not shrink the ring; one that does moves it aside.
  // Each wedge and each arc, in pieces of PIECE at most: the box of a wide wedge
  // (few notes: seven in a small test KB, 51° each) reaches the empty corner, and
  // the ring shrank for a button that covered nothing of it; that of a 2° piece
  // barely leaves the ring (its corner, 2 % of the radius at most). The ring's angles
  // (geometry.ts) start on the x axis; sectorBox's, at the top: + π/2.
  const PIECE = Math.PI / 90;
  const q = Math.PI / 2;
  const pieces = (r0: number, r1: number, a0: number, a1: number): Box[] => {
    const n = Math.max(1, Math.ceil((a1 - a0) / PIECE));
    return Array.from({ length: n }, (_, i) =>
      sectorBox(r0, r1, a0 + ((a1 - a0) * i) / n + q, a0 + ((a1 - a0) * (i + 1)) / n + q),
    );
  };
  const ringParts: Box[] = ring.nodes.flatMap((n) =>
    pieces(inner, outer, n.angle - half, n.angle + half),
  );
  if (large)
    for (const g of ring.groups) ringParts.push(...pieces(outer + 1, outer + 1.6, g.start, g.end));
  const labelBoxes = (): Box[] =>
    [...svg.querySelectorAll<SVGTextElement>(".chord-graph-group-label")].map((t) => {
      const b = t.getBBox();
      return { x: b.x, y: b.y, w: b.width, h: b.height };
    });
  let sized = "";
  let firstFit = true;
  const fitRing = () => {
    const r = svg.getBoundingClientRect();
    // Hidden (the sidebar collapsed): with no size there is no framing; the
    // observer does it as soon as it has one.
    if (r.width <= 0 || r.height <= 0) return;
    sized = `${r.width}x${r.height}`;
    const labels = labelBoxes();
    const design = { x: -VIEWBOX / 2, y: -VIEWBOX / 2, w: VIEWBOX, h: VIEWBOX };
    const box = union([design, ...labels])!;
    // With room beside the current note for its «You are here» label (here.ts,
    // reserve): the ring gives up a little of its size (FIT_FLOOR at most).
    const v = fitWith(
      box,
      (k) =>
        currentNode && hereShown
          ? [reserve(polar(0, 0, rest, currentNode.angle), [0, 0], here.size, k)]
          : [],
      r.width,
      r.height,
      0,
      large ? FIT_FLOOR : hereFloor(r.width),
      { controls: overlayBoxes(svg), parts: [...ringParts, ...labels] },
    );
    svg.setAttribute("viewBox", viewBox(v, r.width, r.height));
    if (!ringScale.has(view)) ringScale.set(view, new Map());
    ringScale.get(view)!.set(`${sized}|${hereShown}`, v.k);
    const k0 = drawnAt / VIEWBOX;
    // Only on this draw's first framing: a later one (the box resized) keeps the
    // wedges as drawn. Redrawing then could loop: a redraw resets the caption, the
    // caption's height can bring a page scrollbar and back, and with it the box.
    if (firstFit && hinted === null && Math.abs(v.k - k0) > 0.02 * k0) {
      firstFit = false;
      // Drawn for another scale: once more, at this one (then hinted).
      void drawView(view, container, current);
      return;
    }
    firstFit = false;
    // «You are here»: on the current wedge's outer edge; the inside of the ring
    // (the links) counts as taken.
    if (currentNode) here.update(polar(0, 0, rest, currentNode.angle), [[0, 0, inner]]);
    else here.clear();
  };
  fitRing();
  // The switch: the marker only; the ring stays framed as it is.
  register(
    view,
    {
      setHere(shown) {
        if (shown === hereShown) return;
        hereShown = shown;
        here.setShown(shown);
        if (shown && currentNode)
          here.update(polar(0, 0, rest, currentNode.angle), [[0, 0, inner]]);
      },
      // A link of the page body under the pointer (owner, 2026-11-17): its note
      // lifted and marked, the current page's chord to it in the accent, heavier,
      // on top.
      peek(slug) {
        const next = slug && slug !== current && byslug.has(slug) ? slug : null;
        if (next === peeked) return;
        const previous = peeked;
        if (previous) {
          nodeLayer
            .querySelector(`[data-slug="${CSS.escape(previous)}"]`)
            ?.classList.remove("peek");
          for (const p of peekChords(previous)) p.classList.remove("peek");
        }
        peeked = next;
        if (previous) settle(previous);
        if (next) {
          nodeLayer.querySelector(`[data-slug="${CSS.escape(next)}"]`)?.classList.add("peek");
          for (const p of peekChords(next)) p.classList.add("peek");
          settle(next);
        }
        restack();
      },
    },
    signal,
  );
  // A frame of another size: the controls do not shrink with it, so it is framed again.
  const resize = new ResizeObserver(() => {
    const r = svg.getBoundingClientRect();
    if (`${r.width}x${r.height}` !== sized) fitRing();
  });
  resize.observe(svg);
  signal.addEventListener("abort", () => resize.disconnect());

  // Two transient layers on top of the standing state, both shown by a lift
  // (and a shadow, in the stylesheet). The hovered note: its chords show in ink
  // and its title in the caption. The keyboard-focused note: it stays lifted
  // while the pointer is elsewhere, because the lift is the focus indicator
  // that shows on every fill (a focus ring cannot contrast with all of them).
  // Only keyboard focus counts (:focus-visible): a click focuses a link too,
  // and must not leave it lifted once the pointer has gone.
  let hovered: string | null = null;
  let focused: string | null = null;
  // The note found in the search box (below): lifted while it stays found.
  let found: string | null = null;
  // The note a link of the page body under the pointer leads to (peek, below).
  let peeked: string | null = null;
  const settle = (slug: string) =>
    shapes
      .get(slug)
      ?.setAttribute(
        "d",
        slug === hovered || slug === focused || slug === found || slug === peeked
          ? shapeD(slug, growth)
          : restingD(slug),
      );
  // The chords' order (order.ts): the found note's over the rest, the current
  // page's over those, a hovered note's on top while it is hovered.
  const restack = () =>
    raise(edgeLayer, [
      found ? (edgesBySlug.get(found) ?? []) : [],
      edgesBySlug.get(current) ?? [],
      peekChords(peeked),
      hovered && hovered !== current ? (edgesBySlug.get(hovered) ?? []) : [],
    ]);
  // The current page's chords to `slug`.
  function peekChords(slug: string | null) {
    if (!slug) return [];
    const theirs = new Set(edgesBySlug.get(slug) ?? []);
    return (edgesBySlug.get(current) ?? []).filter((p) => theirs.has(p));
  }
  const hover = (slug: string | null) => {
    // Resting twice is harmless and the first call must run: it writes the
    // current note's title into the caption.
    if (slug !== null && hovered === slug) return;
    const previous = hovered;
    hovered = slug && byslug.has(slug) ? slug : null;
    if (previous) {
      nodeLayer.querySelector(`[data-slug="${CSS.escape(previous)}"]`)?.classList.remove("hover");
      for (const path of edgesBySlug.get(previous) ?? []) path.classList.remove("hover");
      settle(previous);
    }
    // At rest, the note found in the search box while it stays found (owner,
    // 2026-11-18: the pointer left on the ring took its name away), else the current one.
    const n = byslug.get(hovered ?? found ?? current);
    caption.textContent = n?.title ?? "";
    if (hovered) {
      settle(hovered);
      nodeLayer.querySelector(`[data-slug="${CSS.escape(hovered)}"]`)?.classList.add("hover");
      if (hovered !== current) {
        for (const path of edgesBySlug.get(hovered) ?? []) path.classList.add("hover");
      }
    }
    restack();
  };
  // Resting returns to the keyboard focus, if the ring has it.
  const restAll = () => hover(focused);
  restAll();

  const wedgeOf = (target: EventTarget | null) =>
    (target as Element | null)?.closest?.<SVGAElement>(".chord-graph-wedge") ?? null;
  svg.addEventListener(
    "pointerover",
    (ev) => {
      const w = wedgeOf(ev.target);
      if (w) hover(w.dataset.slug ?? null);
    },
    { signal },
  );
  svg.addEventListener("pointerleave", restAll, { signal });

  // A wedge is an SVG <a>, whose href is an SVGAnimatedString, not a string: a
  // client-side router that reads `a.href` as a string fails on it, and the browser falls back to
  // a full page load. So a plain left click on a wedge goes through the engine's navigate here,
  // with the link's real URL; a modified or middle click (new tab, new window, save)
  // is the browser's, as it is for any link, and so is a link to another origin or a
  // mount with no navigate.
  svg.addEventListener(
    "click",
    (ev) => {
      const w = wedgeOf(ev.target);
      // A click on empty space stops showing the found note.
      if (!w && found && ev.button === 0) {
        view.dispatchEvent(new CustomEvent(FIND_EVENT, { detail: null }));
        return;
      }
      if (!w || ev.defaultPrevented || ev.button !== 0) return;
      if (ev.metaKey || ev.ctrlKey || ev.shiftKey || ev.altKey) return;
      const raw = w.href?.baseVal ?? w.getAttribute("href");
      const go = ctx.navigate;
      if (!raw || !go) return;
      const url = new URL(raw, window.location.href);
      if (url.origin !== window.location.origin) return;
      ev.preventDefault();
      ev.stopPropagation(); // a page-wide click handler (a router's) must not see this link
      // Leave the modal dialog before leaving the page (see closeDialogs).
      view.closest("dialog")?.close();
      go(url);
    },
    { signal },
  );

  // Keyboard focus, as :focus-visible has it. Read on focus and after every
  // key: in Chromium a key pressed after a click makes the clicked link
  // :focus-visible with no new focus event.
  const syncFocus = (w: SVGAElement) => {
    const previous = focused;
    focused = w.matches(":focus-visible") ? (w.dataset.slug ?? null) : null;
    if (previous && previous !== focused) settle(previous);
    if (focused) settle(focused);
  };
  // Focus and key listeners go on the view, not on the svg: Chromium makes an
  // SVG element with focus listeners a tab stop of its own, and the ring must
  // be a single tab stop (its wedges, roving).
  view.addEventListener(
    "focusin",
    (ev) => {
      const w = wedgeOf(ev.target);
      if (!w) return;
      syncFocus(w);
      hover(w.dataset.slug ?? null);
    },
    { signal },
  );
  view.addEventListener(
    "focusout",
    (ev) => {
      if (!wedgeOf(ev.target) || svg.contains(ev.relatedTarget as Node | null)) return;
      const previous = focused;
      focused = null;
      if (previous) settle(previous);
      restAll();
    },
    { signal },
  );
  view.addEventListener(
    "keyup",
    (ev) => {
      const w = wedgeOf(ev.target);
      if (!w) return;
      const before = focused;
      syncFocus(w);
      // Only when the key has just made it keyboard focus (a click, then a
      // key): show it as focusing it would. Any other key, Shift before a
      // shift-click included, leaves the note under the pointer alone.
      if (focused && focused !== before) hover(focused);
    },
    { signal },
  );

  // Roving tabindex: the ring is one tab stop; arrows move around it.
  const step: Record<string, number> = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 };
  view.addEventListener(
    "keydown",
    (ev) => {
      const w = wedgeOf(ev.target);
      if (!w) return;
      const i = wedges.indexOf(w);
      let next = -1;
      const delta = step[ev.key];
      if (delta !== undefined) next = (i + delta + wedges.length) % wedges.length;
      else if (ev.key === "Home") next = 0;
      else if (ev.key === "End") next = wedges.length - 1;
      const target = wedges[next];
      if (!target) return;
      ev.preventDefault();
      w.setAttribute("tabindex", "-1");
      target.setAttribute("tabindex", "0");
      target.focus();
    },
    { signal },
  );
  // A note found in the box (find.ts) is shown, not opened (owner, 2026-11-17):
  // its wedge lifted and marked «found», its chords in ink over the rest, and the
  // focus on it, so that Enter on it (or a click) opens its page, as on any wedge.
  // The ring has no camera: every note is on it already. An emptied box clears it.
  const foundChords = (slug: string | null) => (slug ? (edgesBySlug.get(slug) ?? []) : []);
  view.addEventListener(
    FIND_EVENT,
    (ev) => {
      const slug = (ev as CustomEvent<string | null>).detail;
      const target = slug ? wedges.find((w) => w.dataset.slug === slug) : undefined;
      const previous = found;
      found = target ? slug : null;
      if (previous) {
        nodeLayer.querySelector(`[data-slug="${CSS.escape(previous)}"]`)?.classList.remove("found");
        for (const path of foundChords(previous)) path.classList.remove("found");
        settle(previous);
      }
      if (!target || !found) {
        // The caption back to what it shows at rest.
        restAll();
        return;
      }
      target.classList.add("found");
      for (const path of foundChords(found)) path.classList.add("found");
      settle(found);
      restack();
      for (const w of wedges) w.setAttribute("tabindex", "-1");
      target.setAttribute("tabindex", "0");
      target.focus();
    },
    { signal },
  );
}

/**
 * Draws a mounted graph for the page `current`: the sidebar view, and the
 * listeners that open, close and search the expanded view. Run again on every
 * page change (mount.ts, update).
 */
export async function render(container: HTMLElement, current: string) {
  const ctx = contexts.get(container);
  if (!ctx) return;
  const inline = container.querySelector<HTMLElement>(":scope > .chord-graph-view");
  const dialog = container.querySelector<HTMLDialogElement>(".chord-graph-dialog");
  const dialogView = dialog?.querySelector<HTMLElement>(".chord-graph-view");
  if (inline) await drawView(inline, container, current);
  // Disposed while the sidebar view waited for its data: nothing more to wire up.
  if (contexts.get(container) !== ctx) return;

  listeners.get(container)?.abort();
  const ac = new AbortController();
  listeners.set(container, ac);
  const signal = ac.signal;
  onCleanup(container, () => ac.abort());

  const expand = container.querySelector<HTMLButtonElement>(".chord-graph-expand");
  const close = dialog?.querySelector<HTMLButtonElement>(".chord-graph-close");
  if (!dialog || !dialogView || !expand) return;
  expand.addEventListener(
    "click",
    () => {
      openDialog(dialog);
      // Drawn after opening: wedge thickness depends on the drawn size.
      void drawView(dialogView, container, current);
    },
    { signal },
  );
  close?.addEventListener("click", () => dialog.close(), { signal });
  // Back in the sidebar, the reader's zoom from the dialog (hier.ts, userView).
  // Only the hierarchical views have a zoom; the ring is not redrawn (a click
  // on it right after closing must find the wedge it was aimed at).
  dialog.addEventListener(
    "close",
    () => {
      if (inline && inline.isConnected && inline.dataset.layout !== "chord")
        void drawView(inline, container, current);
    },
    { signal },
  );

  // Finding a note, in the expanded view: the notes the graph draws (the same
  // exclusions), by title and path (find.ts).
  const findBox = dialog.querySelector<HTMLElement>(".chord-graph-find");
  if (findBox) {
    const opts = JSON.parse(container.dataset.cfg ?? "{}") as Options;
    const prefixes = opts.excludePrefixes ?? [];
    const exclude = (s: string) =>
      prefixes.some((p) => s === p.replace(/\/$/, "") || s.startsWith(p));
    void ctx.data.then((data) => {
      if (signal.aborted) return;
      // With their text, as the site's search bar searches it (src/search.ts).
      const texts = new Map<string, string>();
      for (const [raw, e] of Object.entries(data)) {
        const t = (e as { content?: unknown }).content;
        if (typeof t === "string") texts.set(normaliseSlug(raw), t);
      }
      const notes = [...collectNotes(data, exclude).values()].map((n) => ({
        id: n.slug,
        title: n.title,
        text: texts.get(n.slug),
      }));
      mountFind(
        findBox,
        dialogView,
        notes,
        { count: container.dataset.labelCount ?? "{n}", one: container.dataset.labelOne ?? "1" },
        signal,
      );
    });
  }
  // Over the open dialog, the wheel belongs to the graph, never to the browser (owner,
  // 2026-11-16): it neither scrolls the page behind (over the margins, the header
  // or the selectors it scrolled it) nor, with Ctrl (the trackpad pinch in
  // Chromium and Firefox), zooms the whole page. The hierarchical views zoom
  // with it (hier.ts); the ring has no zoom. Only the search's list
  // scrolls with the plain wheel. The keyboard's page zoom (Ctrl with + − 0) is
  // left alone: it is the reader's (WCAG 1.4.4).
  dialog.addEventListener(
    "wheel",
    (ev) => {
      const zoomKey = ev.ctrlKey || ev.metaKey;
      if (!zoomKey && (ev.target as Element | null)?.closest?.(".chord-graph-find-list")) return;
      ev.preventDefault();
    },
    { signal, passive: false },
  );
  // No backdrop to click: the expanded view is the whole page (chord.scss). The
  // button, Escape, and a page change (closeDialogs, from mount.ts) close it.
}

// While the dialog is open, the page behind does not scroll in any
// way (the wheel, the keys, the scrollbar, a finger on the backdrop): the
// document is left with overflow: hidden, and on closing it returns exactly as it
// was, position included (owner, 2026-11-16: the wheel passed to the page). It is
// released on the «close» event (Escape, the button, the backdrop) and, without waiting for it, on
// a page change (closeDialogs): «close» arrives in another task, and the
// position is restored before the engine changes the page and scrolls it to the top.
let scrollLock: {
  dialog: HTMLDialogElement;
  overflow: string;
  gutter: string;
  overscroll: string;
  x: number;
  y: number;
} | null = null;
function lockScroll(dialog: HTMLDialogElement) {
  if (scrollLock) return;
  const root = document.documentElement;
  scrollLock = {
    dialog,
    overflow: root.style.overflow,
    gutter: root.style.scrollbarGutter,
    overscroll: root.style.overscrollBehavior,
    x: window.scrollX,
    y: window.scrollY,
  };
  // A scrollbar that takes room leaves its gap: the page does not shift
  // sideways when it goes (an overlay scrollbar takes nothing, and none is reserved).
  if (window.innerWidth > root.clientWidth) root.style.scrollbarGutter = "stable";
  root.style.overflow = "hidden";
  root.style.overscrollBehavior = "none";
}
// With `dialog`, only if the lock is its own: the late «close» of a dialog of the
// previous page does not release the lock of the one opened after it.
function unlockScroll(dialog?: HTMLDialogElement) {
  const was = scrollLock;
  if (!was || (dialog && was.dialog !== dialog)) return;
  scrollLock = null;
  const root = document.documentElement;
  root.style.overflow = was.overflow;
  root.style.scrollbarGutter = was.gutter;
  root.style.overscrollBehavior = was.overscroll;
  // Without the scroll-behavior: smooth animation a page may set: back at once.
  if (window.scrollX !== was.x || window.scrollY !== was.y)
    window.scrollTo({ left: was.x, top: was.y, behavior: "instant" });
}
function openDialog(dialog: HTMLDialogElement) {
  dialog.showModal();
  lockScroll(dialog);
  // Outside the page's signal: the cleanup at a page change would abort it
  // before the «close» of the dialog it closed arrives.
  dialog.addEventListener("close", () => unlockScroll(dialog), { once: true });
}

// The expanded view is a MODAL dialog: while it is open, the rest of the page
// is inert. A navigation that rewrites the page's HTML once took the dialog's
// `open` attribute away without closing it: the dialog vanished but stayed
// modal, and nothing on the new page answered a click or a hover (reported by
// the owner, 2026-11-06; reproduced: open=false, :modal=true). So the dialogs of a mount
// are closed at every page change and at the end of the mount (mount.ts), and any
// dialog found in that state is repaired.
// The browser's Back with the view open is such a page change too (owner,
// 2026-11-17): the view closes on the way, its scroll lock released.
// Opening the view adds no entry to the history, so Back is never spent on it.
/** Closes the expanded views inside `root`, and releases the scroll lock if one of them held it. */
export function closeDialogs(root: Element) {
  if (scrollLock && root.contains(scrollLock.dialog)) unlockScroll();
  for (const d of root.querySelectorAll<HTMLDialogElement>(".chord-graph-dialog")) {
    if (d.open) {
      d.close();
      continue;
    }
    let stuck = false;
    try {
      stuck = d.matches(":modal");
    } catch {
      // A browser without :modal cannot report it; nothing to repair then.
    }
    if (stuck) {
      // close() does nothing on a dialog without `open`: put it back, then close.
      d.setAttribute("open", "");
      d.close();
    }
  }
}

// A link of the page body under the pointer (owner, 2026-11-17): every drawn view
// highlights the note it leads to and the current page's link to it (views.ts,
// peek); off the link, nothing. Which links count is the engine's to say: a CSS
// selector per mount (MountOptions.bodyLinks). One pair of listeners per document
// for all mounts, across pages, removed with the last mount that asked for them. The
// href goes to a note by the graph's own rule (../link.ts, slugOfHref).
interface BodyLinks {
  /** Per mounted graph: its selector and the base path its notes' URLs carry. */
  mounts: Map<Element, { selector: string; basePath: string }>;
  over: (ev: Event) => void;
  out: (ev: Event) => void;
}
const bodyLinks = new WeakMap<Document, BodyLinks>();

function bodyLinkOf(
  w: BodyLinks,
  t: EventTarget | null,
): { a: HTMLAnchorElement; basePath: string } | null {
  for (const { selector, basePath } of w.mounts.values()) {
    const a = (t as Element | null)?.closest?.<HTMLAnchorElement>(selector) ?? null;
    if (a) return { a, basePath };
  }
  return null;
}

/** The body links under `selector` peek at the graph mounted at `root`, until unwatchBodyLinks. */
export function watchBodyLinks(root: Element, selector: string, basePath: string): void {
  const doc = root.ownerDocument;
  let w = bodyLinks.get(doc);
  if (!w) {
    const created: BodyLinks = {
      mounts: new Map(),
      over: (ev) => {
        const found = bodyLinkOf(created, ev.target);
        if (!found || found.a.closest(".chord-graph")) return;
        peekAll(slugOfHref(found.a.href, window.location.href, found.basePath));
      },
      out: (ev) => {
        const found = bodyLinkOf(created, ev.target);
        // Onto something inside the same link is still on it.
        if (!found || found.a.contains((ev as PointerEvent).relatedTarget as Node | null)) return;
        peekAll(null);
      },
    };
    w = created;
    bodyLinks.set(doc, w);
    doc.addEventListener("pointerover", w.over);
    doc.addEventListener("pointerout", w.out);
  }
  w.mounts.set(root, { selector, basePath });
}

/** The graph mounted at `root` stops peeking; the last one out removes the listeners. */
export function unwatchBodyLinks(root: Element): void {
  const doc = root.ownerDocument;
  const w = bodyLinks.get(doc);
  if (!w || !w.mounts.delete(root) || w.mounts.size > 0) return;
  doc.removeEventListener("pointerover", w.over);
  doc.removeEventListener("pointerout", w.out);
  bodyLinks.delete(doc);
}
