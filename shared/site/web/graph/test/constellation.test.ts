// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import fc from "fast-check";
import { layoutConstellation } from "../src/constellation";
import { buildHierarchy } from "../src/hierarchy";
import { SHAPES } from "./kbs/shapes";
import { indexArb, params } from "./arbitraries";

const EPS = 1e-6;
const TAU = Math.PI * 2;

// A ring's wedges fill it: the first starts at 0, each starts where the one
// before ends, the last ends at a full turn, and all are the same size. (Ports
// were set apart from notes by one empty slot: n wedges took n of n + 1 parts
// of the ring, owner 2026-11-18.) A folder with only its own page has none.
function expectWhole(wedges: Array<{ a0: number; a1: number }>) {
  if (wedges.length === 0) return;
  expect(wedges[0]!.a0).toBeCloseTo(0, 9);
  for (let i = 1; i < wedges.length; i++) expect(wedges[i]!.a0).toBeCloseTo(wedges[i - 1]!.a1, 9);
  expect(wedges[wedges.length - 1]!.a1).toBeCloseTo(TAU, 9);
  for (const w of wedges) expect(w.a1 - w.a0).toBeCloseTo(TAU / wedges.length, 9);
  expect(wedges.reduce((s, w) => s + (w.a1 - w.a0), 0)).toBeCloseTo(TAU, 9);
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
    // Wedges in order, end to end, filling the turn: no empty slot anywhere.
    expectWhole(d.wedges);
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
        return onEdge && a >= w.a0 - 1e-3 && a <= w.a1 + 1e-3;
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

  it("is deterministic", () => {
    expect(layoutConstellation(SHAPES.deep!())).toEqual(layoutConstellation(SHAPES.deep!()));
  });
});
