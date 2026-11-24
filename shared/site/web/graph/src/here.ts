// SPDX-License-Identifier: GPL-3.0-or-later
// «You are here»: where the current page's marker goes (owner, 2026-11-15:
// with only the page's name it was hard to see where the reader was). Pure:
// no DOM. The script (browser/here.ts) measures what is drawn on an occupancy
// grid, in screen px, and here it is decided where the label goes.
//
// The practice followed:
// - Diagram labelling: the label next to what it names; if it is in the way there, a
//   leader line to a free space; a separate legend, only as a last resort; an
//   order of preferred places to avoid collisions (Wong, «Points of view: labels and
//   callouts», Nature Methods 9, 2012; Datawrapper, «callout lines»).
// - Orientation: the «you are here» mark is the most important part of an orientation
//   map, and must be strong, labelled and unambiguous (map standards of the
//   US National Park Service; NN/g, «Navigation: you are here»).
// - Colour is never the only signal: shape (ring, pin) and text.
//
// One marker only, the pin (owner's decision, 2026-11-16: «the pin is by far
// the best; the reader can show or hide it, but if it is shown, it is the
// pin»): stuck into the note with the label above or below; if it covers
// something there, to the nearest free space, with a leader (and if there is none, to the one that covers
// least): the label always shows. The callout and the radial, offered as choices
// in 0.3.12, no longer exist.
import { FIT_FLOOR, type Box } from "./viewport";

/** Where the browser keeps the reader's choice: show it («shown») or hide it. */
export const HERE_KEY = "chord-graph-here";

export interface HereOptions {
  /** The marker, drawn (hereMarker "pin", the default) or not ("none"). */
  shown: boolean;
  /** The «You are here» switch in the expanded view (hereToggle). */
  toggle: boolean;
}

/**
 * The marker's options, checked when the graph is mounted: what no longer exists
 * or is unknown is an error that says so, not a silent default (a 0.x
 * version may remove options; the error is the notice).
 */
export function resolveHere(opts: {
  hereMarker?: unknown;
  hereToggle?: unknown;
  hereMarkers?: unknown;
}): HereOptions {
  const m = opts.hereMarker ?? "pin";
  if (m === "callout" || m === "radial")
    throw new Error(
      `chord-graph: hereMarker "${m}" was removed; the «You are here» marker is the pin. ` +
        `Set hereMarker to "pin" (the default) or "none" (no marker).`,
    );
  if (m !== "pin" && m !== "none")
    throw new Error(`chord-graph: hereMarker must be "pin" or "none", not ${JSON.stringify(m)}.`);
  if (opts.hereMarkers !== undefined)
    throw new Error(
      `chord-graph: hereMarkers was removed; the «You are here» marker is the pin. ` +
        `Use hereToggle: true to let the reader show or hide it.`,
    );
  const t = opts.hereToggle ?? false;
  if (typeof t !== "boolean")
    throw new Error(`chord-graph: hereToggle must be true or false, not ${JSON.stringify(t)}.`);
  if (m === "none" && t)
    throw new Error(
      `chord-graph: hereMarker "none" draws no marker, so there is nothing to toggle; ` +
        `drop hereToggle or set hereMarker to "pin".`,
    );
  return { shown: m === "pin", toggle: t };
}

/**
 * Is the marker drawn? `r` is what the mount left on the page (resolveHere;
 * without it, the pin). In the expanded view, the reader's choice while the
 * site offers the switch; in the sidebar, always the site's, as
 * «Names» (browser/names.ts): there is no switch there to show it again with.
 */
export function showHere(
  stored: string | null,
  r: Partial<HereOptions> | undefined,
  inDialog: boolean,
): boolean {
  if (r?.shown === false) return false;
  if (!inDialog || r?.toggle !== true) return true;
  return stored !== "hidden";
}

// ---- the occupancy grid ----

/**
 * What is taken in the frame (`w` x `h` px), in cells of `cell` px: each cell, 1 if something
 * drawn (a wedge, a link, a name, a control) touches it. `sat` is its
 * summed-area table, to count what is taken in a box in constant time.
 */
export interface Grid {
  w: number;
  h: number;
  cell: number;
  cols: number;
  rows: number;
  sat: Int32Array;
}

export function makeGrid(w: number, h: number, cell: number, occupied: ArrayLike<number>): Grid {
  const cols = Math.max(1, Math.ceil(w / cell));
  const rows = Math.max(1, Math.ceil(h / cell));
  const sat = new Int32Array((cols + 1) * (rows + 1));
  for (let r = 0; r < rows; r++) {
    let row = 0;
    for (let c = 0; c < cols; c++) {
      row += occupied[r * cols + c] ? 1 : 0;
      sat[(r + 1) * (cols + 1) + c + 1] = sat[r * (cols + 1) + c + 1]! + row;
    }
  }
  return { w, h, cell, cols, rows, sat };
}

/**
 * The cells [first, end) of `n` that the span [start, start + length) touches, with overlaps()'s
 * strict overlap: cell i when i·cell < start + length and start < (i + 1)·cell. The division gives
 * a first estimate and the comparisons make it exact: dividing rounds (an edge within a rounding
 * step of a cell boundary landed in the neighbouring cell, and (x + w) / cell with x + w = 5e-324
 * gave 0; CI, 2026-11-23).
 */
function cellSpan(start: number, length: number, cell: number, n: number): [number, number] {
  const end = start + length;
  let first = Math.min(n, Math.max(0, Math.floor(start / cell)));
  while (first > 0 && first * cell > start) first--;
  while (first < n && (first + 1) * cell <= start) first++;
  let last = Math.min(n, Math.max(0, Math.ceil(end / cell)));
  while (last < n && last * cell < end) last++;
  while (last > 0 && (last - 1) * cell >= end) last--;
  return [first, last];
}

/** How many taken cells the box touches (what falls outside the frame does not count). */
export function busy(g: Grid, b: Box): number {
  const [c0, c1] = cellSpan(b.x, b.w, g.cell, g.cols);
  const [r0, r1] = cellSpan(b.y, b.h, g.cell, g.rows);
  if (c1 <= c0 || r1 <= r0) return 0;
  const W = g.cols + 1;
  return g.sat[r1 * W + c1]! - g.sat[r0 * W + c1]! - g.sat[r1 * W + c0]! + g.sat[r0 * W + c0]!;
}

// ---- the placement ----

/** Clear margin around the label, and up to the frame's edge (px). */
export const GAP = 4;
/** The pin's height (px): from its point, on the note, to the top of its head. */
export const PIN_H = 20;
/**
 * How much the graph may shrink, at most, to leave room for the label in the
 * sidebar (FIT_FLOOR, viewport.ts, is for the dialog's names): there the
 * drawing fills the frame, and without shrinking no free space is left beside the note.
 */
export const HERE_FLOOR = 0.6;
/**
 * Below this width (px) the frame is so narrow that shrinking the graph for
 * the label would leave it at less than half the frame (a browser test, «spans the
 * frame»): there it gives up only the usual (FIT_FLOOR), and the label goes where it fits.
 */
export const HERE_NARROW = 220;
/** How much the graph may shrink for the label, in a frame `width` px wide. */
export const hereFloor = (width: number): number => (width >= HERE_NARROW ? HERE_FLOOR : FIT_FLOOR);
/** Radius of the ring on the note (px). */
export const HALO = 9;

export interface Placement {
  /** The label, in frame px. */
  box: Box;
  /** The leader, from the marker to the label; empty if the label sits next to it. */
  leader: Array<[number, number]>;
  /** Where it went: its own place or the fallback space (for the tests and the styles). */
  where: "above" | "below" | "nearest" | "least";
  /** The pin: which way it points (up: its head above the note). */
  pin?: "up" | "down";
}

export interface PlaceArgs {
  grid: Grid;
  /** The current note (its outer edge), in frame px. */
  anchor: [number, number];
  /** The label's width and height. */
  size: [number, number];
  /** Radius of the ring on the note: neither the label nor its margin overlaps it. */
  halo: number;
  /**
   * The controls (frame px): they are in the grid too, but here they are
   * never overlapped, not even by the fallback («least», where anything else is overlapped if nothing else is left).
   */
  controls?: Box[];
}

const overlapsBox = (a: Box, b: Box) =>
  a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
const inFrame = (g: Grid, b: Box) =>
  b.x >= GAP && b.y >= GAP && b.x + b.w <= g.w - GAP && b.y + b.h <= g.h - GAP;
const grow = (b: Box, m: number): Box => ({
  x: b.x - m,
  y: b.y - m,
  w: b.w + 2 * m,
  h: b.h + 2 * m,
});
/** Distance from a point to a box (0 inside). */
export function distTo(b: Box, [x, y]: [number, number]): number {
  const dx = Math.max(b.x - x, 0, x - (b.x + b.w));
  const dy = Math.max(b.y - y, 0, y - (b.y + b.h));
  return Math.hypot(dx, dy);
}
/** The point of the box nearest to p. */
export function nearestPoint(b: Box, [x, y]: [number, number]): [number, number] {
  return [Math.min(b.x + b.w, Math.max(b.x, x)), Math.min(b.y + b.h, Math.max(b.y, y))];
}
/**
 * The free space nearest to `from`, at least `min` px away: the frame's
 * positions are walked `step` px at a time. With none free, the one that covers
 * least of what is taken (the label always shows).
 */
export function nearestFree(a: PlaceArgs, from: [number, number], min: number, step = 4) {
  const [w, h] = a.size;
  const g = a.grid;
  let best: Box | null = null;
  let bestD = Infinity;
  let least: Box | null = null;
  let leastN = Infinity;
  let leastD = Infinity;
  for (let y = GAP; y + h <= g.h - GAP + 1e-9; y += step) {
    for (let x = GAP; x + w <= g.w - GAP + 1e-9; x += step) {
      const b = { x, y, w, h };
      const d = distTo(b, from);
      if (d < min) continue;
      const n = busy(g, grow(b, GAP));
      if (n === 0 && distTo(b, a.anchor) >= a.halo + GAP) {
        if (d < bestD) {
          bestD = d;
          best = b;
        }
      } else if (
        (n < leastN || (n === leastN && d < leastD)) &&
        !(a.controls ?? []).some((c) => overlapsBox(c, b))
      ) {
        leastN = n;
        leastD = d;
        least = b;
      }
    }
  }
  if (best) return { box: best, where: "nearest" as const };
  // It does not even fit in the frame: top left, and whatever sticks out, sticks out.
  return { box: least ?? { x: GAP, y: GAP, w, h }, where: "least" as const };
}

/**
 * The pin: stuck into the note, the label above its head; failing that, the
 * pin upside down and the label below; failing that, the pin up and the
 * label in the free space nearest its head, with a leader.
 */
export function placePin(a: PlaceArgs): Placement {
  const [w, h] = a.size;
  const [ax, ay] = a.anchor;
  const above = { x: ax - w / 2, y: ay - PIN_H - 2 - h, w, h };
  // What the pin itself covers does not count: the label goes beyond its head.
  const pinned = (b: Box) => inFrame(a.grid, b) && busy(a.grid, grow(b, GAP)) === 0;
  if (pinned(above)) return { box: above, leader: [], where: "above", pin: "up" };
  const below = { x: ax - w / 2, y: ay + PIN_H + 2, w, h };
  if (pinned(below)) return { box: below, leader: [], where: "below", pin: "down" };
  const head: [number, number] = [ax, ay - PIN_H];
  const n = nearestFree(a, head, GAP);
  const end = nearestPoint(n.box, head);
  return { box: n.box, leader: [head, end], where: n.where, pin: "up" };
}

/**
 * The title, cut to `max` px with «…» at the end (by words if it can: it does not
 * leave half a word if that removes less than a third). `measure` gives the width in px.
 */
export function truncate(title: string, max: number, measure: (s: string) => number): string {
  const t = title.trim();
  if (measure(t) <= max) return t;
  let lo = 0;
  let hi = t.length;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (measure(t.slice(0, mid).trimEnd() + "…") <= max) lo = mid;
    else hi = mid - 1;
  }
  let cut = t.slice(0, lo);
  const space = cut.lastIndexOf(" ");
  if (space > (cut.length * 2) / 3) cut = cut.slice(0, space);
  return cut.trimEnd() + "…";
}

/**
 * What the label asks for beside the note, in drawing units at scale `k`
 * (screen px per unit), so that framing leaves it that space: the pin and
 * the label, above the note or below, as the note is above or below its
 * ring (of `centre`).
 */
export function reserve(
  anchor: [number, number],
  centre: [number, number],
  size: [number, number],
  k: number,
): Box {
  const w = size[0] / k;
  const h = size[1] / k;
  const [ax, ay] = anchor;
  const dy = ay - centre[1];
  // With its margins: GAP clear around it and GAP from the frame's edge.
  const m = (2 * GAP) / k;
  const tall = (PIN_H + 2) / k + h;
  return { x: ax - w / 2 - m, y: (dy <= 0 ? ay - tall : ay) - m, w: w + 2 * m, h: tall + 2 * m };
}
