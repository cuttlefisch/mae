// SPDX-License-Identifier: GPL-3.0-or-later
// Emphasis (owner, 2026-11-16): the swatches of each Doom palette and of gruvbox and
// monokai (hand-written, with their Doom themes' swatches), each one's default, what
// follows the emphasis in palette.css and what does not, and the four palettes that were
// removed.
//
// The rules are checked on what the generator produces NOW (scripts/doom.mjs with
// data/doom.json, in memory), not only on palettes/: a generator change that breaks a
// rule fails here because of the rule, not only because palettes/ is no longer its
// output. That palettes/ and dist/ are that output is required by test/doom.test.ts and
// test/generate.test.ts.
import { existsSync, readFileSync } from "node:fs"
import path from "node:path"
import {
  EMPHASIS,
  ROOT,
  candidatesByHand,
  compose,
  swatchDistance,
  emphasisByHand,
  generate as generateDoom,
  readData,
  palette as doomPalette,
  partByHand,
  defaultId,
  separate,
} from "../scripts/doom.mjs"
import { contrast, deltaE, deltaE00, oklab, opaque } from "../src/color.ts"
import { css, pinnedEmphasis, tokensJson, variablesCss } from "../src/generate.ts"
import { MODES, checkSet, resolveText, type Emphasis, type Mode, type Swatch, type Palette } from "../src/tokens.ts"
import { cascade } from "./cascade.ts"

const data = readData()
const DIR = path.join(ROOT, "palettes")
const read = (n: string) => readFileSync(path.join(DIR, `${n}.yaml`), "utf8")
// The hand-written ones with Doom swatches (emphasisOnly): their hand-written part,
// resolved, and their swatches, which come from the generator NOW (not through the
// resolver, which would fail on loading if the generator changed the default: here the
// test that says so fails).
const EMPHASIS_ONLY = Object.keys(data.emphasisOnly)
const withoutEmphasis = (n: string) => resolveText(n, partByHand(read(n)))
const byHand = ["neutral", ...EMPHASIS_ONLY].map((n) => {
  const p = withoutEmphasis(n)
  return EMPHASIS_ONLY.includes(n) ? { ...p, emphasis: emphasisByHand(data, n, p) as Emphasis } : p
})
const fromDoom = [...(generateDoom(data) as Map<string, string>)].map(([n, t]) => resolveText(n, t))
const palettes = checkSet([...byHand, ...fromDoom])
const withEmphasis = palettes.filter((p) => p.emphasis)
const swatchesOf = (p: Palette) => p.emphasis!.swatches
const byHandWithSwatches = (p: Palette) => EMPHASIS_ONLY.includes(p.name)
/** The swatches that come from Doom by the rules: all, except a hand-written palette's default, which is its usual one. */
const fromRules = (p: Palette) => (byHandWithSwatches(p) ? swatchesOf(p).slice(1) : swatchesOf(p))

// The decision's numbers, written here again (not imported from the generator): if the
// generator changes one, these tests say so.
const TEXT = 4.5
const GRAPHIC = 3
const FAR_FROM_WARNING_AND_LINK = 12 // ΔE00 from the danger red and the links
const FAR_FROM_GRAPH = 16 // ΔE00 from ink and neighbour
// Two of a palette's swatches below this, in their graphic or text tone and in any mode,
// look almost the same in the menu (SEP_SWATCHES in scripts/doom.mjs says why 12:
// gruvbox's and xcode's violet and magenta, 2026-11-17).
const BETWEEN_SWATCHES = 12
const BLUE = [200, 275] // OKLCH hue: the blues, as in the audit
const NEAR_LINK = 30 // degrees of OKLCH hue from the links
const BAND: Record<Mode, [number, number]> = { light: [0.43, 0.77], dark: [0.48, 0.67] }

const lch = (h: string) => {
  const [L, a, b] = oklab(h)
  return { L, C: Math.hypot(a, b), h: ((Math.atan2(b, a) * 180) / Math.PI + 360) % 360 }
}
const hueDiff = (a: number, b: number) => Math.min(Math.abs(a - b), 360 - Math.abs(a - b))
const on = (x: string, f: string) => contrast(opaque(x), opaque(f))
/** A swatch's Doom colour in one mode: the theme's, or the composed one if Doom lacks the mode. */
function source(name: string, mode: Mode, id: string): string {
  const o = (data.palettes[name] ?? data.emphasisOnly[name]) as Record<Mode, string | undefined>
  const S = o[mode] ? data.themes[o[mode]!] : compose(data.themes[o[mode === "light" ? "dark" : "light"]!], mode === "light")
  return S[id]
}
function check(bad: string[]) {
  expect(bad).toEqual([])
}

it("the Doom palettes have emphasis swatches, except nova, and so do gruvbox and monokai; neutral has none", () => {
  expect(palettes.filter((p) => !p.emphasis).map((p) => p.name)).toEqual(["neutral", "nova"])
  expect(EMPHASIS_ONLY.sort()).toEqual(["gruvbox", "monokai"])
  expect(withEmphasis.length).toBe(26)
})

// The defaults the rule gives, written here so that a change shows in review (and the
// owner reviews it).
it("each palette's default", () => {
  expect(Object.fromEntries(withEmphasis.map((p) => [p.name, p.emphasis!.defaultId]))).toEqual({
    acario: "violet",
    ayu: "green",
    bluloco: "magenta",
    "challenger-deep": "violet",
    "dark-plus": "green",
    dracula: "green",
    "earl-grey": "green",
    flatwhite: "teal",
    gruvbox: "orange",
    henna: "violet",
    horizon: "violet",
    material: "orange",
    monokai: "orange",
    nord: "orange",
    "oceanic-next": "orange",
    one: "magenta",
    peacock: "magenta",
    rouge: "orange",
    solarized: "magenta",
    sourcerer: "orange",
    spacegrey: "magenta",
    "tokyo-night": "orange",
    tomorrow: "orange",
    vibrant: "magenta",
    xcode: "violet",
    zenburn: "violet",
  })
})

describe.each(withEmphasis.map((p) => [p.name, p] as const))("emphasis of %s", (name, p) => {
  // The rules, for the Doom swatches; gruvbox's and monokai's default is their usual one
  // and has its own tests (below).
  const ms = fromRules(p)

  it("each tone reaches its contrast on background and panel, in both modes (text 4.5:1; graphic and graph 3:1)", () => {
    const bad: string[] = []
    for (const m of ms)
      for (const mode of MODES)
        for (const [hue, min] of [["text", TEXT], ["graphic", GRAPHIC], ["graph", GRAPHIC]] as const)
          for (const f of ["background", "panel"] as const) {
            const r = on(m[mode][hue], p[mode].ui[f])
            if (!(r >= min)) bad.push(`${m.id}.${mode}.${hue} on ${f}: ${r.toFixed(2)} < ${min}`)
          }
    check(bad)
  })

  it("each tone at ΔE00 >= 12 from the danger red and the links, and >= 16 from the graph's ink and neighbour", () => {
    const bad: string[] = []
    for (const m of ms)
      for (const mode of MODES) {
        const v = p[mode]
        const against = [
          ["danger (diagram.blocked)", v.diagram.blocked, FAR_FROM_WARNING_AND_LINK],
          ["link", v.ui.link, FAR_FROM_WARNING_AND_LINK],
          ["ink", v.graph.ink, FAR_FROM_GRAPH],
          ["neighbour", v.graph.neighbour, FAR_FROM_GRAPH],
        ] as const
        for (const hue of ["text", "graphic", "graph"] as const)
          for (const [q, c, min] of against) {
            const d = deltaE00(m[mode][hue], c)
            if (!(d >= min)) bad.push(`${m.id}.${mode}.${hue} to ${q}: ΔE00 ${d.toFixed(1)} < ${min}`)
          }
      }
    check(bad)
  })

  it("the graph tone fits chord-graph's band (chroma >= 0.10, its lightness, ΔE >= 15 from ink and neighbour)", () => {
    const bad: string[] = []
    for (const m of ms)
      for (const mode of MODES) {
        const g = m[mode].graph
        const { L, C } = lch(g)
        const [lo, hi] = BAND[mode]
        if (!(C >= 0.1 && L >= lo && L <= hi)) bad.push(`${m.id}.${mode}: L ${L.toFixed(3)}, C ${C.toFixed(3)}`)
        for (const q of ["ink", "neighbour"] as const)
          if (!(deltaE(g, p[mode].graph[q]) >= 15)) bad.push(`${m.id}.${mode} to ${q}: ${deltaE(g, p[mode].graph[q]).toFixed(1)}`)
      }
    check(bad)
  })

  it("the swatches are distinguishable (ΔE00 >= 12 in graphic and text tone, in both modes) and come in EMPHASIS's order", () => {
    const bad: string[] = []
    const all = swatchesOf(p)
    for (const [i, a] of all.entries())
      for (const b of all.slice(i + 1))
        for (const mode of MODES)
          for (const hue of ["graphic", "text"] as const) {
            const d = deltaE00(a[mode][hue], b[mode][hue])
            if (!(d >= BETWEEN_SWATCHES)) bad.push(`${a.id}/${b.id} ${mode}.${hue}: ${d.toFixed(1)}`)
          }
    check(bad)
    const order = Object.keys(EMPHASIS)
    expect(ms.map((m) => order.indexOf(m.id))).toEqual([...ms.map((m) => order.indexOf(m.id))].sort((x, y) => x - y))
    for (const m of ms) expect(m.name).toBe((EMPHASIS as Record<string, string>)[m.id])
  })

  it("each tone is its Doom colour moved only in OKLCH lightness: the same hue (±2°), gaining no chroma (the graph's, only up to its band)", () => {
    const bad: string[] = []
    for (const m of ms)
      for (const mode of MODES) {
        const src = lch(source(name, mode, m.id))
        for (const hue of ["text", "graphic", "graph"] as const) {
          const x = lch(m[mode][hue])
          const cap = hue === "graph" ? Math.max(src.C, 0.12) : src.C
          if (hueDiff(x.h, src.h) > 2) bad.push(`${m.id}.${mode}.${hue}: hue ${x.h.toFixed(1)} against ${src.h.toFixed(1)}`)
          if (x.C > cap + 0.002) bad.push(`${m.id}.${mode}.${hue}: chroma ${x.C.toFixed(3)} > ${cap.toFixed(3)}`)
        }
      }
    check(bad)
  })

  it("a Doom colour that already reaches its contrast is used as is, and one that does not moves away from the background", () => {
    const bad: string[] = []
    for (const m of ms)
      for (const mode of MODES) {
        const src = source(name, mode, m.id)
        const b = p[mode].ui
        for (const [hue, min] of [["text", TEXT], ["graphic", GRAPHIC]] as const) {
          const reaches = on(src, b.background) >= min && on(src, b.panel) >= min
          const x = m[mode][hue]
          if (reaches && x !== src) bad.push(`${m.id}.${mode}.${hue}: ${x}, and ${src} already reached it`)
          if (!reaches && !(on(x, b.background) > on(src, b.background))) bad.push(`${m.id}.${mode}.${hue}: does not move away from the background`)
        }
      }
    check(bad)
  })

  it.skipIf(byHandWithSwatches(p))("the default is the rule's: the most chromatic of those unlike the links; at equal chroma, the first", () => {
    const link = (m: Swatch) =>
      MODES.some((mode) => {
        const h = lch(m[mode].graphic).h
        return (h >= BLUE[0]! && h <= BLUE[1]!) || hueDiff(h, lch(p[mode].ui.link).h) < NEAR_LINK
      })
    const chroma = (m: Swatch) => Math.round(((lch(m.light.graphic).C + lch(m.dark.graphic).C) / 2) * 1000) / 1000
    const far = ms.filter((m) => !link(m))
    const de = far.length ? far : ms
    const better = Math.max(...de.map(chroma))
    expect(p.emphasis!.defaultId).toBe(de.find((m) => chroma(m) === better)!.id)
  })

  it.skipIf(byHandWithSwatches(p))("it is not blue if some swatch is not", () => {
    const blue = (m: Swatch) => MODES.some((mode) => lch(m[mode].graphic).h >= BLUE[0]! && lch(m[mode].graphic).h <= BLUE[1]!)
    const pred = ms.find((m) => m.id === p.emphasis!.defaultId)!
    if (blue(pred)) expect(ms.filter((m) => !blue(m)).map((m) => m.id)).toEqual([])
  })

  it("the default's tones are ui.accent, diagram.highlight and graph.accent", () => {
    const pred = swatchesOf(p).find((m) => m.id === p.emphasis!.defaultId)!
    for (const mode of MODES) {
      expect(p[mode].ui.accent).toBe(pred[mode].text)
      expect(p[mode].diagram.highlight).toBe(pred[mode].graphic)
      expect(p[mode].graph.accent).toBe(pred[mode].graph)
    }
  })
})

describe("the default rule, with swatches made here", () => {
  const links = { light: { link: "#0b5cad" }, dark: { link: "#6cb4ff" } }
  const swatch = (id: string, hex: string) => ({ id, name: id, light: { graphic: hex }, dark: { graphic: hex } })
  const blue = swatch("blue", "#0066ff") // high chroma, hue ~260°
  const green = swatch("green", "#3a8a3a") // less chroma
  const otherGreen = swatch("magenta", "#3a8a3a") // the same chroma as green

  it("a blue with more chroma does not beat one that is not blue", () => {
    expect(defaultId([blue, green], links).id).toBe("green")
  })
  it("if all are blue, the most chromatic wins", () => {
    expect(defaultId([swatch("cyan", "#4a90a0"), blue], links).id).toBe("blue")
  })
  it("at equal chroma, the first in EMPHASIS's order, whatever the input order", () => {
    expect(defaultId([green, otherGreen], links).id).toBe("green")
    expect(defaultId([otherGreen, green], links).id).toBe("green")
  })
  it("in every Doom palette, the same default with the swatches reversed", () => {
    for (const p of withEmphasis.filter((q) => !byHandWithSwatches(q))) {
      const modes = Object.fromEntries(MODES.map((m) => [m, { link: p[m].ui.link }]))
      expect(defaultId([...swatchesOf(p)].reverse(), modes).id, p.name).toBe(p.emphasis!.defaultId)
    }
  })
  it("without swatches, none", () => {
    expect(defaultId([], links)).toBeUndefined()
  })
})

// Those that pass everything else (before separating) and each palette's default, from
// the generator NOW.
const candidatesOf = (p: Palette): { candidates: Swatch[]; pred: string } =>
  byHandWithSwatches(p)
    ? (({ pred, candidates }) => ({ candidates, pred: pred.id }))(candidatesByHand(data, p.name, withoutEmphasis(p.name)))
    : (({ candidates, emphasis }) => ({ candidates, pred: emphasis!.defaultId }))(doomPalette(data, p.name))

describe("near-identical swatches: which one stays (separate)", () => {
  // Three swatches made here, the same colour in both modes and both tones.
  const swatch = (id: string, hex: string) =>
    ({ id, name: id, light: { graphic: hex, text: hex, graph: hex }, dark: { graphic: hex, text: hex, graph: hex } }) as Swatch
  const ids = (ms: Swatch[]) => ms.map((m) => m.id)
  // violet and magenta nearly identical; magenta a little closer to orange than violet.
  const orange = swatch("orange", "#c05a00")
  const violet = swatch("violet", "#7a3eba")
  const magenta = swatch("magenta", "#8a3eb0")

  // With a table of distances instead of colours, to put each case exactly where it is decided.
  const table = (t: Record<string, number>) => (a: Swatch, b: Swatch) => t[`${a.id}/${b.id}`] ?? t[`${b.id}/${a.id}`] ?? 50
  const [A, B, C, D] = ["orange", "yellow", "green", "violet"].map((id) => swatch(id, "#808080")) as [Swatch, Swatch, Swatch, Swatch]

  it("of a near-identical pair, the one closer to the others (not counting the other of the pair) goes, whether first or second", () => {
    expect(deltaE00(violet.light.graphic, magenta.light.graphic)).toBeLessThan(BETWEEN_SWATCHES)
    expect(deltaE00(magenta.light.graphic, orange.light.graphic)).toBeLessThan(deltaE00(violet.light.graphic, orange.light.graphic))
    expect(ids(separate([orange, violet, magenta], "orange"))).toEqual(["orange", "violet"])
    // A and B nearly identical; A is 20 from C and B 40: A, the first, goes.
    expect(ids(separate([A, B, C], "green", 12, table({ "orange/yellow": 2, "orange/green": 20, "yellow/green": 40 })))).toEqual(["yellow", "green"])
  })
  it("the default is never removed, even if it is the closest to the others", () => {
    expect(ids(separate([orange, violet, magenta], "magenta"))).toEqual(["orange", "magenta"])
    expect(ids(separate([A, B, C], "orange", 12, table({ "orange/yellow": 2, "orange/green": 20, "yellow/green": 40 })))).toEqual(["orange", "green"])
  })
  it("the closest pair first: B (2 from A) goes before deciding between C and D", () => {
    // Closest first: B out, and of C and D (equal to the rest) the later one, D. Furthest
    // first (C/D first): C out, then B, leaving A and D. E, the default, far from all.
    const E = swatch("magenta", "#808080")
    const t = table({ "orange/yellow": 2, "yellow/green": 11, "green/violet": 11.5 })
    expect(ids(separate([A, B, C, D, E], "magenta", 12, t))).toEqual(["orange", "green", "magenta"])
  })
  it("at 12 or more nothing goes; below, it does", () => {
    expect(ids(separate([A, B, C], "green", 12, table({ "orange/yellow": 12 })))).toEqual(["orange", "yellow", "green"])
    expect(ids(separate([A, B, C], "green", 12, table({ "orange/yellow": 11.99 })))).toEqual(["orange", "green"])
  })
  it("at equal closeness to the rest, the later one in EMPHASIS's order goes", () => {
    const otherViolet = swatch("magenta", violet.light.graphic)
    expect(ids(separate([violet, otherViolet, orange], "orange"))).toEqual(["violet", "orange"])
    expect(ids(separate([orange, otherViolet, violet], "orange"))).toEqual(["orange", "violet"])
  })
  it("those far enough apart, all of them, in input order", () => {
    expect(ids(separate([violet, orange], "orange"))).toEqual(["violet", "orange"])
  })
  it("separate fails without the default among the swatches (unless there are none)", () => {
    expect(() => separate([violet, orange], undefined)).toThrow(/the default/)
    expect(() => separate([violet, orange], "magenta")).toThrow(/the default/)
    expect(separate([], undefined)).toEqual([])
  })
  it("the distance is the smallest over both tones in both modes: one near-identical tone in one mode is enough", () => {
    const almost = { ...violet, id: "magenta", dark: { ...violet.dark, graphic: "#00a000" }, light: { ...violet.light, graphic: "#00a000" } }
    expect(swatchDistance(violet, almost)).toBe(0) // the text tone, equal
    const other = { ...almost, light: { ...almost.light, text: "#00a000" } } // the dark text tone, equal
    expect(swatchDistance(violet, other)).toBe(0)
    expect(ids(separate([violet, other, orange], "orange"))).toEqual(["violet", "orange"])
  })

  describe.each(withEmphasis.map((p) => [p.name, p] as const))("%s", (_n, p) => {
    const { candidates, pred } = candidatesOf(p)
    it("the default comes from all the candidates, before separating, and is offered", () => {
      expect(pred).toBe(p.emphasis!.defaultId)
      if (!byHandWithSwatches(p)) {
        const modes = Object.fromEntries(MODES.map((m) => [m, { link: p[m].ui.link }]))
        expect(defaultId(candidates, modes).id).toBe(pred)
      }
      expect(ids(swatchesOf(p))).toContain(pred)
    })
    it("what is offered is separate() of the candidates, whatever their order, and the same every time", () => {
      const expected = ids(swatchesOf(p))
      expect(ids(separate(candidates, pred))).toEqual(expected)
      expect(ids(separate(candidates, pred))).toEqual(expected)
      const reversed = separate([...candidates].reverse(), pred)
      expect(ids(reversed).sort()).toEqual([...expected].sort())
      // And each one not offered is nearly identical to one that is.
      for (const c of candidates.filter((m) => !expected.includes(m.id)))
        expect(Math.min(...swatchesOf(p).map((m) => swatchDistance(c, m))), c.id).toBeLessThan(BETWEEN_SWATCHES)
    })
  })

  // Those the rule removes (2026-11-17), written here so that a change shows in review.
  it("those removed: the violet and magenta twins, and the other near-identical pairs", () => {
    const removed = Object.fromEntries(
      withEmphasis
        .map((p) => [p.name, candidatesOf(p).candidates.map((m) => m.id).filter((id) => !ids(swatchesOf(p)).includes(id))] as const)
        .filter(([, q]) => q.length > 0),
    )
    expect(removed).toEqual({
      ayu: ["yellow"],
      "dark-plus": ["violet"],
      flatwhite: ["cyan"],
      gruvbox: ["magenta"],
      horizon: ["orange"],
      monokai: ["teal"],
      one: ["yellow"],
      peacock: ["yellow"],
      sourcerer: ["yellow"],
      xcode: ["magenta"],
    })
  })
})

describe("palette.css: data-emphasis", () => {
  const sheet = css(palettes)
  const modeAttr = (m: Mode): Record<string, string> => (m === "dark" ? { "data-mode": "dark" } : {})
  const expected = (p: Palette, m: Mode) => new Map(variablesCss(p, m, "all"))

  for (const p of withEmphasis)
    it.each(MODES)(`${p.name}, mode %s: each swatch changes ui.accent, highlight and the graph's accent, and nothing else`, (mode) => {
      for (const m of swatchesOf(p)) {
        const e = expected(p, mode)
        e.set("--ui-accent", m[mode].text)
        e.set("--diagram-highlight", m[mode].graphic)
        e.set("--chord-accent-light", m.light.graph)
        e.set("--chord-accent-dark", m.dark.graph)
        expect(cascade(sheet, { ...modeAttr(mode), "data-theme": p.name, "data-emphasis": m.id }), m.id).toEqual(e)
      }
    })

  it("references to what the emphasis carries: none in the Doom palettes; in monokai, those each block pins", () => {
    const pinned = Object.fromEntries(
      withEmphasis.map((p) => [p.name, Object.fromEntries(MODES.map((md) => [md, pinnedEmphasis(p, md)]))]),
    )
    const withAny = Object.entries(pinned).filter(([, f]) => MODES.some((md) => f[md]!.length > 0))
    expect(Object.fromEntries(withAny)).toEqual({
      // monokai: code.variable is {ui.accent} in light mode and a colour in dark mode:
      // the dark block returns it to that colour.
      monokai: {
        light: [["--code-variable", "#b85400"]],
        dark: [["--code-variable", "#fd971f"]],
      },
    })
  })

  it.each(MODES)("an unknown data-emphasis, or another palette's, leaves the palette as it is (mode %s)", (mode) => {
    for (const p of palettes) {
      const other = withEmphasis.find((q) => q !== p && swatchesOf(q).some((m) => !p.emphasis?.swatches.some((x) => x.id === m.id)))
      const foreign = other ? swatchesOf(other).find((m) => !p.emphasis?.swatches.some((x) => x.id === m.id))!.id : "nothing"
      for (const id of ["nothing", foreign])
        expect(cascade(sheet, { ...modeAttr(mode), "data-theme": p.name, "data-emphasis": id }), `${p.name} ${id}`).toEqual(expected(p, mode))
    }
  })

  it("each swatch that is not the default, two blocks (light and dark); the default, none", () => {
    const blocks = [...sheet.matchAll(/^(:root[^{]*) \{\n/gm)].map((b) => b[1]!).filter((s) => s.includes("data-emphasis"))
    const expected = withEmphasis.flatMap((p) =>
      swatchesOf(p)
        .filter((m) => m.id !== p.emphasis!.defaultId)
        .flatMap((m) => {
          const s = `:root[data-theme="${p.name}"][data-emphasis="${m.id}"]`
          return [s, `${s}[data-mode="dark"]`]
        }),
    )
    expect(blocks).toEqual(expected)
  })
})

it("tokens.json: the attribute, which role each tone carries, and each palette's swatches in each mode", () => {
  const t = JSON.parse(tokensJson(palettes))
  expect(t.emphasis).toEqual({
    attribute: "data-emphasis",
    roles: { text: ["ui.accent"], graphic: ["diagram.highlight"], graph: ["graph.accent"] },
  })
  for (const p of palettes) {
    if (!p.emphasis) {
      expect(t.palettes[p.name].emphasis, p.name).toBeNull()
      continue
    }
    const e = t.palettes[p.name].emphasis
    expect(e.defaultId).toBe(p.emphasis.defaultId)
    for (const mode of MODES)
      expect(e[mode], `${p.name}.${mode}`).toEqual(swatchesOf(p).map((m) => ({ id: m.id, name: m.name, ...m[mode] })))
  }
})

describe("gruvbox and monokai: hand-written, with their Doom themes' swatches (owner, 2026-11-16)", () => {
  const de = (n: string) => palettes.find((p) => p.name === n)!
  const hues = (m: Swatch) => Object.fromEntries(MODES.map((md) => [md, { ...m[md] }]))

  // And their usual tones: ui.accent (text), diagram.highlight (graphic) and graph.accent.
  const PREVIOUS_DEFAULT = {
    gruvbox: {
      light: { text: "#9e3403", graphic: "#af3a03", graph: "#d65d0e" },
      dark: { text: "#fe8019", graphic: "#fe8019", graph: "#e07114" },
    },
    monokai: {
      light: { text: "#b85400", graphic: "#b85400", graph: "#d0700a" },
      dark: { text: "#fd971f", graphic: "#fd971f", graph: "#d87a0f" },
    },
  }

  it.each(EMPHASIS_ONLY)("%s: the default is the usual one, first, with its colour's Doom name (orange)", (n) => {
    const p = de(n)
    const [pred] = swatchesOf(p)
    expect(p.emphasis!.defaultId).toBe(pred!.id)
    expect([pred!.id, pred!.name]).toEqual(["orange", "orange"])
    expect(hues(pred!)).toEqual(PREVIOUS_DEFAULT[n as keyof typeof PREVIOUS_DEFAULT])
    // And it is its Doom colour in each mode Doom provides (hence its name).
    for (const [mode, theme] of Object.entries(data.emphasisOnly[n]) as [Mode, string][])
      expect(pred![mode].graphic, `${n}.${mode}`).toBe(data.themes[theme].orange)
  })

  it.each(EMPHASIS_ONLY)("%s: without data-emphasis, the output is the same as without its swatches (palette.css, tokens.json)", (n) => {
    // The swatches only add: the palette's own blocks and its tokens.json entry (without
    // `emphasis`) are what the same palette gives with no emphasis block at all.
    const sin = checkSet(palettes.map((p) => (p.name === n ? withoutEmphasis(n) : p)))
    expect(sin.find((p) => p.name === n)!.emphasis).toBeNull()
    const blocks = (sheet: string) =>
      [...sheet.matchAll(/^(:root\[data-theme="([a-z-]+)"\](?:\[data-mode="dark"\])?) \{\n[^}]*\}\n/gm)]
        .filter((b) => b[2] === n)
        .map((b) => b[0])
    expect(blocks(css(palettes)).length).toBe(2)
    expect(blocks(css(palettes))).toEqual(blocks(css(sin)))
    const { emphasis, ...rest } = JSON.parse(tokensJson(palettes)).palettes[n]
    const { emphasis: ninguno, ...restWithout } = JSON.parse(tokensJson(sin)).palettes[n]
    expect(emphasis.defaultId).toBe("orange")
    expect(ninguno).toBeNull()
    expect(rest).toEqual(restWithout)
  })

  it("the Doom themes' background is our gruvbox's: the default variant (neither hard nor soft)", () => {
    const g = withoutEmphasis("gruvbox")
    expect([g.light.ui.background, g.dark.ui.background]).toEqual(["#fbf1c7", "#282828"])
    expect(data.emphasisOnly.gruvbox).toEqual({ light: "doom-gruvbox-light", dark: "doom-gruvbox" })
    expect(data.themes["doom-gruvbox-light"].bg).toBe(g.light.ui.background)
    expect(data.themes["doom-gruvbox"].bg).toBe(g.dark.ui.background)
  })

  it("gruvbox offers its yellow and its purple (violet; magenta is nearly identical and not offered); its aqua does not pass", () => {
    const ms = swatchesOf(de("gruvbox"))
    expect(ms.map((m) => m.id)).toEqual(["orange", "yellow", "green", "violet"])
    const yellow = ms.find((m) => m.id === "yellow")!
    expect(yellow.name).toBe("yellow")
    expect(hues(yellow)).toEqual({
      light: { text: "#905b00", graphic: "#b57614", graph: "#b57614" },
      dark: { text: "#fabd2f", graphic: "#fabd2f", graph: "#b38300" },
    })
    // Its measured contrasts, on background and panel, to two decimals.
    const g = de("gruvbox")
    const measured = Object.fromEntries(
      MODES.map((md) => [
        md,
        Object.fromEntries(
          (["text", "graphic", "graph"] as const).map((t) => [
            t,
            (["background", "panel"] as const).map((f) => Number(on(yellow[md][t], g[md].ui[f]).toFixed(2))),
          ]),
        ),
      ]),
    )
    expect(measured).toEqual({
      light: { text: [5.03, 4.54], graphic: [3.33, 3.0], graph: [3.33, 3.0] },
      dark: { text: [8.69, 7.74], graphic: [8.69, 7.74], graph: [4.33, 3.85] },
    })
    // The Doom colours they come from: the faded yellow (light) and the bright one (dark).
    expect([data.themes["doom-gruvbox-light"].yellow, data.themes["doom-gruvbox"].yellow]).toEqual(["#b57614", "#fabd2f"])
  })

  it("monokai: its swatches, from doom-monokai-classic (composed, in light mode)", () => {
    expect(data.emphasisOnly.monokai).toEqual({ dark: "doom-monokai-classic" })
    const ms = swatchesOf(de("monokai"))
    expect(Object.fromEntries(ms.map((m) => [m.id, hues(m)]))).toEqual({
      orange: PREVIOUS_DEFAULT.monokai,
      red: {
        light: { text: "#b9180e", graphic: "#b9180e", graph: "#b9180e" },
        dark: { text: "#ff7664", graphic: "#e74c3c", graph: "#e74c3c" },
      },
      yellow: {
        light: { text: "#756b00", graphic: "#8f8304", graph: "#8f8304" },
        dark: { text: "#e6db74", graphic: "#e6db74", graph: "#9a8f1e" },
      },
      green: {
        light: { text: "#527500", graphic: "#638c00", graph: "#638c00" },
        dark: { text: "#a6e22e", graphic: "#a6e22e", graph: "#709d00" },
      },
    })
  })
})

describe("the palettes that were removed (owner, 2026-11-16)", () => {
  const REMOVED = { palenight: "material", opera: "nord", snazzy: "dracula", moonlight: "tokyo-night" }
  const FROM_DOOM = ["doom-palenight", "doom-opera", "doom-opera-light", "doom-snazzy", "doom-moonlight"]
  const dist = (f: string) => path.join(ROOT, "dist", f)
  const tokens = JSON.parse(readFileSync(dist("tokens.json"), "utf8"))
  const sheet = readFileSync(dist("palette.css"), "utf8")

  it.each(Object.keys(REMOVED))("%s is absent: not in the data, palettes/ or dist/", (n) => {
    expect(Object.keys(data.palettes)).not.toContain(n)
    expect(existsSync(path.join(ROOT, "palettes", `${n}.yaml`))).toBe(false)
    expect(Object.keys(tokens.palettes)).not.toContain(n)
    expect(sheet).not.toContain(`[data-theme="${n}"]`)
  })

  it("their Doom themes are not in the data either, and the palette that resembled each is", () => {

    for (const t of FROM_DOOM) {
      expect(Object.keys(data.themes)).not.toContain(t)
      expect(data.provenance.files).not.toContain(`themes/${t}-theme.el`)
    }
    for (const similar of Object.values(REMOVED)) expect(Object.keys(tokens.palettes)).toContain(similar)
  })
})
