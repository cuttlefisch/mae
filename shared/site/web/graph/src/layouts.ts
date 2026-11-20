// SPDX-License-Identifier: GPL-3.0-or-later
// Which views a site offers, which one a page opens on, and which way the
// sunburst runs. Pure: no DOM. Anything a site's configuration gets wrong falls
// back to something that draws (a typo must not leave an empty box).
import type { Orientation } from "./sunburst";

export const LAYOUTS = ["chord", "sunburst", "balloon"] as const;
export type LayoutName = (typeof LAYOUTS)[number];
export const ORIENTATIONS = ["top-inside", "leaves-inside"] as const;

export const isLayout = (x: unknown): x is LayoutName =>
  typeof x === "string" && (LAYOUTS as readonly string[]).includes(x);

export interface ResolvedLayouts {
  /** The site's default view. */
  layout: LayoutName;
  /** The views the in-page switch offers, in LAYOUTS order; the default always among them. */
  offered: LayoutName[];
  /** The sunburst's orientation by default. */
  orientation: Orientation;
  /**
   * The orientations the reader can flip between, in ORIENTATIONS order; the
   * default always among them. Two only when the site asks for the flip button
   * and offers the sunburst at all.
   */
  orientations: Orientation[];
}

export function resolveLayouts(opts: {
  layout?: unknown;
  layouts?: unknown;
  orientation?: unknown;
  orientations?: unknown;
}): ResolvedLayouts {
  const layout: LayoutName = isLayout(opts.layout) ? opts.layout : "chord";
  // Not given (or not a list): only the default, so no switch appears unasked.
  const asked = Array.isArray(opts.layouts) ? opts.layouts.filter(isLayout) : [];
  // The default is always offered: otherwise the switch could not get back to it.
  const set = new Set<LayoutName>([...asked, layout]);
  const offered = LAYOUTS.filter((l) => set.has(l));
  const orientation: Orientation = (ORIENTATIONS as readonly unknown[]).includes(opts.orientation)
    ? (opts.orientation as Orientation)
    : "top-inside";
  // Same rule as the views: not asked, no button; the default always offered.
  // Anything unknown drops out in the filter over ORIENTATIONS below.
  const askedO: unknown[] = Array.isArray(opts.orientations) ? opts.orientations : [];
  const setO = new Set<unknown>([...askedO, orientation]);
  // A flip for a view the site does not offer would be a button that never shows.
  const orientations = offered.includes("sunburst")
    ? ORIENTATIONS.filter((o) => setO.has(o))
    : [orientation];
  return { layout, offered, orientation, orientations };
}

/** The view to draw: the reader's choice if the site still offers it, else the site's default. */
export function chooseLayout(stored: string | null, r: ResolvedLayouts): LayoutName {
  return isLayout(stored) && r.offered.includes(stored) ? stored : r.layout;
}

/** The sunburst's orientation: the reader's flip if the site still offers it, else the site's. */
export function chooseOrientation(stored: string | null, r: ResolvedLayouts): Orientation {
  const o = r.orientations.find((x) => x === stored);
  return o ?? r.orientation;
}
