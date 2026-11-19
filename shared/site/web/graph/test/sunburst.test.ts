// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import fc from "fast-check";
import {
  allocate,
  layoutSunburst,
  ownId,
  polar,
  type Orientation,
  type Segment,
} from "../src/sunburst";
import { buildHierarchy } from "../src/hierarchy";
import { SHAPES } from "./kbs/shapes";
import { indexArb, params, real } from "./arbitraries";

const TAU = Math.PI * 2;
const EPS = 1e-9;

describe("allocate", () => {
  it("splits by weight", () => {
    const s = allocate([1, 3], 0, 4, 0);
    expect(s).toEqual([
      [0, 1],
      [1, 4],
    ]);
  });

  it("raises a small share to the minimum and takes it from the others by weight", () => {
    const [a, b, c] = allocate([1, 50, 50], 0, 10, 1);
    expect(a![1] - a![0]).toBeCloseTo(1, 12);
    expect(b![1] - b![0]).toBeCloseTo(4.5, 12);
    expect(c![1] - c![0]).toBeCloseTo(4.5, 12);
  });

  it("shares equally when even the minimum does not fit", () => {
    const s = allocate([1, 100, 5], 0, 3, 2);
    for (const [x, y] of s) expect(y - x).toBeCloseTo(1, 12);
  });

  it("property: contiguous, covers the span, never below the minimum when it fits", () => {
    fc.assert(
      fc.property(
        fc.array(fc.integer({ min: 1, max: 1000 }), { minLength: 1, maxLength: 60 }),
        real(0, 0.2),
        (ws, min) => {
          const s = allocate(ws, 1, 1 + TAU, min);
          expect(s[0]![0]).toBe(1);
          expect(s[s.length - 1]![1]).toBe(1 + TAU);
          for (let i = 1; i < s.length; i++) expect(s[i]![0]).toBe(s[i - 1]![1]);
          if (min * ws.length < TAU)
            for (const [x, y] of s) expect(y - x).toBeGreaterThanOrEqual(min - EPS);
        },
      ),
      params,
    );
  });
});

function checkLayout(ix: Parameters<typeof buildHierarchy>[0], orientation: Orientation) {
  const h = buildHierarchy(ix);
  const sb = layoutSunburst(h, { orientation });
  const byId = new Map<string, Segment>(sb.segments.map((s) => [s.id, s]));
  // 1. One segment per node except the root, plus one "own" per folder page
  // (the root's included), in both orientations.
  const nonRoot = [...h.nodes.values()].filter((n) => n.depth > 0);
  for (const n of nonRoot) expect(byId.has(n.id)).toBe(true);
  const owns = sb.segments.filter((s) => s.kind === "own");
  const withPage = [...h.nodes.values()].filter((n) => n.kind === "folder" && n.own);
  expect(owns.map((s) => s.id).sort()).toEqual(withPage.map((n) => ownId(n.id)).sort());
  expect(sb.segments.length).toBe(nonRoot.length + withPage.length);
  for (const s of sb.segments) {
    expect(s.a1).toBeGreaterThan(s.a0 - EPS);
    expect(s.r1).toBeGreaterThan(s.r0);
    // 2. Inside the parent's span.
    if (s.parent) {
      const p = byId.get(s.parent)!;
      expect(s.a0).toBeGreaterThanOrEqual(p.a0 - EPS);
      expect(s.a1).toBeLessThanOrEqual(p.a1 + EPS);
      // 3. On the right side of the parent: further out (top-inside) or further in (leaves-inside).
      if (orientation === "top-inside") expect(s.ring).toBe(p.ring + 1);
      else expect(s.ring).toBeLessThan(p.ring);
    }
  }
  // 4. Siblings do not overlap and keep the tree's order.
  for (const n of h.nodes.values()) {
    const kids = n.children.map((c) => byId.get(c.id)!);
    for (let i = 1; i < kids.length; i++)
      expect(kids[i]!.a0).toBeGreaterThanOrEqual(kids[i - 1]!.a1 - EPS);
  }
  // 5. The top level (the root's own page, then its children) covers the whole turn, contiguously.
  const top = sb.segments.filter((s) => s.parent === undefined).sort((a, b) => a.a0 - b.a0);
  if (top.length > 0) {
    expect(top[0]!.a0).toBeCloseTo(0, 9);
    expect(top[top.length - 1]!.a1).toBeCloseTo(TAU, 9);
    for (let i = 1; i < top.length; i++) expect(top[i]!.a0).toBeCloseTo(top[i - 1]!.a1, 9);
  }
  // 6. leaves-inside: every note on ring 0; top-inside: notes on their own depth.
  for (const n of nonRoot.filter((x) => x.kind === "note")) {
    expect(byId.get(n.id)!.ring).toBe(orientation === "leaves-inside" ? 0 : n.depth - 1);
  }
  // 7. One chord per real link, starting and ending on the disc's rim.
  const real = h.edges.filter((e) => !e.structural);
  expect(sb.chords.length).toBe(real.length);
  for (const c of sb.chords) {
    const nums = c.d.match(/-?\d+(?:\.\d+)?/g)!.map(Number);
    const [x0, y0] = [nums[0]!, nums[1]!];
    const [x1, y1] = [nums[nums.length - 2]!, nums[nums.length - 1]!];
    expect(Math.hypot(x0, y0)).toBeCloseTo(sb.disc, 1);
    expect(Math.hypot(x1, y1)).toBeCloseTo(sb.disc, 1);
    // The box holds both ends.
    for (const [x, y] of [
      [x0, y0],
      [x1, y1],
    ] as const) {
      expect(x).toBeGreaterThanOrEqual(c.box.x - 0.01);
      expect(x).toBeLessThanOrEqual(c.box.x + c.box.w + 0.01);
      expect(y).toBeGreaterThanOrEqual(c.box.y - 0.01);
      expect(y).toBeLessThanOrEqual(c.box.y + c.box.h + 0.01);
    }
  }
  // 7b. Each end of a chord sits at its own page's angle: inside the note's
  // segment, or a folder's own-page segment.
  const angleOf = (x: number, y: number) => (Math.atan2(x, -y) + TAU) % TAU;
  for (const c of sb.chords) {
    const nums = c.d.match(/-?\d+(?:\.\d+)?/g)!.map(Number);
    const ends: Array<[string, number, number]> = [
      [c.source, nums[0]!, nums[1]!],
      [c.target, nums[nums.length - 2]!, nums[nums.length - 1]!],
    ];
    for (const [id, x, y] of ends) {
      const a = angleOf(x, y);
      const node = h.nodes.get(id)!;
      const seg = byId.get(node.kind === "folder" ? ownId(id) : id)!;
      // The path rounds coordinates to 0.01: allow that much angle.
      const tol = 0.02 / sb.disc + EPS;
      const inside = a >= seg.a0 - tol && a <= seg.a1 + tol;
      const wrapped = a + TAU >= seg.a0 - tol && a + TAU <= seg.a1 + tol;
      expect(inside || wrapped).toBe(true);
    }
  }
  // 7c. top-inside: nothing is drawn outside a note (scene.ts starts a note's
  // callout label at the note's own edge on the strength of this).
  if (orientation === "top-inside") {
    const pages = sb.segments.filter((x) => x.kind !== "folder");
    for (const p of pages) {
      for (const o of sb.segments) {
        if (o === p || o.r0 < p.r1) continue;
        const overlap = Math.min(o.a1, p.a1) - Math.max(o.a0, p.a0);
        expect(overlap).toBeLessThanOrEqual(1e-9);
      }
    }
  }
  // 8. Every segment within the outer radius.
  for (const s of sb.segments) expect(s.r1).toBeLessThanOrEqual(sb.outer + EPS);
  return sb;
}

describe("layoutSunburst", () => {
  for (const orientation of ["top-inside", "leaves-inside"] as const) {
    for (const [name, make] of Object.entries(SHAPES)) {
      it(`${orientation}: holds its invariants on the ${name} test KB`, () => {
        checkLayout(make(), orientation);
      });
    }
    it(`${orientation}: holds its invariants for any index`, () => {
      fc.assert(
        fc.property(indexArb, (ix) => {
          checkLayout(ix, orientation);
        }),
        params,
      );
    });
  }

  it("draws structural links only when asked", () => {
    const ix = SHAPES.inventory!();
    const h = buildHierarchy(ix);
    const all = layoutSunburst(h, { structural: true });
    expect(all.chords.length).toBe(h.edges.length);
    expect(layoutSunburst(h).chords.length).toBe(h.edges.filter((e) => !e.structural).length);
  });

  it("widens tiny top-level folders to the minimum angle", () => {
    const sb = layoutSunburst(SHAPES.inventory!(), { minAngle: [3, 1] });
    const top = sb.segments.filter((s) => s.ring === 0);
    for (const s of top) expect(s.a1 - s.a0).toBeGreaterThanOrEqual((3 * Math.PI) / 180 - EPS);
  });

  it("is deterministic", () => {
    const a = layoutSunburst(SHAPES.zettel!());
    const b = layoutSunburst(SHAPES.zettel!());
    expect(b).toEqual(a);
  });

  it("anchors a chord between siblings near their folder, not through the centre", () => {
    // Two notes in one deep folder: their chord's control point stays near the rim.
    const ix = {
      "a/b/one": { title: "1", links: ["a/b/two"] },
      "a/b/two": { title: "2", links: [] },
      "z/far": { title: "far", links: ["a/b/one"] },
    };
    const sb = layoutSunburst(ix);
    const near = sb.chords.find((c) => c.source === "a/b/one" && c.target === "a/b/two")!;
    const far = sb.chords.find((c) => c.target === "z/far" || c.source === "z/far")!;
    const closest = (d: string) => {
      const n = d.match(/-?\d+(?:\.\d+)?/g)!.map(Number);
      let m = Infinity;
      for (let i = 0; i + 1 < n.length; i += 2) m = Math.min(m, Math.hypot(n[i]!, n[i + 1]!));
      return m;
    };
    expect(closest(near.d)).toBeGreaterThan(closest(far.d));
    expect(closest(far.d)).toBeLessThan(sb.disc * 0.3);
  });

  it("polar measures clockwise from the top", () => {
    const [x, y] = polar(1, Math.PI / 2);
    expect(x).toBeCloseTo(1, 12);
    expect(y).toBeCloseTo(0, 12);
    expect(polar(1, 0)[1]).toBeCloseTo(-1, 12);
  });
});
