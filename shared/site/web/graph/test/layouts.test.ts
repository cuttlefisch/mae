// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { chooseLayout, chooseOrientation, resolveLayouts } from "../src/layouts";
import { defaultOptions } from "../src/types";

describe("resolveLayouts", () => {
  it("defaults: the chord ring alone (no switch: a site that upgrades sees no change), top level inside", () => {
    expect(resolveLayouts({})).toEqual({
      layout: "chord",
      offered: ["chord"],
      orientation: "top-inside",
      orientations: ["top-inside"],
    });
    expect(resolveLayouts({ layout: "sunburst" }).offered).toEqual(["sunburst"]);
  });

  it("keeps the site's choices, in a fixed order", () => {
    expect(
      resolveLayouts({
        layout: "balloon",
        layouts: ["balloon", "sunburst"],
        orientation: "leaves-inside",
      }),
    ).toEqual({
      layout: "balloon",
      offered: ["sunburst", "balloon"],
      orientation: "leaves-inside",
      orientations: ["leaves-inside"],
    });
  });

  it("always offers the default, so the switch can return to it", () => {
    expect(resolveLayouts({ layout: "sunburst", layouts: ["chord"] }).offered).toEqual([
      "chord",
      "sunburst",
    ]);
  });

  it("falls back on anything it does not know", () => {
    const r = resolveLayouts({
      layout: "Sunburst",
      layouts: ["globos", 3, null],
      orientation: "inside",
    });
    expect(r).toEqual({
      layout: "chord",
      offered: ["chord"],
      orientation: "top-inside",
      orientations: ["top-inside"],
    });
    expect(resolveLayouts({ layouts: "sunburst" }).offered).toEqual(["chord"]);
  });

  it("one view only: no switch to offer", () => {
    expect(resolveLayouts({ layout: "balloon", layouts: [] }).offered).toEqual(["balloon"]);
  });
});

describe("resolveLayouts over the merged defaults (as mount.ts calls it)", () => {
  it("a site that only sets layout gets no switch", () => {
    expect(resolveLayouts({ ...defaultOptions, layout: "sunburst" }).offered).toEqual(["sunburst"]);
    expect(resolveLayouts({ ...defaultOptions }).offered).toEqual(["chord"]);
  });
  it("a site that lists views gets them", () => {
    expect(
      resolveLayouts({ ...defaultOptions, layouts: ["chord", "sunburst", "balloon"] }).offered,
    ).toEqual(["chord", "sunburst", "balloon"]);
  });
});

describe("chooseLayout", () => {
  const r = resolveLayouts({ layout: "sunburst", layouts: ["chord", "sunburst"] });
  it("the reader's choice when the site offers it", () => {
    expect(chooseLayout("chord", r)).toBe("chord");
  });
  it("the site's default otherwise: nothing stored, not offered, or garbage", () => {
    expect(chooseLayout(null, r)).toBe("sunburst");
    expect(chooseLayout("balloon", r)).toBe("sunburst");
    expect(chooseLayout("{}", r)).toBe("sunburst");
  });
});

describe("the sunburst flip (orientations)", () => {
  const both = ["top-inside", "leaves-inside"];
  it("not asked: no flip, even with the sunburst offered (a site that upgrades sees no change)", () => {
    expect(
      resolveLayouts({ ...defaultOptions, layouts: ["chord", "sunburst"] }).orientations,
    ).toEqual(["top-inside"]);
  });
  it("asked: both, in a fixed order, the default always among them", () => {
    expect(
      resolveLayouts({ layout: "sunburst", orientations: ["leaves-inside", "top-inside"] })
        .orientations,
    ).toEqual(both);
    // Listing only the other one still offers the default: the flip can come back.
    expect(
      resolveLayouts({ layout: "sunburst", orientations: ["leaves-inside"] }).orientations,
    ).toEqual(both);
  });
  it("no sunburst on offer: no flip (it would be a button that never shows)", () => {
    expect(
      resolveLayouts({ layout: "balloon", layouts: ["chord"], orientations: both }).orientations,
    ).toEqual(["top-inside"]);
  });
  it("garbage falls back to the default alone", () => {
    expect(
      resolveLayouts({ layout: "sunburst", orientations: ["outside", 1] }).orientations,
    ).toEqual(["top-inside"]);
    expect(
      resolveLayouts({ layout: "sunburst", orientations: "leaves-inside" }).orientations,
    ).toEqual(["top-inside"]);
  });
});

describe("chooseOrientation", () => {
  const flip = resolveLayouts({
    layout: "sunburst",
    orientations: ["top-inside", "leaves-inside"],
  });
  const fixed = resolveLayouts({ layout: "sunburst", orientation: "leaves-inside" });
  it("the reader's flip when the site offers it", () => {
    expect(chooseOrientation("leaves-inside", flip)).toBe("leaves-inside");
    expect(chooseOrientation("top-inside", flip)).toBe("top-inside");
  });
  it("the site's otherwise: nothing stored, flip withdrawn, or garbage", () => {
    expect(chooseOrientation(null, flip)).toBe("top-inside");
    expect(chooseOrientation("top-inside", fixed)).toBe("leaves-inside");
    expect(chooseOrientation("sideways", flip)).toBe("top-inside");
  });
});
