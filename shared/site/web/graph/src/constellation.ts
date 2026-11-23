// SPDX-License-Identifier: GPL-3.0-or-later
// Linked chord diagrams ("constellation"): one small chord ring per folder,
// laid out as a tree, with links between rings drawn across the canvas. Pure:
// no DOM. Nothing here changes as the reader moves; the view zooms and pans
// over it (viewport.ts).
//
//   - Each folder is a ring of its own notes, plus one "port" wedge per
//     subfolder, first, where the line to that subfolder's ring leaves. The
//     wedges fill the ring: n of them, n equal parts, no empty slot.
//     The folder's own page is the ring's centre (its title, clickable).
//   - Ring size follows its wedge count, so a wedge is about the same size in
//     every ring and the rings read alike.
//   - The rings form a tidy tree, top to bottom: a folder above its
//     subfolders, centred over them. Deterministic: the same site always gives
//     the same picture.
//   - A link inside one folder is a chord in that ring. A link between folders
//     is a curve between the two wedges, bundled through the ring centres along
//     the tree path between them (as in sunburst.ts), so links between two
//     branches run together.
//   - Structural links (hierarchy.ts) are not drawn by default: the tree shows
//     them.
import { buildHierarchy, treePath, type HNode, type Hierarchy } from "./hierarchy";
import type { IndexEntry } from "./geometry";
import { curve, polar } from "./sunburst";
import type { Box } from "./viewport";

export interface Wedge {
  /** Note id, or for a port the subfolder's id. */
  id: string;
  kind: "note" | "port";
  title: string;
  href?: string;
  /** The ring (folder) it sits on. */
  ring: string;
  a0: number;
  a1: number;
}

export interface Disc {
  /** Folder id. */
  id: string;
  title: string;
  href?: string;
  parent?: string;
  depth: number;
  cx: number;
  cy: number;
  /** Inner and outer radius of its wedges. */
  r0: number;
  r1: number;
  wedges: Wedge[];
  size: number;
}

export interface Link {
  source: string;
  target: string;
  structural: boolean;
  /** Same ring (a chord) or across rings. */
  local: boolean;
  d: string;
  /** Bounds of the control points: the curve lies inside them. */
  box: Box;
  /** Smaller boxes that together hold the curve (cull.ts, pieceBoxes). */
  boxes: Box[];
}

export interface Constellation {
  discs: Disc[];
  /** Lines from each port to its subfolder's ring. */
  tree: Array<{ from: string; to: string; d: string }>;
  links: Link[];
  bounds: { x: number; y: number; w: number; h: number };
}

export interface ConstellationOptions {
  exclude?: (slug: string) => boolean;
  locale?: string;
  structural?: boolean;
  /** Arc length of one wedge on the ring's outer edge, in drawing units. */
  wedge?: number;
  /** Smallest outer radius of a ring. */
  minRadius?: number;
  /** Band width of a ring's wedges. */
  band?: number;
  /** Space between neighbouring rings and between levels. */
  spacing?: number;
  /** How straight links are: 0 follows the tree, 1 is a straight line. */
  straightness?: number;
}

const TAU = Math.PI * 2;

/** The angle between two directions, 0 to pi. */
function angleGap(a: number, b: number): number {
  const x = Math.abs((((a - b) % TAU) + TAU) % TAU);
  return Math.min(x, TAU - x);
}

/**
 * Which of `slots` equal slots round a ring (slot j centred at (j + 1/2) of a
 * step, clockwise from the top) each port takes, given the direction of its
 * subfolder's ring, `targets` in increasing order in [0, 2 pi). The ports keep
 * that order round the ring and take distinct slots; of those assignments, the
 * one with the least sum of squared offsets. (Nearest free slot, port by port,
 * left the last of four ports in a four-slot ring 143 degrees off, on the far
 * side.)
 *
 * The first port tries the slots within k + 1 of its own direction (k ports:
 * a cluster of them pushes it at most that far), the rest follow clockwise by
 * dynamic programming. `exhaustive` tries every slot (the tests compare).
 */
export function portSlots(
  targets: number[],
  slots: number,
  exhaustive = false,
  phase = 0,
): number[] {
  const k = targets.length;
  if (k === 0) return [];
  const step = TAU / slots;
  const cost = (i: number, j: number) => angleGap(phase + (j + 0.5) * step, targets[i]!) ** 2;
  const starts = new Set<number>();
  if (exhaustive || 2 * (k + 1) + 1 >= slots) for (let r = 0; r < slots; r++) starts.add(r);
  else {
    const home = Math.floor(((((targets[0]! - phase) % TAU) + TAU) % TAU) / step);
    for (let d = -(k + 1); d <= k + 1; d++) starts.add((((home + d) % slots) + slots) % slots);
  }
  let bestTotal = Infinity;
  let best: number[] = [];
  const dp: Float64Array[] = Array.from({ length: k }, () => new Float64Array(slots));
  const from: Int32Array[] = Array.from({ length: k }, () => new Int32Array(slots));
  for (const r of [...starts].sort((a, b) => a - b)) {
    // dp[i][q]: port i at position q of the slots taken clockwise from r.
    for (let i = 0; i < k; i++) dp[i]!.fill(Infinity);
    dp[0]![0] = cost(0, r);
    for (let i = 1; i < k; i++) {
      let run = Infinity;
      let arg = -1;
      for (let q = i; q <= slots - (k - i); q++) {
        if (dp[i - 1]![q - 1]! < run) {
          run = dp[i - 1]![q - 1]!;
          arg = q - 1;
        }
        if (arg < 0) continue;
        dp[i]![q] = run + cost(i, (r + q) % slots);
        from[i]![q] = arg;
      }
    }
    let q = k - 1;
    for (let x = k; x < slots; x++) if (dp[k - 1]![x]! < dp[k - 1]![q]!) q = x;
    const total = dp[k - 1]![q]!;
    if (total < bestTotal - 1e-12) {
      bestTotal = total;
      const picked: number[] = [];
      for (let i = k - 1; i >= 0; i--) {
        picked[i] = (r + q) % slots;
        q = from[i]![q]!;
      }
      best = picked;
    }
  }
  return best;
}

/** Signed angle from b to a, in (-pi, pi]. */
function signedGap(a: number, b: number): number {
  const x = (((a - b) % TAU) + TAU) % TAU;
  return x > Math.PI ? x - TAU : x;
}

/**
 * The ring's rotation and each port's slot: `portSlots` on a slot grid turned
 * by `phase` (0 to one slot), the phase chosen so the ports face their
 * subfolders as closely as the equal slots allow. With the grid fixed at the
 * top, ports were up to half a slot off their subfolders, 45 degrees in a
 * four-slot ring. Candidates: no turn, and the turn that
 * puts each port's subfolder (up to 24 of them) exactly on a slot centre; each
 * candidate is then moved by the mean offset of its assignment and assigned
 * again, until that stops improving. The least sum of squared offsets wins (ties: the smaller turn).
 */
export function facePorts(
  targets: number[],
  slots: number,
): { phase: number; slots: number[]; cost: number } {
  const k = targets.length;
  const step = TAU / slots;
  const modStep = (a: number) => ((a % step) + step) % step;
  const evaluate = (phase: number) => {
    const js = portSlots(targets, slots, false, phase);
    const cost = js.reduce(
      (s2, j, i) => s2 + angleGap(phase + (j + 0.5) * step, targets[i]!) ** 2,
      0,
    );
    return { phase, slots: js, cost };
  };
  const candidates = new Set<number>([0]);
  const every = Math.max(1, Math.ceil(k / 24));
  for (let i = 0; i < k; i += every) candidates.add(modStep(targets[i]! - step / 2));
  let best = evaluate(0);
  const consider = (r: { phase: number; slots: number[]; cost: number }) => {
    if (
      r.cost < best.cost - 1e-12 ||
      (Math.abs(r.cost - best.cost) <= 1e-12 && r.phase < best.phase)
    )
      best = r;
  };
  for (const c of [...candidates].sort((a, b) => a - b)) {
    // Alternate: the mean offset turns the ring to the best turn for this
    // assignment, which may change the assignment; again, until it stops
    // improving (each round only lowers the cost). One round was not enough:
    // a random test found the result 1e-6 above a grid turn.
    let r = evaluate(c);
    consider(r);
    for (let round = 0; round < 32; round++) {
      const mean =
        r.slots.reduce((s2, j, i) => s2 + signedGap(targets[i]!, r.phase + (j + 0.5) * step), 0) /
        k;
      const next = evaluate(modStep(r.phase + mean));
      if (next.cost >= r.cost - 1e-15) break;
      r = next;
      consider(r);
    }
  }
  return best;
}

export function layoutConstellation(
  input: Record<string, IndexEntry> | Hierarchy,
  opts: ConstellationOptions = {},
): Constellation {
  const h =
    "root" in input && "nodeOf" in input
      ? (input as Hierarchy)
      : buildHierarchy(input as Record<string, IndexEntry>, opts);
  const wedgeLen = opts.wedge ?? 10;
  const minR = opts.minRadius ?? 40;
  const band = opts.band ?? 12;
  const spacing = opts.spacing ?? 60;
  const beta = 1 - (opts.straightness ?? 0.15);

  // Which folders get a ring: every folder with notes or subfolders (all of them).
  const folders: HNode[] = [];
  const walk = (n: HNode) => {
    if (n.kind !== "folder") return;
    folders.push(n);
    n.children.forEach(walk);
  };
  walk(h.root);

  const discs = new Map<string, Disc>();
  const wedgeOf = new Map<string, Wedge>(); // note id -> its wedge
  const portOf = new Map<string, Wedge>(); // subfolder id -> its port wedge on the parent
  for (const f of folders) {
    const items = f.children; // subfolders (ports) first, then notes: hierarchy order
    // One slot per item, and they fill the ring: n items, n parts of the turn.
    // (Ports were set apart from notes by one empty slot, so n items took n of
    // n + 1 parts and the last one stayed empty; read as a missing piece of the
    // ring, owner 2026-11-18. Ports already read as a group: they are drawn as
    // folders, pale and outlined, chord.scss.)
    const slots = Math.max(1, items.length);
    const r1 = Math.max(minR, (slots * wedgeLen) / TAU);
    const disc: Disc = {
      id: f.id,
      title: f.title,
      href: f.href,
      parent: f.parent?.id,
      depth: f.depth,
      cx: 0,
      cy: 0,
      r0: r1 - band,
      r1,
      wedges: [],
      size: f.size,
    };
    const step = TAU / slots;
    items.forEach((c, i) => {
      const w: Wedge = {
        id: c.id,
        kind: c.kind === "folder" ? "port" : "note",
        title: c.title,
        href: c.href,
        ring: f.id,
        a0: i * step,
        a1: (i + 1) * step,
      };
      disc.wedges.push(w);
      (c.kind === "folder" ? portOf : wedgeOf).set(c.id, w);
    });
    discs.set(f.id, disc);
  }

  // Balloon tree: each folder's subfolders circle around it, every subtree in
  // its own enclosing circle, so the picture stays compact whatever the
  // tree's shape. (Tried first: a top-down tree, six times wider than tall for
  // 17 sibling lists; then rings of levels around the root, where one big
  // branch fanned out to one side and left most of the circle empty.)
  //
  // Bottom up, the radius of each subtree's enclosing circle: its subfolders'
  // circles sit on a circle of radius R around it, R the smallest that clears
  // the folder's own ring and leaves every child the angle its circle needs.
  // Children of the root may use the whole turn; deeper ones a 300-degree arc
  // that leaves the side facing the parent free for the line to it.
  // Balloon tree with rigid subtrees. Bottom up, every folder's subtree gets
  // a shape in its own frame: the folder at the origin, its subtree opening
  // "up" (away from its parent). A parent then sets each child's subtree as
  // close as its actual rings allow (not its enclosing circle: that left long
  // empty spans) and turns it to its share of the arc. Children of the root
  // use the whole turn; deeper ones a 300-degree arc, leaving the side facing
  // the parent free for the line to it.
  //
  // (Tried first: a top-down tree, six times wider than tall for 17 sibling
  // lists; rings of levels round the root, where one big branch fanned out to
  // one side; the textbook balloon, one radius for all children, which put
  // three small folders as far out as the one holding 800 notes.)
  type Placed = { id: string; x: number; y: number; r: number };
  const kidsOf = (f: HNode) => f.children.filter((c) => c.kind === "folder");
  const arcFor = (f: HNode) => (f.depth === 0 ? TAU : (TAU * 300) / 360);
  // Clockwise rotation on screen (y down), as polar() measures angles.
  const rot = (x: number, y: number, t: number): [number, number] => [
    x * Math.cos(t) - y * Math.sin(t),
    x * Math.sin(t) + y * Math.cos(t),
  ];
  const shape = (f: HNode): Placed[] => {
    const d = discs.get(f.id)!;
    const own: Placed = { id: f.id, x: 0, y: 0, r: d.r1 };
    const kids = kidsOf(f);
    if (kids.length === 0) return [own];
    const shapes = kids.map(shape);
    // How far each subtree reaches back toward its parent (+y in its frame).
    const back = shapes.map((sh) => Math.max(...sh.map((p) => p.y + p.r)));
    // The angle each subtree needs, either side of its axis, at distance dd.
    const sides = (sh: Placed[], dd: number): [number, number] => {
      let left = 0;
      let right = 0;
      for (const p of sh) {
        const ux = p.x;
        const uy = dd - p.y; // > 0: dd clears the parent's ring by the reach back
        const len = Math.hypot(ux, uy);
        const half = Math.asin(Math.min(1, (p.r + spacing / 2) / len));
        right = Math.max(right, Math.atan2(ux, uy) + half);
        left = Math.max(left, Math.atan2(-ux, uy) + half);
      }
      return [left, right];
    };
    const A = arcFor(f);
    const base = back.map((b) => d.r1 + spacing + b);
    const need = (off: number) => shapes.map((sh, i) => sides(sh, base[i]! + off));
    const total = (off: number) => need(off).reduce((s2, [l, r]) => s2 + l + r, 0);
    let off = 0;
    if (total(0) > A) {
      let hi = spacing;
      while (total(hi) > A) hi *= 2;
      let lo = 0;
      for (let it = 0; it < 50; it++) {
        const mid = (lo + hi) / 2;
        if (total(mid) > A) lo = mid;
        else hi = mid;
      }
      off = hi;
    }
    const ns = need(off);
    const slack = Math.max(0, A - ns.reduce((s2, [l, r]) => s2 + l + r, 0)) / kids.length;
    const out: Placed[] = [own];
    let a = f.depth === 0 ? 0 : -A / 2;
    shapes.forEach((sh, i) => {
      const [l, r] = ns[i]!;
      const axis = a + slack / 2 + l;
      const dd = base[i]! + off;
      for (const p of sh) {
        // In the child's frame the parent is at (0, dd); turn about the parent.
        const [x, y] = rot(p.x, p.y - dd, axis);
        out.push({ id: p.id, x, y, r: p.r });
      }
      a += l + r + slack;
    });
    return out;
  };
  for (const p of shape(h.root)) {
    const d = discs.get(p.id)!;
    d.cx = p.x;
    d.cy = p.y;
  }

  // Now that every ring has its place, each port moves to the slot that faces
  // its subfolder's ring, and the notes fill the other slots in their order.
  // The slots were dealt in list order above, before anyone knew where the
  // subfolders would land: a port could sit on the far side, its line crossing
  // its own ring, and a click on the right of the ring led to a ring on the
  // left.
  for (const d of discs.values()) {
    const ports = d.wedges.filter((w) => w.kind === "port");
    if (ports.length === 0) continue;
    const step = d.wedges[0]!.a1 - d.wedges[0]!.a0;
    const slots = Math.round(TAU / step);
    const norm = (a: number) => ((a % TAU) + TAU) % TAU;
    // polar(): clockwise from the top, so the angle of (dx, dy) is atan2(dx, -dy).
    const toward = (w: Wedge) => {
      const c = discs.get(w.id)!;
      return norm(Math.atan2(c.cx - d.cx, -(c.cy - d.cy)));
    };
    const sorted = ports.map((w) => ({ w, t: toward(w) })).sort((x, y) => x.t - y.t);
    // The whole ring turns with the ports (the notes keep their order in it).
    const { phase, slots: bestSlots } = facePorts(
      sorted.map((x) => x.t),
      slots,
    );
    const free = new Set(Array.from({ length: slots }, (_, j) => j));
    sorted.forEach(({ w }, i) => {
      const j = bestSlots[i]!;
      free.delete(j);
      w.a0 = phase + j * step;
      w.a1 = phase + (j + 1) * step;
    });
    const rest = [...free].sort((x, y) => x - y);
    let n = 0;
    for (const w of d.wedges) {
      if (w.kind === "port") continue;
      const j = rest[n++]!;
      w.a0 = phase + j * step;
      w.a1 = phase + (j + 1) * step;
    }
  }

  const at = (w: Wedge, r: "inner" | "outer" | "mid"): [number, number] => {
    const d = discs.get(w.ring)!;
    const rr = r === "inner" ? d.r0 : r === "outer" ? d.r1 : (d.r0 + d.r1) / 2;
    const [x, y2] = polar(rr, (w.a0 + w.a1) / 2);
    return [d.cx + x, d.cy + y2];
  };

  const tree: Constellation["tree"] = [];
  for (const [childId, port] of portOf) {
    const c = discs.get(childId)!;
    const p = discs.get(port.ring)!;
    const [x0, y0] = at(port, "outer");
    const dx = p.cx - c.cx;
    const dy = p.cy - c.cy;
    const len = Math.hypot(dx, dy) || 1;
    const x1 = c.cx + (dx / len) * c.r1;
    const y1 = c.cy + (dy / len) * c.r1;
    // Leave the port outward, arrive at the child straight on.
    const [ox, oy] = polar(p.r1 + spacing / 2, (port.a0 + port.a1) / 2);
    const q1: [number, number] = [p.cx + ox, p.cy + oy];
    const q2: [number, number] = [
      c.cx + (dx / len) * (c.r1 + spacing / 2),
      c.cy + (dy / len) * (c.r1 + spacing / 2),
    ];
    const f = (v: number) => v.toFixed(2);
    tree.push({
      from: port.ring,
      to: childId,
      d: `M${f(x0)},${f(y0)}C${f(q1[0])},${f(q1[1])} ${f(q2[0])},${f(q2[1])} ${f(x1)},${f(y1)}`,
    });
  }

  // A note's anchor: its wedge. A folder (its own page): the ring's centre.
  const anchor = (n: HNode, toward: [number, number]): [number, number] => {
    if (n.kind === "folder") {
      const d = discs.get(n.id)!;
      const dx = toward[0] - d.cx;
      const dy = toward[1] - d.cy;
      const len = Math.hypot(dx, dy) || 1;
      const r = d.r0 * 0.5;
      return [d.cx + (dx / len) * r, d.cy + (dy / len) * r];
    }
    return at(wedgeOf.get(n.id)!, "outer");
  };
  const links: Link[] = [];
  for (const e of h.edges) {
    if (e.structural && !opts.structural) continue;
    const A = h.nodes.get(e.source)!;
    const B = h.nodes.get(e.target)!;
    const local = A.kind === "note" && B.kind === "note" && A.parent === B.parent;
    if (local) {
      const d = discs.get(A.parent!.id)!;
      const p = at(wedgeOf.get(A.id)!, "inner");
      const q = at(wedgeOf.get(B.id)!, "inner");
      // A chord pulled toward the ring's centre, as on the main ring.
      const pull = 0.55;
      const c: [number, number] = [
        ((p[0] + q[0]) / 2) * (1 - pull) + d.cx * pull,
        ((p[1] + q[1]) / 2) * (1 - pull) + d.cy * pull,
      ];
      links.push({ ...e, local, ...curve([p, c, q]) });
      continue;
    }
    // Across rings: through the centres of the folders on the tree path between them.
    const path = treePath(A, B).filter((n) => n.kind === "folder");
    const inner = path
      .filter((n) => n !== A && n !== B)
      .map((n): [number, number] => [discs.get(n.id)!.cx, discs.get(n.id)!.cy]);
    const pa = anchor(
      A,
      inner[0] ??
        (B.kind === "folder"
          ? [discs.get(B.id)!.cx, discs.get(B.id)!.cy]
          : at(wedgeOf.get(B.id)!, "outer")),
    );
    const pb = anchor(B, inner[inner.length - 1] ?? pa);
    const k = inner.length + 1;
    const pts: Array<[number, number]> = [
      pa,
      ...inner.map((q, i): [number, number] => {
        const t = (i + 1) / k;
        return [
          beta * q[0] + (1 - beta) * (pa[0] + t * (pb[0] - pa[0])),
          beta * q[1] + (1 - beta) * (pa[1] + t * (pb[1] - pa[1])),
        ];
      }),
      pb,
    ];
    links.push({ ...e, local, ...curve(pts) });
  }

  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const d of discs.values()) {
    minX = Math.min(minX, d.cx - d.r1);
    maxX = Math.max(maxX, d.cx + d.r1);
    minY = Math.min(minY, d.cy - d.r1);
    maxY = Math.max(maxY, d.cy + d.r1);
  }
  return {
    discs: [...discs.values()],
    tree,
    links,
    bounds: { x: minX, y: minY, w: maxX - minX, h: maxY - minY },
  };
}
