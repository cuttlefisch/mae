// SPDX-License-Identifier: GPL-3.0-or-later
// Zoom and pan over a fixed drawing. Pure: no DOM. The browser script applies a
// View as the SVG's viewBox, so the drawing never changes, only what is framed.

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** What is framed: the centre (cx, cy) and the scale k (screen px per drawing unit). */
export interface View {
  cx: number;
  cy: number;
  k: number;
}

export function union(boxes: Box[]): Box | null {
  if (boxes.length === 0) return null;
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
 * The view that frames `box` in a screen of `sw` x `sh` px, with `pad` px of
 * margin on every side, never zoomed in past `kMax` (a single small wedge must
 * not fill the screen) nor out past `kMin`. With `controls` (boxes in screen
 * px, those of the controls floating over the drawing), the framing goes in
 * the best space they leave free (freeAreas), centred in it.
 */
export function fit(
  box: Box,
  sw: number,
  sh: number,
  pad = 24,
  kMin = 0,
  kMax = Infinity,
  controls: Box[] = [],
): View {
  const w = Math.max(box.w, 1e-6);
  const h = Math.max(box.h, 1e-6);
  // Of the free spaces, the one that leaves the drawing largest (not the one with most area:
  // a long, narrow strip is no use to a round drawing).
  let area: Box | null = null;
  let best = -Infinity;
  for (const a of freeAreas(sw, sh, controls, pad)) {
    const k = Math.min(a.w / w, a.h / h);
    if (k > best) {
      best = k;
      area = a;
    }
  }
  const k = clamp(best, kMin, kMax);
  // The box's centre, at the centre of the space (not of the screen).
  const ox = (area!.x + area!.w / 2 - Math.max(1, sw) / 2) / k;
  const oy = (area!.y + area!.h / 2 - Math.max(1, sh) / 2) / k;
  return { cx: box.x + box.w / 2 - ox, cy: box.y + box.h / 2 - oy, k };
}

/**
 * Minimum gap, in screen px, between a control and what is drawn: without
 * it a node could sit right against a button's edge, and a click
 * aimed at one landed on the other. Small on purpose: every px is taken from the
 * drawing in the sidebar's narrow box.
 */
export const CONTROL_GAP = 4;

/** Do two boxes overlap? (touching along an edge does not count) */
export const overlaps = (a: Box, b: Box): boolean =>
  a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

const grow = (b: Box, d: number): Box => ({
  x: b.x - d,
  y: b.y - d,
  w: b.w + 2 * d,
  h: b.h + 2 * d,
});

/**
 * The spaces where drawing can go on a screen of `sw` x `sh` px: the
 * screen less its margin `pad`, less what the controls cover (boxes in px,
 * measured at the moment of framing, and `gap` px around them).
 *
 * Why maximal free rectangles and not a fixed strip per edge: a
 * control in a corner can be avoided from above or from one side, and which
 * suits depends on the drawing's shape (a wide, low one fits above the
 * zoom buttons without shrinking; a round one, better to their left). So
 * every free rectangle that does not fit inside a larger one is given, and fit picks the
 * one that leaves the drawing largest. Each side of a maximal free rectangle touches
 * the area's edge or a control's side, so trying those
 * coordinates is enough: with a frame's few controls, a few dozen cases. If
 * the controls leave no space at all (a tiny frame), the whole area: better
 * a drawing under the buttons than no drawing.
 */
export function freeAreas(
  sw: number,
  sh: number,
  controls: Box[] = [],
  pad = 24,
  gap = CONTROL_GAP,
): Box[] {
  const W = Math.max(1, sw);
  const H = Math.max(1, sh);
  // The padding never takes more than a quarter of the screen: on a screen
  // narrower than twice the padding, the scale came out negative and clamped
  // to 0 (an Infinity viewBox), found in review.
  const p = Math.min(pad, Math.min(W, H) / 4);
  const all: Box = { x: p, y: p, w: W - 2 * p, h: H - 2 * p };
  const obs = controls.map((c) => grow(c, gap)).filter((c) => overlaps(c, all));
  if (obs.length === 0) return [all];
  const x1 = all.x + all.w;
  const y1 = all.y + all.h;
  const within = (v: number, lo: number, hi: number) => v >= lo && v <= hi;
  const uniq = (vs: number[]) => [...new Set(vs)];
  const lefts = uniq([all.x, ...obs.map((o) => o.x + o.w)].filter((v) => within(v, all.x, x1)));
  const rights = uniq([x1, ...obs.map((o) => o.x)].filter((v) => within(v, all.x, x1)));
  const tops = uniq([all.y, ...obs.map((o) => o.y + o.h)].filter((v) => within(v, all.y, y1)));
  const bottoms = uniq([y1, ...obs.map((o) => o.y)].filter((v) => within(v, all.y, y1)));
  // By its edges (left, top, right, bottom) and not by x + w: the sum
  // rounds, and a space that ended exactly where a control begins overlapped it
  // by a billionth (found by the property test).
  type Edges = [number, number, number, number];
  const hits = (o: Box, [l, t, r, b]: Edges) =>
    o.x < r && l < o.x + o.w && o.y < b && t < o.y + o.h;
  const free: Edges[] = [];
  for (const l of lefts)
    for (const r of rights)
      for (const t of tops)
        for (const b of bottoms) {
          if (r <= l || b <= t) continue;
          if (!obs.some((o) => hits(o, [l, t, r, b]))) free.push([l, t, r, b]);
        }
  const inside = (a: Edges, b: Edges) =>
    a[0] >= b[0] && a[1] >= b[1] && a[2] <= b[2] && a[3] <= b[3];
  const maximal = free
    .filter((c) => !free.some((d) => d !== c && inside(c, d)))
    .map(([l, t, r, b]) => ({ x: l, y: t, w: r - l, h: b - t }));
  return maximal.length > 0 ? maximal : [all];
}

/** Where a box of the drawing lands on a screen of sw x sh px under view v. */
export function toScreen(b: Box, v: View, sw: number, sh: number): Box {
  return {
    x: (b.x - v.cx) * v.k + sw / 2,
    y: (b.y - v.cy) * v.k + sh / 2,
    w: b.w * v.k,
    h: b.h * v.k,
  };
}

/**
 * Frames `box` with nothing drawn under a control. Without `parts`
 * (null), as fit with every control: the whole box clear of them. With
 * `parts` (the boxes of what is really drawn: wedges, balloons, names),
 * a control only takes room from the drawing if something drawn would fall under it: a
 * ring leaves the corners of its box empty, and a button in a corner
 * need not shrink it. It frames without controls; if some part falls under
 * one, it frames again avoiding those; and so on until no part
 * touches a control (at most one round per control: each round avoids one
 * more, and with all of them avoided the whole box, which holds the parts, is
 * clear of them all).
 */
export function fitClear(
  box: Box,
  sw: number,
  sh: number,
  pad: number,
  controls: Box[],
  parts: Box[] | null,
  kMin = 0,
  kMax = Infinity,
  gap = CONTROL_GAP,
): View {
  if (parts === null) return fit(box, sw, sh, pad, kMin, kMax, controls);
  const W = Math.max(1, sw);
  const H = Math.max(1, sh);
  const kept: Box[] = [];
  for (;;) {
    const v = fit(box, sw, sh, pad, kMin, kMax, kept);
    const onScreen = parts.map((p) => toScreen(p, v, W, H));
    const hit = controls.filter(
      (c) => !kept.includes(c) && onScreen.some((p) => overlaps(p, grow(c, gap))),
    );
    if (hit.length === 0) return v;
    kept.push(...hit);
  }
}

export const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/**
 * Fits `box` and what `extra(k)` reaches at scale k (names, whose size is fixed on
 * screen: the smaller the scale, the more of the drawing they cover), in two passes;
 * but never zoomed out to less than `floor` of the plain fit. A name wider than the
 * screen (a long title in the narrow sidebar) has no fit at all: chasing it zoomed
 * the graph out to almost nothing, and the fit button "did not reset" (owner,
 * 2026-11-12). Then the plain fit: the graph fills the box and the name is cut.
 */
export const FIT_FLOOR = 0.75;
/**
 * What the controls cover when framing (fitClear): their boxes in screen
 * px, and the parts of the drawing, if known.
 */
export interface Room {
  controls?: Box[];
  parts?: Box[] | null;
}
export function fitWith(
  box: Box,
  extra: (k: number) => Box[],
  sw: number,
  sh: number,
  pad = 24,
  floor = FIT_FLOOR,
  room: Room = {},
): View {
  const controls = room.controls ?? [];
  const parts = room.parts ?? null;
  // The names are drawn too: not under a control either.
  const at = (b: Box, e: Box[]) => fitClear(b, sw, sh, pad, controls, parts && [...parts, ...e]);
  const base = at(box, []);
  let v = base;
  // Until the scale stops changing (by a thousandth): with two fixed rounds, the
  // names measured at the previous round's scale were shorter than at the
  // final one, and one ended up under the control that had been avoided. It rarely goes past
  // three; eight, in case two framings alternated.
  for (let pass = 0; pass < 8; pass++) {
    const e = extra(v.k);
    if (e.length === 0) break;
    const next = at(union([box, ...e])!, e);
    const settled = Math.abs(next.k - v.k) <= 1e-3 * v.k;
    v = next;
    if (settled) break;
  }
  return v.k >= base.k * floor ? v : base;
}

/** The viewBox attribute for a view on a screen of sw x sh px. */
export function viewBox(v: View, sw: number, sh: number): string {
  const w = sw / v.k;
  const h = sh / v.k;
  const f = (n: number) => (Math.round(n * 1000) / 1000).toString();
  return `${f(v.cx - w / 2)} ${f(v.cy - h / 2)} ${f(w)} ${f(h)}`;
}

/**
 * Where a drawing point lands, in px of an sw x sh frame, given the viewBox attribute
 * `vb` (as viewBox, above, writes it) and the default preserveAspectRatio (xMidYMid
 * meet): the browser's own arithmetic, without asking it. Asking (getScreenCTM,
 * getBoundingClientRect) right after changing the viewBox made it restyle and lay
 * out the whole drawing inside the wheel handler, on every event (2026-11-19: the
 * "You are here" marker following its note).
 */
export function onFrame(
  vb: string,
  sw: number,
  sh: number,
  [x, y]: readonly [number, number],
): [number, number] {
  const [bx, by, bw, bh] = vb.split(" ").map(Number) as [number, number, number, number];
  const s = Math.min(sw / bw, sh / bh);
  return [(x - bx) * s + (sw - bw * s) / 2, (y - by) * s + (sh - bh * s) / 2];
}

/** Zoom by `factor` keeping the drawing point under screen point (px, py) fixed. */
export function zoomAt(
  v: View,
  factor: number,
  px: number,
  py: number,
  sw: number,
  sh: number,
  kMin = 0,
  kMax = Infinity,
): View {
  const k = clamp(v.k * factor, kMin, kMax);
  // Drawing point under the pointer before and after must be the same.
  const dx = (px - sw / 2) / v.k;
  const dy = (py - sh / 2) / v.k;
  const ux = v.cx + dx;
  const uy = v.cy + dy;
  return { cx: ux - (px - sw / 2) / k, cy: uy - (py - sh / 2) / k, k };
}

/**
 * The screen point the + and − buttons and keys zoom on: the current
 * note (`point`, in drawing units: where «You are here» points) if it is in
 * view in `v`, else the frame's centre (owner, 2026-11-16: on the centre, the
 * note moved away with every press). In view means inside the frame, edges
 * included: the rule by which the marker draws its ring on the note
 * (browser/here.ts), so if the reader sees the ring, the note does not move. A note
 * outside the frame, as the fixed point, would take the view even further from what the reader is looking at.
 */
export function zoomAnchor(
  point: readonly [number, number] | null | undefined,
  v: View,
  sw: number,
  sh: number,
): [number, number] {
  if (point) {
    const x = (point[0] - v.cx) * v.k + sw / 2;
    const y = (point[1] - v.cy) * v.k + sh / 2;
    if (x >= 0 && y >= 0 && x <= sw && y <= sh) return [x, y];
  }
  return [sw / 2, sh / 2];
}

/** Pan by (dx, dy) screen px. */
export const panBy = (v: View, dx: number, dy: number): View => ({
  cx: v.cx - dx / v.k,
  cy: v.cy - dy / v.k,
  k: v.k,
});

/**
 * Smooth zoom and pan between two views (van Wijk and Nuij, "Smooth and
 * efficient zooming and panning", 2003, the same formula d3's interpolateZoom
 * uses). Returns the interpolator and its natural duration factor S: animate
 * over roughly S * 400 ms. `sw` is the screen width the views are framed for.
 */
export function smoothZoom(a: View, b: View, sw: number): { at: (t: number) => View; S: number } {
  const rho = Math.SQRT2;
  const w0 = sw / a.k;
  const w1 = sw / b.k;
  const dx = b.cx - a.cx;
  const dy = b.cy - a.cy;
  const d2 = dx * dx + dy * dy;
  const d1 = Math.sqrt(d2);
  // A pan that is tiny next to the view is a pure zoom. The threshold is
  // RELATIVE to the view's width: an absolute one (d3 uses d² < 1e-12) means a
  // different thing at every zoom level, and sends near-zero pans at wide views
  // into the general formula, the ill-conditioned case. (The property test's
  // first counterexample: a 1.4e-6 pan at a view 80000 units wide.)
  if (d1 <= 1e-9 * Math.max(w0, w1)) {
    // Only the width changes, exponentially; the leftover pan, linearly.
    const S = Math.abs(Math.log(w1 / w0)) / rho;
    const sign = w1 < w0 ? -1 : 1;
    return {
      S,
      at: (t) => ({
        cx: a.cx + t * dx,
        cy: a.cy + t * dy,
        k: sw / (w0 * Math.exp(sign * rho * t * S)),
      }),
    };
  }
  const b0 = (w1 * w1 - w0 * w0 + rho ** 4 * d2) / (2 * w0 * rho * rho * d1);
  const b1 = (w1 * w1 - w0 * w0 - rho ** 4 * d2) / (2 * w1 * rho * rho * d1);
  // log(sqrt(b² + 1) - b) is -asinh(b). Written as the log, it cancels for
  // large b: reverting to it fails the screen-space property test (an end
  // centre off by more than 0.001 px).
  const r0 = -Math.asinh(b0);
  const r1 = -Math.asinh(b1);
  const S = (r1 - r0) / rho;
  const cosh = Math.cosh;
  const sinh = Math.sinh;
  return {
    S,
    at: (t) => {
      const s = t * S;
      const coshr0 = cosh(r0);
      // cosh r0 · tanh(ρs + r0) − sinh r0 is exactly sinh(ρs) / cosh(ρs + r0).
      // Written as the difference it cancels whenever r0 is large (a big zoom
      // with a small pan), and the prefactor magnifies what is left: the end
      // centre missed by 0.016 px in CI (seed -1816382072).
      const u = (w0 / (rho * rho * d1)) * (sinh(rho * s) / cosh(rho * s + r0));
      const w = (w0 * coshr0) / cosh(rho * s + r0);
      return { cx: a.cx + u * dx, cy: a.cy + u * dy, k: sw / w };
    },
  };
}
