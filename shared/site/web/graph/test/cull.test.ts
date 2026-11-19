// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import fc from "fast-check";
import {
  MARGIN,
  PIECES,
  boxOf,
  labelReach,
  linkMeets,
  meets,
  pieceBoxes,
  splitBezier,
  visibleArea,
  type Pt,
} from "../src/cull";
import { buildScene, type Label, type Layout } from "../src/scene";
import { rng, zettel } from "./kbs/shapes";
import { fit, panBy, viewBox, type Box, type View } from "../src/viewport";
import { logReal, params, real } from "./arbitraries";

const viewArb = fc.record({ cx: real(-1e4, 1e4), cy: real(-1e4, 1e4), k: logReal(0.01, 200) });
const screenArb = fc.tuple(real(50, 3000), real(50, 3000));
const boxArb = fc.record({
  x: real(-2e4, 2e4),
  y: real(-2e4, 2e4),
  w: real(0, 5e3),
  h: real(0, 5e3),
});
const inside = (a: Box, b: Box, eps: number) =>
  a.x >= b.x - eps &&
  a.y >= b.y - eps &&
  a.x + a.w <= b.x + b.w + eps &&
  a.y + a.h <= b.y + b.h + eps;

describe("what is drawn", () => {
  it("without margin, the area is exactly what the viewBox frames", () => {
    fc.assert(
      fc.property(viewArb, screenArb, (v, [sw, sh]) => {
        const [x, y, w, h] = viewBox(v, sw, sh).split(" ").map(Number) as [
          number,
          number,
          number,
          number,
        ];
        const a = visibleArea(v, sw, sh, 0);
        const eps = 1e-3 + 1e-9 * (Math.abs(x) + Math.abs(y) + w + h);
        return (
          Math.abs(a.x - x) <= eps &&
          Math.abs(a.y - y) <= eps &&
          Math.abs(a.w - w) <= eps &&
          Math.abs(a.h - h) <= eps
        );
      }),
      params,
    );
  });

  it("anything with a part on screen is drawn (its box meets the area)", () => {
    fc.assert(
      fc.property(viewArb, screenArb, boxArb, (v: View, [sw, sh], b) => {
        // Its corners on screen.
        const x0 = (b.x - v.cx) * v.k + sw / 2;
        const y0 = (b.y - v.cy) * v.k + sh / 2;
        const x1 = x0 + b.w * v.k;
        const y1 = y0 + b.h * v.k;
        const onScreen = x0 < sw && x1 > 0 && y0 < sh && y1 > 0;
        fc.pre(onScreen);
        return meets(b, visibleArea(v, sw, sh, 0)) && meets(b, visibleArea(v, sw, sh));
      }),
      params,
    );
  });

  it("and what is far off screen is not", () => {
    // A box a whole margin beyond the screen's edge, on any side.
    const v: View = { cx: 0, cy: 0, k: 2 };
    const a = visibleArea(v, 800, 600);
    const far = 1 + 1e-6;
    for (const b of [
      { x: a.x + a.w * far, y: 0, w: 1, h: 1 },
      { x: a.x - a.w * (far - 1) - 1 - 1e-3, y: 0, w: 1, h: 1 },
      { x: 0, y: a.y + a.h * far, w: 1, h: 1 },
      { x: 0, y: a.y - 1 - 1e-3, w: 1, h: 1 },
    ])
      expect(meets(b, a), JSON.stringify(b)).toBe(false);
    expect(meets({ x: a.x + a.w, y: 0, w: 1, h: 1 }, a)).toBe(true); // touching counts
  });

  it(`MARGIN: after a pan of up to half a screen, all of the new screen was already drawn`, () => {
    // Why ${MARGIN}: labels are placed again only at rest (60 ms after the last
    // move); until then the ones near the old view must cover the new one.
    fc.assert(
      fc.property(viewArb, screenArb, real(-0.5, 0.5), real(-0.5, 0.5), (v, [sw, sh], fx, fy) => {
        const moved = panBy(v, fx * sw, fy * sh);
        return inside(
          visibleArea(moved, sw, sh, 0),
          visibleArea(v, sw, sh),
          1e-6 * (1 + Math.abs(v.cx) + Math.abs(v.cy)),
        );
      }),
      params,
    );
    // And no more than needed: with less, a half-screen pan shows undrawn space.
    const v: View = { cx: 0, cy: 0, k: 1 };
    expect(
      inside(visibleArea(panBy(v, 400, 0), 800, 600, 0), visibleArea(v, 800, 600, MARGIN * 0.9), 0),
    ).toBe(false);
  });

  const labelArb = fc.record({
    x: real(-1e4, 1e4),
    y: real(-1e4, 1e4),
    rotate: real(-360, 360),
    anchor: fc.constantFrom<Label["anchor"]>("start", "middle", "end"),
    room: real(0, 2e3),
    height: real(0, 200),
  });

  it("a straight label shown (no longer than room, no taller than height) stays inside its reach", () => {
    fc.assert(
      fc.property(labelArb, real(0, 1), real(0, 1), (l, fl, fh) => {
        const len = fl * l.room;
        const hgt = fh * l.height;
        const r = (l.rotate * Math.PI) / 180;
        const start = l.anchor === "start" ? 0 : l.anchor === "middle" ? -len / 2 : -len;
        const reach = labelReach(l);
        // The text's four corners, along the rotated baseline and across it.
        for (const along of [start, start + len])
          for (const across of [-hgt / 2, hgt / 2]) {
            const x = l.x + along * Math.cos(r) - across * Math.sin(r);
            const y = l.y + along * Math.sin(r) + across * Math.cos(r);
            if (!inside({ x, y, w: 0, h: 0 }, reach, 1e-6 * (1 + Math.abs(l.x) + Math.abs(l.y))))
              return false;
          }
        return true;
      }),
      params,
    );
  });

  it("a label along an arc (centred on its middle, no longer than room) stays inside its reach", () => {
    fc.assert(
      fc.property(
        real(1, 5e3),
        real(-10, 10),
        real(0, 1),
        real(0, 200),
        real(0, 1),
        (rm, mid, fl, height, t) => {
          // The arc of radius rm around the origin, middle at angle `mid`
          // (clockwise from the top, as polar() in geometry.ts); the text covers
          // len of it, centred, and stands up to height/2 off it.
          const room = rm * Math.min(2 * Math.PI, 2);
          const len = fl * room;
          const l: Label = {
            x: rm * Math.sin(mid),
            y: -rm * Math.cos(mid),
            rotate: 0,
            anchor: "middle",
            room,
            height,
            path: "M0,0",
          };
          const a = mid + ((t - 0.5) * len) / rm;
          const reach = labelReach(l);
          for (const dr of [-height / 2, height / 2]) {
            const p = { x: (rm + dr) * Math.sin(a), y: -(rm + dr) * Math.cos(a), w: 0, h: 0 };
            if (!inside(p, reach, 1e-6 * rm)) return false;
          }
          return true;
        },
      ),
      params,
    );
  });
});

// ---- links: the boxes of their pieces (cull.ts, PIECES) ----

/** A point of a Bézier given by its control points, at parameter t (de Casteljau). */
function at(ctrl: Pt[], t: number): Pt {
  let pts = ctrl;
  while (pts.length > 1)
    pts = pts.slice(1).map((b, j): Pt => {
      const a = pts[j]!;
      return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    });
  return pts[0]!;
}

/** The Béziers of an SVG path as drawn (M, then L, Q and C only: what the layouts write). */
function beziersOf(d: string): Pt[][] {
  const tok = d.match(/[MLQC]|-?\d+(?:\.\d+)?(?:e-?\d+)?/g)!;
  const out: Pt[][] = [];
  let cur: Pt = [0, 0];
  let i = 0;
  while (i < tok.length) {
    const cmd = tok[i++]!;
    const n = { M: 1, L: 1, Q: 2, C: 3 }[cmd as "M"]!;
    expect(n, `command ${cmd} in ${d}`).toBeDefined();
    const pts: Pt[] = [];
    for (let k = 0; k < n; k++) pts.push([Number(tok[i++]), Number(tok[i++])]);
    if (cmd !== "M") out.push([cur, ...pts]);
    cur = pts[pts.length - 1]!;
  }
  return out;
}

const ptArb = fc.tuple(real(-1e4, 1e4), real(-1e4, 1e4));
const bezierArb = fc.array(ptArb, { minLength: 2, maxLength: 4 });
const holds = (b: Box, p: Pt, eps: number) =>
  p[0] >= b.x - eps && p[0] <= b.x + b.w + eps && p[1] >= b.y - eps && p[1] <= b.y + b.h + eps;

describe("links: what is drawn by the boxes of their pieces", () => {
  it("splitBezier: n pieces of equal parameter, each exactly that stretch of the curve", () => {
    fc.assert(
      fc.property(bezierArb, fc.integer({ min: 1, max: 9 }), real(0, 1), (ctrl, n, u) => {
        const pieces = splitBezier(ctrl, n);
        if (pieces.length !== n) return false;
        const eps = 1e-6 * (1 + Math.max(...ctrl.flat().map(Math.abs)));
        return pieces.every((piece, i) => {
          if (piece.length !== ctrl.length) return false;
          const p = at(piece, u);
          const q = at(ctrl, (i + u) / n);
          return Math.abs(p[0] - q[0]) <= eps && Math.abs(p[1] - q[1]) <= eps;
        });
      }),
      params,
    );
  });

  it("pieceBoxes: the boxes hold the whole curve, piece by piece", () => {
    fc.assert(
      fc.property(bezierArb, fc.integer({ min: 1, max: 9 }), real(0, 1), (ctrl, n, t) => {
        const boxes = pieceBoxes([ctrl], n);
        const i = Math.min(n - 1, Math.floor(t * n));
        const eps = 1e-6 * (1 + Math.max(...ctrl.flat().map(Math.abs)));
        return boxes.length === n && holds(boxes[i]!, at(ctrl, t), eps);
      }),
      params,
    );
  });

  const scenes = (["sunburst", "balloon"] as Layout[]).map((layout) => ({
    layout,
    scene: buildScene(zettel(2, 400), layout),
  }));

  it("every link of both layouts: its pieces' boxes hold the path as drawn, and its box holds them", () => {
    for (const { layout, scene } of scenes) {
      expect(scene.edges.length, layout).toBeGreaterThan(500);
      for (const e of scene.edges) {
        // The path is rounded to 0.01 when written.
        for (const bz of beziersOf(e.d))
          for (let k = 0; k <= 32; k++) {
            const p = at(bz, k / 32);
            if (!e.boxes.some((b) => holds(b, p, 0.011)))
              expect.fail(`${layout} ${e.source}->${e.target}: ${p} outside its boxes`);
          }
        for (const b of e.boxes)
          if (!holds(e.box, [b.x, b.y], 1e-6) || !holds(e.box, [b.x + b.w, b.y + b.h], 1e-6))
            expect.fail(`${layout} ${e.source}->${e.target}: a piece outside its box`);
      }
    }
  });

  // Zoomed far in (64 times the whole site) on the zettel test site, the 1280 x
  // 800 screen moved over 60 places: how many links are drawn in all.
  const SW = 1280;
  const SH = 800;
  function drawnAt64(layout: Layout, boxesOf: (e: { d: string; boxes: Box[] }) => Box[]) {
    const { scene } = scenes.find((s) => s.layout === layout)!;
    const links = scene.edges.map((e) => ({ box: e.box, boxes: boxesOf(e) }));
    const all = scene.bounds;
    const k = fit(all, SW, SH).k * 64;
    const r = rng(11);
    let n = 0;
    for (let i = 0; i < 60; i++) {
      const area = visibleArea({ cx: all.x + r() * all.w, cy: all.y + r() * all.h, k }, SW, SH);
      for (const l of links) if (linkMeets(l, area)) n++;
    }
    return n;
  }

  it("a link whose curve crosses the area around the view is drawn", () => {
    for (const { layout, scene } of scenes) {
      const all = scene.bounds;
      const r = rng(5);
      for (let i = 0; i < 15; i++) {
        const k = fit(all, SW, SH).k * 4 ** (1 + (i % 3));
        const area = visibleArea({ cx: all.x + r() * all.w, cy: all.y + r() * all.h, k }, SW, SH);
        for (const e of scene.edges)
          for (const bz of beziersOf(e.d))
            for (let s = 0; s <= 12; s++)
              if (holds(area, at(bz, s / 12), -0.011) && !linkMeets(e, area))
                expect.fail(`${layout} ${e.source}->${e.target} crosses the area and is not drawn`);
      }
    }
  });

  it(`PIECES (${PIECES}): half as many draws far more links, twice as many saves little`, () => {
    // The pieces each link carries, and the same links cut finer and coarser.
    const recut = (n: number) => (e: { d: string }) => pieceBoxes(beziersOf(e.d), n);
    for (const layout of ["balloon"] as Layout[]) {
      const now = drawnAt64(layout, (e) => e.boxes);
      const whole = drawnAt64(layout, (e) => [boxOf(beziersOf(e.d).flat())]);
      const half = drawnAt64(layout, recut(PIECES / 2));
      const twice = drawnAt64(layout, recut(PIECES * 2));
      const seen = JSON.stringify({ layout, whole, half, now, twice });
      // Measured (2026-11-12): 324, 95, 70 and 59 links a view. With 8, half as
      // many (4) draws 19% more; with 2, twice as many saves 26%.
      expect(now, seen).toBeLessThan(whole / 3);
      expect(half, seen).toBeGreaterThan(now * 1.2);
      expect(twice, seen).toBeGreaterThan(now * 0.8);
    }
  });
});
