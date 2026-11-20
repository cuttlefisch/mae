// SPDX-License-Identifier: GPL-3.0-or-later
// Property tests for the ring geometry: invariants that must hold for ANY
// content index, not just the examples in geometry.test.ts. fast-check
// generates the inputs; on a failure it prints the shrunk counterexample and
// the seed, and `FC_SEED=<seed> npx vitest run` replays it.
import { describe, it, expect } from "vitest";
import fc from "fast-check";
import { real } from "./arbitraries";
import {
  buildRing,
  chordPath,
  normaliseSlug,
  wedgeCorner,
  wedgePath,
  wedgeThickness,
  VISITED_BAND,
  type IndexEntry,
} from "../src/geometry";

const seed = process.env.FC_SEED ? Number(process.env.FC_SEED) : undefined;
const params = { numRuns: 200, ...(seed !== undefined ? { seed } : {}) };

// Slugs shaped like a static site's: optional folder, with the variants that name the
// same page ("x/", "x/index", "/x") so normalisation collisions do happen.
// "Foo"/"foo" and "n1"/"n01" are equal to a base-sensitivity numeric collator:
// the order between them must still not depend on the index.
const segment = fc.constantFrom(
  "a",
  "b",
  "c",
  "notes",
  "adrs",
  "kb",
  "Ünïcode",
  "10",
  "2",
  "Foo",
  "foo",
  "n1",
  "n01",
);
const slugArb = fc
  .tuple(
    fc.option(segment, { nil: undefined }),
    segment,
    fc.constantFrom("", "/", "/index", "/index/", "#frag"),
    fc.boolean(),
  )
  .map(
    ([folder, name, suffix, lead]) =>
      `${lead ? "/" : ""}${folder ? `${folder}/` : ""}${name}${suffix}`,
  );
const indexArb = fc
  .array(
    fc.tuple(
      slugArb,
      fc.record({
        title: fc.option(fc.string({ maxLength: 6 }), { nil: undefined }),
        links: fc.array(slugArb, { maxLength: 4 }),
      }),
    ),
    { maxLength: 40 },
  )
  // A content index is an object: one entry per raw slug. Distinct raw slugs
  // may still normalise to the same note, which is the case that matters.
  .map((entries) => [...new Map(entries).entries()] as [string, IndexEntry][]);

const toIndex = (entries: [string, IndexEntry][]) => Object.fromEntries(entries);
const shape = (r: ReturnType<typeof buildRing>) => ({
  nodes: r.nodes.map((n) => [n.slug, n.href, n.title, n.group, n.angle.toFixed(9), n.degree]),
  edges: r.edges.map((e) => [e.source, e.target].sort().join("|")).sort(),
  groups: r.groups.map((g) => [g.name, g.size]),
});

describe("buildRing: invariants for any content index", () => {
  it("does not depend on the index's insertion order", () => {
    fc.assert(
      fc.property(indexArb, fc.integer({ min: 0, max: 3 }), (entries, gap) => {
        const a = buildRing(toIndex(entries), { groupGap: gap, locale: "en" });
        const b = buildRing(toIndex([...entries].reverse()), { groupGap: gap, locale: "en" });
        expect(shape(b)).toEqual(shape(a));
      }),
      params,
    );
  });

  it("places notes on strictly increasing angles, exactly one slot apart inside a group", () => {
    fc.assert(
      fc.property(indexArb, fc.integer({ min: 0, max: 3 }), (entries, gap) => {
        const r = buildRing(toIndex(entries), { groupGap: gap, locale: "en" });
        for (let i = 1; i < r.nodes.length; i++) {
          const prev = r.nodes[i - 1]!;
          const cur = r.nodes[i]!;
          const steps = (cur.angle - prev.angle) / r.slot;
          if (cur.group === prev.group) expect(steps).toBeCloseTo(1, 9);
          else expect(steps).toBeCloseTo(r.groups.length > 1 ? gap + 1 : 1, 9);
        }
        // The whole ring, gaps included, covers exactly one turn.
        const slots = r.nodes.length + (r.groups.length > 1 ? gap * r.groups.length : 0);
        if (r.nodes.length > 0) expect(r.slot * slots).toBeCloseTo(2 * Math.PI, 9);
      }),
      params,
    );
  });

  it("keeps each folder contiguous, root notes first", () => {
    fc.assert(
      fc.property(indexArb, (entries) => {
        const r = buildRing(toIndex(entries), { locale: "en" });
        const order = r.nodes.map((n) => n.group).filter((g, i, a) => i === 0 || g !== a[i - 1]);
        expect(new Set(order).size).toBe(order.length);
        if (order.includes("")) expect(order[0]).toBe("");
        for (const g of r.groups) expect(g.end).toBeGreaterThan(g.start);
      }),
      params,
    );
  });

  it("has undirected, unique edges between notes on the ring, and consistent degrees", () => {
    fc.assert(
      fc.property(indexArb, (entries) => {
        const r = buildRing(toIndex(entries), { locale: "en" });
        const onRing = new Set(r.nodes.map((n) => n.slug));
        const keys = r.edges.map((e) => [e.source, e.target].sort().join("|"));
        expect(new Set(keys).size).toBe(keys.length);
        for (const e of r.edges) {
          expect(e.source).not.toBe(e.target);
          expect(onRing.has(e.source) && onRing.has(e.target)).toBe(true);
        }
        expect(r.nodes.reduce((s, n) => s + n.degree, 0)).toBe(2 * r.edges.length);
      }),
      params,
    );
  });

  it("merges entries that name the same note: the union of their links is kept", () => {
    const r = buildRing(
      {
        "kb/": { title: "KB", links: ["a"] },
        "kb/index": { title: "KB index", links: ["b"] },
        a: {},
        b: {},
      },
      { locale: "en" },
    );
    const kb = r.nodes.filter((n) => n.slug === "kb");
    expect(kb).toHaveLength(1);
    expect(r.edges.map((e) => [e.source, e.target].sort().join("|")).sort()).toEqual([
      "a|kb",
      "b|kb",
    ]);
  });
});

describe("normaliseSlug", () => {
  it("is idempotent", () => {
    // Block body on purpose: an expression body would RETURN the matcher's
    // result (an object) to fast-check, which reads it as the verdict.
    fc.assert(
      fc.property(slugArb, (s) => {
        expect(normaliseSlug(normaliseSlug(s))).toBe(normaliseSlug(s));
      }),
      params,
    );
  });
});

describe("paths stay finite at any scale", () => {
  it("wedgePath never emits NaN or Infinity, from 1 to 2,500 notes and any corner radius", () => {
    fc.assert(
      fc.property(
        fc.integer({ min: 1, max: 2500 }),
        real(0, 50),
        real(1, 400),
        real(0.5, 40),
        (n, corner, innerR, thickness) => {
          const slot = (2 * Math.PI) / n;
          const d = wedgePath(500, 500, innerR, innerR + thickness, 0, slot, corner);
          expect(d).not.toMatch(/NaN|Infinity/);
          expect(d.startsWith("M ")).toBe(true);
          expect(d.endsWith("Z")).toBe(true);
        },
      ),
      params,
    );
  });

  it("the visited band's inner corners are its wedge's, at any thickness and ring size", () => {
    // The band is drawn from the wedge's inner edge with the wedge's corner;
    // wedgePath clamps a corner to half the thickness it is drawn on, so a
    // corner larger than the band can carry would round the band less than
    // its wedge and the band would poke out (the dialog, before wedgeCorner).
    // Segments of a rounded path: M, outer arc, outer fillet, L, inner fillet,
    // inner arc, inner fillet, L, outer fillet, Z; unrounded: M, A, L, A, Z.
    const innerPart = (d: string) => {
      const seg = d.split(/ (?=[MALZ])/);
      return seg.length > 5 ? seg.slice(3, 7) : seg.slice(2, 4);
    };
    fc.assert(
      fc.property(
        fc.integer({ min: 1, max: 2500 }),
        real(1, 200),
        real(0.5, 16),
        (n, innerR, thickness) => {
          const slot = (2 * Math.PI) / n;
          const corner = wedgeCorner(thickness);
          const wedge = wedgePath(0, 0, innerR, innerR + thickness, 0, slot, corner);
          const band = wedgePath(
            0,
            0,
            innerR,
            innerR + thickness * VISITED_BAND,
            0,
            slot,
            corner,
            0,
          );
          // The band's own radial line runs to its own outer edge: compare from
          // its inner fillet on.
          expect(innerPart(band).slice(1)).toEqual(innerPart(wedge).slice(1));
        },
      ),
      params,
    );
  });

  it("chordPath's control point sits (1 - pull) of the way from the centre to the chord's midpoint", () => {
    fc.assert(
      fc.property(real(-Math.PI, Math.PI), real(-Math.PI, Math.PI), real(0, 1), (a0, a1, pull) => {
        const d = chordPath(0, 0, 100, a0, a1, pull);
        const [, x0, y0, qx, qy, x1, y1] = d
          .match(/^M (\S+) (\S+) Q (\S+) (\S+) (\S+) (\S+)$/)!
          .map(Number);
        const mx = (x0! + x1!) / 2;
        const my = (y0! + y1!) / 2;
        expect(Math.hypot(qx!, qy!)).toBeCloseTo((1 - pull) * Math.hypot(mx, my), 1);
      }),
      params,
    );
  });

  it("wedgeThickness stays within its bounds and never shrinks as the diagram shrinks", () => {
    fc.assert(
      fc.property(fc.integer({ min: 1, max: 4000 }), fc.integer({ min: 1, max: 4000 }), (a, b) => {
        const [small, big] = a < b ? [a, b] : [b, a];
        const ts = wedgeThickness(small, 1000);
        const tb = wedgeThickness(big, 1000);
        for (const t of [ts, tb]) expect(t >= 1 && t <= 14).toBe(true);
        expect(ts).toBeGreaterThanOrEqual(tb);
      }),
      params,
    );
  });
});
