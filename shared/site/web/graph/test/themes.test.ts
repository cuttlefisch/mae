// SPDX-License-Identifier: GPL-3.0-or-later
// Pins what can be recomputed in CI from each theme (src/themes.ts says which
// role must meet what): ink, accent and neighbour clear 3:1 against the page
// (WCAG non-text contrast) on the theme's own surface, and stay apart under
// normal vision (OKLab ΔE ×100 >= 15, the palette validator's floor); the visited band clears 3:1 on every wedge it is drawn
// on. The colour-blind checks were run with the validator itself; their results
// are recorded in src/themes.ts.
import { describe, it, expect } from "vitest";
import fc from "fast-check";
import { THEMES, ROLES, resolveColors, cssVariables } from "../src/themes";

const lin = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const rgb = (hex: string) =>
  [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255) as [number, number, number];
const luminance = (hex: string) => {
  const [r, g, b] = rgb(hex).map(lin) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
const contrast = (a: string, b: string) => {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p) as [number, number];
  return (x + 0.05) / (y + 0.05);
};
const oklab = (hex: string) => {
  const [r, g, b] = rgb(hex).map(lin) as [number, number, number];
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ];
};
const deltaE = (a: string, b: string) => {
  const [p, q] = [oklab(a), oklab(b)];
  return 100 * Math.hypot(p[0]! - q[0]!, p[1]! - q[1]!, p[2]! - q[2]!);
};

describe.each(Object.entries(THEMES))("theme %s", (_name, theme) => {
  for (const mode of ["light", "dark"] as const) {
    const c = theme[mode];
    const surface = theme.surface[mode];
    it(`${mode} on ${surface}: ink, accent and neighbour >= 3:1`, () => {
      for (const role of ["ink", "accent", "neighbour"] as const) {
        expect(contrast(c[role], surface), role).toBeGreaterThanOrEqual(3);
      }
    });
    it(`${mode}: the three roles stay apart (ΔE >= 15)`, () => {
      expect(deltaE(c.ink, c.accent)).toBeGreaterThanOrEqual(15);
      expect(deltaE(c.ink, c.neighbour)).toBeGreaterThanOrEqual(15);
      expect(deltaE(c.accent, c.neighbour)).toBeGreaterThanOrEqual(15);
    });
    it(`${mode}: accent and neighbour have the chroma and lightness of their band (neither grey nor glaring)`, () => {
      // The palette validator's thresholds: OKLCH chroma >= 0.10; L within
      // [0.43, 0.77] in light mode and [0.48, 0.67] in dark. These are what
      // forced the gruvbox refinements (see src/themes.ts).
      const [lo, hi] = mode === "light" ? [0.43, 0.77] : [0.48, 0.67];
      for (const role of ["accent", "neighbour"] as const) {
        const [L, a, b] = oklab(c[role]) as [number, number, number];
        expect(Math.hypot(a, b), `${role} chroma`).toBeGreaterThanOrEqual(0.1);
        expect(L, `${role} L`).toBeGreaterThanOrEqual(lo);
        expect(L, `${role} L`).toBeLessThanOrEqual(hi);
      }
    });
    it(`${mode}: the visited band shows on every wedge it is drawn on (>= 3:1)`, () => {
      // Visited notes are drawn in ink or, next to the current note, in the
      // neighbour colour; the current note itself never carries the band.
      expect(contrast(c.visited, c.ink), "ink").toBeGreaterThanOrEqual(3);
      expect(contrast(c.visited, c.neighbour), "neighbour").toBeGreaterThanOrEqual(3);
    });
  }
});

describe("resolveColors and cssVariables", () => {
  it("gruvbox is the default theme and an unknown name falls back to it", () => {
    expect(resolveColors("gruvbox")).toEqual({
      light: THEMES.gruvbox.light,
      dark: THEMES.gruvbox.dark,
    });
    expect(resolveColors("nope" as never)).toEqual({
      light: THEMES.gruvbox.light,
      dark: THEMES.gruvbox.dark,
    });
  });
  it("applies a site's overrides role by role", () => {
    const c = resolveColors("gruvbox", { light: { accent: "#123456" } });
    expect(c.light.accent).toBe("#123456");
    expect(c.light.ink).toBe(THEMES.gruvbox.light.ink);
    expect(c.dark).toEqual(THEMES.gruvbox.dark);
  });
  it("an override that is not a hex colour keeps the theme's value", () => {
    // Dropping the role would leave its variable unset: an unset fill is black.
    for (const bad of ["red;background:url(x)", "#12345", "#1234567", "red", 123, null]) {
      const c = resolveColors("gruvbox", { dark: { ink: bad as never } });
      expect(c.dark.ink, String(bad)).toBe(THEMES.gruvbox.dark.ink);
    }
    expect(resolveColors("gruvbox", { light: { accent: "#ABC" } }).light.accent).toBe("#ABC");
  });
  it("only the known roles are read: a key carrying CSS never reaches the style attribute", () => {
    const css = cssVariables(
      resolveColors("gruvbox", { light: { "x:y;background:url(z)": "#fff" } as never }),
    );
    expect(css).not.toContain("url(");
    expect(css).not.toContain("x:y");
    // And cssVariables holds the line on its own, whatever it is handed.
    const handed = { ...THEMES.gruvbox.light, "x:y;background:url(z)": "#fff" };
    const own = cssVariables({ light: handed, dark: THEMES.gruvbox.dark });
    expect(own).not.toContain("x:y");
  });
  it("theme names are looked up as own keys only", () => {
    for (const name of ["constructor", "__proto__", "toString"]) {
      expect(resolveColors(name as never), name).toEqual({
        light: THEMES.gruvbox.light,
        dark: THEMES.gruvbox.dark,
      });
    }
  });
  it("whatever a site configures, the output is the ten variables, each a hex colour", () => {
    const expected = ROLES.flatMap((r) => [`--chord-${r}-light`, `--chord-${r}-dark`]).sort();
    const anyValue = fc.oneof(
      fc.string(),
      fc.constantFrom("#fff", "#12345", "#abcdef", "#abcdef12", ";}", "url(x)"),
      fc.integer(),
      fc.constant(null),
    );
    const roles = fc.dictionary(fc.oneof(fc.constantFrom(...ROLES), fc.string()), anyValue);
    fc.assert(
      fc.property(
        fc.oneof(fc.constantFrom("gruvbox"), fc.string()),
        fc.record({ light: roles, dark: roles }, { requiredKeys: [] }),
        (theme, overrides) => {
          const parts = cssVariables(resolveColors(theme as never, overrides as never)).split(";");
          const names = parts.map((p) => p.slice(0, p.indexOf(":"))).sort();
          expect(names).toEqual(expected);
          for (const p of parts) {
            expect(p.slice(p.indexOf(":") + 1)).toMatch(
              /^#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i,
            );
          }
        },
      ),
    );
  });
});
