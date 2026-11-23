// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import fc from "fast-check";
import {
  GAP,
  PIN_H,
  busy,
  distTo,
  makeGrid,
  nearestFree,
  nearestPoint,
  placePin,
  HERE_FLOOR,
  HERE_NARROW,
  hereFloor,
  reserve,
  resolveHere,
  showHere,
  truncate,
  type Grid,
  type PlaceArgs,
  type Placement,
} from "../src/here";
import { defaultOptions } from "../src/types";
import { FIT_FLOOR, type Box } from "../src/viewport";
import { real } from "./arbitraries";

const CELL = 3;
/** A grid of w x h px with the given boxes (px) taken. */
function grid(
  w: number,
  h: number,
  taken: Box[] = [],
  discs: Array<[number, number, number]> = [],
) {
  const cols = Math.ceil(w / CELL);
  const rows = Math.ceil(h / CELL);
  const occ = new Uint8Array(cols * rows);
  for (let r = 0; r < rows; r++)
    for (let c = 0; c < cols; c++) {
      const x0 = c * CELL;
      const y0 = r * CELL;
      const hit =
        taken.some((b) => x0 < b.x + b.w && b.x < x0 + CELL && y0 < b.y + b.h && b.y < y0 + CELL) ||
        discs.some(([cx, cy, rr]) => Math.hypot(x0 + CELL / 2 - cx, y0 + CELL / 2 - cy) <= rr);
      occ[r * cols + c] = hit ? 1 : 0;
    }
  return makeGrid(w, h, CELL, occ);
}
const overlaps = (a: Box, b: Box) =>
  a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
const grown = (b: Box, m: number): Box => ({
  x: b.x - m,
  y: b.y - m,
  w: b.w + 2 * m,
  h: b.h + 2 * m,
});
const inside = (g: Grid, b: Box) =>
  b.x >= GAP - 1e-9 &&
  b.y >= GAP - 1e-9 &&
  b.x + b.w <= g.w - GAP + 1e-9 &&
  b.y + b.h <= g.h - GAP + 1e-9;
/** A placement's label is clear: in the frame, nothing taken under it or its margin. */
const clear = (g: Grid, p: Placement) => inside(g, p.box) && busy(g, grown(p.box, GAP)) === 0;

// The ring the views draw: a disc in the middle of a square frame, the current
// note on its edge (right 310,200; top 200,90; bottom 200,310; left 90,200).
const W = 400;
const ring = (extra: Box[] = []) => grid(W, W, extra, [[200, 200, 110]]);
const SIZE: [number, number] = [120, 34];
const at = (g: Grid, anchor: [number, number], size = SIZE): PlaceArgs => ({
  grid: g,
  anchor,
  size,
  halo: 9,
});

// The owner's decision (2026-11-16): the pin is the marker; the reader may show or
// hide it, but if it is shown it is the pin. Callout and radial are gone.
describe("resolveHere, over the merged defaults (as mount.ts calls it)", () => {
  const as = (opts: Record<string, unknown>) => resolveHere({ ...defaultOptions, ...opts });
  it("no options: the pin, and no switch", () => {
    expect(as({})).toEqual({ shown: true, toggle: false });
    expect(resolveHere({})).toEqual({ shown: true, toggle: false });
  });
  it('"pin" still accepted; "none" hides it, with no switch', () => {
    expect(as({ hereMarker: "pin" })).toEqual({ shown: true, toggle: false });
    expect(as({ hereMarker: "none" })).toEqual({ shown: false, toggle: false });
  });
  it("hereToggle: true offers the switch", () => {
    expect(as({ hereToggle: true })).toEqual({ shown: true, toggle: true });
    expect(as({ hereMarker: "pin", hereToggle: true })).toEqual({ shown: true, toggle: true });
  });
  it("the removed markers are refused, saying what to write instead", () => {
    for (const m of ["callout", "radial"])
      expect(() => as({ hereMarker: m })).toThrow(
        new RegExp(`hereMarker "${m}" was removed.*"pin".*"none"`),
      );
  });
  it("the removed hereMarkers is refused, pointing to hereToggle", () => {
    for (const list of [["pin", "callout", "radial"], ["pin"], []])
      expect(() => as({ hereMarkers: list })).toThrow(/hereMarkers was removed.*hereToggle: true/);
  });
  it("anything else unknown is an error too, not a silent default", () => {
    expect(() => as({ hereMarker: "Pin" })).toThrow(/must be "pin" or "none", not "Pin"/);
    expect(() => as({ hereMarker: true })).toThrow(/must be "pin" or "none"/);
    expect(() => as({ hereToggle: "yes" })).toThrow(/hereToggle must be true or false/);
    expect(() => as({ hereMarker: "none", hereToggle: true })).toThrow(/nothing to toggle/);
  });
});

describe("showHere: is the pin drawn (as the page script asks)", () => {
  const toggle = { shown: true, toggle: true };
  it("in the dialog, shown until the reader hides it; garbage or nothing stored is shown", () => {
    expect(showHere(null, toggle, true)).toBe(true);
    expect(showHere("shown", toggle, true)).toBe(true);
    expect(showHere("{}", toggle, true)).toBe(true);
    expect(showHere("hidden", toggle, true)).toBe(false);
  });
  it("in the sidebar, the site's, whatever the reader chose (like «Names»)", () => {
    expect(showHere("hidden", toggle, false)).toBe(true);
    expect(showHere(null, { shown: false, toggle: false }, false)).toBe(false);
  });
  it("without the switch, the site's: a hidden choice from before is not kept", () => {
    expect(showHere("hidden", { shown: true, toggle: false }, true)).toBe(true);
    expect(showHere("shown", { shown: false, toggle: false }, true)).toBe(false);
    expect(showHere(null, { shown: false, toggle: false }, true)).toBe(false);
  });
  it("nothing from the mount (an older page): the pin", () => {
    expect(showHere("hidden", undefined, true)).toBe(true);
    expect(showHere(null, {}, false)).toBe(true);
  });
});

describe("the occupancy grid", () => {
  it("counts the taken cells under any box, as a cell-by-cell count does", () => {
    // Half the time an edge or a size falls exactly on a cell boundary (a multiple of 3): a box that
    // only touches a cell does not take it.
    const onBoundary = (lo: number, hi: number) =>
      fc.oneof(
        real(lo, hi),
        fc.integer({ min: Math.ceil(lo / 3), max: Math.floor(hi / 3) }).map((k) => k * 3),
      );
    fc.assert(
      fc.property(
        fc.array(fc.boolean(), { minLength: 12 * 9, maxLength: 12 * 9 }),
        onBoundary(-10, 40),
        onBoundary(-10, 30),
        onBoundary(0, 40),
        onBoundary(0, 30),
        (cells, x, y, w, h) => {
          const g = makeGrid(36, 27, 3, cells.map(Number));
          let n = 0;
          for (let r = 0; r < 9; r++)
            for (let c = 0; c < 12; c++)
              if (cells[r * 12 + c] && overlaps({ x: c * 3, y: r * 3, w: 3, h: 3 }, { x, y, w, h }))
                n++;
          expect(busy(g, { x, y, w, h })).toBe(n);
        },
      ),
    );
  });
  it("counts a cell that the box only grazes by a hair, as the exact overlap does (CI, 2026-11-23)", () => {
    // Found by a fresh CI seed: the box's right edge, x + w = 5e-324, is past the cell's left edge (0),
    // but (x + w) / 3 underflows to 0 and the grid's last column came out before the first. The same
    // rounding can misplace any edge that lies within a rounding step of a cell boundary.
    const cells = new Array<number>(12 * 9).fill(0);
    cells[8 * 12] = 1; // row 8 (y 24 to 27), column 0 (x 0 to 3)
    const g = makeGrid(36, 27, 3, cells);
    const b = { x: -5e-324, y: 24.000000000931323, w: 1e-323, h: 0 };
    expect(overlaps({ x: 0, y: 24, w: 3, h: 3 }, b)).toBe(true);
    expect(busy(g, b)).toBe(1);
    // And at an ordinary size: a right edge a rounding step past a cell boundary.
    const edge = { x: 0, y: 0, w: 24 + 24 * Number.EPSILON, h: 3 };
    const row = new Array<number>(12 * 9).fill(0);
    row[8] = 1; // row 0, column 8 (x 24 to 27)
    expect(overlaps({ x: 24, y: 0, w: 3, h: 3 }, edge)).toBe(true);
    expect(busy(makeGrid(36, 27, 3, row), edge)).toBe(1);
  });
  it("a box that only touches a cell's edge does not take it", () => {
    const row = new Array<number>(12 * 9).fill(0);
    row[0] = 1; // row 0, column 0 (x 0 to 3)
    row[2] = 1; // row 0, column 2 (x 6 to 9)
    const g = makeGrid(36, 27, 3, row);
    expect(busy(g, { x: 3, y: 0, w: 3, h: 3 })).toBe(0);
    expect(busy(g, { x: 3, y: 0, w: 3.5, h: 3 })).toBe(1);
    expect(busy(g, { x: 2.5, y: 0, w: 3.5, h: 3 })).toBe(1);
  });
  it("with a fractional cell, an edge on a cell boundary still only touches it, though the division rounds", () => {
    // Cell 0.1: 3 * 0.1 / 0.1 rounds to just over 3 (so ceil says 4), and 43 * 0.1 / 0.1 to just
    // under 43 (so floor says 42); the products put the edge exactly on the boundary.
    const cell = 0.1;
    const end = 3 * cell;
    const start = 43 * cell;
    expect(Math.ceil(end / cell)).toBe(4);
    expect(Math.floor(start / cell)).toBe(42);
    const one = (c: number) => {
      const cells = new Array<number>(45).fill(0);
      cells[c] = 1;
      return makeGrid(4.5, cell, cell, cells);
    };
    expect(busy(one(3), { x: 0, y: 0, w: end, h: cell })).toBe(0);
    expect(busy(one(2), { x: 0, y: 0, w: end, h: cell })).toBe(1);
    expect(busy(one(42), { x: start, y: 0, w: 0.05, h: cell })).toBe(0);
    expect(busy(one(43), { x: start, y: 0, w: 0.05, h: cell })).toBe(1);
  });
  it("geometry helpers: distance to and nearest point of a box", () => {
    const b = { x: 10, y: 10, w: 10, h: 10 };
    expect(distTo(b, [15, 15])).toBe(0);
    expect(distTo(b, [23, 24])).toBe(5);
    expect(nearestPoint(b, [0, 15])).toEqual([10, 15]);
  });
});

describe("pin: on the note, the label over its head or under it", () => {
  it("above, centred over the pin, no leader", () => {
    const g = ring();
    // (The ring's top is at y 90; a note at 120 has the ring above it.)
    expect(placePin(at(g, [200, 120], [60, 20])).where).not.toBe("above");
    const top = placePin(at(g, [200, 90], [60, 20]));
    expect(top.where).toBe("above");
    expect(top.pin).toBe("up");
    expect(top.leader).toEqual([]);
    expect(top.box.y + top.box.h).toBeCloseTo(90 - PIN_H - 2);
    expect(top.box.x + top.box.w / 2).toBeCloseTo(200);
  });
  it("below, the pin upside down, when above is taken", () => {
    const g = ring();
    const p = placePin(at(g, [200, 310], [60, 20]));
    expect(p.where).toBe("below");
    expect(p.pin).toBe("down");
    expect(p.box.y).toBeCloseTo(310 + PIN_H + 2);
  });
  it("neither: the nearest free space to the pin's head, with a leader from it", () => {
    const g = ring();
    const p = placePin(at(g, [310, 200], [60, 20]));
    expect(p.where).toBe("nearest");
    expect(p.pin).toBe("up");
    expect(p.leader[0]).toEqual([310, 200 - PIN_H]);
    expect(distTo(p.box, p.leader[1]!)).toBeCloseTo(0);
    expect(clear(g, p)).toBe(true);
  });
  it("never over a control: above is taken by one, so it goes elsewhere", () => {
    const control = { x: 170, y: 30, w: 60, h: 24 };
    const g = ring([control]);
    const p = placePin(at(ring([control]), [200, 90], [60, 14]));
    expect(placePin(at(ring(), [200, 90], [60, 14])).where).toBe("above");
    expect(p.where).toBe("nearest");
    expect(overlaps(p.box, control)).toBe(false);
    expect(clear(g, p)).toBe(true);
  });
  it("no place free near it: the nearest free space, else the least taken", () => {
    // Everything taken but a window away from the note.
    const win = { x: 250, y: 120, w: 80, h: 40 };
    const g = grid(W, W, [
      { x: 0, y: 0, w: W, h: win.y },
      { x: 0, y: win.y + win.h, w: W, h: W },
      { x: 0, y: 0, w: win.x, h: W },
      { x: win.x + win.w, y: 0, w: W, h: W },
    ]);
    const p = placePin(at(g, [200, 200], [60, 20]));
    expect(p.where).toBe("nearest");
    expect(clear(g, p)).toBe(true);
    expect(overlaps(p.box, win)).toBe(true);
    const full = grid(W, W, [{ x: 0, y: 0, w: W, h: W }]);
    expect(placePin(at(full, [150, 150])).where).toBe("least");
    // Even then, never under a control (the owner's rule: nothing under the controls).
    const controls = [{ x: 0, y: 0, w: W, h: 300 }];
    const least = placePin({ ...at(full, [150, 150], [60, 20]), controls });
    expect(least.where).toBe("least");
    expect(overlaps(least.box, controls[0]!)).toBe(false);
  });
});

describe("the pin, any drawing", () => {
  it("a label is clear of everything drawn and of the controls whenever any place is", () => {
    const boxArb = fc.record({
      x: real(0, 280),
      y: real(0, 280),
      w: real(5, 120),
      h: real(5, 120),
    });
    fc.assert(
      fc.property(
        fc.array(boxArb, { maxLength: 6 }),
        real(20, 280),
        real(20, 280),
        (taken, ax, ay) => {
          const g = grid(W, W, taken);
          const a = at(g, [ax, ay], [60, 20]);
          const p = placePin(a);
          if (p.where === "least") {
            // Only when nothing is free anywhere in the frame.
            expect(nearestFree(a, a.anchor, 0).where).toBe("least");
            return;
          }
          expect(clear(g, p)).toBe(true);
          for (const t of taken) expect(overlaps(t, p.box)).toBe(false);
          // Off the marker's ring: on the pin's head, or a free place beyond the ring.
          if (p.where === "nearest")
            expect(distTo(p.box, a.anchor)).toBeGreaterThanOrEqual(a.halo + GAP - 1e-9);
        },
      ),
    );
  });
});

describe("reserve: the room framing leaves beside the note", () => {
  const within = (inner: Box, outer: Box) =>
    inner.x >= outer.x - 1e-9 &&
    inner.y >= outer.y - 1e-9 &&
    inner.x + inner.w <= outer.x + outer.w + 1e-9 &&
    inner.y + inner.h <= outer.y + outer.h + 1e-9;
  // At scale 1 a drawing unit is a px: what place() puts on an empty frame must lie
  // in what reserve() asked for, margins included (or framing leaves too little).
  it("holds the label where the pin puts it, with its margins", () => {
    const g = ring();
    for (const anchor of [
      [200, 90],
      [200, 310],
    ] as Array<[number, number]>) {
      const p = placePin(at(g, anchor, [60, 20]));
      const r = reserve(anchor, [200, 200], [60, 20], 1);
      expect(within(grown(p.box, GAP), r), anchor.join()).toBe(true);
    }
  });
  it("in drawing units: half the size at twice the scale", () => {
    const one = reserve([0, -50], [0, 0], [60, 20], 1);
    const two = reserve([0, -50], [0, 0], [60, 20], 2);
    expect(two.w).toBeCloseTo(one.w / 2);
    expect(two.h).toBeCloseTo(one.h / 2);
    expect(one.y + one.h).toBeCloseTo(-50 + 2 * GAP);
  });
  it("a note at its ring's centre (a folder's page): above it", () => {
    const r = reserve([0, 0], [0, 0], [60, 20], 1);
    expect(r.y + r.h).toBeCloseTo(2 * GAP);
    expect(r.y).toBeCloseTo(-(PIN_H + 2 + 20) - 2 * GAP);
  });
});

describe("hereFloor", () => {
  it("the sidebar's graph gives up more for the label, but not in a narrow frame", () => {
    expect(hereFloor(255)).toBe(HERE_FLOOR);
    expect(hereFloor(HERE_NARROW)).toBe(HERE_FLOOR);
    expect(hereFloor(168)).toBe(FIT_FLOOR);
    expect(HERE_FLOOR).toBeLessThan(FIT_FLOOR);
  });
});

describe("nearestFree", () => {
  it("the free place nearest the point, no nearer than asked", () => {
    const g = grid(W, W);
    const a = at(g, [100, 100], [40, 20]);
    const n = nearestFree(a, [200, 200], 30);
    expect(n.where).toBe("nearest");
    expect(distTo(n.box, [200, 200])).toBeGreaterThanOrEqual(30);
    expect(distTo(n.box, [200, 200])).toBeLessThan(34);
  });
});

describe("truncate", () => {
  const px = (s: string) => s.length * 6;
  it("leaves a title that fits", () => {
    expect(truncate("  Plan  ", 100, px)).toBe("Plan");
  });
  it("cuts a long one to the width, with an ellipsis, at a word when it can", () => {
    const t = truncate("Procedimiento de alta de servidores físicos", 120, px);
    expect(t.endsWith("…")).toBe(true);
    expect(px(t)).toBeLessThanOrEqual(120);
    expect(t).toBe("Procedimiento de…");
    // One long word: cut inside it.
    const w = truncate("Supercalifragilisticoespialidoso", 60, px);
    expect(px(w)).toBeLessThanOrEqual(60);
    expect(w).toBe("Supercali…");
    // A space early on: cutting there would lose most of what fits.
    expect(truncate("Ab cdefghijklmnopq", 72, px)).toBe("Ab cdefghij…");
  });
});
