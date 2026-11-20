// SPDX-License-Identifier: GPL-3.0-or-later
import type { Colors, ThemeName } from "./themes";
import type { IndexEntry } from "./geometry";

/**
 * One page of the site, as the graph reads it: its title and links (IndexEntry),
 * and its text, which the expanded view's search looks in.
 */
export interface GraphEntry extends IndexEntry {
  content?: string;
}

/** The site's pages, keyed by slug: everything the graph draws. */
export type GraphData = Record<string, GraphEntry>;

export interface ChordGraphOptions {
  /**
   * The view a page opens on: "chord" (one ring of notes, the default),
   * "sunburst" (the folder tree as concentric rings, links bundled through the
   * centre) or "balloon" (a chord ring per folder, laid out as a tree). The
   * hierarchical views never redraw as the reader moves: they zoom and pan.
   */
  layout?: "chord" | "sunburst" | "balloon";
  /**
   * The views the in-page switch offers. Default: only `layout`, so no switch
   * and no change for a site that upgrades; list two or more to offer it, e.g.
   * ["chord", "sunburst", "balloon"]. The default `layout` is always among them.
   */
  layouts?: Array<"chord" | "sunburst" | "balloon">;
  /**
   * Sunburst only: "top-inside" (top-level folders innermost, each level
   * further out) or "leaves-inside" (every note on the innermost ring, next to
   * the links, as in Holten's radial edge bundling; top level outermost).
   */
  orientation?: "top-inside" | "leaves-inside";
  /**
   * Sunburst only: the orientations the reader can flip between with a button
   * on the drawing. Default: only `orientation`, so no button; list both,
   * ["top-inside", "leaves-inside"], to offer it. Needs the sunburst among the
   * views; the reader's choice is remembered like the view.
   */
  orientations?: Array<"top-inside" | "leaves-inside">;
  /**
   * The current page's «You are here» marker: "pin" (the default: a
   * pin on the note, its title in a label above or below it, or in the nearest
   * empty space with a leader line) or "none" (no marker; the page is named like
   * the others). "callout" and "radial" were removed: mounting throws an error.
   */
  hereMarker?: "pin" | "none";
  /**
   * A switch in the expanded view, beside «Names», with which the reader shows or
   * hides the pin (shown at first); the choice is remembered like the view. Default
   * false: no switch. Not with hereMarker "none". Replaces `hereMarkers`, which was
   * removed: setting it makes mounting throw an error.
   */
  hereToggle?: boolean;
  /**
   * Leave notes off the ring whose slug starts with any of these prefixes,
   * e.g. ["tags/"] for tag listing pages.
   */
  excludePrefixes?: string[];
  /** Pull of each chord toward the centre: 0 straight, 1 through the centre. */
  chordPull?: number;
  /** Remember visited notes in localStorage and mark them on the ring. */
  markVisited?: boolean;
  /** Empty slots between folders on the ring, so each folder reads as an arc. */
  groupGap?: number;
  /**
   * From this many notes on, chords that do not touch the current or hovered
   * note fade almost out, so that note's own links stand out.
   */
  denseFrom?: number;
  /**
   * Colour theme: "gruvbox" (the default, and for now the only one; gruvbox refined
   * for accessibility). See src/themes.ts.
   */
  theme?: ThemeName;
  /**
   * Per-role overrides on top of the theme, for light and dark mode: ink (every
   * note), accent (the current note and its chords), neighbour (its neighbours),
   * visited (the visited band), edge (chords at rest). Hex colours only;
   * anything else is ignored. Validate a custom palette before using it.
   */
  colors?: { light?: Partial<Colors>; dark?: Partial<Colors> };
}

export const defaultOptions: Required<ChordGraphOptions> = {
  layout: "chord",
  // Empty: only `layout` (no switch). Not ["chord"]: merged over a site's
  // options, that offered a Chord button to a site that only set `layout`.
  layouts: [],
  orientation: "top-inside",
  // Empty for the same reason as `layouts`: no button unless asked.
  orientations: [],
  hereMarker: "pin",
  // No switch unless asked, as `layouts`: a site that upgrades sees no change.
  hereToggle: false,
  excludePrefixes: ["tags/"],
  chordPull: 0.55,
  markVisited: true,
  groupGap: 2,
  denseFrom: 150,
  theme: "gruvbox",
  colors: {},
};
