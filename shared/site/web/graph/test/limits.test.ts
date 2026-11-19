// SPDX-License-Identifier: GPL-3.0-or-later
// The zoom's limits from what is drawn (scene.ts, zoomRange; owner, 2026-11-16):
// up to the scale at which the smallest leaf's name reads whole, times
// LEGIBLE_MARGIN (and at least twice the fitted scale); down to the whole graph
// fitted with a margin (FIT_MARGIN).
//
// Names have a fixed size on screen (11 px: hier.ts, LABEL_PX); what
// grows on zooming in is their room. «Legible» means the name fits whole in its room
// (hier.ts shows it then, by the same rule: legibleAt); at the top, the smallest
// leaf's has 1.5 times the room it needs.
import { describe, expect, it } from "vitest";
import {
  FIT_MARGIN,
  LABEL_SLACK_PX,
  LEGIBLE_MARGIN,
  LINE_SLACK_PX,
  MIN_ZOOM_IN,
  buildScene,
  legibleAt,
  smallestLeaf,
  zoomRange,
  type Item,
  type Label,
  type Layout,
} from "../src/scene";
import { fit } from "../src/viewport";
import { SHAPES } from "./kbs/shapes";

const LINE = 11;
// A deterministic text width: 6.2 px per character (hier.ts, CHAR_PX).
const textPx = (t: string) => t.length * 6.2;
const label = (room: number, height: number): Label => ({
  x: 0,
  y: 0,
  rotate: 0,
  anchor: "middle",
  room,
  height,
});
const item = (
  key: string,
  shape: Item["shape"],
  title: string,
  room: number,
  height: number,
): Item => ({
  key,
  shape,
  title,
  d: "",
  box: { x: 0, y: 0, w: 1, h: 1 },
  label: label(room, height),
  callout: { angle: 0, radius: 0, from: 0, around: [0, 0] },
  ...(shape === "note" ? { page: key } : { folder: key }),
});
// Does it read whole at scale k? (What hier.ts checks before showing a name.)
const fits = (i: Item, k: number) =>
  i.label!.room * k >= textPx(i.title) + LABEL_SLACK_PX &&
  i.label!.height * k >= LINE + LINE_SLACK_PX;
// How many times the room it needs it has, in its tightest dimension, at scale k.
const roomy = (i: Item, k: number) =>
  Math.min(
    (i.label!.room * k) / (textPx(i.title) + LABEL_SLACK_PX),
    (i.label!.height * k) / (LINE + LINE_SLACK_PX),
  );

describe("the smallest leaf", () => {
  it("is the note with the least room in its tightest dimension, wherever it sits", () => {
    const items = [
      item("a", "note", "A long title that needs a lot of room", 40, 40),
      item("f", "folder", "F", 1, 1), // a folder is not a leaf
      item("p", "port", "P", 1, 1),
      item("b", "note", "B", 30, 6), // the tightest: 6, across
      item("c", "note", "C", 9, 50),
    ];
    expect(smallestLeaf(items)?.key).toBe("b");
  });

  it("in a real layout: no other note has less room", () => {
    for (const layout of ["sunburst", "balloon"] as Layout[]) {
      const scene = buildScene(SHAPES.inventory!(), layout);
      const leaf = smallestLeaf(scene.items)!;
      const ext = (i: Item) => Math.min(i.label!.room, i.label!.height);
      const notes = scene.items.filter((i) => i.shape === "note" && i.page && i.label);
      expect(notes.length).toBeGreaterThan(100);
      for (const i of notes) expect(ext(leaf)).toBeLessThanOrEqual(ext(i));
    }
  });
});

describe("zoomRange: the zoom's limits from what is drawn", () => {
  it("at the top, the smallest leaf's name has 1.5 times the room it needs", () => {
    const items = [item("b", "note", "Budget 2026", 4, 20), item("c", "note", "C", 50, 50)];
    const [, kMax] = zoomRange(items, 1, textPx, LINE);
    const leaf = items[0]!;
    expect(roomy(leaf, kMax)).toBeCloseTo(LEGIBLE_MARGIN, 9);
    // Legible from kMax / 1.5 on, not before.
    expect(fits(leaf, (kMax / LEGIBLE_MARGIN) * (1 + 1e-9))).toBe(true);
    expect(fits(leaf, (kMax / LEGIBLE_MARGIN) * 0.99)).toBe(false);
    expect(LEGIBLE_MARGIN).toBe(1.5);
  });

  it("the line across can be what binds (a thin balloon wedge)", () => {
    const items = [item("t", "note", "T", 30, 2)];
    const [, kMax] = zoomRange(items, 1, textPx, LINE);
    expect(kMax).toBeCloseTo((LEGIBLE_MARGIN * (LINE + LINE_SLACK_PX)) / 2, 9);
  });

  it("a tiny graph, legible already when fitted, still zooms to twice the fit", () => {
    const items = [item("a", "note", "A", 500, 500)];
    const fitK = 3;
    expect(legibleAt(items[0]!.label!, textPx("A"), LINE)).toBeLessThan(fitK);
    expect(zoomRange(items, fitK, textPx, LINE)).toEqual([FIT_MARGIN * fitK, MIN_ZOOM_IN * fitK]);
    expect(MIN_ZOOM_IN).toBe(2);
    // No note at all (only folders): the same.
    expect(zoomRange([item("f", "folder", "F", 1, 1)], fitK, textPx, LINE)).toEqual([
      FIT_MARGIN * fitK,
      MIN_ZOOM_IN * fitK,
    ]);
  });

  it("at the bottom, the whole graph with a margin: 0.8 of the fitted size", () => {
    for (const layout of ["sunburst", "balloon"] as Layout[]) {
      const scene = buildScene(SHAPES.manual!(), layout);
      const [sw, sh, pad] = [900, 900, 48];
      const fitK = fit(scene.bounds, sw, sh, pad).k;
      const [kMin, kMax] = zoomRange(scene.items, fitK, textPx, LINE);
      const span = Math.max(scene.bounds.w, scene.bounds.h);
      // Fitted, the graph spans the frame less its padding; at the bottom, 80% of that.
      expect(span * fitK).toBeCloseTo(sw - 2 * pad, 6);
      expect((span * kMin) / (sw - 2 * pad)).toBeCloseTo(0.8, 9);
      expect(kMax).toBeGreaterThan(MIN_ZOOM_IN * fitK);
    }
  });

  it("on a large site the smallest name is what sets the top, not a fixed multiple", () => {
    for (const layout of ["sunburst", "balloon"] as Layout[]) {
      const scene = buildScene(SHAPES.inventory!(), layout);
      const fitK = fit(scene.bounds, 900, 900, 48).k;
      const [, kMax] = zoomRange(scene.items, fitK, textPx, LINE);
      const leaf = smallestLeaf(scene.items)!;
      expect(kMax).toBeGreaterThan(MIN_ZOOM_IN * fitK);
      expect(roomy(leaf, kMax)).toBeCloseTo(LEGIBLE_MARGIN, 9);
      expect(fits(leaf, kMax)).toBe(true);
    }
  });
});
