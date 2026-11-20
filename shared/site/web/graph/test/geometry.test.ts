// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, test } from "vitest";
import {
  pageHref,
  buildRing,
  chordPath,
  groupOf,
  neighbours,
  normaliseSlug,
  polar,
  wedgePath,
  wedgeThickness,
} from "../src/geometry";

const index = {
  index: { title: "Home", links: ["requirements/index", "systems/server-a"] },
  "requirements/index": { title: "Requirements hub", links: ["requirements/req-8"] },
  "requirements/req-8": {
    title: "req.8",
    links: ["systems/server-a", "requirements/req-8", "missing/note"],
  },
  "systems/server-a": { title: "Server A", links: ["requirements/req-8"] },
  "systems/index": { title: "Systems hub", links: [] },
};

describe("slugs", () => {
  test("folder pages and trailing slashes normalise to the folder", () => {
    expect(normaliseSlug("requirements/index")).toBe("requirements");
    expect(normaliseSlug("requirements/")).toBe("requirements");
    expect(normaliseSlug("/index")).toBe("index");
    expect(normaliseSlug("a/b#heading")).toBe("a/b");
  });
  test("group is the top-level folder", () => {
    expect(groupOf("requirements/req-8")).toBe("requirements");
    expect(groupOf("index")).toBe("");
    expect(groupOf("requirements/index")).toBe("");
  });
});

describe("buildRing", () => {
  const ring = buildRing(index, { locale: "en" });

  test("root notes first, then folders in order, titles within a folder", () => {
    expect(ring.nodes.map((n) => n.slug)).toEqual([
      "index",
      "requirements",
      "systems",
      "requirements/req-8",
      "systems/server-a",
    ]);
  });

  test("one undirected edge per linked pair; self and dangling links dropped", () => {
    const pairs = ring.edges.map((e) => [e.source, e.target].sort().join(" | ")).sort();
    expect(pairs).toEqual([
      "index | requirements",
      "index | systems/server-a",
      "requirements | requirements/req-8",
      "requirements/req-8 | systems/server-a",
    ]);
  });

  test("href is the page's canonical URL, a folder index as the folder (as the site links it)", () => {
    expect(ring.nodes.find((n) => n.slug === "requirements")?.href).toBe("requirements/");
  });

  test("degree counts distinct neighbours", () => {
    const deg = Object.fromEntries(ring.nodes.map((n) => [n.slug, n.degree]));
    expect(deg).toEqual({
      index: 2,
      requirements: 2,
      systems: 0,
      "requirements/req-8": 2,
      "systems/server-a": 2,
    });
  });

  test("equal slots, first node at the top", () => {
    expect(ring.slot).toBeCloseTo((2 * Math.PI) / 5);
    expect(ring.nodes[0]!.angle).toBeCloseTo(-Math.PI / 2);
    expect(ring.nodes[1]!.angle - ring.nodes[0]!.angle).toBeCloseTo(ring.slot);
  });

  test("deterministic regardless of the index's key order", () => {
    const shuffled = Object.fromEntries(Object.entries(index).reverse());
    expect(buildRing(shuffled, { locale: "en" })).toEqual(ring);
  });

  test("groups record their arc, in ring order", () => {
    expect(ring.groups.map((g) => [g.name, g.size])).toEqual([
      ["", 3],
      ["requirements", 1],
      ["systems", 1],
    ]);
    for (const g of ring.groups) expect(g.end).toBeGreaterThan(g.start);
  });

  test("groupGap leaves empty slots between groups and nowhere else", () => {
    const gapped = buildRing(index, { locale: "en", groupGap: 2 });
    // 5 notes + 2 empty slots per group (3 groups) = 11 slots.
    expect(gapped.slot).toBeCloseTo((2 * Math.PI) / 11);
    const steps = gapped.nodes
      .slice(1)
      .map((n, i) => Math.round((n.angle - gapped.nodes[i]!.angle) / gapped.slot));
    // Within the root group: 1 slot apart; across a group boundary: 1 + 2.
    expect(steps).toEqual([1, 1, 3, 3]);
  });

  test("groupGap is ignored when there is only one group", () => {
    const one = buildRing({ a: { title: "A" }, b: { title: "B" } }, { groupGap: 5 });
    expect(one.slot).toBeCloseTo(Math.PI);
  });

  test("excluded notes leave the ring and take their edges with them", () => {
    const r = buildRing(index, { exclude: (s) => s.startsWith("systems"), locale: "en" });
    expect(r.nodes.map((n) => n.slug)).not.toContain("systems/server-a");
    expect(
      r.edges.every((e) => !e.source.startsWith("systems") && !e.target.startsWith("systems")),
    ).toBe(true);
  });

  test("neighbours in both directions", () => {
    expect([...neighbours(ring, "requirements/req-8")].sort()).toEqual([
      "requirements",
      "systems/server-a",
    ]);
  });

  test("an empty index gives an empty ring, not NaN angles", () => {
    expect(buildRing({})).toEqual({ nodes: [], edges: [], groups: [], slot: 0 });
  });
});

describe("paths", () => {
  const numbers = (d: string) => (d.match(/-?\d+(\.\d+)?(e-?\d+)?/g) ?? []).map(Number);

  test("wedge path is closed and finite, rounded and not", () => {
    for (const corner of [0, 2]) {
      const d = wedgePath(0, 0, 80, 92, 0, 0.3, corner);
      expect(d.startsWith("M ")).toBe(true);
      expect(d.endsWith("Z")).toBe(true);
      expect(numbers(d).every(Number.isFinite)).toBe(true);
    }
  });

  test("a wedge too thin or too short for its corner degrades instead of breaking", () => {
    // 618 notes: a slot of ~0.01 rad, far smaller than the requested corner.
    const d = wedgePath(0, 0, 80, 81, 0, (2 * Math.PI) / 618, 5);
    expect(numbers(d).every(Number.isFinite)).toBe(true);
  });

  test("rounded corners never cross: the outer edge still runs forward", () => {
    // 618 notes on a ring of radius 80, a corner far larger than the slot
    // allows. Without the angular clamp the two outer fillets overlap and the
    // outer arc runs backwards, a self-intersecting petal.
    const slot = (2 * Math.PI) / 618;
    const n = numbers(wedgePath(0, 0, 80, 81, 0, slot, 5));
    const startAngle = Math.atan2(n[1]!, n[0]!);
    const endAngle = Math.atan2(n[8]!, n[7]!);
    expect(endAngle).toBeGreaterThan(startAngle);
  });

  test("inner and outer corners round separately: the visited band is square outside", () => {
    // The visited band sits on the wedge's inner edge: rounded there like the
    // wedge, square on its outer edge, which lies inside the wedge.
    const d = wedgePath(0, 0, 80, 92, 0, 0.3, 3, 0);
    const n = numbers(d);
    const [x, y] = polar(0, 0, 92, 0);
    // Starts exactly on the outer corner: no fillet there.
    expect(n[0]).toBeCloseTo(x);
    expect(n[1]).toBeCloseTo(y);
    // Arcs in drawing order: outer edge, outer fillet, inner fillet, inner edge,
    // inner fillet, outer fillet.
    const radii = [...d.matchAll(/A (\S+) /g)].map((m) => Number(m[1]));
    expect(radii).toEqual([92, 0, 3, 80, 3, 0]);
    // Omitting the outer radius keeps the old behaviour: both corners alike.
    expect(wedgePath(0, 0, 80, 92, 0, 0.3, 3)).toBe(wedgePath(0, 0, 80, 92, 0, 0.3, 3, 3));
  });

  test("chord runs from one ring point to the other, bent toward the centre", () => {
    const d = chordPath(0, 0, 50, 0, Math.PI / 2, 0.5);
    const [x0, y0] = polar(0, 0, 50, 0);
    const [x1, y1] = polar(0, 0, 50, Math.PI / 2);
    const n = numbers(d);
    expect(n[0]).toBeCloseTo(x0);
    expect(n[1]).toBeCloseTo(y0);
    expect(n[4]).toBeCloseTo(x1);
    expect(n[5]).toBeCloseTo(y1);
    // Midpoint (25, 25) pulled halfway to the centre.
    expect(n[2]).toBeCloseTo(12.5);
    expect(n[3]).toBeCloseTo(12.5);
  });

  test("pull 1 sends the chord through the centre", () => {
    const n = numbers(chordPath(10, 20, 50, 0, 2, 1));
    expect(n[2]).toBeCloseTo(10);
    expect(n[3]).toBeCloseTo(20);
  });

  test("wedge thickness keeps a minimum on-screen size, within a cap", () => {
    // 300px wide drawing of a 200-unit viewBox: 12px is 8 units.
    expect(wedgeThickness(300, 200)).toBeCloseTo(8);
    // Very small drawing: capped.
    expect(wedgeThickness(50, 200)).toBe(14);
  });
});

describe("pageHref", () => {
  test("a folder's index page is the folder, as the site links it", () => {
    expect(pageHref("systems/index")).toBe("systems/");
    expect(pageHref("standard/operational-framework/index")).toBe(
      "standard/operational-framework/",
    );
  });
  test("the site's root index is the root", () => {
    expect(pageHref("index")).toBe("");
  });
  test("any other page keeps its path; a leading slash goes", () => {
    expect(pageHref("systems/server-a")).toBe("systems/server-a");
    expect(pageHref("/requirements/req-8")).toBe("requirements/req-8");
  });
  test("a name that only ends in index is not a folder index", () => {
    expect(pageHref("notes/reindex")).toBe("notes/reindex");
    expect(pageHref("notes/index-of-changes")).toBe("notes/index-of-changes");
  });
});
