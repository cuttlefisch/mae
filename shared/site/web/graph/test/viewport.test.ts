// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import fc from "fast-check";
import {
  CONTROL_GAP,
  FIT_FLOOR,
  fit,
  fitClear,
  fitWith,
  freeAreas,
  overlaps,
  panBy,
  smoothZoom,
  toScreen as boxOnScreen,
  union,
  viewBox,
  zoomAnchor,
  zoomAt,
  type Box,
  type View,
} from "../src/viewport";
import { logReal, params, real } from "./arbitraries";

const finite = (min: number, max: number) => real(min, max);
const viewArb = fc.record({ cx: finite(-1e4, 1e4), cy: finite(-1e4, 1e4), k: logReal(0.01, 50) });

/** Screen point of a drawing point under view v on a sw x sh screen. */
const toScreen = (v: View, x: number, y: number, sw: number, sh: number) => [
  (x - v.cx) * v.k + sw / 2,
  (y - v.cy) * v.k + sh / 2,
];

describe("viewport", () => {
  it("fit frames the box inside the padding, centred", () => {
    const v = fit({ x: 10, y: 20, w: 100, h: 50 }, 400, 300, 20);
    expect(v.cx).toBe(60);
    expect(v.cy).toBe(45);
    expect(v.k).toBeCloseTo(3.6, 12); // min(360/100, 260/50)
  });

  it("property: fit's scale is finite and positive on any screen, however small (review: k was 0 under twice the padding)", () => {
    fc.assert(
      fc.property(
        fc.record({
          x: finite(-1e4, 1e4),
          y: finite(-1e4, 1e4),
          w: finite(0, 1e4),
          h: finite(0, 1e4),
        }),
        finite(0, 2000),
        finite(0, 2000),
        finite(0, 200),
        (b, sw, sh, pad) => {
          const v = fit(b, sw, sh, pad);
          expect(Number.isFinite(v.k)).toBe(true);
          expect(v.k).toBeGreaterThan(0);
          expect(viewBox(v, Math.max(1, sw), Math.max(1, sh))).not.toMatch(/NaN|Infinity/);
        },
      ),
      params,
    );
  });

  it("fit clamps the scale", () => {
    expect(fit({ x: 0, y: 0, w: 1, h: 1 }, 400, 300, 0, 0, 5).k).toBe(5);
    expect(fit({ x: 0, y: 0, w: 1e6, h: 1 }, 400, 300, 0, 0.01).k).toBe(0.01);
  });

  it("property: what fit frames is on screen, within the padding", () => {
    fc.assert(
      fc.property(
        fc.record({
          x: finite(-1e4, 1e4),
          y: finite(-1e4, 1e4),
          w: finite(0, 1e4),
          h: finite(0, 1e4),
        }),
        (b) => {
          const v = fit(b, 800, 600, 24);
          for (const [x, y] of [
            [b.x, b.y],
            [b.x + b.w, b.y + b.h],
          ] as const) {
            const [sx, sy] = toScreen(v, x, y, 800, 600);
            // 0.001 px: a box of zero size frames at scales near 1e8, where the
            // float error alone is ~1e-6 px (a flaky 1e-6 bound, found in review).
            expect(sx).toBeGreaterThanOrEqual(24 - 1e-3);
            expect(sx).toBeLessThanOrEqual(776 + 1e-3);
            expect(sy).toBeGreaterThanOrEqual(24 - 1e-3);
            expect(sy).toBeLessThanOrEqual(576 + 1e-3);
          }
        },
      ),
      params,
    );
  });

  it("property: zoomAt keeps the drawing point under the pointer where it was", () => {
    fc.assert(
      fc.property(viewArb, finite(0, 800), finite(0, 600), finite(0.2, 5), (v, px, py, f) => {
        const before = [(px - 400) / v.k + v.cx, (py - 300) / v.k + v.cy];
        const w = zoomAt(v, f, px, py, 800, 600);
        const [sx, sy] = toScreen(w, before[0]!, before[1]!, 800, 600);
        expect(sx).toBeCloseTo(px, 6);
        expect(sy).toBeCloseTo(py, 6);
        expect(w.k).toBeCloseTo(v.k * f, 9);
      }),
      params,
    );
  });

  it("panBy moves the drawing with the pointer", () => {
    const v = { cx: 0, cy: 0, k: 2 };
    const w = panBy(v, 10, -4);
    const [sx, sy] = toScreen(w, 0, 0, 800, 600);
    expect(sx).toBeCloseTo(410, 12);
    expect(sy).toBeCloseTo(296, 12);
  });

  it("viewBox spans the screen at the view's scale", () => {
    expect(viewBox({ cx: 0, cy: 0, k: 2 }, 800, 600)).toBe("-200 -150 400 300");
  });

  // Pairs of views: independent, or the second a hair's breadth from the first
  // (a pan from 1e-9 to 1e4 times the view's width, drawn log-uniformly). The
  // near pairs sit just above the pure-zoom threshold, where the log form of
  // -asinh cancels; independent views never land there, so without them the
  // mutation that brings that form back survived four runs in five.
  const near = fc
    // From 1e-13: both sides of the pure-zoom threshold (a pan of 1e-9 of
    // the view's width); from 1e-9 the pure-zoom branch was never drawn, and
    // only fc.double's pile-up at zero had ever covered it.
    .tuple(viewArb, logReal(0.01, 50), logReal(1e-13, 1e4), real(0, 2 * Math.PI))
    .map(([a, k, rel, ang]): [View, View] => {
      const d = (rel * 800) / Math.min(a.k, k);
      return [a, { cx: a.cx + d * Math.cos(ang), cy: a.cy + d * Math.sin(ang), k }];
    });
  // And the same centre exactly: a zoom in place.
  const inPlace = fc
    .tuple(viewArb, logReal(0.01, 50))
    .map(([a, k]): [View, View] => [a, { cx: a.cx, cy: a.cy, k }]);
  const pairArb = fc.oneof(fc.tuple(viewArb, viewArb), near, inPlace);

  it("smoothZoom in place (same centre) is a pure zoom: the centre stays, the width changes exponentially", () => {
    const a = { cx: 10, cy: -20, k: 1 };
    const b = { cx: 10, cy: -20, k: 8 };
    const z = smoothZoom(a, b, 800);
    // Width 800 -> 100: ln(8) / rho, rho = sqrt(2) (van Wijk and Nuij's default).
    expect(z.S).toBeCloseTo(Math.log(8) / Math.SQRT2, 9);
    for (const t of [0, 0.25, 0.5, 1]) {
      const v = z.at(t);
      expect(v.cx).toBe(10);
      expect(v.cy).toBe(-20);
      // Exponential in t: the width at t is 800 * 8^-t.
      expect(800 / v.k).toBeCloseTo(800 * 8 ** -t, 6);
    }
  });

  it("property: smoothZoom starts at a and ends at b", () => {
    fc.assert(
      fc.property(pairArb, ([a, b]) => {
        const z = smoothZoom(a, b, 800);
        const s = z.at(0);
        const e = z.at(1);
        // In screen pixels, which is what a reader sees: an absolute tolerance in
        // drawing units means nothing at a view 80000 units wide.
        for (const [p, q] of [
          [s, a],
          [e, b],
        ] as const) {
          expect(Math.abs(p.cx - q.cx) * q.k).toBeLessThan(1e-3);
          expect(Math.abs(p.cy - q.cy) * q.k).toBeLessThan(1e-3);
          // Scale error, as pixels of drift at the screen's edge (400 px out).
          expect(400 * Math.abs(p.k / q.k - 1)).toBeLessThan(1e-3);
        }
        expect(Number.isFinite(z.S)).toBe(true);
        expect(z.S).toBeGreaterThanOrEqual(0);
      }),
      params,
    );
  });

  it("smoothZoom between two far points zooms out on the way (the point of van Wijk and Nuij)", () => {
    const a = { cx: 0, cy: 0, k: 4 };
    const b = { cx: 5000, cy: 0, k: 4 };
    const mid = smoothZoom(a, b, 800).at(0.5);
    expect(mid.k).toBeLessThan(a.k);
  });

  it("union bounds every box; null for none", () => {
    expect(union([])).toBeNull();
    expect(
      union([
        { x: 0, y: 0, w: 1, h: 1 },
        { x: -2, y: 3, w: 1, h: 1 },
      ]),
    ).toEqual({ x: -2, y: 0, w: 3, h: 4 });
  });
});

describe("fitWith: the drawing and its names, never zoomed out to nothing", () => {
  const box = { x: -100, y: -100, w: 200, h: 200 };
  // A name of `px` screen pixels to the right of the drawing: at scale k it reaches px/k.
  const name = (px: number) => (k: number) => [{ x: 100, y: -5, w: px / k, h: 10 / k }];

  it("a short name: zoomed out just enough for it, and it is in view", () => {
    const base = fit(box, 300, 300, 20);
    const v = fitWith(box, name(40), 300, 300, 20);
    expect(v.k).toBeLessThan(base.k);
    expect(v.k).toBeGreaterThanOrEqual(base.k * FIT_FLOOR);
    // The name's far end, on screen.
    expect((100 + 40 / v.k - v.cx) * v.k + 150).toBeLessThanOrEqual(300);
  });

  it("a name wider than the box: the plain fit, not a drawing zoomed out to almost nothing", () => {
    const base = fit(box, 300, 300, 20);
    // 600 px of name in a 300 px box: there is no scale at which both fit.
    expect(fitWith(box, name(600), 300, 300, 20)).toEqual(base);
    // Nor anything between: whatever the names, never below the floor.
    fc.assert(
      fc.property(real(0, 5000), real(100, 2000), (px, sw) => {
        const b = fit(box, sw, sw, 20);
        return fitWith(box, name(px), sw, sw, 20).k >= b.k * FIT_FLOOR - 1e-9;
      }),
      params,
    );
  });

  it("a name that would halve the graph: the plain fit (the floor is 3/4, not lower)", () => {
    // 260 px of room, a 200-unit drawing: with 130 px of name beside it, both fit at
    // k = 0.65, half of the plain 1.3. Above the floor it would have been taken.
    const base = fit(box, 300, 300, 20);
    expect(fitWith(box, name(130), 300, 300, 20)).toEqual(base);
    expect(fitWith(box, name(130), 300, 300, 20, 0.4).k).toBeCloseTo(base.k / 2, 2);
  });

  it("no names: the plain fit", () => {
    expect(fitWith(box, () => [], 300, 300, 20)).toEqual(fit(box, 300, 300, 20));
  });
});

// Fitting with nothing under the controls (owner, 2026-11-13: in the sidebar,
// after «fit», part of the balloons sat behind the + and − buttons).
describe("freeAreas and fit with controls: nothing framed under an overlay control", () => {
  // The sidebar box as measured: 250 px square, a 20 px margin, the zoom column
  // in the bottom-right corner and the expand button in the top-right one.
  const S = 250;
  const zoom = { x: 220, y: 160, w: 26, h: 86 };
  const expand = { x: 218, y: 4, w: 28, h: 30 };
  const grow = (b: Box, d: number): Box => ({
    x: b.x - d,
    y: b.y - d,
    w: b.w + 2 * d,
    h: b.h + 2 * d,
  });
  const inside = (a: Box, b: Box, eps = 1e-6) =>
    a.x >= b.x - eps &&
    a.y >= b.y - eps &&
    a.x + a.w <= b.x + b.w + eps &&
    a.y + a.h <= b.y + b.h + eps;

  it("no control: the screen inside its margin", () => {
    expect(freeAreas(S, S, [], 20)).toEqual([{ x: 20, y: 20, w: 210, h: 210 }]);
  });

  it("a control in the corner: the room beside it and the room above it, CONTROL_GAP px clear", () => {
    const areas = freeAreas(S, S, [zoom], 20);
    const g = CONTROL_GAP;
    expect(areas).toContainEqual({ x: 20, y: 20, w: zoom.x - g - 20, h: 210 });
    expect(areas).toContainEqual({ x: 20, y: 20, w: 210, h: zoom.y - g - 20 });
    expect(areas).toHaveLength(2);
    // With the expand button above it, on the same side: the room to the left of
    // both, and between them, the full width.
    const both = freeAreas(S, S, [zoom, expand], 20);
    expect(both).toContainEqual({ x: 20, y: 20, w: expand.x - g - 20, h: 210 });
    expect(both).toContainEqual({
      x: 20,
      y: expand.y + expand.h + g,
      w: 210,
      h: zoom.y - g - (expand.y + expand.h + g),
    });
  });

  it("a control within the margin takes nothing (the dialog's controls, outside its 48 px)", () => {
    expect(freeAreas(800, 800, [{ x: 770, y: 680, w: 26, h: 86 }], 48)).toEqual([
      { x: 48, y: 48, w: 704, h: 704 },
    ]);
  });

  it("controls that leave no room: the whole screen inside its margin (a drawing under them, not none)", () => {
    expect(freeAreas(100, 100, [{ x: 0, y: 0, w: 100, h: 100 }], 10)).toEqual([
      { x: 10, y: 10, w: 80, h: 80 },
    ]);
  });

  const controlArb = (sw: number, sh: number) =>
    fc.record({
      x: real(-20, sw),
      y: real(-20, sh),
      w: real(1, sw / 4),
      h: real(1, sh / 4),
    });
  it("property: every free area is inside the margin, clear of every control, and maximal", () => {
    fc.assert(
      fc.property(fc.array(controlArb(300, 300), { maxLength: 4 }), real(0, 40), (cs, pad) => {
        const areas = freeAreas(300, 300, cs, pad);
        const all = { x: pad, y: pad, w: 300 - 2 * pad, h: 300 - 2 * pad };
        const obs = cs.map((c) => grow(c, CONTROL_GAP)).filter((c) => overlaps(c, all));
        expect(areas.length).toBeGreaterThan(0);
        for (const a of areas) {
          expect(inside(a, all)).toBe(true);
          // (1e-9 px: a side can sit exactly on a control's, give or take rounding)
          const shrunk = { x: a.x + 1e-9, y: a.y + 1e-9, w: a.w - 2e-9, h: a.h - 2e-9 };
          expect(obs.some((o) => overlaps(o, shrunk))).toBe(false);
          // Maximal: no side can move out by a pixel without leaving the
          // margin or meeting a control.
          for (const bigger of [
            { ...a, x: a.x - 1, w: a.w + 1 },
            { ...a, w: a.w + 1 },
            { ...a, y: a.y - 1, h: a.h + 1 },
            { ...a, h: a.h + 1 },
          ])
            expect(!inside(bigger, all) || obs.some((o) => overlaps(o, bigger))).toBe(true);
        }
      }),
      params,
    );
  });

  it("fit picks the room that leaves the drawing largest: a round graph beside the zoom, a wide one above it", () => {
    const round = fit({ x: -100, y: -100, w: 200, h: 200 }, S, S, 20, 0, Infinity, [zoom]);
    expect(round.k).toBeCloseTo((zoom.x - CONTROL_GAP - 20) / 200, 12);
    const wide = fit({ x: -200, y: -50, w: 400, h: 100 }, S, S, 20, 0, Infinity, [zoom]);
    expect(wide.k).toBeCloseTo(210 / 400, 12);
  });

  it("fit centres the drawing in its room, not on the screen", () => {
    const box = { x: -100, y: -100, w: 200, h: 200 };
    const v = fit(box, S, S, 20, 0, Infinity, [zoom]);
    const on = boxOnScreen(box, v, S, S);
    const room = { x: 20, w: zoom.x - CONTROL_GAP - 20 };
    expect(on.x + on.w / 2).toBeCloseTo(room.x + room.w / 2, 9);
    expect(on.y + on.h / 2).toBeCloseTo(S / 2, 9);
    expect(overlaps(on, grow(zoom, CONTROL_GAP))).toBe(false);
  });

  it("property: what fit frames overlaps no control, whenever the controls leave room", () => {
    fc.assert(
      fc.property(
        fc.record({ x: real(-1e3, 1e3), y: real(-1e3, 1e3), w: real(1, 1e3), h: real(1, 1e3) }),
        fc.array(controlArb(300, 200), { maxLength: 4 }),
        (b, cs) => {
          const v = fit(b, 300, 200, 20, 0, Infinity, cs);
          const on = boxOnScreen(b, v, 300, 200);
          const areas = freeAreas(300, 200, cs, 20);
          // The one fallback: controls that cover everything.
          // (1e-9 px: a side can sit exactly on a control's, give or take rounding)
          const room = areas.filter((a) =>
            cs.every(
              (c) =>
                !overlaps(grow(c, CONTROL_GAP), {
                  x: a.x + 1e-9,
                  y: a.y + 1e-9,
                  w: a.w - 2e-9,
                  h: a.h - 2e-9,
                }),
            ),
          );
          if (room.length === 0) return;
          expect(room.some((a) => inside(on, a, 1e-6))).toBe(true);
        },
      ),
      params,
    );
  });
});

describe("fitClear: a control takes room only where something drawn would be under it", () => {
  const S = 250;
  // A ring of 24 small parts, radius 100: the corners of its box are empty.
  const ring: Box[] = Array.from({ length: 24 }, (_, i) => {
    const a = (i / 24) * 2 * Math.PI;
    return { x: 100 * Math.cos(a) - 4, y: 100 * Math.sin(a) - 4, w: 8, h: 8 };
  });
  const box = union(ring)!;
  const corner = { x: 226, y: 4, w: 20, h: 20 }; // over the empty corner of the ring's box
  const side = { x: 220, y: 110, w: 26, h: 30 }; // over the ring's right edge
  const clearOf = (v: View, cs: Box[]) =>
    ring.every((p) =>
      cs.every(
        (c) =>
          // (1e-6 px: a part can sit exactly at the gap, give or take rounding)
          !overlaps(boxOnScreen(p, v, S, S), {
            x: c.x - CONTROL_GAP + 1e-6,
            y: c.y - CONTROL_GAP + 1e-6,
            w: c.w + 2 * CONTROL_GAP - 2e-6,
            h: c.h + 2 * CONTROL_GAP - 2e-6,
          }),
      ),
    );

  it("a control over an empty corner: the plain fit, the ring as large as before", () => {
    expect(fitClear(box, S, S, 20, [corner], ring)).toEqual(fit(box, S, S, 20));
  });

  it("a control over the ring: moved and shrunk away from it", () => {
    expect(clearOf(fit(box, S, S, 20), [side])).toBe(false); // it was under it
    const v = fitClear(box, S, S, 20, [corner, side], ring);
    expect(clearOf(v, [corner, side])).toBe(true);
    expect(v.k).toBeLessThan(fit(box, S, S, 20).k);
  });

  it("without parts, every control counts: the whole box out of each", () => {
    expect(fitClear(box, S, S, 20, [corner], null)).toEqual(
      fit(box, S, S, 20, 0, Infinity, [corner]),
    );
  });

  it("property: no part under any control, whenever the controls leave room", () => {
    fc.assert(
      fc.property(
        fc.array(fc.record({ x: real(-10, S), y: real(-10, S), w: real(4, 60), h: real(4, 60) }), {
          maxLength: 4,
        }),
        (cs) => {
          const areas = freeAreas(S, S, cs, 20);
          const room = areas.filter(
            (a) =>
              !cs.some((c) =>
                overlaps(
                  {
                    x: c.x - CONTROL_GAP,
                    y: c.y - CONTROL_GAP,
                    w: c.w + 2 * CONTROL_GAP,
                    h: c.h + 2 * CONTROL_GAP,
                  },
                  a,
                ),
              ),
          );
          if (room.length === 0) return;
          expect(clearOf(fitClear(box, S, S, 20, cs, ring), cs)).toBe(true);
        },
      ),
      params,
    );
  });

  it("fitWith keeps the names off the controls too", () => {
    // A name of 20 px to the right of the ring, at its middle height: with it, the
    // drawing reaches the right margin, where `side` is.
    const name = (k: number) => [{ x: 104, y: -6 / k, w: 20 / k, h: 12 / k }];
    const plain = fitWith(box, name, S, S, 20);
    const bare = boxOnScreen(name(plain.k)[0]!, plain, S, S);
    expect(overlaps(bare, { x: side.x - 4, y: side.y - 4, w: side.w + 8, h: side.h + 8 })).toBe(
      true,
    );
    const v = fitWith(box, name, S, S, 20, FIT_FLOOR, { controls: [side], parts: ring });
    expect(v.k).toBeGreaterThanOrEqual(plain.k * FIT_FLOOR); // not the floor's fallback
    // At its final scale, within 0.1 px of where the last pass measured it.
    const on = boxOnScreen(name(v.k)[0]!, v, S, S);
    const g = CONTROL_GAP - 0.1;
    expect(
      overlaps(on, { x: side.x - g, y: side.y - g, w: side.w + 2 * g, h: side.h + 2 * g }),
    ).toBe(false);
  });
});

// + and − about the current note if it is in view, else about the centre (owner,
// 2026-11-16). In view: inside the frame, edges included, as the ring of
// «You are here» (browser/here.ts: outside is x < 0, y < 0, x > w or y > h).
describe("zoomAnchor: what + and − zoom about", () => {
  const v: View = { cx: 100, cy: 50, k: 2 };
  const [sw, sh] = [400, 300];
  // The drawing point that lands on screen point (x, y).
  const at = (x: number, y: number): [number, number] => [
    v.cx + (x - sw / 2) / v.k,
    v.cy + (y - sh / 2) / v.k,
  ];

  it("a note in view: its own place on screen", () => {
    expect(zoomAnchor(at(37, 251), v, sw, sh)).toEqual([37, 251]);
  });

  it("on the frame's edge counts as in view (the marker still rings it)", () => {
    for (const [x, y] of [
      [0, 0],
      [sw, sh],
      [0, sh],
      [sw, 120],
    ] as const) {
      const [px, py] = zoomAnchor(at(x, y), v, sw, sh);
      expect(px).toBeCloseTo(x, 9);
      expect(py).toBeCloseTo(y, 9);
    }
  });

  it("a note out of view, on any side, or none at all: the centre", () => {
    for (const [x, y] of [
      [-0.5, 150],
      [sw + 0.5, 150],
      [200, -0.5],
      [200, sh + 0.5],
      [-50, -50],
    ] as const)
      expect(zoomAnchor(at(x, y), v, sw, sh)).toEqual([sw / 2, sh / 2]);
    expect(zoomAnchor(null, v, sw, sh)).toEqual([sw / 2, sh / 2]);
    expect(zoomAnchor(undefined, v, sw, sh)).toEqual([sw / 2, sh / 2]);
  });

  it("zooming about it leaves a note in view where it was, and the centre where it was otherwise", () => {
    fc.assert(
      fc.property(viewArb, finite(-2e4, 2e4), finite(-2e4, 2e4), logReal(0.2, 5), (v0, x, y, f) => {
        const [px, py] = zoomAnchor([x, y], v0, sw, sh);
        const v1 = zoomAt(v0, f, px, py, sw, sh);
        const [sx0, sy0] = toScreen(v0, x, y, sw, sh);
        const [sx1, sy1] = toScreen(v1, x, y, sw, sh);
        const seen = sx0! >= 0 && sy0! >= 0 && sx0! <= sw && sy0! <= sh;
        if (seen) {
          expect(Math.hypot(sx1! - sx0!, sy1! - sy0!)).toBeLessThan(1e-6 * (1 + Math.abs(sx0!)));
        } else {
          expect(v1.cx).toBeCloseTo(v0.cx, 6);
          expect(v1.cy).toBeCloseTo(v0.cy, 6);
        }
      }),
      params,
    );
  });
});
