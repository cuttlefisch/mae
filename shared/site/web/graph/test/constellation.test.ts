// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import fc from "fast-check";
import { facePorts, layoutConstellation, portSlots } from "../src/constellation";
import { buildHierarchy } from "../src/hierarchy";
import { SHAPES } from "./kbs/shapes";
import { indexArb, params, real } from "./arbitraries";

const EPS = 1e-6;
const TAU = Math.PI * 2;

// A ring's wedges fill it: in angular order, each starts where the one before
// ends, the last ends a full turn after the first starts, and all are the same
// size. (In angular order, not list order: a port sits where it faces its
// subfolder's ring, so ports and notes interleave; and the ring turns, by less
// than one wedge, so its ports face their subfolders squarely.)
function expectWhole(inList: Array<{ a0: number; a1: number }>) {
  if (inList.length === 0) return;
  const wedges = [...inList].sort((a, b) => a.a0 - b.a0);
  const size = TAU / wedges.length;
  expect(wedges[0]!.a0).toBeGreaterThanOrEqual(-1e-9);
  expect(wedges[0]!.a0).toBeLessThan(size + 1e-9);
  for (let i = 1; i < wedges.length; i++) expect(wedges[i]!.a0).toBeCloseTo(wedges[i - 1]!.a1, 9);
  expect(wedges[wedges.length - 1]!.a1).toBeCloseTo(wedges[0]!.a0 + TAU, 9);
  for (const w of wedges) expect(w.a1 - w.a0).toBeCloseTo(size, 9);
}

function checkLayout(ix: Parameters<typeof buildHierarchy>[0]) {
  const h = buildHierarchy(ix);
  const cs = layoutConstellation(h);
  const folders = [...h.nodes.values()].filter((n) => n.kind === "folder");
  // 1. One ring per folder, one wedge per item (a note, or a port per subfolder).
  expect(cs.discs.length).toBe(folders.length);
  const byId = new Map(cs.discs.map((d) => [d.id, d]));
  for (const f of folders) {
    const d = byId.get(f.id)!;
    expect(d.wedges.map((w) => w.id)).toEqual(f.children.map((c) => c.id));
    for (const w of d.wedges)
      expect(w.kind).toBe(h.nodes.get(w.id)!.kind === "folder" ? "port" : "note");
    // Wedges end to end, filling the turn: no empty slot anywhere.
    expectWhole(d.wedges);
    // The notes, clockwise in their list order (the ports go where they face).
    const notes = d.wedges.filter((w) => w.kind === "note").map((w) => w.a0);
    expect(notes).toEqual([...notes].sort((a, b) => a - b));
  }
  // 2. No two rings overlap, whatever the tree's shape.
  for (let i = 0; i < cs.discs.length; i++) {
    for (let j = i + 1; j < cs.discs.length; j++) {
      const a = cs.discs[i]!;
      const b = cs.discs[j]!;
      expect(Math.hypot(a.cx - b.cx, a.cy - b.cy)).toBeGreaterThanOrEqual(a.r1 + b.r1 - EPS);
    }
  }
  // 3. A line from every subfolder's port to its ring.
  expect(cs.tree.length).toBe(folders.length - 1);
  // 4. One link per real link; local ones only between notes of one folder.
  const real = h.edges.filter((e) => !e.structural);
  expect(cs.links.length).toBe(real.length);
  for (const l of cs.links) {
    if (!l.local) continue;
    const a = h.nodes.get(l.source)!;
    const b = h.nodes.get(l.target)!;
    expect(a.kind === "note" && b.kind === "note" && a.parent === b.parent).toBe(true);
  }
  // 4b. A link to a note ends on that note's wedge (inner edge for a chord in
  // its ring, outer edge for a link across rings), at the wedge's angle.
  const wedgeOf = new Map<string, { d: (typeof cs.discs)[number]; a0: number; a1: number }>();
  for (const d of cs.discs)
    for (const w of d.wedges) if (w.kind === "note") wedgeOf.set(w.id, { d, a0: w.a0, a1: w.a1 });
  for (const l of cs.links) {
    const nums = l.d.match(/-?\d+(?:\.\d+)?/g)!.map(Number);
    const ends: Array<[string, number, number]> = [
      [l.source, nums[0]!, nums[1]!],
      [l.target, nums[nums.length - 2]!, nums[nums.length - 1]!],
    ];
    // Which end is which is not fixed by the path's direction: match each end to the nearer note.
    for (const [id] of ends) {
      const w = wedgeOf.get(id);
      if (!w) continue; // a folder's own page: the ring's centre
      const hit = ends.some(([, x, y]) => {
        const r = Math.hypot(x - w.d.cx, y - w.d.cy);
        const a = (Math.atan2(x - w.d.cx, -(y - w.d.cy)) + Math.PI * 2) % (Math.PI * 2);
        const onEdge = Math.abs(r - (l.local ? w.d.r0 : w.d.r1)) < 0.02;
        // Modulo a turn: a ring turned by less than a wedge can have one that crosses the top.
        const into = (((a - w.a0) % (Math.PI * 2)) + Math.PI * 2) % (Math.PI * 2);
        const within = into <= w.a1 - w.a0 + 1e-3 || into >= Math.PI * 2 - 1e-3;
        return onEdge && within;
      });
      expect(hit).toBe(true);
    }
  }
  // 5. The bounds hold every ring.
  for (const d of cs.discs) {
    expect(d.cx - d.r1).toBeGreaterThanOrEqual(cs.bounds.x - EPS);
    expect(d.cx + d.r1).toBeLessThanOrEqual(cs.bounds.x + cs.bounds.w + EPS);
    expect(d.cy - d.r1).toBeGreaterThanOrEqual(cs.bounds.y - EPS);
    expect(d.cy + d.r1).toBeLessThanOrEqual(cs.bounds.y + cs.bounds.h + EPS);
  }
  return cs;
}

describe("layoutConstellation", () => {
  for (const [name, make] of Object.entries(SHAPES)) {
    it(`holds its invariants on the ${name} test KB`, () => {
      checkLayout(make());
    });
  }

  it("holds its invariants for any index", () => {
    fc.assert(
      fc.property(indexArb, (ix) => {
        checkLayout(ix);
      }),
      params,
    );
  });

  it("puts the root at the centre and its subfolders around it", () => {
    const cs = layoutConstellation(SHAPES.manual!());
    const root = cs.discs.find((d) => d.depth === 0)!;
    expect(root.cx).toBe(0);
    expect(root.cy).toBe(0);
    const parts = cs.discs.filter((d) => d.depth === 1);
    // Four parts, in four different directions.
    const dirs = new Set(parts.map((d) => Math.round((Math.atan2(d.cx, -d.cy) * 2) / Math.PI)));
    expect(dirs.size).toBe(4);
  });

  it("sizes a ring by its wedges, so a wedge is about the same size everywhere", () => {
    const cs = layoutConstellation(SHAPES.inventory!(), { wedge: 10, minRadius: 40 });
    for (const d of cs.discs) {
      if (d.r1 <= 40 + EPS) continue; // the minimum ring
      const slots = (Math.PI * 2 * d.r1) / 10;
      // One slot per wedge, no more: no empty slot makes a ring bigger.
      expect(slots).toBeCloseTo(d.wedges.length, 6);
    }
  });

  it("keeps a subtree close to its parent: no child further than it needs", () => {
    // A small sibling must not sit as far out as a big one (the textbook
    // balloon's flaw, which this layout replaced).
    const cs = layoutConstellation(SHAPES.inventory!());
    const root = cs.discs.find((d) => d.depth === 0)!;
    const kids = cs.discs.filter((d) => d.parent === root.id);
    const dist = (d: (typeof kids)[number]) => Math.hypot(d.cx - root.cx, d.cy - root.cy);
    const big = kids.reduce((a, b) => (a.size > b.size ? a : b));
    const small = kids.reduce((a, b) => (a.size < b.size ? a : b));
    expect(dist(small)).toBeLessThan(dist(big));
  });

  it("draws structural links too when asked, between a folder and its own contents", () => {
    const ix = SHAPES.manual!();
    const h = buildHierarchy(ix);
    const cs = layoutConstellation(h, { structural: true });
    expect(cs.links.length).toBe(h.edges.length);
    for (const l of cs.links) expect(l.d).not.toMatch(/NaN|Infinity/);
  });

  // n = 1, 2, 3, 7 items in one folder, with every mix of subfolders (ports,
  // first) and notes: the n wedges take the whole ring, n equal parts.
  for (const n of [1, 2, 3, 7]) {
    it(`fills a ring with its ${n} item${n > 1 ? "s" : ""}, whatever mix of subfolders and notes`, () => {
      for (let ports = 0; ports <= n; ports++) {
        const ix: Record<string, { title: string; links: string[] }> = {};
        for (let i = 0; i < ports; i++) ix[`f/s${i}/hoja`] = { title: `hoja ${i}`, links: [] };
        for (let i = ports; i < n; i++) ix[`f/nota-${i}`] = { title: `nota ${i}`, links: [] };
        const cs = layoutConstellation(ix);
        const f = cs.discs.find((d) => d.depth === 1 && d.title === "f");
        expect(f, `the ring of f (${ports} subfolders, ${n - ports} notes)`).toBeDefined();
        expect(f!.wedges.map((w) => w.kind)).toEqual([
          ...Array<string>(ports).fill("port"),
          ...Array<string>(n - ports).fill("note"),
        ]);
        expect(f!.wedges.length).toBe(n);
        expectWhole(f!.wedges);
        // Every ring of the site, the subfolders' too.
        for (const d of cs.discs) expectWhole(d.wedges);
      }
    });
  }

  it("puts every port on the side of its ring that faces its subfolder's ring", () => {
    // A port on the far side sends its line through its own ring, and a click
    // on the right of the ring goes to a ring on the left. polar(): angle
    // clockwise from the top.
    for (const shape of Object.values(SHAPES)) {
      const cs = layoutConstellation(shape());
      const byId = new Map(cs.discs.map((d) => [d.id, d]));
      for (const d of cs.discs) {
        for (const w of d.wedges) {
          if (w.kind !== "port") continue;
          const c = byId.get(w.id)!;
          const toward = Math.atan2(c.cx - d.cx, -(c.cy - d.cy));
          const mid = (w.a0 + w.a1) / 2;
          const off = Math.abs(Math.atan2(Math.sin(mid - toward), Math.cos(mid - toward)));
          expect(off, `${d.id} → ${w.id}`).toBeLessThan(Math.PI / 2);
        }
      }
    }
  });

  it("gives the ports the best slots there are: the narrowed search finds the full search's optimum", () => {
    const sq = (t: number[], n: number, js: number[]) =>
      js.reduce((s2, j, i) => {
        const x = Math.abs((j + 0.5) * (TAU / n) - t[i]!) % TAU;
        return s2 + Math.min(x, TAU - x) ** 2;
      }, 0);
    fc.assert(
      fc.property(
        fc.integer({ min: 1, max: 40 }).chain((n) =>
          fc.tuple(
            fc.constant(n),
            fc.array(real(0, TAU - 1e-9), {
              minLength: 1,
              maxLength: n,
            }),
          ),
        ),
        ([n, raw]) => {
          const t = [...raw].sort((a, b) => a - b);
          const fast = portSlots(t, n);
          const full = portSlots(t, n, true);
          // Distinct slots, in the ports' order round the ring.
          expect(new Set(fast).size).toBe(t.length);
          // Round the ring there is exactly one step down (where it wraps).
          if (fast.length > 1) {
            const downs = fast.filter((j, i) => j < fast[(i + fast.length - 1) % fast.length]!);
            expect(downs.length).toBe(1);
          }
          expect(sq(t, n, fast)).toBeCloseTo(sq(t, n, full), 9);
        },
      ),
      { ...params, numRuns: 2000 },
    );
  });

  it("turns a ring so its ports face their subfolders squarely, not half a slot off", () => {
    // Subfolders evenly round a ring with one slot each, the whole set turned
    // by any angle: every port exactly on its subfolder. With the slots fixed
    // at the top they were up to half a slot off (45 degrees with four).
    for (const k of [2, 3, 4, 6]) {
      for (const turn of [0, 0.1, 0.37, 0.5, 0.9]) {
        const step = TAU / k;
        const t = Array.from({ length: k }, (_, i) => (i + turn) * step).sort((a, b) => a - b);
        const r = facePorts(t, k);
        r.slots.forEach((j, i) => {
          const off = Math.abs(r.phase + (j + 0.5) * step - t[i]!) % TAU;
          expect(Math.min(off, TAU - off), `${k} slots, turned ${turn}`).toBeLessThan(1e-9);
        });
      }
    }
  });

  it("turns a ring no worse than not turning it, to the best turn for its slots, with the best slots for its turn", () => {
    // What facePorts guarantees, and nothing more: it searches from a few
    // starting turns and stops when a round no longer helps, so it may miss
    // the very best turn by a hair (a test comparing it with a grid of turns
    // failed at 1e-6). A property claims only what the code guarantees, so a
    // fresh seed finds only real bugs.
    const step = (n: number) => TAU / n;
    const sq = (t: number[], n: number, phase: number, js: number[]) =>
      js.reduce((s2, j, i) => {
        const x = Math.abs(phase + (j + 0.5) * step(n) - t[i]!) % TAU;
        return s2 + Math.min(x, TAU - x) ** 2;
      }, 0);
    const signed = (a: number) => {
      const x = ((a % TAU) + TAU) % TAU;
      return x > Math.PI ? x - TAU : x;
    };
    fc.assert(
      fc.property(
        fc
          .integer({ min: 1, max: 24 })
          .chain((n) =>
            fc.tuple(fc.constant(n), fc.array(real(0, TAU - 1e-9), { minLength: 1, maxLength: n })),
          ),
        ([n, raw]) => {
          const t = [...raw].sort((a, b) => a - b);
          const r = facePorts(t, n);
          expect(r.phase).toBeGreaterThanOrEqual(0);
          expect(r.phase).toBeLessThan(step(n));
          expect(sq(t, n, r.phase, r.slots)).toBeCloseTo(r.cost, 9);
          // Never worse than the ring at the top.
          expect(r.cost).toBeLessThanOrEqual(sq(t, n, 0, portSlots(t, n, true, 0)) + 1e-12);
          // The best turn for these slots: no mean offset left to take out.
          const mean =
            r.slots.reduce((s2, j, i) => s2 + signed(t[i]! - (r.phase + (j + 0.5) * step(n))), 0) /
            t.length;
          expect(Math.abs(mean)).toBeLessThan(1e-6);
          // The best slots for this turn.
          expect(r.cost).toBeCloseTo(sq(t, n, r.phase, portSlots(t, n, true, r.phase)), 9);
        },
      ),
      { ...params, numRuns: 300 },
    );
  });

  // What the reader sees: a subfolder's line left from the far side
  // of its ring and crossed it, and a click on the right of the ring led to a
  // ring on the left. Checked on the drawn line itself, the cubic from the
  // port to the subfolder's ring, sampled along its length.
  const crossings = (cs: ReturnType<typeof layoutConstellation>) => {
    const byId = new Map(cs.discs.map((d) => [d.id, d]));
    const bad: string[] = [];
    for (const t of cs.tree) {
      const n = t.d.match(/-?\d+(?:\.\d+)?/g)!.map(Number);
      const [x0, y0, x1, y1, x2, y2, x3, y3] = n as [
        number,
        number,
        number,
        number,
        number,
        number,
        number,
        number,
      ];
      const p = byId.get(t.from)!;
      const c = byId.get(t.to)!;
      for (let i = 1; i < 64; i++) {
        const u = i / 64;
        const v = 1 - u;
        const x = v ** 3 * x0 + 3 * v * v * u * x1 + 3 * v * u * u * x2 + u ** 3 * x3;
        const y = v ** 3 * y0 + 3 * v * v * u * y1 + 3 * v * u * u * y2 + u ** 3 * y3;
        // 0.02: the path's coordinates are rounded to hundredths.
        if (Math.hypot(x - p.cx, y - p.cy) < p.r1 - 0.02) {
          bad.push(`${t.from} → ${t.to}: inside its own ring at ${u.toFixed(2)}`);
          break;
        }
        if (Math.hypot(x - c.cx, y - c.cy) < c.r1 - 0.02) {
          bad.push(`${t.from} → ${t.to}: inside the subfolder's ring at ${u.toFixed(2)}`);
          break;
        }
      }
    }
    return bad;
  };

  it("draws no subfolder's line through its own ring or the subfolder's, on every test KB", () => {
    for (const [name, shape] of Object.entries(SHAPES))
      expect(crossings(layoutConstellation(shape())), name).toEqual([]);
  });

  it("draws no subfolder's line through a ring, for any index", () => {
    fc.assert(
      fc.property(indexArb, (ix) => {
        expect(crossings(layoutConstellation(ix))).toEqual([]);
      }),
      params,
    );
  });

  it("is deterministic", () => {
    expect(layoutConstellation(SHAPES.deep!())).toEqual(layoutConstellation(SHAPES.deep!()));
  });
});
