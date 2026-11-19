// SPDX-License-Identifier: GPL-3.0-or-later
// Client-side drawing for the hierarchical views (sunburst and balloon). All
// layout comes from ../scene (unit-tested); this file turns a scene into SVG
// and moves a camera over it. The drawing never changes as the reader moves:
// navigation only zooms and pans (the viewBox), which keeps the reader's mental
// map of the site. Text goes through textContent and setAttribute only.
import { chooseNames } from "./names";
import {
  buildScene,
  focusBox,
  legibleAt,
  navigate,
  zoomRange,
  type Item,
  type Layout,
  type Scene,
} from "../scene";
import { foldedName } from "../labels";
import type { IndexEntry } from "../geometry";
import type { Orientation } from "../sunburst";
import { polar, sectorPath } from "../sunburst";
import { spreadAngles } from "../labels";
import { labelReach, linkMeets, meets, visibleArea } from "../cull";
import { FIND_EVENT } from "./find";
import { overlayBoxes } from "./controls";
import { mountHere, type Disc } from "./here";
import { register } from "./views";
import { raise } from "./order";
import { onCleanup } from "./runtime";
import { hereFloor, reserve } from "../here";
import {
  fit,
  fitWith,
  panBy,
  smoothZoom,
  union,
  viewBox,
  zoomAnchor,
  zoomAt,
  type Box,
  type View,
} from "../viewport";

const SVG_NS = "http://www.w3.org/2000/svg";
// Label text: px on screen, whatever the zoom (set through --k in the stylesheet).
const LABEL_PX = 11;
const CALLOUT_PX = 12; // and callouts, bold (the stylesheet's sizes)
const CHAR_PX = 6.2; // rough width of one character at LABEL_PX, if nothing can measure
const DRAG_PX = 4; // movement before a press becomes a drag (and not a click)
// One step of the + and − buttons and their keys. It was 1.6: the owner found it
// too coarse (2026-11-16), two presses took the view from a folder to a
// note. 1.25 comes close to what one notch of the wheel does (×1.16).
const ZOOM_STEP = 1.25;
// How far a click on a folder, a found note or the arrows frames, in
// multiples of the fitted scale: what it was before (a quarter of the fixed cap of 120), and never
// beyond the cap (zoomRange).
const FRAME_ZOOM_MAX = 30;

// Safari's gestures (pinch on the trackpad and on touch screens): not
// in lib.dom.
interface GestureLike extends Event {
  scale: number;
  clientX: number;
  clientY: number;
}

// Text widths in screen px, measured with the label's own font. A width per
// character fell short for wide capitals and CJK (review: a 110 px title judged
// to need 70), and a curved label longer than its arc loses glyphs: SVG drops
// whatever runs off a textPath. Not cached until the page's fonts have loaded,
// or a fallback font's widths would stick.
const widths = new Map<string, number>();
let measurer: CanvasRenderingContext2D | null | undefined;
function textPx(text: string, font: string): number {
  const key = `${font}\u0000${text}`;
  const known = widths.get(key);
  if (known !== undefined) return known;
  if (measurer === undefined) measurer = document.createElement("canvas").getContext("2d");
  if (!measurer) return text.length * CHAR_PX;
  measurer.font = font;
  const w = measurer.measureText(text).width;
  if (document.fonts?.status !== "loading") widths.set(key, w);
  return w;
}

// A sunburst's scene depends on its orientation; a balloon's does not (keyed on
// it, a flip rebuilt the balloon's scene and reset its camera for nothing).
const sceneKey = (layout: Layout, orientation: Orientation) =>
  layout === "sunburst" ? `${layout}|${orientation}` : layout;

function svgEl<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string> = {}) {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
  return el;
}

// Scenes are worth keeping: the index does not change during a visit, and a
// site of a thousand notes takes tens of milliseconds to lay out.
const scenes = new WeakMap<object, Map<string, Scene>>();
function sceneFor(
  data: Record<string, IndexEntry>,
  layout: Layout,
  orientation: Orientation,
  exclude: (s: string) => boolean,
  locale: string | undefined,
): Scene {
  let m = scenes.get(data);
  if (!m) scenes.set(data, (m = new Map()));
  const key = `${sceneKey(layout, orientation)}|${locale ?? ""}`;
  let s = m.get(key);
  if (!s) m.set(key, (s = buildScene(data, layout, { exclude, locale, orientation })));
  return s;
}

// Where each view's camera was, and on which page, so that a new page in the same
// box moves on from there instead of jumping.
const cameras = new WeakMap<HTMLElement, { layout: string; page: string; view: View }>();
// The reader's own zoom (owner, 2026-11-12): every view opens on the whole
// graph, as the fit button leaves it, until the reader zooms or pans; from then
// on what they were looking at (the region of the drawing on screen) is kept,
// in the sidebar and in the dialog alike, until they open another page or
// switch to another view. The fit button and the 0 key forget it.
let userView: { page: string; scene: string; region: Box } | null = null;
// Ids for the arcs labels are written along: unique across views and redraws
// (the sidebar and the dialog are in the same document).
let arcIds = 0;

export interface HierArgs {
  view: HTMLElement;
  container: HTMLElement;
  svg: SVGSVGElement;
  caption: HTMLElement;
  data: Record<string, IndexEntry>;
  current: string;
  layout: Layout;
  orientation: Orientation;
  exclude: (s: string) => boolean;
  locale: string | undefined;
  visited: Set<string>;
  denseFrom: number;
  /**
   * Whether the current page carries its «You are here» pin (here.ts). Without it the
   * page is named by a callout, as every other page.
   */
  hereShown: boolean;
  hereLabel: string;
  /** The site's base path ("" or "/something"), before every page's URL. */
  basePath: string;
  /** Opens a page in place; absent, a click on a page is the browser's. */
  navigate?: (url: URL) => void;
  signal: AbortSignal;
}

export function drawHierarchical(a: HierArgs): void {
  const { view, container, svg, caption, signal } = a;
  // A draw whose redraw already happened (it waited on the data meanwhile) must
  // not start: nothing would ever stop what it starts.
  if (signal.aborted) return;
  const scene = sceneFor(a.data, a.layout, a.orientation, a.exclude, a.locale);
  const h = scene.hierarchy;
  const base = a.basePath;
  const here = h.nodeOf.get(a.current); // the current page's node, if it is drawn
  const byKey = new Map(scene.items.map((i) => [i.key, i]));
  const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;

  view.classList.add("hier");
  view.classList.toggle("dense", scene.edges.length >= a.denseFrom);
  svg.replaceChildren();
  // No page at all (the balloon layout still has its root ring): the empty caption.
  if (h.nodeOf.size === 0) {
    caption.textContent = container.dataset.labelEmpty ?? "";
    return;
  }

  // ---- drawing (once) ----
  const treeLayer = svgEl("g", { class: "chord-graph-tree-lines", "aria-hidden": "true" });
  const edgeLayer = svgEl("g", { class: "chord-graph-edges" });
  const nodeLayer = svgEl("g", { class: "chord-graph-nodes" });
  const labelLayer = svgEl("g", { class: "chord-graph-labels", "aria-hidden": "true" });
  const calloutLayer = svgEl("g", { class: "chord-graph-callouts", "aria-hidden": "true" });
  svg.append(treeLayer, edgeLayer, nodeLayer, labelLayer, calloutLayer);

  if (scene.layout === "balloon") {
    // Lines from each subfolder's port to its ring: the tree, faint.
    for (const i of scene.items) {
      if (i.shape !== "port" || !i.folder) continue;
      const child = byKey.get(scene.folderItem.get(i.folder)!);
      if (!child?.at || !i.at) continue;
      const mid = i.callout.angle;
      const [x0, y0] = polar(i.callout.radius, mid);
      // From the port to the near edge of the child's ring, not its middle.
      const [cx, cy] = child.at;
      const px = x0 + i.at[0];
      const py = y0 + i.at[1];
      const len = Math.hypot(px - cx, py - cy) || 1;
      const r = child.box.w / 2;
      const ex = cx + ((px - cx) / len) * r;
      const ey = cy + ((py - cy) / len) * r;
      treeLayer.append(
        svgEl("path", {
          class: "chord-graph-tree",
          d: `M${px.toFixed(2)},${py.toFixed(2)}L${ex.toFixed(2)},${ey.toFixed(2)}`,
        }),
      );
    }
  }

  // ---- what is drawn: only what is near the view (cull.ts) ----
  // Every element is made once; each layer holds, in drawing order, only those
  // whose box meets the area around the view, and the rest wait outside the
  // document (owner, 2026-11-11: zoomed in, every wedge, chord and label of the
  // site was laid out and restyled on each frame).
  interface Drawn {
    el: Element;
    box: Box;
    on: boolean;
    key?: string;
  }
  interface DrawnLink extends Drawn {
    boxes: Box[];
  }
  // Puts in `layer` what `want` asks for and takes out the rest, keeping the
  // drawing order (what is painted over what) and never touching what stays.
  // Whether it put anything in: the links raised on top (restack) go back there.
  const sync = <D extends Drawn>(list: D[], layer: Element, want: (d: D) => boolean) => {
    let next: Element | null = null;
    let added = false;
    for (let n = list.length - 1; n >= 0; n--) {
      const d = list[n]!;
      const w = want(d);
      if (w && !d.on) {
        layer.insertBefore(d.el, next);
        added = true;
      } else if (!w && d.on) d.el.remove();
      d.on = w;
      if (w) next = d.el;
    }
    return added;
  };
  const edgesDrawn: DrawnLink[] = [];
  const wedgesDrawn: Drawn[] = [];

  const edgesOf = new Map<string, SVGPathElement[]>();
  for (const e of scene.edges) {
    const p = svgEl("path", { class: "chord-graph-edge", d: e.d });
    edgesDrawn.push({ el: p, box: e.box, boxes: e.boxes, on: false });
    for (const n of [e.source, e.target]) {
      if (!edgesOf.has(n)) edgesOf.set(n, []);
      edgesOf.get(n)!.push(p);
    }
  }

  const elOf = new Map<string, SVGAElement>();
  const keyOf = new Map<Element, string>();
  const count = container.dataset.labelCount ?? "{n}";
  const one = container.dataset.labelOne ?? "1";
  const nameOf = (i: Item) =>
    i.folder !== undefined
      ? foldedName(i.title, h.nodes.get(i.folder)?.size ?? 0, count, one)
      : i.title;
  for (const i of scene.items) {
    const isPage = i.page !== undefined;
    const link = svgEl(
      "a",
      isPage
        ? {
            href: `${base}/${i.href ?? ""}`,
            class: `chord-graph-wedge ${i.shape}`,
            tabindex: "-1",
            "aria-label": i.title,
          }
        : {
            class: `chord-graph-wedge ${i.shape} folder`,
            role: "button",
            tabindex: "-1",
            "aria-label": nameOf(i),
          },
    );
    if (i.at)
      link.setAttribute("transform", `translate(${i.at[0].toFixed(2)} ${i.at[1].toFixed(2)})`);
    // No <title>: the browser showed it as its own tooltip over the callout the
    // graph draws (owner, 2026-11-17). The accessible name is the aria-label.
    link.append(svgEl("path", { class: "chord-graph-shape", d: i.d }));
    if (isPage && i.page === here) {
      link.classList.add("current");
      link.setAttribute("aria-current", "page");
    }
    // The visited band, as on the ring: an inner band on a page's wedge.
    const slug = isPage ? (h.nodes.get(i.page!)?.own ?? i.page!) : undefined;
    if (slug && i.band && a.visited.has(slug) && i.page !== here) {
      link.classList.add("visited");
      link.append(svgEl("path", { class: "chord-graph-visited", d: sectorPath(...i.band) }));
    }
    wedgesDrawn.push({ el: link, box: i.box, on: false, key: i.key });
    elOf.set(i.key, link);
    keyOf.set(link, i.key);
  }

  // Resting labels: created once, shown when they have room on screen.
  const labelEls: Array<[Item, SVGTextElement]> = [];
  const arcOf = new Map<SVGTextElement, SVGPathElement>();
  const labelsDrawn: Drawn[] = [];
  const arcs = svgEl("defs", {});
  for (const i of scene.items) {
    if (!i.label) continue;
    const cls = `chord-graph-label ${i.shape}${i.label.onShape ? " on-shape" : ""}`;
    let t: SVGTextElement;
    if (i.label.path) {
      // Along its ring: the text on an arc, centred on it ("central": the
      // middle of the letters on the arc, so they sit halfway across the ring).
      const id = `chord-graph-arc-${++arcIds}`;
      const arc = svgEl("path", { id, d: i.label.path });
      t = svgEl("text", { class: cls, "dominant-baseline": "central" });
      arcOf.set(t, arc);
      const tp = svgEl("textPath", { href: `#${id}`, startOffset: "50%", "text-anchor": "middle" });
      tp.textContent = i.title;
      t.append(tp);
    } else {
      t = svgEl("text", {
        class: cls,
        x: i.label.x.toFixed(2),
        y: i.label.y.toFixed(2),
        "text-anchor": i.label.anchor,
        "dominant-baseline": "middle",
        transform: `rotate(${i.label.rotate.toFixed(1)} ${i.label.x.toFixed(2)} ${i.label.y.toFixed(2)})`,
      });
      t.textContent = i.title;
    }
    labelEls.push([i, t]);
    labelsDrawn.push({ el: t, box: { x: 0, y: 0, w: 0, h: 0 }, on: false });
  }
  if (arcOf.size > 0) svg.prepend(arcs);
  // The labels' own font family, at their size on screen (and the callouts'):
  // read from one in the document for a moment (out of it, it has no style).
  let family = "system-ui";
  if (labelEls[0]) {
    const t = labelEls[0][1];
    t.style.display = "none";
    labelLayer.append(t);
    family = getComputedStyle(t).fontFamily || family;
    t.remove();
    t.style.display = "";
  }
  const labelFont = `${LABEL_PX}px ${family}`;
  const calloutFont = `600 ${CALLOUT_PX}px ${family}`;

  // ---- «You are here» (here.ts): the current page's marker, in its own layer ----
  const hereItem = here ? byKey.get(scene.pageItem.get(here) ?? "") : undefined;
  // Shown or hidden: the expanded view's switch changes it without a redraw (setHere).
  let hereShown = a.hereShown;
  const mark = mountHere({
    svg,
    shown: hereShown,
    title: hereItem?.title ?? "",
    label: a.hereLabel,
    large: view.closest("dialog") !== null,
    signal,
  });
  // What counts as taken besides what is drawn: the sunburst's central disc, where
  // the links run (a balloon ring's middle is a shape of its own, and counts).
  const discR =
    scene.layout === "sunburst"
      ? Math.min(
          ...scene.items.filter((i) => i.span && i.label).map((i) => i.span![2] - i.label!.height),
        )
      : NaN;
  const discs: Disc[] = Number.isFinite(discR) && discR > 0 ? [[0, 0, discR]] : [];
  // Where the marker points: the note's outer edge, on its ring; a folder's page
  // (a balloon ring's middle) is the whole ring.
  const hereAt = (): {
    anchor: [number, number];
    centre: [number, number];
    ring?: number;
  } | null => {
    if (!hereItem) return null;
    const [cx, cy] = hereItem.callout.around;
    if (hereItem.shape === "centre")
      return { anchor: [cx, cy], centre: [cx, cy], ring: hereItem.callout.radius };
    const [x, y] = polar(hereItem.callout.from, hereItem.callout.angle);
    return { anchor: [cx + x, cy + y], centre: [cx, cy] };
  };
  const placeHere = () => {
    const at = hereAt();
    if (at) mark.update(at.anchor, discs, at.ring);
    else mark.clear();
  };

  // ---- standing state: the current page, its neighbours and their links ----
  const neighbours = here ? (scene.neighbours.get(here) ?? new Set<string>()) : new Set<string>();
  // Their items: labelled by callouts, never by resting labels.
  const standing = new Set(
    [...(here ? [here] : []), ...neighbours]
      .map((n) => scene.pageItem.get(n))
      .filter((k): k is string => !!k),
  );
  for (const n of neighbours) elOf.get(scene.pageItem.get(n) ?? "")?.classList.add("neighbour");
  for (const p of here ? (edgesOf.get(here) ?? []) : []) p.classList.add("current");

  // ---- camera ----
  // The svg's box, read once a frame: read after a move changed the viewBox,
  // it made the browser lay out the whole drawing again at once, three times a
  // pointer move (measured: most of a pan's time). A resize clears it.
  let rectNow: DOMRect | null = null;
  const rect = () => {
    if (!rectNow) {
      rectNow = svg.getBoundingClientRect();
      requestAnimationFrame(() => (rectNow = null));
    }
    return rectNow;
  };
  const size = () => {
    const r = rect();
    return [r.width || 300, r.height || 300] as const;
  };
  const inDialog = view.closest("dialog") !== null;
  // Which pages are named (names.ts): in the sidebar box, the current page only.
  const names = inDialog ? chooseNames(view.dataset.names ?? null) : "current";
  // sidebarLabels: in the sidebar box, the only name drawn is the current page's
  // (and the one under the pointer); every other label waits for the dialog
  // (owner, 2026-11-12: minimized, too many labels crowded the small graph).
  const pad = () => (inDialog ? 48 : 20);
  const thisScene = sceneKey(a.layout, a.orientation);
  const all = () => {
    const [w, hh] = size();
    return fit(scene.bounds, w, hh, pad());
  };
  // The zoom's limits, from what is drawn (scene.ts, zoomRange): from the fitted
  // scale (the fit button's, with the names) to the one that makes the smallest
  // leaf's name readable. Per frame size: remade when it changes.
  let range: { size: string; r: readonly [number, number] } | null = null;
  const limits = () => {
    const now = size().join("x");
    if (range?.size !== now) {
      const r = zoomRange(scene.items, wholeLabelled().k, (t) => textPx(t, labelFont), LABEL_PX);
      range = { size: now, r };
    }
    return range.r;
  };
  let cam: View = all();
  let anim = 0;
  // Where the running animation is going (goTo): + and − carry on from there, and two
  // presses in a row add up their steps instead of jumping from where it is.
  let aim: View | null = null;
  const halt = () => {
    cancelAnimationFrame(anim);
    aim = null;
  };
  // What must stay drawn wherever the view is: the one tab stop (Tab has to
  // reach the graph), what has the focus, what the pointer is on.
  let tabStop = "";
  const kept = (key: string | undefined) =>
    key !== undefined &&
    (key === tabStop || key === hovered || elOf.get(key) === document.activeElement);
  const cull = () => {
    const [w, hh] = size();
    const area = visibleArea(cam, w, hh);
    sync(wedgesDrawn, nodeLayer, (d) => kept(d.key) || meets(d.box, area));
    // A link by the boxes of its pieces: its whole box can reach far around the view.
    // Put back in list order: the ones that go on top go there again.
    if (sync(edgesDrawn, edgeLayer, (d) => linkMeets(d, area))) restack();
  };
  const apply = (v: View) => {
    const [w, hh] = size();
    cam = v;
    svg.setAttribute("viewBox", viewBox(v, w, hh));
    svg.style.setProperty("--k", String(v.k));
    cameras.set(view, { layout: thisScene, page: a.current, view: v });
    cull();
    // The marker moves with its note while the camera does; placed anew on settling.
    const at = hereAt();
    if (at) mark.follow(at.anchor);
  };
  // What the reader is looking at, kept for the other box and for redraws.
  const remember = (v: View) => {
    const [w, hh] = size();
    userView = { page: a.current, scene: thisScene, region: visibleArea(v, w, hh, 0) };
    fitted = false;
  };
  // Does the view show the whole graph, as «fit» leaves it, without the reader
  // having moved it? Then a change of size fits again (the controls
  // do not shrink with the frame: in a narrower one they cover more).
  let fitted = false;
  const goTo = (target: View, animate = !reduced) => {
    halt();
    if (!animate) {
      apply(target);
      settle();
      return;
    }
    aim = target;
    const [w] = size();
    const z = smoothZoom(cam, target, w);
    const ms = Math.min(900, Math.max(180, z.S * 400));
    const t0 = performance.now();
    const step = (now: number) => {
      const t = Math.min(1, (now - t0) / ms);
      // Ease in and out over the path's natural speed.
      const e = t < 0.5 ? 2 * t * t : 1 - (-2 * t + 2) ** 2 / 2;
      apply(t === 1 ? target : z.at(e));
      if (t < 1) anim = requestAnimationFrame(step);
      else {
        aim = null;
        settle();
      }
    };
    anim = requestAnimationFrame(step);
  };
  // Where a callout label reaches at scale k: its far end, so framing can
  // hold it (labels are sized in screen pixels, so their reach in drawing
  // units depends on the scale being chosen).
  // How far out the drawing reaches at an angle (sunburst): a callout spread
  // sideways onto deeper rings must start beyond them, not at its own note's
  // edge (on a real site one ran across a neighbouring folder's notes).
  const spans = scene.items.filter((i) => i.span).map((i) => i.span!);
  const outerAt = (a: number) => {
    const t = ((a % (2 * Math.PI)) + 2 * Math.PI) % (2 * Math.PI);
    let r = 0;
    for (const [a0, a1, r1] of spans) if (t >= a0 && t <= a1 && r1 > r) r = r1;
    return r;
  };
  const calloutRadius = (i: Item, ang: number) =>
    scene.layout === "sunburst"
      ? Math.max(i.callout.radius, outerAt(ang), outerAt(i.callout.angle))
      : i.callout.radius;
  const reach = (i: Item, k: number): Box => {
    const len = (textPx(i.title, calloutFont) + 16) / k;
    const [cx, cy] = i.callout.around;
    if (scene.layout === "balloon" && i.shape === "centre") {
      // Centred under its ring: half its length to either side.
      const y = cy + i.callout.radius + (6 + LABEL_PX + 4) / k;
      return { x: cx - len / 2, y: cy, w: len, h: y - cy };
    }
    if (scene.layout === "balloon") {
      // In the balloons the name is written horizontally from beside its wedge
      // (callouts, below), to the right or the left by its side: its box,
      // half a line high on either side. Taken along the radius,
      // as in the sunburst, framing left it under the zoom buttons (decisions).
      const half = CALLOUT_PX / 2 / k;
      const [x, y] = polar(i.callout.radius + 10 / k, i.callout.angle);
      const right = Math.sin(i.callout.angle) >= 0;
      return { x: cx + x - (right ? 0 : len), y: cy + y - half, w: len, h: 2 * half };
    }
    const [x, y] = polar(calloutRadius(i, i.callout.angle) + len, i.callout.angle);
    return { x: x + cx, y: y + cy, w: 0, h: 0 };
  };
  const pagesOf = (node: string | undefined) => {
    if (!node) return [];
    // In the sidebar, only the page itself is labelled (sidebarLabels, below).
    const nodes = names !== "current" ? [node, ...(scene.neighbours.get(node) ?? [])] : [node];
    const keys = nodes.map((n) => scene.pageItem.get(n));
    return keys.filter((k): k is string => !!k).map((k) => byKey.get(k)!);
  };
  // Frames `box`, and the callouts of `labelled` around it: fit, see where the
  // labels land at that scale, fit again with them (twice settles it).
  // What is framed, entirely clear of the controls (controls.ts): around a
  // folder there is more drawing, which may end up under them; the folder may not.
  const framing = (box: Box, labelled: Item[] = []): View => {
    const [w, hh] = size();
    const [kMin, kMax] = limits();
    const controls = overlayBoxes(svg);
    const kTop = Math.min(kMax, all().k * FRAME_ZOOM_MAX);
    let v = fit(box, w, hh, pad(), kMin, kTop, controls);
    for (let pass = 0; pass < 2 && labelled.length > 0; pass++) {
      const b = union([
        box,
        ...labelled.flatMap((i) => (i === hereItem && hereShown ? hereRoom(v.k) : [reach(i, v.k)])),
      ])!;
      v = fit(b, w, hh, pad(), kMin, kTop, controls);
    }
    return v;
  };
  const frame = (box: Box | null, animate = !reduced, labelled: Item[] = []) => {
    // Always the reader's doing (a folder clicked, a page reached with the keys).
    if (!box) return;
    const v = framing(box, labelled);
    remember(v);
    goTo(v, animate);
  };
  // The whole graph, and the names around it (the current page's, and in the
  // dialog its neighbours'): fit, see where the names land, fit again.
  // Nothing drawn under a control (controls.ts, viewport.ts: fitClear):
  // the controls are measured now, and the parts are each wedge or balloon and the names.
  const parts = scene.items.map((i) => i.box);
  // The current page's name is its «You are here» label: room for it beside the note
  // (here.ts, reserve), not a callout's reach. Hidden, the page has its callout.
  const hereRoom = (k: number): Box[] => {
    const at = hereShown ? hereAt() : null;
    return at ? [reserve(at.anchor, at.centre, mark.size, k)] : [];
  };
  const wholeLabelled = () => {
    const [w, hh] = size();
    const labelled = pagesOf(here).filter((i) => i !== hereItem || !hereShown);
    return fitWith(
      scene.bounds,
      (k) => [...labelled.map((i) => reach(i, k)), ...hereRoom(k)],
      w,
      hh,
      pad(),
      // In the sidebar the drawing fills the frame: it gives up more for the label.
      inDialog ? undefined : hereFloor(w),
      { controls: overlayBoxes(svg), parts },
    );
  };

  // ---- labels that depend on the scale: resting ones culled, callouts placed ----
  let settleTimer = 0;
  const settle = () => {
    window.clearTimeout(settleTimer);
    settleTimer = window.setTimeout(labels, 60);
  };
  // The screen boxes of a text's letters.
  const glyphs = (t: SVGTextElement): DOMRect[] => {
    const m = t.getScreenCTM();
    if (!m) return [];
    const out: DOMRect[] = [];
    for (let c = 0; c < t.getNumberOfChars(); c++) {
      const e = t.getExtentOfChar(c);
      const ps = [
        [e.x, e.y],
        [e.x + e.width, e.y],
        [e.x, e.y + e.height],
        [e.x + e.width, e.y + e.height],
      ].map(([x, y]) => new DOMPoint(x, y).matrixTransform(m));
      const xs = ps.map((p) => p.x);
      const ys = ps.map((p) => p.y);
      const x0 = Math.min(...xs);
      const y0 = Math.min(...ys);
      out.push(new DOMRect(x0, y0, Math.max(...xs) - x0, Math.max(...ys) - y0));
    }
    return out;
  };
  const labels = () => {
    callouts();
    {
      // The whole text and a margin, measured: for a curved label, room is
      // never more than its arc (scene.ts), so a label that shows is whole.
      // The same rule that sets the zoom's cap (scene.ts, legibleAt).
      const fits = (l: Item["label"], title: string) =>
        !!l && cam.k >= legibleAt(l, textPx(title, labelFont), LABEL_PX);
      const [w, hh] = size();
      const area = visibleArea(cam, w, hh);
      const shown = new Set<SVGTextElement>();
      for (const [i, t] of labelEls) {
        // Its own place if it fits there, else its other place, else hidden.
        // Never on the current page or a neighbour: they have a callout, and
        // page-coloured text on their accent or neighbour fill would fall below
        // 4.5:1 (light gruvbox neighbour: 4.14).
        const l =
          names !== "all" || standing.has(i.key)
            ? null
            : fits(i.label, i.title)
              ? i.label!
              : fits(i.alt, i.title)
                ? i.alt!
                : null;
        // Only near the view: its letters reach no farther than labelReach.
        if (!l || !meets(labelReach(l), area)) continue;
        shown.add(t);
        // Written along its arc: placed once, when it was made.
        if (l.path) continue;
        t.setAttribute("x", l.x.toFixed(2));
        t.setAttribute("y", l.y.toFixed(2));
        t.setAttribute("dominant-baseline", l === i.alt ? "hanging" : "middle");
        t.setAttribute(
          "transform",
          `rotate(${l.rotate.toFixed(1)} ${l.x.toFixed(2)} ${l.y.toFixed(2)})`,
        );
      }
      sync(labelsDrawn, labelLayer, (d) => shown.has(d.el as SVGTextElement));
      for (const [t, arc] of arcOf) {
        if (shown.has(t) && !arc.isConnected) arcs.append(arc);
        else if (!shown.has(t) && arc.isConnected) arc.remove();
      }
      // A resting label that would sit under a callout gives way: measured on
      // screen, so it holds across rings and layouts (a real site had a ring's
      // title under another ring's callout, which no angle test would see).
      const taken = [...calloutLayer.querySelectorAll("text")].map((c) =>
        c.getBoundingClientRect(),
      );
      const meetsCallout = (b: DOMRect) =>
        taken.some(
          (c) => b.left < c.right && c.left < b.right && b.top < c.bottom && c.top < b.bottom,
        );
      if (taken.length > 0) {
        for (const d of labelsDrawn) {
          if (!d.on) continue;
          const t = d.el as SVGTextElement;
          if (!meetsCallout(t.getBoundingClientRect())) continue;
          // A curved label's box spans its arc's bulge, far more than its
          // letters (review): only a letter that meets a callout counts.
          if (t.firstElementChild?.tagName === "textPath" && !glyphs(t).some(meetsCallout))
            continue;
          t.remove();
          d.on = false;
          const arc = arcOf.get(t);
          if (arc) arc.remove();
        }
      }
    }
    // Last, over what is now drawn: it goes where nothing is.
    placeHere();
  };
  let hovered: string | null = null; // item key
  // The note found in the search box (FIND_EVENT, below).
  let found: string | null = null; // item key
  // The note a link of the page body under the pointer leads to (peek, below).
  let peeked: string | null = null; // item key
  const callouts = () => {
    calloutLayer.replaceChildren();
    const keys = new Set<string>();
    if (here) {
      const k = scene.pageItem.get(here);
      if (k) keys.add(k);
      // In the sidebar, the current page's name and nothing else (sidebarLabels).
      if (names !== "current")
        for (const n of neighbours) {
          const nk = scene.pageItem.get(n);
          if (nk) keys.add(nk);
        }
    }
    if (hovered) keys.add(hovered);
    // The found note carries its name while it stays found, whatever the
    // pointer does (owner, 2026-11-18: chosen with the mouse, the pointer stayed over the
    // graph when the list closed, and without «hover» it lost its name).
    if (found) keys.add(found);
    // The current page is named by its marker («You are here»), not by a callout;
    // with the marker hidden, by its callout.
    const list = [...keys]
      .map((k) => byKey.get(k)!)
      .filter((i) => i.page !== undefined || i.key === hovered)
      .filter((i) => !hereShown || i.page === undefined || i.page !== here);
    const cls = (i: Item) =>
      i.page !== undefined && i.page === here
        ? "current"
        : i.key === hovered
          ? "hover"
          : i.key === found
            ? "found"
            : "neighbour";
    if (scene.layout === "sunburst") {
      const want = list.map((i) => i.callout.angle);
      // Spread at the largest radius: labels at different radii are spaced at
      // least as far apart as they need to be.
      const R = Math.max(0, ...list.map((i) => calloutRadius(i, i.callout.angle)));
      const got = spreadAngles(want, (LABEL_PX + 4) / (cam.k * (R + 14 / cam.k)));
      list.forEach((i, n) => {
        const ang = got[n]!;
        const r = calloutRadius(i, ang);
        const deg = ((((ang * 180) / Math.PI) % 360) + 360) % 360;
        const [x, y] = polar(r + 14 / cam.k, ang);
        const left = deg > 180;
        const [lx0, ly0] = polar(i.callout.from + 1 / cam.k, want[n]!);
        const [lx1, ly1] = polar(r + 4 / cam.k, want[n]!);
        const [lx2, ly2] = polar(r + 11 / cam.k, ang);
        calloutLayer.append(
          svgEl("path", {
            class: `chord-graph-leader ${cls(i)}`,
            d: `M${lx0.toFixed(3)},${ly0.toFixed(3)}L${lx1.toFixed(3)},${ly1.toFixed(3)}L${lx2.toFixed(3)},${ly2.toFixed(3)}`,
          }),
        );
        calloutLayer.append(
          svgEl("circle", {
            class: `chord-graph-dot ${cls(i)}`,
            cx: lx2.toFixed(3),
            cy: ly2.toFixed(3),
            r: (2.5 / cam.k).toFixed(4),
          }),
        );
        const t = svgEl("text", {
          class: `chord-graph-callout ${cls(i)}`,
          x: x.toFixed(3),
          y: y.toFixed(3),
          "text-anchor": left ? "end" : "start",
          "dominant-baseline": "middle",
          transform: `rotate(${(left ? deg + 90 : deg - 90).toFixed(1)} ${x.toFixed(3)} ${y.toFixed(3)})`,
        });
        t.textContent = i.title;
        calloutLayer.append(t);
      });
    } else {
      // Per ring: the labels of one ring's wedges spread apart like the
      // sunburst's (neighbouring notes on one ring overlapped otherwise).
      const rings = new Map<string, Item[]>();
      for (const i of list) {
        const k = i.callout.around.join(",");
        if (!rings.has(k)) rings.set(k, []);
        rings.get(k)!.push(i);
      }
      for (const group of rings.values()) {
        const wedges = group.filter((i) => i.shape !== "centre");
        const r = wedges[0]?.callout.radius ?? 0;
        const got = spreadAngles(
          wedges.map((i) => i.callout.angle),
          (LABEL_PX + 3) / (cam.k * (r + 10 / cam.k)),
        );
        const angleOf = new Map(wedges.map((i, n) => [i, got[n]!]));
        for (const i of group) {
          const [cx, cy] = i.callout.around;
          const centre = i.shape === "centre";
          const ang = centre ? Math.PI : angleOf.get(i)!;
          const [x, y] = polar(i.callout.radius + (centre ? 6 : 10) / cam.k, ang);
          if (!centre) {
            // The role's colour on a dot by the wedge; the text stays ink.
            const [dx, dy] = polar(i.callout.radius + 4 / cam.k, ang);
            calloutLayer.append(
              svgEl("circle", {
                class: `chord-graph-dot ${cls(i)}`,
                cx: (dx + cx).toFixed(3),
                cy: (dy + cy).toFixed(3),
                r: (2.5 / cam.k).toFixed(4),
              }),
            );
          }
          const t = svgEl("text", {
            class: `chord-graph-callout ${cls(i)}`,
            x: (x + cx).toFixed(3),
            y: (y + cy).toFixed(3),
            "text-anchor": centre ? "middle" : Math.sin(ang) >= 0 ? "start" : "end",
            "dominant-baseline": centre ? "hanging" : "middle",
          });
          t.textContent = i.title;
          calloutLayer.append(t);
        }
      }
    }
  };

  // ---- hover and focus ----
  const pageOf = (key: string | null) => (key ? byKey.get(key)?.page : undefined);
  const hover = (key: string | null) => {
    if (key === hovered) return;
    if (hovered) {
      elOf.get(hovered)?.classList.remove("hover");
      for (const p of edgesOf.get(pageOf(hovered) ?? "") ?? []) p.classList.remove("hover");
    }
    hovered = key;
    const it = key ? byKey.get(key) : here ? byKey.get(scene.pageItem.get(here) ?? "") : undefined;
    caption.textContent = it ? nameOf(it) : "";
    if (key) {
      elOf.get(key)?.classList.add("hover");
      if (pageOf(key) !== here)
        for (const p of edgesOf.get(pageOf(key) ?? "") ?? []) p.classList.add("hover");
    }
    restack();
    labels();
  };
  // The links' order (order.ts): the found note's over the rest, the current page's
  // over those, a hovered note's on top while it is hovered.
  const linksOf = (key: string | null) => (key ? (edgesOf.get(pageOf(key) ?? "") ?? []) : []);
  function restack() {
    raise(edgeLayer, [
      linksOf(found),
      here ? (edgesOf.get(here) ?? []) : [],
      peekLinks(peeked),
      hovered && pageOf(hovered) !== here ? linksOf(hovered) : [],
    ]);
  }
  // The current page's links to the note of `key`.
  function peekLinks(key: string | null) {
    if (!key || !here) return [];
    const theirs = new Set(linksOf(key));
    return (edgesOf.get(here) ?? []).filter((p) => theirs.has(p));
  }

  const itemOf = (t: EventTarget | null) => {
    const w = (t as Element | null)?.closest?.<SVGAElement>(".chord-graph-wedge");
    return w ? (keyOf.get(w) ?? null) : null;
  };
  svg.addEventListener("pointerover", (ev) => hover(itemOf(ev.target)), { signal });
  svg.addEventListener("pointerleave", () => hover(null), { signal });

  // ---- zoom and pan: wheel, drag, pinch, buttons, keys ----
  const pointers = new Map<number, [number, number]>();
  let dragged = false;
  let pinch: { d: number; v: View; mx: number; my: number } | null = null;
  const local = (ev: PointerEvent | WheelEvent): [number, number] => {
    const r = rect();
    return [ev.clientX - r.left, ev.clientY - r.top];
  };
  // A point on the screen, inside the frame: the wheel over the dialog's margin
  // zooms on the drawing's nearest edge, not on a point outside it.
  const inside = ([x, y]: [number, number]): [number, number] => {
    const [w, hh] = size();
    return [Math.min(w, Math.max(0, x)), Math.min(hh, Math.max(0, y))];
  };
  // In the sidebar the wheel scrolls the page, as it does over any other part
  // of it; Ctrl or Cmd with the wheel zooms (and a trackpad's pinch, which
  // Chromium and Firefox send as the wheel with Ctrl). In the dialog it zooms
  // over the whole dialog, not only over the drawing (owner, 2026-11-16: over
  // its margins and its controls the page behind scrolled); there
  // chord.ts takes every wheel from the browser, page zoom included.
  // A Safari pinch in progress (gesturestart): the wheel with Ctrl that might
  // come with it does not zoom a second time.
  let gesture: View | null = null;
  const onWheel = (ev: WheelEvent) => {
    const zoomKey = ev.ctrlKey || ev.metaKey;
    if (!inDialog && !zoomKey) return;
    // The search's list scrolls with the wheel (without passing it to the page:
    // overscroll-behavior in chord.scss).
    if (!zoomKey && (ev.target as Element | null)?.closest?.(".chord-graph-find-list")) return;
    if (!inDialog) ev.preventDefault();
    if (gesture) return;
    halt();
    const [w, hh] = size();
    const [px, py] = inside(local(ev));
    const dy = ev.deltaMode === 1 ? ev.deltaY * 16 : ev.deltaY;
    const [kMin, kMax] = limits();
    apply(zoomAt(cam, Math.exp(-dy * 0.0015), px, py, w, hh, kMin, kMax));
    remember(cam);
    settle();
  };
  const dialog = view.closest("dialog");
  if (dialog) dialog.addEventListener("wheel", onWheel, { signal, passive: false });
  else svg.addEventListener("wheel", onWheel, { signal, passive: false });
  // Safari does not send the wheel with Ctrl on a trackpad pinch: it sends
  // gesturestart / gesturechange (with the scale since the gesture began), which
  // without preventDefault zoom the page. Only in the dialog: in the sidebar,
  // on iOS, the pinch zooms the page as always (touch-action: pan-y). A
  // pinch with the fingers on the drawing is already handled by the pointers (below): that one
  // is not counted twice.
  if (dialog) {
    const gestureAt = (ev: GestureLike) => {
      const r = rect();
      return inside([ev.clientX - r.left, ev.clientY - r.top]);
    };
    dialog.addEventListener(
      "gesturestart",
      (ev) => {
        ev.preventDefault();
        if (pointers.size >= 2) return;
        halt();
        gesture = cam;
      },
      { signal, passive: false },
    );
    dialog.addEventListener(
      "gesturechange",
      (ev) => {
        ev.preventDefault();
        if (!gesture || pointers.size >= 2) return;
        const g = ev as GestureLike;
        const [w, hh] = size();
        const [px, py] = gestureAt(g);
        const [kMin, kMax] = limits();
        apply(zoomAt(gesture, g.scale, px, py, w, hh, kMin, kMax));
        remember(cam);
        settle();
      },
      { signal, passive: false },
    );
    dialog.addEventListener(
      "gestureend",
      (ev) => {
        ev.preventDefault();
        gesture = null;
      },
      { signal, passive: false },
    );
  }
  svg.addEventListener(
    "pointerdown",
    (ev) => {
      if (ev.button !== 0) return;
      pointers.set(ev.pointerId, local(ev));
      dragged = false;
      if (pointers.size === 2) {
        const [p, q] = [...pointers.values()] as [[number, number], [number, number]];
        pinch = {
          d: Math.hypot(p[0] - q[0], p[1] - q[1]),
          v: cam,
          mx: (p[0] + q[0]) / 2,
          my: (p[1] + q[1]) / 2,
        };
      }
    },
    { signal },
  );
  svg.addEventListener(
    "pointermove",
    (ev) => {
      const prev = pointers.get(ev.pointerId);
      if (!prev) return;
      const now = local(ev);
      if (!dragged && Math.hypot(now[0] - prev[0], now[1] - prev[1]) < DRAG_PX && !pinch) return;
      if (!dragged) {
        dragged = true;
        halt();
        svg.setPointerCapture?.(ev.pointerId);
      }
      pointers.set(ev.pointerId, now);
      const [w, hh] = size();
      const [kMin, kMax] = limits();
      if (pinch && pointers.size === 2) {
        const [p, q] = [...pointers.values()] as [[number, number], [number, number]];
        const d = Math.hypot(p[0] - q[0], p[1] - q[1]);
        apply(zoomAt(pinch.v, d / pinch.d, pinch.mx, pinch.my, w, hh, kMin, kMax));
      } else {
        apply(panBy(cam, now[0] - prev[0], now[1] - prev[1]));
      }
      remember(cam);
      settle();
    },
    { signal },
  );
  const release = (ev: PointerEvent) => {
    pointers.delete(ev.pointerId);
    if (pointers.size < 2) pinch = null;
    // No reset here: a touch tap's click comes after a timeout would have
    // cleared the flag (a small nudge then opened the page). The next press
    // resets it, and a keyboard click (detail 0) is never taken for a drag.
  };
  svg.addEventListener("pointerup", release, { signal });
  svg.addEventListener("pointercancel", release, { signal });

  // + and −: from where the running animation is going, if any (two presses
  // in a row add up their steps), and on the current note if it is in view
  // (viewport.ts, zoomAnchor), else on the centre.
  const zoomBy = (f: number) => {
    const from = aim ?? cam;
    const [w, hh] = size();
    const [kMin, kMax] = limits();
    const [px, py] = zoomAnchor(hereAt()?.anchor, from, w, hh);
    const v = zoomAt(from, f, px, py, w, hh, kMin, kMax);
    remember(v);
    goTo(v);
  };
  const whole = () => {
    userView = null;
    fitted = true;
    goTo(wholeLabelled());
  };
  view
    .querySelector(".chord-graph-zoom-in")
    ?.addEventListener("click", () => zoomBy(ZOOM_STEP), { signal });
  view
    .querySelector(".chord-graph-zoom-out")
    ?.addEventListener("click", () => zoomBy(1 / ZOOM_STEP), { signal });
  view.querySelector(".chord-graph-fit")?.addEventListener("click", whole, { signal });

  // ---- clicks: a page opens (through the engine's navigate, when given), a folder is framed ----
  svg.addEventListener(
    "click",
    (ev) => {
      const key = itemOf(ev.target);
      if (dragged && ev.detail !== 0) {
        // The end of a drag is not a click (a keyboard click has detail 0).
        ev.preventDefault();
        ev.stopPropagation();
        dragged = false;
        return;
      }
      // A click on empty space stops showing the found note (what the reader does
      // after finding).
      if (!key && found && ev.button === 0) {
        setFound(null);
        restack();
        labels();
        return;
      }
      if (!key || ev.defaultPrevented || ev.button !== 0) return;
      const it = byKey.get(key)!;
      if (it.folder !== undefined) {
        ev.preventDefault();
        ev.stopPropagation();
        frame(scene.folderBox.get(it.folder) ?? it.box);
        return;
      }
      if (ev.metaKey || ev.ctrlKey || ev.shiftKey || ev.altKey) return;
      const w = elOf.get(key)!;
      const raw = w.href?.baseVal ?? w.getAttribute("href");
      const go = a.navigate;
      if (!raw || !go) return;
      const url = new URL(raw, window.location.href);
      if (url.origin !== window.location.origin) return;
      ev.preventDefault();
      ev.stopPropagation();
      // Leave the modal dialog before leaving the page (chord.ts, closeDialogs).
      view.closest("dialog")?.close();
      go(url);
    },
    { signal },
  );

  // ---- keyboard: one tab stop, moving through the site's tree ----
  const onScreen = (b: Box) => {
    const [w, hh] = size();
    const x0 = (b.x - cam.cx) * cam.k + w / 2;
    const y0 = (b.y - cam.cy) * cam.k + hh / 2;
    const sw = b.w * cam.k;
    const sh = b.h * cam.k;
    return x0 >= 0 && y0 >= 0 && x0 + sw <= w && y0 + sh <= hh && Math.max(sw, sh) >= 6;
  };
  const stopKey = (here && scene.pageItem.get(here)) || scene.navTop[0]!;
  elOf.get(stopKey)?.setAttribute("tabindex", "0");
  tabStop = stopKey;
  view.addEventListener(
    "keydown",
    (ev) => {
      // Typing in the search box is text, never the graph's keys (+ − 0).
      if ((ev.target as Element | null)?.closest?.(".chord-graph-find")) return;
      const inSvg = svg.contains(ev.target as Node);
      // With Ctrl, Cmd or Alt these are the browser's own zoom keys (text
      // resize): never taken from it.
      const zoomKey =
        ["+", "=", "-", "0"].includes(ev.key) && !ev.ctrlKey && !ev.metaKey && !ev.altKey;
      if (zoomKey) {
        if (ev.key === "+" || ev.key === "=") zoomBy(ZOOM_STEP);
        else if (ev.key === "-") zoomBy(1 / ZOOM_STEP);
        else whole();
        ev.preventDefault();
        return;
      }
      if (!inSvg) return;
      const from = itemOf(ev.target);
      if (!from) return;
      const it = byKey.get(from)!;
      if (it.folder !== undefined && (ev.key === "Enter" || ev.key === " ")) {
        ev.preventDefault();
        frame(scene.folderBox.get(it.folder) ?? it.box);
        return;
      }
      const to = navigate(scene, ev.key, from);
      if (!to) return;
      ev.preventDefault();
      elOf.get(from)?.setAttribute("tabindex", "-1");
      const el = elOf.get(to)!;
      el.setAttribute("tabindex", "0");
      // Drawn before it takes the focus (it may be far off screen).
      tabStop = to;
      cull();
      el.focus({ preventScroll: true });
      const target = byKey.get(to)!;
      if (!onScreen(target.box)) {
        frame(
          target.page !== undefined ? (focusBox(scene, target.page) ?? target.box) : target.box,
          !reduced,
          target.page !== undefined ? pagesOf(target.page) : [],
        );
      }
    },
    { signal },
  );
  // A note found in the box (find.ts) is shown, not opened (owner, 2026-11-17):
  // the camera centres it, at the scale that shows its links and their notes as the
  // keys frame a page; it is marked «found», its links in ink over the rest; and it
  // takes the focus, so that Enter on it (or a click) opens its page, as on any
  // note. An emptied box clears the mark.
  const setFound = (key: string | null) => {
    if (found) {
      elOf.get(found)?.classList.remove("found");
      for (const p of edgesOf.get(pageOf(found) ?? "") ?? []) p.classList.remove("found");
    }
    found = key;
    if (!key) return;
    elOf.get(key)?.classList.add("found");
    for (const p of edgesOf.get(pageOf(key) ?? "") ?? []) p.classList.add("found");
  };
  view.addEventListener(
    FIND_EVENT,
    (ev) => {
      const slug = (ev as CustomEvent<string | null>).detail;
      if (!slug) {
        setFound(null);
        restack();
        labels();
        return;
      }
      const node = h.nodeOf.get(slug);
      const key = node ? scene.pageItem.get(node) : undefined;
      const el = key ? elOf.get(key) : undefined;
      if (!node || !key || !el) return;
      // The scale that frames it with its links (their box made even round it), the
      // note itself in the middle.
      const own = byKey.get(key)!.box;
      const cx = own.x + own.w / 2;
      const cy = own.y + own.h / 2;
      const around = focusBox(scene, node) ?? own;
      const hw = Math.max(cx - around.x, around.x + around.w - cx);
      const hh = Math.max(cy - around.y, around.y + around.h - cy);
      const even = { x: cx - hw, y: cy - hh, w: 2 * hw, h: 2 * hh };
      const v = { ...framing(even, pagesOf(node)), cx, cy };
      setFound(key);
      restack();
      elOf.get(tabStop)?.setAttribute("tabindex", "-1");
      el.setAttribute("tabindex", "0");
      tabStop = key;
      cull();
      remember(v);
      goTo(v);
      el.focus({ preventScroll: true });
    },
    { signal },
  );
  view.addEventListener(
    "focusin",
    (ev) => {
      const k = itemOf(ev.target);
      if (k) hover(k);
    },
    { signal },
  );
  // The expanded view's «You are here» switch (chord.ts): the marker goes or
  // comes back, and the current page's callout with it; the camera stays where it
  // is (owner, 2026-11-17: a redraw sent the graph off screen and back).
  register(
    view,
    {
      setHere(shown) {
        if (shown === hereShown) return;
        hereShown = shown;
        mark.setShown(shown);
        labels();
      },
      // A link of the page body under the pointer (owner, 2026-11-17): its note
      // outlined as a hovered one, the current page's link to it in the accent,
      // heavier, on top.
      peek(slug) {
        const node = slug ? h.nodeOf.get(slug) : undefined;
        const key = node && node !== here ? (scene.pageItem.get(node) ?? null) : null;
        if (key === peeked) return;
        if (peeked) {
          elOf.get(peeked)?.classList.remove("peek");
          for (const p of peekLinks(peeked)) p.classList.remove("peek");
        }
        peeked = key;
        if (key) {
          elOf.get(key)?.classList.add("peek");
          for (const p of peekLinks(key)) p.classList.add("peek");
        }
        restack();
      },
    },
    signal,
  );
  view.addEventListener(
    "focusout",
    (ev) => {
      if (itemOf(ev.target) && !svg.contains(ev.relatedTarget as Node | null)) hover(null);
    },
    { signal },
  );
  // A redraw aborts this one: everything it started stops with it.
  // A view drawn while hidden (a collapsed sidebar, the closed dialog) had no
  // size: it is framed for real the first time it has one.
  let measured = svg.getBoundingClientRect().width > 0;
  // The framed size: the observer also reports when it starts observing, with no
  // change at all, and fitting again then cut the opening animation short.
  let sized = size().join("x");
  const resize = new ResizeObserver(() => {
    rectNow = null;
    if (!measured && svg.getBoundingClientRect().width > 0) {
      measured = true;
      sized = size().join("x");
      firstFrame(false);
      return;
    }
    const now = size().join("x");
    const changed = now !== sized;
    sized = now;
    if (fitted && changed) {
      goTo(wholeLabelled(), false);
      return;
    }
    apply(cam);
    settle();
  });
  resize.observe(svg);
  // What this view owns stops on a redraw (the signal) and on the engine's
  // cleanup at a page change or the end of the mount, whichever comes first.
  const stop = () => {
    resize.disconnect();
    cancelAnimationFrame(anim);
    window.clearTimeout(settleTimer);
  };
  signal.addEventListener("abort", stop);
  onCleanup(view, stop);

  // ---- first frame: from where this view's camera was, to the whole graph, or
  // to what the reader was looking at on this page and in this view (userView) ----
  const prev = cameras.get(view);
  const sameScene = prev?.layout === thisScene;
  apply(sameScene ? prev!.view : all());
  hover(null);
  function firstFrame(animate: boolean) {
    if (userView && (userView.page !== a.current || userView.scene !== thisScene)) userView = null;
    if (!userView) {
      fitted = true;
      goTo(wholeLabelled(), animate);
      return;
    }
    fitted = false;
    // The same region, whichever box shows it: fitted, so all of it is in view.
    const [w, hh] = size();
    const [kMin, kMax] = limits();
    goTo(fit(userView.region, w, hh, 0, kMin, kMax), animate);
  }
  // Only a new page in the sidebar flies from where its camera was. Opening the
  // expanded view (on this page or another) and returning to the sidebar on closing it (the
  // same page) set the camera once, already in place (owner, 2026-11-18: on
  // expanding, the zoom jumped before settling). The saved camera of a closed
  // dialog is that of a frame with no size (the observer fits it at 300×300 on
  // closing): flying from it meant zooming out ×2.7 (rings) or ×3.9 (balloons) and back
  // in 300–400 ms.
  firstFrame(sameScene && !reduced && !inDialog && prev!.page !== a.current);
}
