// SPDX-License-Identifier: GPL-3.0-or-later
// WCAG AA in every palette and every mode. The pairs come from where the page uses
// each role:
// - foreground (body), muted (secondary text: dates, tags, listings), link and accent
//   (emphasised text) are text colours;
// - they sit on the page (background) and on panels (panel: inline code, group
//   headers);
// - marked text sits on mark, a translucent veil over the page;
// - background is the text on accent fills (link);
// - the gallery title, on its scrim (#000000bf over a white image).
// And the chord graph's (graph test/themes.test.ts):
// - ink, accent and neighbour, >= 3:1 on the page (non-text contrast);
// - visited, >= 3:1 on ink and neighbour (the band sits on those wedges);
// - ink, accent and neighbour apart (OKLab ΔE ×100 >= 15), and accent and neighbour
//   inside the validator's chroma and lightness bands.
// And the diagrams': the ink is text (4.5:1 on the background) and the border a
// graphic (3:1, WCAG 1.4.11). In the maps, the ink sits on the nodes and the zones;
// blocked is both an edge and text (4.5:1 on the background, the nodes and the zones);
// lines, outlines, highlight and blocked, 3:1 on everything they can cross or surround:
// the background, the nodes and the zones.
import { readFileSync } from "node:fs"
import path from "node:path"
import { contrast, deltaE, deltaE00, deltaE00Lab, lab, oklab, opaque, parse, on, type Rgb } from "../src/color.ts"
import { tokensJson } from "../src/generate.ts"
import { GROUPS, MODES, loadPalettes, type Palette } from "../src/tokens.ts"

const palettes = loadPalettes(path.join(import.meta.dirname, "..", "palettes"))
const TEXT = ["foreground", "muted", "link", "accent"] as const
const AA = 4.5
const NOT_TEXT = 3
/** The least for any palette's code to be legible; scripts/doom.mjs uses the same. */
const LEGIBLE = 3

// How many contrast checks passed for the default palette: if they were skipped (or never
// ran), this says so (afterAll, below).
let defaultContrasts = 0
afterAll(() => {
  // 11 per mode: text, mark, accent fill, gallery, diagrams, three for the maps, visited,
  // the graph on the page, and code.
  expect(defaultContrasts).toBe(11 * MODES.length)
})

function check(pairs: [string, number, number][]) {
  const bad = pairs.filter(([, r, min]) => !(r >= min)).map(([q, r, min]) => `${q}: ${r.toFixed(2)} < ${min}`)
  expect(bad).toEqual([])
}

describe.each(palettes.map((p) => [p.name, p] as [string, Palette]))("palette %s", (_n, palette) => {
  describe.each(MODES)("mode %s", (mode) => {
    const b = palette[mode].ui
    // The contrasts (4.5:1 and 3:1) are required of the default palette, the accessible
    // one; the others are the reader's choice (owner, 2026-11-13). What makes the site
    // work (opaque colours, zones and nodes that are distinguishable, the graph's bands),
    // of all.
    const aa = (name: string, f: () => void) =>
      palette.isDefault
        ? it(name, () => {
            f()
            defaultContrasts++
          })
        : it.skip(name, f)

    it("the text and surface colours are opaque", () => {
      for (const role of [...TEXT, "background", "panel"] as const) expect(() => opaque(b[role]), role).not.toThrow()
    })

    // The links, of all palettes (2026-11-17): they are text, and choosing a palette gives no other link colour.
    it("the links reach 4.5:1 on background and panel, in every palette", () => {
      check(
        (["background", "panel"] as const).map(
          (sup) => [`link on ${sup}`, contrast(opaque(b.link), opaque(b[sup])), AA] as [string, number, number],
        ),
      )
    })

    aa("the text reaches 4.5:1 on background and panel", () => {
      check(
        (["background", "panel"] as const).flatMap((sup) =>
          TEXT.map((t) => [`${t} on ${sup}`, contrast(opaque(b[t]), opaque(b[sup])), AA] as [string, number, number]),
        ),
      )
    })

    aa("marked text (foreground on mark) reaches 4.5:1", () => {
      check([["foreground on mark", contrast(opaque(b.foreground), on(b.mark, b.background)), AA]])
    })

    aa("text on accent fills (background on link) reaches 4.5:1", () => {
      check([["background on link", contrast(opaque(b.background), opaque(b.link)), AA]])
    })

    aa("the gallery title reaches 4.5:1 on its scrim", () => {
      const veil = on("#000000bf", "#ffffff")
      check([["gallery.title", contrast(opaque(palette[mode].gallery.title), veil), AA]])
    })

    aa("diagrams: the ink reaches 4.5:1 and the border 3:1 on the background", () => {
      const d = palette[mode].diagram
      check([
        ["ink on background", contrast(opaque(d.ink), opaque(d.background)), AA],
        ["border on background", contrast(opaque(d.border), opaque(d.background)), NOT_TEXT],
      ])
    })

    const d = palette[mode].diagram
    const NODES = ["node", "node-soft", "node-control"] as const
    const ZONES = GROUPS.diagram.filter((r) => r.startsWith("zone-"))
    const overAll = (color: keyof typeof d, surfaces: readonly (keyof typeof d)[], min: number) =>
      surfaces.map((s) => [`${color} on ${s}`, contrast(opaque(d[color]), opaque(d[s])), min] as [string, number, number])

    it("maps: there are seven zones, and nodes and zones are opaque", () => {
      expect(ZONES).toHaveLength(7)
      for (const r of [...NODES, ...ZONES]) expect(() => opaque(d[r]), r).not.toThrow()
    })

    aa("maps: the ink reaches 4.5:1 on every node and every zone", () => {
      check(overAll("ink", [...NODES, ...ZONES], AA))
    })

    aa("maps: blocked, as text, reaches 4.5:1 on the background, every node and every zone", () => {
      check(overAll("blocked", ["background", ...NODES, ...ZONES], AA))
    })

    aa("maps: line, outline, highlight and blocked reach 3:1 on the background, every node and every zone", () => {
      check(
        (["line", "outline", "highlight", "blocked"] as const).flatMap((r) => overAll(r, ["background", ...NODES, ...ZONES], NOT_TEXT)),
      )
    })

    it("maps: every node is distinguishable from every zone (ΔE >= 3) and every zone from the others (ΔE >= 2)", () => {
      const near: string[] = []
      for (const n of NODES)
        for (const z of ZONES)
          if (deltaE(d[n], d[z]) < NODE_SEPARATION)
            near.push(`${n}/${z}: ${deltaE(d[n], d[z]).toFixed(2)}`)
      for (const [i, a] of ZONES.entries())
        for (const b of ZONES.slice(i + 1))
          if (deltaE(d[a], d[b]) < ZONE_SEPARATION && `${a}/${b}` !== "zone-amber/zone-orange")
            near.push(`${a}/${b}: ${deltaE(d[a], d[b]).toFixed(2)}`)
      expect(near).toEqual([])
    })

    const g = palette[mode].graph
    // On the palette's page.
    aa(`graph: ink, accent and neighbour reach 3:1 on the background (${b.background})`, () => {
      check(
        (["ink", "accent", "neighbour"] as const).map(
          (r) => [`${r} on background`, contrast(opaque(g[r]), opaque(b.background)), NOT_TEXT] as [string, number, number],
        ),
      )
    })

    aa("graph: the visited band reaches 3:1 on ink and neighbour", () => {
      check([
        ["visited on ink", contrast(opaque(g.visited), opaque(g.ink)), NOT_TEXT],
        ["visited on neighbour", contrast(opaque(g.visited), opaque(g.neighbour)), NOT_TEXT],
      ])
    })

    it("graph: ink, accent and neighbour are distinguishable (ΔE >= 15)", () => {
      const d = deltaE
      check([
        ["ink/accent", d(g.ink, g.accent), 15],
        ["ink/neighbour", d(g.ink, g.neighbour), 15],
        ["accent/neighbour", d(g.accent, g.neighbour), 15],
      ])
    })

    // The code blocks: each role is text on the block's background.
    const cod = palette[mode].code
    const CODE_ROLES = GROUPS.code.filter((r) => r !== "background")
    it("code: the text, the background and every role are opaque", () => {
      for (const r of GROUPS.code) expect(() => opaque(cod[r]), r).not.toThrow()
    })
    aa("code: the text and every role reach 4.5:1 on the block's background", () => {
      check(CODE_ROLES.map((r) => [`${r} on background`, contrast(opaque(cod[r]), opaque(cod.background)), AA] as [string, number, number]))
    })
    // Of all, even if not the default: legible (3:1, WCAG's minimum for large text). The
    // comments of many dark themes do not reach it as they are: the Doom generator moves
    // them away from the background just enough (scripts/doom.mjs).
    it("code: the text and every role reach 3:1 on the block's background (legible)", () => {
      check(CODE_ROLES.map((r) => [`${r} on background`, contrast(opaque(cod[r]), opaque(cod.background)), LEGIBLE] as [string, number, number]))
    })

    it("graph: accent and neighbour have their band's chroma and lightness", () => {
      const [lo, hi] = mode === "light" ? [0.43, 0.77] : [0.48, 0.67]
      for (const r of ["accent", "neighbour"] as const) {
        const [L, a, bb] = oklab(g[r])
        expect(Math.hypot(a, bb), `${r} chroma`).toBeGreaterThanOrEqual(0.1)
        expect(L, `${r} L`).toBeGreaterThanOrEqual(lo)
        expect(L, `${r} L`).toBeLessThanOrEqual(hi)
      }
    })
  })
})

// Distance between fills, in OKLab ×100: around 2 is the smallest difference noticeable
// between two colours side by side. The zones are neighbouring areas, each with its
// outline: 2 is enough for them. A node sits inside a zone and its fill is what
// separates it (a box3d without it gets lost): 3, with a margin. Amber and orange are
// exempt from each other: in the maps neutral's light values come from, they are 1.5
// apart.
const NODE_SEPARATION = 3
const ZONE_SEPARATION = 2

// The org renderer swaps each house colour in the maps for its variable: it looks up
// the default palette's light value. Two roles with the same value could not be told
// apart. ink, background and border are not part of that swap: they may coincide with a map
// role (node is #ffffff, like background).
describe("maps: the roles looked up by value", () => {
  const pred = palettes.find((p) => p.isDefault)!
  const BY_VALUE = GROUPS.diagram.filter((r) => !["ink", "background", "border"].includes(r))

  it("they are the maps': nodes, lines, highlight, blocked and seven zones", () => {
    expect(BY_VALUE).toHaveLength(14)
    expect(pred.name).toBe("neutral")
  })

  it("in the default palette, in light mode, their values are pairwise distinct", () => {
    const d = pred.light.diagram
    const repeated = BY_VALUE.flatMap((a, i) =>
      BY_VALUE.slice(i + 1).flatMap((b) => (d[a] === d[b] ? [`${a} and ${b}: ${d[a]}`] : [])),
    )
    expect(repeated).toEqual([])
  })
})


// The formulas, against known values: if one breaks, every test above could pass by
// mistake.
describe("the formulas", () => {
  it("WCAG 2 contrast: black on white 21:1, a colour with itself 1:1, #767676 on white 4.54:1", () => {
    expect(contrast([0, 0, 0], [255, 255, 255])).toBeCloseTo(21, 5)
    expect(contrast([18, 52, 86], [18, 52, 86])).toBe(1)
    expect(contrast(opaque("#767676"), opaque("#ffffff"))).toBeCloseTo(4.54, 2)
  })
  it("compositing: a 50 % black veil on white is #808080; the alpha channel is respected", () => {
    expect(on("#00000080", "#ffffff")).toEqual([127, 127, 127] satisfies Rgb)
    expect(on("#000000bf", "#ffffff")).toEqual([64, 64, 64])
    expect(on("#123456", "#ffffff")).toEqual([0x12, 0x34, 0x56])
    expect(parse("#0b5cad14").alpha).toBeCloseTo(0x14 / 255, 10)
  })
  it("CIEDE2000 (ΔE00, the emphasis one): Sharma, Wu and Dalal's (2005) pairs, to four decimals", () => {
    // [L, a, b] of each colour and the published table's distance; those with hues 180°
    // apart test the jumps of the mean hue.
    const pairs: [number[], number[], number][] = [
      [[50, 2.6772, -79.7751], [50, 0, -82.7485], 2.0425],
      [[50, 0, 0], [50, -1, 2], 2.3669],
      [[50, 2.49, -0.001], [50, -2.49, 0.0009], 7.1792],
      [[50, 2.49, -0.001], [50, -2.49, 0.0011], 7.2195],
      [[50, -0.001, 2.49], [50, 0.001, -2.49], 4.8045],
      [[50, -0.001, 2.49], [50, 0.0011, -2.49], 4.7461],
      [[50, 2.5, 0], [56, -27, -3], 31.903],
      [[50, 2.5, 0], [58, 24, 15], 19.4535],
      [[60.2574, -34.0099, 36.2677], [60.4626, -34.1751, 39.4387], 1.2644],
      [[22.7233, 20.0904, -46.694], [23.0331, 14.973, -42.5619], 2.0373],
      [[2.0776, 0.0795, -1.135], [0.9033, -0.0636, -0.5514], 0.9082],
    ]
    for (const [a, b, d] of pairs) expect(deltaE00Lab(a, b).toFixed(4), `${a} / ${b}`).toBe(d.toFixed(4))
    // And from sRGB: D65 CIELAB (white L=100, black L=0).
    const [Lb, ab, bb] = lab("#ffffff")
    expect([Lb, Math.abs(ab), Math.abs(bb)].map((x) => Math.round(x * 100) / 100)).toEqual([100, 0, 0])
    expect(deltaE00("#ffffff", "#000000")).toBeCloseTo(100, 4)
  })

  it("OKLab: white is L=1 with no chroma", () => {
    const [L, a, b] = oklab("#ffffff")
    expect(L).toBeCloseTo(1, 4)
    expect(Math.hypot(a, b)).toBeLessThan(1e-4)
  })
  it("only lowercase #rrggbb and #rrggbbaa", () => {
    for (const bad of ["#FFF", "#fff", "#FFFFFF", "#12345", "red", "#1234567", "#gggggg"])
      expect(() => parse(bad), bad).toThrow()
  })
})

// What is checked here is what is published: dist/tokens.json is these palettes' (also
// required by test/generate.test.ts).
it("dist/tokens.json is the checked palettes'", () => {

  expect(readFileSync(path.join(import.meta.dirname, "..", "dist", "tokens.json"), "utf8")).toBe(tokensJson(palettes))
})
