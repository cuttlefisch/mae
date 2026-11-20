// SPDX-License-Identifier: GPL-3.0-or-later
// One renderable scene for either hierarchical layout (sunburst or balloon), so
// the browser script draws and navigates both the same way. Pure: no DOM.
//
// Every item is one clickable shape. A PAGE item opens a page (a note, or a
// folder's own page); a FOLDER item frames a folder (the camera moves, the
// drawing does not). Keyboard order follows the site's tree: a folder's own
// page first, then its contents in the hierarchy's order.
import { buildHierarchy, type HNode, type Hierarchy } from "./hierarchy";
import type { IndexEntry } from "./geometry";
import { layoutSunburst, ownId, polar, sectorPath, type Orientation } from "./sunburst";
import { layoutConstellation } from "./constellation";
import type { Box } from "./viewport";

/** The hierarchical views (the chord ring is drawn by browser/chord.ts itself). */
export type Layout = "sunburst" | "balloon";

export interface Label {
  x: number;
  y: number;
  /** Degrees. */
  rotate: number;
  anchor: "start" | "middle" | "end";
  /** Shown when this much drawing length, times the scale, holds the text (px). */
  room: number;
  /** And when this much drawing height, times the scale, holds a line (px). */
  height: number;
  /** Drawn over its own shape (text in the page colour) rather than over the page. */
  onShape?: boolean;
  /**
   * Sunburst: the text is written along this arc, centred on it, instead of in
   * a straight line at x, y turned by `rotate` (owner, 2026-11-07: the letters
   * follow the ring).
   */
  path?: string;
}

export interface Item {
  key: string;
  /** Hierarchy node whose page this item opens; absent on folder items. */
  page?: string;
  /** Hierarchy folder this item frames; absent on page items. */
  folder?: string;
  /** Wedges: "note" for a page, "folder" for a folder's arc, "port", "centre" (balloon ring middle). */
  shape: "note" | "folder" | "port" | "centre";
  title: string;
  href?: string;
  d: string;
  /** Balloon: the ring's centre, which the shape is drawn around. */
  at?: [number, number];
  box: Box;
  /** Where its label sits when it has room. Absent: never labelled at rest. */
  label?: Label;
  /** Where else it may sit when the first place has no room (a ring's title, below the ring). */
  alt?: Label;
  /**
   * Where a highlighted label goes: at `angle`, from `radius` outward, around
   * the point `around`; its leader line starts at radius `from` (the shape's
   * own outer edge).
   */
  callout: { angle: number; radius: number; from: number; around: [number, number] };
  /** Sunburst: the segment's angles and outer radius ([a0, a1, r1]). */
  span?: [number, number, number];
  /** Band for "visited", as [r0, r1] around `at` (or the centre). */
  band?: [number, number, number, number];
}

export interface SceneEdge {
  source: string;
  target: string;
  d: string;
  box: Box;
  /** Smaller boxes that together hold the path (cull.ts, pieceBoxes). */
  boxes: Box[];
}

export interface Scene {
  layout: Layout;
  items: Item[];
  edges: SceneEdge[];
  /** Hierarchy node id -> the item that opens its page. */
  pageItem: Map<string, string>;
  /** Folder id -> the item that frames it, and the box of everything in it. */
  folderItem: Map<string, string>;
  folderBox: Map<string, Box>;
  /** Hierarchy node id -> ids it has real links with. */
  neighbours: Map<string, Set<string>>;
  /** Keyboard tree: item key -> parent item key, and ordered children. */
  navParent: Map<string, string>;
  navChildren: Map<string, string[]>;
  /** The top-level items, in order. */
  navTop: string[];
  bounds: Box;
  hierarchy: Hierarchy;
}

const TAU = Math.PI * 2;

/** Bounds of an annular sector (exact: its corners plus any axis it crosses). */
export function sectorBox(r0: number, r1: number, a0: number, a1: number, cx = 0, cy = 0): Box {
  const pts: Array<[number, number]> = [];
  const angles = [a0, a1];
  for (let q = Math.ceil(a0 / (Math.PI / 2)); q * (Math.PI / 2) <= a1; q++)
    angles.push(q * (Math.PI / 2));
  for (const a of angles) for (const r of [r0, r1]) pts.push(polar(r, a));
  const xs = pts.map((p) => p[0] + cx);
  const ys = pts.map((p) => p[1] + cy);
  const x = Math.min(...xs);
  const y = Math.min(...ys);
  return { x, y, w: Math.max(...xs) - x, h: Math.max(...ys) - y };
}

/** Bounds of at least one box (every caller has one: the item itself). */
function unionBoxes(boxes: [Box, ...Box[]]): Box {
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (const b of boxes) {
    x0 = Math.min(x0, b.x);
    y0 = Math.min(y0, b.y);
    x1 = Math.max(x1, b.x + b.w);
    y1 = Math.max(y1, b.y + b.h);
  }
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
}

/**
 * The arc a label is written along: on the circle of radius r, centred on angle
 * `mid` (radians, clockwise from the top), running clockwise on the top half
 * and counter-clockwise on the bottom half, so the text always reads left to
 * right, never upside down (the same rule as `tangential`). It reaches past its
 * segment (`span`), up to LABEL_ARC_MAX either side of `mid`. A label's room is
 * never longer than that (sunburstScene), and the client shows a label only
 * when its measured width fits its room (hier.ts), so no text is cut where the
 * path ends (SVG drops the glyphs that fall off a textPath).
 *
 * Never close to a half circle nor to a whole one: there an SVG arc's centre is
 * badly determined by its endpoints. Rounded to 0.01, a half circle put its
 * centre a whole unit off (zoomed in, labels sat several lines off their ring;
 * the browser test found it), and 0.05 short of a whole circle, a tenth of a
 * unit (the property test found it). Between LABEL_ARC_MIN and LABEL_ARC_MAX
 * either side, rounded to 0.001, it moves by under 0.005: zoomed all the way
 * in a line of text is a few tenths of a unit tall, so 0.05 (rounding to 0.01)
 * still showed, as a fifth of a line (the browser test again).
 */
export const LABEL_ARC_MIN = Math.PI / 2 + 0.3;
export const LABEL_ARC_MAX = Math.PI - 0.3;
export function arcLabelPath(r: number, mid: number, span: number): string {
  const f = (v: number) => (Math.round(v * 1000) / 1000).toString();
  const half = Math.min(LABEL_ARC_MAX, Math.max(LABEL_ARC_MIN, span / 2 + 0.2));
  const deg = ((((mid * 180) / Math.PI) % 360) + 360) % 360;
  const flip = deg > 90 && deg < 270; // bottom half: read counter-clockwise
  const [x0, y0] = polar(r, flip ? mid + half : mid - half);
  const [x1, y1] = polar(r, flip ? mid - half : mid + half);
  // More than a half circle (large-arc flag 1); sweep 1 runs towards
  // increasing angle, clockwise on screen.
  return `M${f(x0)},${f(y0)}A${f(r)},${f(r)} 0 1 ${flip ? 0 : 1} ${f(x1)},${f(y1)}`;
}

/** Upright tangential text at angle a (radians, clockwise from the top). */
function tangential(a: number): number {
  const deg = ((((a * 180) / Math.PI) % 360) + 360) % 360;
  return deg > 90 && deg < 270 ? deg - 180 : deg;
}

function neighboursOf(h: Hierarchy): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>();
  for (const e of h.edges) {
    if (e.structural) continue;
    if (!out.has(e.source)) out.set(e.source, new Set());
    if (!out.has(e.target)) out.set(e.target, new Set());
    out.get(e.source)!.add(e.target);
    out.get(e.target)!.add(e.source);
  }
  return out;
}

export interface SceneOptions {
  exclude?: (slug: string) => boolean;
  locale?: string;
  orientation?: Orientation;
}

export function buildScene(
  index: Record<string, IndexEntry>,
  layout: Layout,
  opts: SceneOptions = {},
): Scene {
  const h = buildHierarchy(index, opts);
  return layout === "sunburst"
    ? sunburstScene(h, opts.orientation ?? "top-inside")
    : balloonScene(h);
}

function navFrom(
  h: Hierarchy,
  primary: (n: HNode) => string | undefined,
  own: (f: HNode) => string | undefined,
  rootIsItem: boolean,
) {
  const navParent = new Map<string, string>();
  const navChildren = new Map<string, string[]>();
  const kidsOf = (f: HNode) => {
    const out: string[] = [];
    const o = own(f);
    if (o) out.push(o);
    for (const c of f.children) {
      const k = primary(c);
      if (k) out.push(k);
    }
    return out;
  };
  const walk = (f: HNode) => {
    const key = primary(f);
    const kids = kidsOf(f);
    if (key) {
      navChildren.set(key, kids);
      for (const k of kids) if (k !== key) navParent.set(k, key);
    }
    for (const c of f.children) if (c.kind === "folder") walk(c);
  };
  walk(h.root);
  const navTop = rootIsItem ? [primary(h.root)!] : kidsOf(h.root);
  return { navParent, navChildren, navTop };
}

function sunburstScene(h: Hierarchy, orientation: Orientation): Scene {
  const sb = layoutSunburst(h, { orientation });
  const items: Item[] = [];
  const pageItem = new Map<string, string>();
  const folderItem = new Map<string, string>();
  const folderBox = new Map<string, Box>();
  for (const s of sb.segments) {
    const mid = (s.a0 + s.a1) / 2;
    const rm = (s.r0 + s.r1) / 2;
    const [lx, ly] = polar(rm, mid);
    const isOwn = s.kind === "own";
    const owner = isOwn ? s.id.slice(0, -"\u0000own".length) : s.id;
    const item: Item = {
      key: s.id,
      shape: s.kind === "folder" ? "folder" : "note",
      title: s.title || "/",
      href: s.href,
      d: sectorPath(s.r0, s.r1, s.a0, s.a1),
      box: sectorBox(s.r0, s.r1, s.a0, s.a1),
      span: [s.a0, s.a1, s.r1],
      label: {
        x: lx,
        y: ly,
        rotate: tangential(mid),
        anchor: "middle",
        // No longer than its arc, or the end of the text would be cut off.
        room: rm * Math.min(s.a1 - s.a0, 2 * LABEL_ARC_MAX),
        height: s.r1 - s.r0,
        // A note's label sits on its ink fill; a folder's on its pale one.
        onShape: s.kind !== "folder",
        path: arcLabelPath(rm, mid, s.a1 - s.a0),
      },
      // top-inside: nothing is ever drawn outside a note (notes are leaves), so
      // its label starts at its own edge; leaves-inside: the folder rings are
      // outside the notes, so labels go beyond the outermost ring.
      callout: {
        angle: mid,
        radius: orientation === "top-inside" ? s.r1 : sb.outer,
        from: s.r1,
        around: [0, 0],
      },
      band: s.kind === "folder" ? undefined : [s.r0, s.r0 + 0.25 * (s.r1 - s.r0), s.a0, s.a1],
    };
    if (s.kind === "folder") {
      item.folder = s.id;
      folderItem.set(s.id, s.id);
      // Everything in the folder: its own ring and every ring of its contents.
      const rs = orientation === "top-inside" ? [s.r0, sb.outer] : [sb.disc, s.r1];
      folderBox.set(s.id, sectorBox(rs[0]!, rs[1]!, s.a0, s.a1));
    } else {
      item.page = owner;
      pageItem.set(owner, s.id);
    }
    items.push(item);
  }
  folderBox.set(h.root.id, sectorBox(sb.disc, sb.outer, 0, TAU));
  const nav = navFrom(
    h,
    (n) => (n.depth === 0 ? undefined : n.id),
    (f) => (f.own ? ownId(f.id) : undefined),
    false,
  );
  return {
    layout: "sunburst",
    items,
    edges: sb.chords.map((c) => ({
      source: c.source,
      target: c.target,
      d: c.d,
      box: c.box,
      boxes: c.boxes,
    })),
    pageItem,
    folderItem,
    folderBox,
    neighbours: neighboursOf(h),
    ...nav,
    bounds: { x: -sb.outer, y: -sb.outer, w: 2 * sb.outer, h: 2 * sb.outer },
    hierarchy: h,
  };
}

// Room for a balloon note's radial label: about half the space the layout
// keeps between rings (constellation.ts, spacing 60).
const BALLOON_LABEL_ROOM = 30;

function radialLabel(
  cx: number,
  cy: number,
  r: number,
  a: number,
  room: number,
  height: number,
): Label {
  const [x, y] = polar(r, a);
  const deg = ((((a * 180) / Math.PI) % 360) + 360) % 360;
  const left = deg > 180;
  return {
    x: x + cx,
    y: y + cy,
    rotate: left ? deg + 90 : deg - 90,
    anchor: left ? "end" : "start",
    room,
    height,
  };
}

function balloonScene(h: Hierarchy): Scene {
  const cs = layoutConstellation(h);
  const items: Item[] = [];
  const pageItem = new Map<string, string>();
  const folderItem = new Map<string, string>();
  const folderBox = new Map<string, Box>();
  const discBy = new Map(cs.discs.map((d) => [d.id, d]));
  const centreKey = (id: string) => `${id}\u0000centre`;
  for (const d of cs.discs) {
    const node = h.nodes.get(d.id)!;
    // The ring's middle: the folder's own page if it has one, else it frames the folder.
    const r = d.r0 * 0.92;
    const centre: Item = {
      key: centreKey(d.id),
      shape: "centre",
      title: d.title || "/",
      href: d.href,
      d: `M${-r},0A${r},${r} 0 1 1 ${r},0A${r},${r} 0 1 1 ${-r},0Z`,
      at: [d.cx, d.cy],
      box: { x: d.cx - d.r1, y: d.cy - d.r1, w: 2 * d.r1, h: 2 * d.r1 },
      label: { x: d.cx, y: d.cy, rotate: 0, anchor: "middle", room: 2 * d.r0 * 0.9, height: d.r0 },
      // Below the ring, in the space the layout keeps between rings.
      alt: {
        x: d.cx,
        y: d.cy + d.r1 + 14,
        rotate: 0,
        anchor: "middle",
        room: 2 * d.r1 + 50,
        height: 24,
      },
      callout: { angle: 0, radius: d.r1, from: d.r1, around: [d.cx, d.cy] },
    };
    if (node.own) {
      centre.page = d.id;
      pageItem.set(d.id, centre.key);
    } else {
      centre.folder = d.id;
    }
    folderItem.set(d.id, centre.key);
    items.push(centre);
    for (const w of d.wedges) {
      const mid = (w.a0 + w.a1) / 2;
      const it: Item = {
        key: w.kind === "port" ? `${w.id}\u0000port` : w.id,
        shape: w.kind === "port" ? "port" : "note",
        title: w.title || "/",
        href: w.href,
        d: sectorPath(d.r0, d.r1, w.a0, w.a1),
        at: [d.cx, d.cy],
        box: sectorBox(d.r0, d.r1, w.a0, w.a1, d.cx, d.cy),
        callout: { angle: mid, radius: d.r1, from: d.r1, around: [d.cx, d.cy] },
        // Radial, just outside the ring, over the page: shown once the wedge is
        // a line of text wide on screen. (Without it, a ring's notes could only
        // be named by hovering them, however far one zoomed in.)
        label:
          w.kind === "port"
            ? undefined
            : radialLabel(d.cx, d.cy, d.r1 + 3, mid, BALLOON_LABEL_ROOM, d.r1 * (w.a1 - w.a0)),
        band: w.kind === "port" ? undefined : [d.r0, d.r0 + 0.3 * (d.r1 - d.r0), w.a0, w.a1],
      };
      if (w.kind === "port") it.folder = w.id;
      else {
        it.page = w.id;
        pageItem.set(w.id, it.key);
      }
      items.push(it);
    }
  }
  // A folder's box: its ring and every ring below it.
  const boxOf = (n: HNode): Box => {
    const d = discBy.get(n.id)!;
    const own: Box = { x: d.cx - d.r1, y: d.cy - d.r1, w: 2 * d.r1, h: 2 * d.r1 };
    const b = unionBoxes([own, ...n.children.filter((c) => c.kind === "folder").map(boxOf)]);
    folderBox.set(n.id, b);
    return b;
  };
  boxOf(h.root);
  const nav = navFrom(
    h,
    (n) => (n.kind === "folder" ? centreKey(n.id) : n.id),
    () => undefined,
    true,
  );
  return {
    layout: "balloon",
    items,
    edges: cs.links.map((l) => ({
      source: l.source,
      target: l.target,
      d: l.d,
      box: l.box,
      boxes: l.boxes,
    })),
    pageItem,
    folderItem,
    folderBox,
    neighbours: neighboursOf(h),
    ...nav,
    bounds: cs.bounds,
    hierarchy: h,
  };
}

/** The box to frame for a page: it, its neighbours, and the links between them. */
export function focusBox(scene: Scene, node: string): Box | null {
  const byKey = new Map(scene.items.map((i) => [i.key, i]));
  const own = scene.pageItem.get(node);
  if (!own) return null;
  const boxes: [Box, ...Box[]] = [byKey.get(own)!.box];
  for (const n of scene.neighbours.get(node) ?? []) {
    const k = scene.pageItem.get(n);
    if (k) boxes.push(byKey.get(k)!.box);
  }
  for (const e of scene.edges) if (e.source === node || e.target === node) boxes.push(e.box);
  return unionBoxes(boxes);
}

/**
 * The next item for a key press in the keyboard tree, or null. As in the ARIA
 * treeview pattern: Down/Up to the next/previous item at the same level (wrapping),
 * Right into a folder (its first item: its own page, if any), Left out to the
 * folder holding it, Home/End to the first/last at that level.
 */
export function navigate(scene: Scene, key: string, from: string): string | null {
  const parent = scene.navParent.get(from);
  const siblings = parent ? scene.navChildren.get(parent)! : scene.navTop;
  const i = siblings.indexOf(from);
  switch (key) {
    case "ArrowDown":
      return i < 0 ? null : siblings[(i + 1) % siblings.length]!;
    case "ArrowUp":
      return i < 0 ? null : siblings[(i - 1 + siblings.length) % siblings.length]!;
    case "Home":
      return siblings[0] ?? null;
    case "End":
      return siblings[siblings.length - 1] ?? null;
    case "ArrowLeft":
      return parent ?? null;
    case "ArrowRight": {
      const kids = scene.navChildren.get(from)?.filter((k) => k !== from);
      return kids && kids.length > 0 ? kids[0]! : null;
    }
    default:
      return null;
  }
}

// ---- The zoom's limits, from what is drawn (owner, 2026-11-16) ----
// Before, from 0.5 to 120 times the fitted scale: a fixed number, which on a small
// site let the reader zoom in far beyond what there is to read and on a large one could
// fall short.

/**
 * How far in it zooms: the scale at which the smallest leaf's name
 * reads whole (fits in its place, at the usual text size), times this margin,
 * so it reads with room to spare and not right at the limit.
 */
export const LEGIBLE_MARGIN = 1.5;
/** How far out it zooms: the whole graph fitted, with a margin around it. */
export const FIT_MARGIN = 0.8;
/**
 * A tiny graph, all of it readable once fitted, still zooms in to at least twice
 * the fitted scale: so that + and the wheel do something.
 */
export const MIN_ZOOM_IN = 2;
/** A name's slack in its place, in px: along it (the text) and across it (the line). */
export const LABEL_SLACK_PX = 8;
export const LINE_SLACK_PX = 3;

/**
 * The scale (px per drawing unit) from which a name `textPx` px
 * wide, in lines of `linePx` px, fits whole in `l`: names have a fixed
 * size on screen, so they read once their place, at scale, holds them.
 */
export function legibleAt(l: Label, textPx: number, linePx: number): number {
  return Math.max((textPx + LABEL_SLACK_PX) / l.room, (linePx + LINE_SLACK_PX) / l.height);
}

/**
 * The smallest leaf: the note (a page, not a folder) whose name has the least
 * room in its narrowest dimension. In the sunburst it is its arc's length (the
 * ring's width is the same for all); in the balloons, the wedge's angular width
 * (the name runs outward, with a fixed room along it).
 */
export function smallestLeaf(items: readonly Item[]): Item | undefined {
  let best: Item | undefined;
  let least = Infinity;
  for (const i of items) {
    if (i.shape !== "note" || i.page === undefined || !i.label) continue;
    const extent = Math.min(i.label.room, i.label.height);
    if (extent < least) {
      least = extent;
      best = i;
    }
  }
  return best;
}

/**
 * [minimum, maximum] zoom scale for `items`, with `fitK` the fitted scale (the
 * fit button's) and `textPx` a title's width on screen.
 */
export function zoomRange(
  items: readonly Item[],
  fitK: number,
  textPx: (title: string) => number,
  linePx: number,
): [number, number] {
  const leaf = smallestLeaf(items);
  const legible = leaf ? LEGIBLE_MARGIN * legibleAt(leaf.label!, textPx(leaf.title), linePx) : 0;
  return [FIT_MARGIN * fitK, Math.max(MIN_ZOOM_IN * fitK, legible)];
}
