// SPDX-License-Identifier: GPL-3.0-or-later
/**
 * Colour themes for the chord graph. Colour is assigned by ROLE, not by folder:
 * one muted ink for every note, one accent for the current note and its chords,
 * a second hue for the current note's neighbours. Folders are told apart by the
 * gaps, arcs and labels on the ring. (An earlier graph export arrived at this after
 * its seven gruvbox hues failed as a categorical palette.)
 *
 * What each role must meet, and test/themes.test.ts checks:
 * - ink, accent and neighbour: >= 3:1 against the theme's own page surface,
 *   and apart from each other (OKLab ΔE >= 15); accent and neighbour within the validator's
 *   chroma and lightness bands.
 * - visited: the band is drawn on a wedge, never on the page, and never on the
 *   current note, so >= 3:1 against ink and neighbour.
 * - edge: chords at rest are recessive context, deliberately below 3:1; the
 *   chords that carry meaning (the current and hovered notes') are drawn in
 *   accent and ink.
 * ink, accent and neighbour were also run through a palette validator
 * (all pairs) for colour-blind separation;
 * those results are recorded per theme below. The ink is a deliberate neutral,
 * so the validator's lightness band and chroma floor do not apply to it.
 */

export interface Colors {
  /** Every note at rest. */
  ink: string;
  /** The current note and its chords. */
  accent: string;
  /** The current note's neighbours. */
  neighbour: string;
  /** The inner band that marks a visited note. */
  visited: string;
  /** Chords at rest. */
  edge: string;
}

export interface Theme {
  light: Colors;
  dark: Colors;
  /** The page surfaces this theme was validated against. */
  surface: { light: string; dark: string };
}

export const THEMES = {
  /**
   * Gruvbox (the default), as in an earlier graph export, with three values refined
   * to pass the validator. Measured 2026-10-30:
   * - light neighbour: gruvbox blue #458588 reads grey (OKLCH C 0.066 < 0.10), so
   *   #2f7aa8, the same family with enough chroma;
   * - dark accent/neighbour: bright_orange #fe8019 and bright_blue #83a598 sit above
   *   the dark lightness band (L 0.731 / 0.693 > 0.67) and the blue reads grey
   *   (C 0.042), so #e07114 and #389ec1: gruvbox's hues, L 0.665 / 0.655.
   * - visited: bg2 #d5c4a1 / #504945 fall below 3:1 on the neighbour blue
   *   (2.74 / 2.87), so light1 #ebdbb2 (3.42) and bg0_s #32302f (4.27).
   * Validator results (ink, accent, neighbour): light worst CVD ΔE 13.8, normal
   * 23.6; dark worst CVD ΔE 20.7, normal 26.0.
   */
  gruvbox: {
    light: {
      ink: "#3c3836",
      accent: "#d65d0e",
      neighbour: "#2f7aa8",
      visited: "#ebdbb2",
      edge: "#928374",
    },
    dark: {
      ink: "#ebdbb2",
      accent: "#e07114",
      neighbour: "#389ec1",
      visited: "#32302f",
      edge: "#928374",
    },
    surface: { light: "#fbf1c7", dark: "#282828" },
  },
} satisfies Record<string, Theme>;

export type ThemeName = keyof typeof THEMES;

export const ROLES = ["ink", "accent", "neighbour", "visited", "edge"] as const;
const MODES = ["light", "dark"] as const;

/** A CSS hex colour: 3, 4, 6 or 8 digits. (5 or 7 digits is no colour at all.) */
const HEX = /^#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i;

/**
 * A theme's colours with a site's overrides applied on top, role by role. Only
 * the known roles are read, and an override that is not a hex colour keeps the
 * theme's value: dropping the role instead would leave its CSS variable unset,
 * and an unset fill draws black, invisible on a dark page.
 */
export function resolveColors(
  theme: ThemeName,
  overrides: { light?: Partial<Colors>; dark?: Partial<Colors> } = {},
): { light: Colors; dark: Colors } {
  // Own keys only: "constructor" or "__proto__" must not resolve to a theme.
  const base: Theme = Object.hasOwn(THEMES, theme) ? THEMES[theme] : THEMES.gruvbox;
  const resolve = (mode: (typeof MODES)[number]): Colors => {
    const colors = { ...base[mode] };
    for (const role of ROLES) {
      const value: unknown = overrides?.[mode]?.[role];
      if (typeof value === "string" && HEX.test(value)) colors[role] = value;
    }
    return colors;
  };
  return { light: resolve("light"), dark: resolve("dark") };
}

/**
 * The CSS custom properties the stylesheet reads, for both modes. Names come
 * from the fixed role list and values must be hex colours, so a site config
 * cannot write anything else into the style attribute.
 */
export function cssVariables(colors: { light: Colors; dark: Colors }): string {
  const parts: string[] = [];
  for (const mode of MODES) {
    for (const role of ROLES) {
      const value = colors[mode][role];
      if (HEX.test(value)) parts.push(`--chord-${role}-${mode}:${value}`);
    }
  }
  return parts.join(";");
}
