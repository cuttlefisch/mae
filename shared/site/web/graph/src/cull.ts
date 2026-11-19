// SPDX-License-Identifier: GPL-3.0-or-later
// What is drawn: only what is near the view (owner, 2026-11-11: zoomed in, the
// graph laid out and restyled every wedge, chord and label of the site, most of
// them far off screen, and panning dragged; with several KBs of thousands of
// notes it would not scale). Pure: no DOM. The browser script (hier.ts) keeps in
// the document only what meets the area around the view, and puts back what
// comes near it as the view moves.
import type { Label } from "./scene";
import type { Box, View } from "./viewport";

/**
 * Around the view, this much of the screen on every side is drawn too (a
 * fraction of its width and height): a pan of up to half a screen shows
 * nothing missing even before the labels are placed again at rest.
 */
export const MARGIN = 0.5;

/** The drawing area on a screen of sw x sh px, and `margin` of it on every side. */
export function visibleArea(v: View, sw: number, sh: number, margin = MARGIN): Box {
  const w = (sw / v.k) * (1 + 2 * margin);
  const h = (sh / v.k) * (1 + 2 * margin);
  return { x: v.cx - w / 2, y: v.cy - h / 2, w, h };
}

/** Do two boxes overlap or touch? */
export function meets(a: Box, b: Box): boolean {
  return a.x <= b.x + b.w && b.x <= a.x + a.w && a.y <= b.y + b.h && b.y <= a.y + a.h;
}

/** A point of the drawing. */
export type Pt = [number, number];

/**
 * Into how many pieces each Bézier of a link is cut for the test below. One box
 * for a whole link held a long one wherever its bends were: zoomed in on the
 * balloons, about seven times as many links stayed drawn as crossed the screen
 * (2026-11-12, 2000 notes at 64x: 1491 drawn, about 193 in view). Measured on
 * the test sites: one box per Bézier kept 642, in 2 pieces 395, in 4 309, in
 * 8 260. Four is where cutting finer stops paying: on the zettel test site,
 * half as many pieces draws a third more links, twice as many saves a sixth,
 * for twice the boxes to test on every frame (test/cull.test.ts).
 */
export const PIECES = 4;

/**
 * Cuts a Bézier (its control points, from the start point on: 2 for a line, 3
 * for a quadratic, 4 for a cubic) into `n` pieces of equal parameter, each with
 * its own control points (de Casteljau). Each piece lies inside the box of its
 * own control points (convex hull property), so the boxes together hold the
 * whole curve, and each is far smaller than the box of the whole.
 */
export function splitBezier(ctrl: Pt[], n: number): Pt[][] {
  const out: Pt[][] = [];
  let rest = ctrl;
  for (let k = n; k > 1; k--) {
    // Cut off 1/k of what is left: n pieces of equal parameter.
    const t = 1 / k;
    const left: Pt[] = [];
    const right: Pt[] = [];
    let pts = rest;
    while (pts.length > 0) {
      left.push(pts[0]!);
      right.unshift(pts[pts.length - 1]!);
      const next: Pt[] = [];
      for (let j = 0; j + 1 < pts.length; j++) {
        const [a, b] = [pts[j]!, pts[j + 1]!];
        next.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
      }
      pts = next;
    }
    out.push(left);
    rest = right;
  }
  out.push(rest);
  return out;
}

/** The box of some points. */
export function boxOf(pts: Pt[]): Box {
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (const [x, y] of pts) {
    if (x < x0) x0 = x;
    if (y < y0) y0 = y;
    if (x > x1) x1 = x;
    if (y > y1) y1 = y;
  }
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
}

/** Boxes that hold a link drawn as these Béziers: PIECES boxes for each. */
export function pieceBoxes(beziers: Pt[][], n = PIECES): Box[] {
  return beziers.flatMap((b) => splitBezier(b, n).map(boxOf));
}

/**
 * Is a link near the area? Its whole box first (cheap, and most links far
 * away fail it), then the boxes of its pieces: a link whose bends reach around
 * the screen without crossing it is not drawn.
 */
export function linkMeets(link: { box: Box; boxes: Box[] }, area: Box): boolean {
  return meets(link.box, area) && link.boxes.some((b) => meets(b, area));
}

/**
 * Where a label's text can reach, in drawing units: a label is only shown when
 * its text is no longer than `room` and its line no taller than `height`
 * (hier.ts, `fits`). Straight, it runs from (x, y) in any direction (anchored
 * at its start, middle or end, turned by `rotate`); along an arc, it is
 * centred on (x, y), the arc's middle, and no point of an arc is farther from
 * its middle than the arc's length. Either way, no letter is farther than
 * room + height from (x, y).
 */
export function labelReach(l: Label): Box {
  const r = l.room + l.height;
  return { x: l.x - r, y: l.y - r, w: 2 * r, h: 2 * r };
}
