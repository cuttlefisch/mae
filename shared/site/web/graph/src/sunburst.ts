// SPDX-License-Identifier: GPL-3.0-or-later
// Concentric layout ("sunburst"): the site's folder tree as rings around a disc.
// Pure: no DOM. Nothing here changes as the reader moves around; the view zooms
// and pans over it (viewport.ts).
//
//   - Ring 0, innermost, holds the top level: top-level folders and root notes.
//     Each deeper level is the next ring out, and every node sits inside the
//     angular span of its parent, so a folder's arc covers everything in it.
//   - Every note gets an equal share of the circle, so a folder's angle is
//     proportional to how many notes it holds; the smallest folders on the
//     inner rings are widened to a minimum angle so they stay visible and
//     clickable at the overview.
//   - Links are chords in the central disc, each anchored on the disc's rim at
//     the angle of its two notes. They are bundled along the tree (Holten,
//     "Hierarchical edge bundling", 2006): a chord bends through the positions
//     of the folders between its notes, so links inside one folder stay near
//     that folder's stretch of rim and links across the site pass the centre.
//   - Structural links (hierarchy.ts) are not drawn by default: the rings
//     already show them.
import { buildHierarchy, treePath, type HEdge, type HNode, type Hierarchy } from "./hierarchy";
import type { IndexEntry } from "./geometry";
import { boxOf, pieceBoxes, type Pt } from "./cull";
import type { Box } from "./viewport";

export interface Segment {
  id: string;
  /** "own": a folder's own page (id: ownId(folder)), drawn as a note. */
  kind: HNode["kind"] | "own";
  title: string;
  href?: string;
  /** Ring index: 0 innermost. */
  ring: number;
  /** Angles in radians, clockwise from the top; a0 < a1. */
  a0: number;
  a1: number;
  /** Radii of the ring. */
  r0: number;
  r1: number;
  /** Parent segment id; absent on ring 0. */
  parent?: string;
  /** Ancestor ids, nearest first. */
  ancestors: string[];
  size: number;
}

export interface Chord {
  source: string;
  target: string;
  structural: boolean;
  /** SVG path data. */
  d: string;
  /** Bounds of the control points: the curve lies inside them (convex hull property). */
  box: Box;
  /** Smaller boxes that together hold the curve (cull.ts, pieceBoxes). */
  boxes: Box[];
}

export interface Sunburst {
  segments: Segment[];
  chords: Chord[];
  /** Radius of the central disc (where chords live). */
  disc: number;
  /** Outer radius of the outermost ring. */
  outer: number;
  rings: number;
}

/**
 * Which way the rings run.
 *   top-inside     the top level innermost, each level further out; notes sit
 *                  on the ring of their own depth. What was asked for: the site
 *                  reads from the centre out. Chords anchor on the inner rim,
 *                  so a note and its chords meet only by angle.
 *   leaves-inside  every note on the innermost ring, touching the chords, and
 *                  folders further out by level, the top level outermost: the
 *                  layout of Holten's radial hierarchical edge bundles, where
 *                  leaves sit on the circle and links curve inward.
 */
export type Orientation = "top-inside" | "leaves-inside";

export interface SunburstOptions {
  orientation?: Orientation;
  exclude?: (slug: string) => boolean;
  locale?: string;
  /** Draw structural links too (folder page <-> its contents). Default false. */
  structural?: boolean;
  /** Minimum angle, in degrees, of a segment on ring 0 and ring 1. */
  minAngle?: [number, number];
  /** How straight chords are: 0 follows the tree, 1 is a straight line. Default 0.85 bundling (0.15). */
  straightness?: number;
  /**
   * Radius of the central disc, in drawing units. Default: from 2 to 5 bands
   * wide, growing with the number of chords drawn, so a site with few real
   * links does not get a large empty middle.
   */
  disc?: number;
  band?: number;
  gap?: number;
}

const TAU = Math.PI * 2;

/**
 * Splits [a0, a1] among children by weight, raising any child below `min` to
 * `min` and sharing the rest among the others by weight. When even `min` each
 * does not fit, the children share equally.
 */
export function allocate(
  weights: number[],
  a0: number,
  a1: number,
  min: number,
): Array<[number, number]> {
  const span = a1 - a0;
  const n = weights.length;
  if (n === 0) return [];
  let out: number[];
  if (min * n >= span) {
    out = weights.map(() => span / n);
  } else {
    const fixed = new Set<number>();
    for (;;) {
      const freeSpan = span - fixed.size * min;
      const freeW = weights.reduce((s, w, i) => (fixed.has(i) ? s : s + w), 0);
      let changed = false;
      for (let i = 0; i < n; i++) {
        if (!fixed.has(i) && (freeW === 0 ? 0 : (weights[i]! / freeW) * freeSpan) < min) {
          fixed.add(i);
          changed = true;
        }
      }
      if (!changed) {
        out = weights.map((w, i) =>
          fixed.has(i) ? min : freeW === 0 ? 0 : (w / freeW) * freeSpan,
        );
        break;
      }
    }
  }
  const res: Array<[number, number]> = [];
  let a = a0;
  for (let i = 0; i < n; i++) {
    const b = i === n - 1 ? a1 : a + out[i]!;
    res.push([a, b]);
    a = b;
  }
  return res;
}

/** The id of a folder's own-page segment. */
export const ownId = (folder: string) => `${folder}\u0000own`;

/** x, y of a polar point; angle clockwise from the top. */
export const polar = (r: number, a: number): [number, number] => [
  r * Math.sin(a),
  -r * Math.cos(a),
];

/**
 * The Béziers of a uniform cubic B-spline through control points (clamped), each
 * as its control points from its start point on: a line for 2 points, a
 * quadratic for 3, cubics beyond. What bezierPath draws and what pieceBoxes cuts.
 */
export function bsplineBeziers(pts: Pt[]): Pt[][] {
  if (pts.length <= 3) return [pts];
  // Clamp: repeat the ends so the curve starts and ends on them.
  const P = [pts[0]!, pts[0]!, ...pts, pts[pts.length - 1]!, pts[pts.length - 1]!];
  const out: Pt[][] = [];
  for (let i = 0; i + 3 < P.length; i++) {
    const [p0, p1, p2, p3] = [P[i]!, P[i + 1]!, P[i + 2]!, P[i + 3]!];
    const s: Pt =
      i === 0
        ? [(p0[0] + 4 * p1[0] + p2[0]) / 6, (p0[1] + 4 * p1[1] + p2[1]) / 6]
        : out[i - 1]![3]!;
    const c1: Pt = [(2 * p1[0] + p2[0]) / 3, (2 * p1[1] + p2[1]) / 3];
    const c2: Pt = [(p1[0] + 2 * p2[0]) / 3, (p1[1] + 2 * p2[1]) / 3];
    const e: Pt = [(p1[0] + 4 * p2[0] + p3[0]) / 6, (p1[1] + 4 * p2[1] + p3[1]) / 6];
    out.push([s, c1, c2, e]);
  }
  return out;
}

/** SVG path data of Béziers given as bsplineBeziers gives them. */
export function bezierPath(beziers: Pt[][]): string {
  const f = (v: number) => (Math.round(v * 100) / 100).toString();
  const xy = (p: Pt) => `${f(p[0])},${f(p[1])}`;
  let d = `M${xy(beziers[0]![0]!)}`;
  for (const b of beziers) {
    const cmd = b.length === 2 ? "L" : b.length === 3 ? "Q" : "C";
    d += cmd + b.slice(1).map(xy).join(" ");
  }
  return d;
}

/** A link's path, the box of its control points, and the boxes of its pieces (cull.ts). */
export function curve(pts: Pt[]): { d: string; box: Box; boxes: Box[] } {
  const bz = bsplineBeziers(pts);
  return { d: bezierPath(bz), box: boxOf(pts), boxes: pieceBoxes(bz) };
}

export function layoutSunburst(
  input: Record<string, IndexEntry> | Hierarchy,
  opts: SunburstOptions = {},
): Sunburst {
  const h =
    "root" in input && "nodeOf" in input
      ? (input as Hierarchy)
      : buildHierarchy(input as Record<string, IndexEntry>, opts);
  const band = opts.band ?? 24;
  const drawn = h.edges.filter((e) => opts.structural || !e.structural).length;
  const disc = opts.disc ?? band * Math.min(5, 2 + Math.log10(1 + drawn));
  const gap = opts.gap ?? 2;
  const [min0, min1] = (opts.minAngle ?? [3, 1]).map((d) => (d * Math.PI) / 180) as [
    number,
    number,
  ];
  const beta = 1 - (opts.straightness ?? 0.15);
  const orientation: Orientation = opts.orientation ?? "top-inside";
  // Deepest folder (the root is depth 0, so a site of root notes only has 0).
  let maxFolder = 0;
  for (const n of h.nodes.values())
    if (n.kind === "folder") maxFolder = Math.max(maxFolder, n.depth);
  const rings = orientation === "top-inside" ? Math.max(1, h.maxDepth) : maxFolder + 1;
  const ringOf = (node: HNode) =>
    orientation === "top-inside"
      ? node.depth - 1
      : node.kind === "note"
        ? 0
        : maxFolder - node.depth + 1;
  const radii = (ring: number): [number, number] => {
    const r0 = disc + gap + ring * (band + gap);
    return [r0, r0 + band];
  };

  const segments: Segment[] = [];
  const byId = new Map<string, Segment>();
  const place = (node: HNode, a0: number, a1: number, ancestors: string[]) => {
    if (node.depth > 0) {
      const ring = ringOf(node);
      const [r0] = radii(ring);
      const s: Segment = {
        id: node.id,
        kind: node.kind,
        title: node.title,
        href: node.href,
        ring,
        a0,
        a1,
        r0,
        r1: r0 + band,
        parent: ancestors[0],
        ancestors,
        size: node.size,
      };
      segments.push(s);
      byId.set(s.id, s);
    }
    // A leaf, or a root with nothing but its own page (a one-page site), has
    // nothing inside; the root's own page still needs its segment, below.
    if (node.children.length === 0 && !(node.depth === 0 && node.own)) return;
    // A folder's own page is a note too: it takes a share at the start of the
    // folder's span, drawn as its own segment, so every page has one wedge to
    // click and its chords one place to anchor, and the folder's segment is only
    // ever "this folder". top-inside: on the ring of the folder's contents (the
    // gap their span leaves); leaves-inside: on the notes' ring, like every
    // note. The site's own page ("index") is the root's, on ring 0.
    // The own page is allocated with the children, so the minimum angle widens
    // it too (the site's home page was 0.4 degrees of the top ring without it).
    const ownShare = node.own ? 1 : 0;
    const min = node.depth === 0 ? min0 : node.depth === 1 ? min1 : 0;
    const all = allocate(
      [...(ownShare ? [1] : []), ...node.children.map((c) => c.size)],
      a0,
      a1,
      min,
    );
    const start = ownShare ? all[0]![1] : a0;
    const spans = all.slice(ownShare);
    const anc = node.depth > 0 ? [node.id, ...ancestors] : ancestors;
    if (ownShare) {
      const ring = orientation === "top-inside" ? node.depth : 0;
      const [r0, r1] = radii(ring);
      const own: Segment = {
        id: ownId(node.id),
        kind: "own",
        title: node.title,
        href: node.href,
        ring,
        a0,
        a1: start,
        r0,
        r1,
        parent: node.depth > 0 ? node.id : undefined,
        ancestors: anc,
        size: 1,
      };
      segments.push(own);
      byId.set(own.id, own);
    }
    node.children.forEach((c, i) => place(c, spans[i]![0], spans[i]![1], anc));
  };
  place(h.root, 0, TAU, []);

  // Where a node's chords anchor: the middle of the segment of its page (a
  // note's own segment; a folder's own-page segment).
  const anchorAngle = (n: HNode): number => {
    const s = byId.get(n.kind === "folder" ? ownId(n.id) : n.id)!;
    return (s.a0 + s.a1) / 2;
  };
  // Control points for the tree path: a folder at depth d sits at radius
  // disc * (d / (rings + 1)) on its own angle, so the root is the centre and a
  // deep folder is near the rim.
  const ctrl = (n: HNode): [number, number] => {
    if (n.depth === 0) return [0, 0];
    const s = byId.get(n.id)!;
    return polar(disc * (n.depth / (rings + 1)), (s.a0 + s.a1) / 2);
  };
  const chords: Chord[] = [];
  for (const e of h.edges as HEdge[]) {
    if (e.structural && !opts.structural) continue;
    const A = h.nodes.get(e.source)!;
    const B = h.nodes.get(e.target)!;
    const path = treePath(A, B);
    const p0 = polar(disc, anchorAngle(A));
    const pn = polar(disc, anchorAngle(B));
    const inner = path.slice(1, -1).map(ctrl);
    // Straighten (Holten's beta): pull each control point toward the straight line.
    const k = inner.length + 1;
    const pts: Array<[number, number]> = [
      p0,
      ...inner.map((q, i): [number, number] => {
        const t = (i + 1) / k;
        return [
          beta * q[0] + (1 - beta) * (p0[0] + t * (pn[0] - p0[0])),
          beta * q[1] + (1 - beta) * (p0[1] + t * (pn[1] - p0[1])),
        ];
      }),
      pn,
    ];
    chords.push({
      source: e.source,
      target: e.target,
      structural: e.structural,
      ...curve(pts),
    });
  }
  const outer = disc + gap + rings * (band + gap) - gap;
  return { segments, chords, disc, outer, rings };
}

/** SVG path of an annular sector, clockwise from the top. */
export function sectorPath(r0: number, r1: number, a0: number, a1: number): string {
  const f = (v: number) => (Math.round(v * 100) / 100).toString();
  const large = a1 - a0 > Math.PI ? 1 : 0;
  if (a1 - a0 >= TAU - 1e-9) {
    // A full ring: two half arcs each way (one arc cannot start and end on the same point).
    const [x0, y0] = polar(r1, 0);
    const [x1, y1] = polar(r1, Math.PI);
    const [u0, v0] = polar(r0, 0);
    const [u1, v1] = polar(r0, Math.PI);
    return `M${f(x0)},${f(y0)}A${r1},${r1} 0 1 1 ${f(x1)},${f(y1)}A${r1},${r1} 0 1 1 ${f(x0)},${f(y0)}ZM${f(u0)},${f(v0)}A${r0},${r0} 0 1 0 ${f(u1)},${f(v1)}A${r0},${r0} 0 1 0 ${f(u0)},${f(v0)}Z`;
  }
  const [x0, y0] = polar(r1, a0);
  const [x1, y1] = polar(r1, a1);
  const [u1, v1] = polar(r0, a1);
  const [u0, v0] = polar(r0, a0);
  return `M${f(x0)},${f(y0)}A${r1},${r1} 0 ${large} 1 ${f(x1)},${f(y1)}L${f(u1)},${f(v1)}A${r0},${r0} 0 ${large} 0 ${f(u0)},${f(v0)}Z`;
}
