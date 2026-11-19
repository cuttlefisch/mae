// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import fc from "fast-check";
import {
  arcLabelPath,
  buildScene,
  focusBox,
  LABEL_ARC_MAX,
  LABEL_ARC_MIN,
  navigate,
  sectorBox,
  type Layout,
  type Scene,
} from "../src/scene";
import { polar } from "../src/sunburst";
import { collectNotes, folderId } from "../src/hierarchy";
import { SHAPES } from "./kbs/shapes";
import { indexArb, params, real } from "./arbitraries";

const LAYOUTS: Array<[Layout, "top-inside" | "leaves-inside" | undefined]> = [
  ["sunburst", "top-inside"],
  ["sunburst", "leaves-inside"],
  ["balloon", undefined],
];

function walkNav(scene: Scene): string[] {
  const seen: string[] = [];
  const visit = (k: string) => {
    seen.push(k);
    for (const c of scene.navChildren.get(k) ?? []) if (c !== k) visit(c);
  };
  for (const k of scene.navTop) visit(k);
  return seen;
}

function check(
  ix: Parameters<typeof buildScene>[0],
  layout: Layout,
  orientation?: "top-inside" | "leaves-inside",
) {
  const scene = buildScene(ix, layout, { orientation });
  const notes = collectNotes(ix);
  const keys = new Set(scene.items.map((i) => i.key));
  // 1. Item keys are unique.
  expect(keys.size).toBe(scene.items.length);
  // 2. Every page has exactly one item that opens it, and that item links to it.
  const opening = scene.items.filter((i) => i.page !== undefined);
  expect(opening.length).toBe(notes.size);
  for (const slug of notes.keys()) {
    const node = scene.hierarchy.nodeOf.get(slug)!;
    const k = scene.pageItem.get(node)!;
    const it = scene.items.find((i) => i.key === k)!;
    expect(it.page).toBe(node);
    expect(it.href).toBe(notes.get(slug)!.href);
  }
  // 3. An item either opens a page or frames a folder, never both, never neither.
  for (const i of scene.items)
    expect((i.page === undefined) !== (i.folder === undefined)).toBe(true);
  // 4. The keyboard tree reaches every page exactly once, and only real items.
  const order = walkNav(scene);
  expect(new Set(order).size).toBe(order.length);
  for (const k of order) expect(keys.has(k)).toBe(true);
  for (const k of scene.pageItem.values()) expect(order).toContain(k);
  // 5. Right then Left comes back; Down cycles through a level.
  for (const k of order) {
    const into = navigate(scene, "ArrowRight", k);
    if (into) expect(navigate(scene, "ArrowLeft", into)).toBe(k);
    const parent = scene.navParent.get(k);
    const level = parent ? scene.navChildren.get(parent)! : scene.navTop;
    let at = k;
    for (let n = 0; n < level.length; n++) at = navigate(scene, "ArrowDown", at)!;
    expect(at).toBe(k);
  }
  // 6. A page's focus box holds it and its neighbours.
  const byKey = new Map(scene.items.map((i) => [i.key, i]));
  const inside = (a: { x: number; y: number; w: number; h: number }, b: typeof a) =>
    a.x >= b.x - 1e-6 &&
    a.y >= b.y - 1e-6 &&
    a.x + a.w <= b.x + b.w + 1e-6 &&
    a.y + a.h <= b.y + b.h + 1e-6;
  for (const [node, nbs] of scene.neighbours) {
    const f = focusBox(scene, node)!;
    expect(inside(byKey.get(scene.pageItem.get(node)!)!.box, f)).toBe(true);
    for (const n of nbs) expect(inside(byKey.get(scene.pageItem.get(n)!)!.box, f)).toBe(true);
  }
  // 7. Every item is inside the scene's bounds, and every folder's box holds its items.
  for (const i of scene.items) expect(inside(i.box, scene.bounds)).toBe(true);
  return scene;
}

describe("buildScene", () => {
  for (const [layout, orientation] of LAYOUTS) {
    const tag = `${layout}${orientation ? ` ${orientation}` : ""}`;
    for (const [name, make] of Object.entries(SHAPES)) {
      it(`${tag}: holds on the ${name} test KB`, () => {
        check(make(), layout, orientation);
      });
    }
    it(`${tag}: holds for any index`, () => {
      fc.assert(
        fc.property(indexArb, (ix) => {
          check(ix, layout, orientation);
        }),
        params,
      );
    });
  }

  it("sunburst: a folder's own page is its first item, inside it", () => {
    const scene = buildScene(SHAPES.manual!(), "sunburst");
    const part = scene.folderItem.get("\u0000folder:part-0")!;
    const first = navigate(scene, "ArrowRight", part)!;
    expect(scene.items.find((i) => i.key === first)!.title).toBe("Part 0");
    expect(scene.items.find((i) => i.key === first)!.page).toBe("\u0000folder:part-0");
  });
});

describe("buildScene: edge cases found in review", () => {
  it("a site whose only page is its home page draws that page, in every layout", () => {
    for (const [layout, orientation] of LAYOUTS) {
      const scene = buildScene({ index: { title: "Home", links: [] } }, layout, { orientation });
      const pages = scene.items.filter((i) => i.page !== undefined);
      expect(
        pages.map((i) => i.title),
        `${layout} ${orientation ?? ""}`,
      ).toEqual(["Home"]);
      expect(scene.navTop.length).toBe(1);
    }
  });

  it("Up, Home and End move within a level, wrapping", () => {
    const scene = buildScene(SHAPES.manual!(), "sunburst");
    const part = scene.folderItem.get("\u0000folder:part-0")!;
    const level = scene.navChildren.get(part)!;
    expect(level.length).toBeGreaterThan(2);
    expect(navigate(scene, "ArrowUp", level[1]!)).toBe(level[0]);
    expect(navigate(scene, "ArrowUp", level[0]!)).toBe(level[level.length - 1]);
    expect(navigate(scene, "Home", level[2]!)).toBe(level[0]);
    expect(navigate(scene, "End", level[0]!)).toBe(level[level.length - 1]);
    expect(navigate(scene, "ArrowLeft", scene.navTop[0]!)).toBeNull();
    expect(navigate(scene, "x", level[0]!)).toBeNull(); // not a navigation key
  });

  it("property: a folder's box holds every item inside that folder", () => {
    fc.assert(
      fc.property(indexArb, (ix) => {
        for (const [layout, orientation] of LAYOUTS) {
          const scene = buildScene(ix, layout, { orientation });
          const h = scene.hierarchy;
          const inside = (a: { x: number; y: number; w: number; h: number }, b: typeof a) =>
            a.x >= b.x - 1e-6 &&
            a.y >= b.y - 1e-6 &&
            a.x + a.w <= b.x + b.w + 1e-6 &&
            a.y + a.h <= b.y + b.h + 1e-6;
          for (const it of scene.items) {
            const node = it.page ?? it.folder!;
            for (let p = h.nodes.get(node)?.parent; p; p = p.parent) {
              const box = scene.folderBox.get(p.id);
              if (box) expect(inside(it.box, box), `${layout} ${it.key} in ${p.id}`).toBe(true);
            }
          }
        }
      }),
      params,
    );
  });
});

describe("sectorBox", () => {
  it("property: holds every point of the sector", () => {
    fc.assert(
      fc.property(
        real(1, 100),
        real(0.1, 50),
        real(0, 6.2),
        real(0.001, 6.28),
        (r0, dr, a0, span) => {
          const a1 = Math.min(Math.PI * 2, a0 + span);
          const b = sectorBox(r0, r0 + dr, a0, a1);
          for (let i = 0; i <= 40; i++) {
            for (const r of [r0, r0 + dr]) {
              const [x, y] = polar(r, a0 + ((a1 - a0) * i) / 40);
              expect(x).toBeGreaterThanOrEqual(b.x - 1e-9);
              expect(x).toBeLessThanOrEqual(b.x + b.w + 1e-9);
              expect(y).toBeGreaterThanOrEqual(b.y - 1e-9);
              expect(y).toBeLessThanOrEqual(b.y + b.h + 1e-9);
            }
          }
        },
      ),
      params,
    );
  });
});

describe("arcLabelPath: sunburst labels written along their ring", () => {
  const TAU = 2 * Math.PI;
  const num = String.raw`(-?\d+(?:\.\d+)?)`;
  const re = new RegExp(`^M${num},${num}A${num},${num} 0 ([01]) ([01]) ${num},${num}$`);
  // Angle of a point, clockwise from the top (the inverse of polar).
  const angleOf = (x: number, y: number) => Math.atan2(x, -y);
  // Signed difference of two angles, in (-pi, pi].
  const diff = (a: number, b: number) => {
    const d = (((a - b) % TAU) + TAU) % TAU;
    return d > Math.PI ? d - TAU : d;
  };

  it("on its circle, centred on it, through the segment's middle, past its ends, upright", () => {
    fc.assert(
      fc.property(real(20, 600), real(-20, 20), real(0, TAU), (r, mid, span) => {
        const m = re.exec(arcLabelPath(r, mid, span));
        expect(m).not.toBeNull();
        const [x0, y0, rx, , large, sweep, x1, y1] = m!.slice(1).map(Number) as number[];
        expect(rx).toBeCloseTo(r, 1);
        expect(Math.hypot(x0!, y0!)).toBeCloseTo(r, 1);
        expect(Math.hypot(x1!, y1!)).toBeCloseTo(r, 1);
        // It runs from mid - h to mid + h in its direction (sweep 1:
        // clockwise), the same h either side: so its middle is the segment's.
        const dir = sweep === 1 ? 1 : -1;
        const h0 = -dir * diff(angleOf(x0!, y0!), mid);
        const h1 = dir * diff(angleOf(x1!, y1!), mid);
        expect(Math.abs(h0 - h1)).toBeLessThan(0.01);
        // Between the bounds where its centre is well determined; more than
        // a half circle, so the large-arc flag.
        expect(h0).toBeGreaterThanOrEqual(LABEL_ARC_MIN - 0.01);
        expect(h0).toBeLessThanOrEqual(LABEL_ARC_MAX + 0.01);
        expect(large).toBe(1);
        // Where a renderer puts the arc's centre from what the path says
        // (SVG 1.1, F.6.5, for a circle): the ring's own centre, or the label
        // is off its ring. A half circle with rounded endpoints put it up to
        // a unit away, and one 0.05 short of a whole circle, 0.1.
        const hx = (x0! - x1!) / 2;
        const hy = (y0! - y1!) / 2;
        const q = hx * hx + hy * hy;
        const rr = Math.max(rx!, Math.sqrt(q));
        const coef = (large === sweep ? -1 : 1) * Math.sqrt(Math.max(0, (rr * rr - q) / q));
        const cx = coef * hy + (x0! + x1!) / 2;
        const cy = -coef * hx + (y0! + y1!) / 2;
        expect(Math.hypot(cx, cy)).toBeLessThan(0.005);
        // Past the segment's ends (up to the bound).
        expect(h0).toBeGreaterThanOrEqual(Math.min(span / 2 + 0.2, LABEL_ARC_MAX) - 0.01);
        // Reading direction at the middle: left to right, never leftwards.
        // The tangent of polar() at angle a, clockwise, is (cos a, sin a).
        expect(dir * Math.cos(mid)).toBeGreaterThanOrEqual(-1e-9);
      }),
      params,
    );
  });

  it("every sunburst label has its arc and room no longer than it; balloon labels stay straight", () => {
    for (const o of ["top-inside", "leaves-inside"] as const) {
      for (const shape of Object.values(SHAPES)) {
        const s = buildScene(shape(), "sunburst", { orientation: o });
        const labelled = s.items.filter((i) => i.label);
        expect(labelled.length).toBeGreaterThan(0);
        for (const i of labelled) {
          expect(i.label!.path).toBeTruthy();
          // The text is centred on the arc: it fits if it is no longer than it.
          const rm = Math.hypot(i.label!.x, i.label!.y);
          expect(i.label!.room).toBeLessThanOrEqual(2 * LABEL_ARC_MAX * rm + 1e-6);
        }
      }
    }
    const b = buildScene(SHAPES.manual!(), "balloon");
    expect(b.items.some((i) => i.label?.path || i.alt?.path)).toBe(false);
  });

  it("a folder that is nearly the whole ring: its label's room stops at its arc", () => {
    // One folder holding every page but one: its segment spans almost 360
    // degrees, more than the arc can (LABEL_ARC_MAX either side).
    const index: Record<string, { title: string; links: string[] }> = {
      index: { title: "Home", links: [] },
    };
    for (let n = 0; n < 40; n++) index[`big/p-${n}`] = { title: `P ${n}`, links: [] };
    const s = buildScene(index, "sunburst", { orientation: "top-inside" });
    const big = s.items.find((i) => i.folder === folderId("big"))!;
    const [a0, a1] = big.span!;
    expect(a1 - a0).toBeGreaterThan(2 * LABEL_ARC_MAX);
    const rm = Math.hypot(big.label!.x, big.label!.y);
    expect(big.label!.room).toBeCloseTo(2 * LABEL_ARC_MAX * rm, 6);
  });
});
