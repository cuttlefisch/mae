// SPDX-License-Identifier: GPL-3.0-or-later
// The generators: dist/ is up to date with palettes/, the output is deterministic
// and does not depend on the order the files are written in, and palette.css gives
// each combination of palette and mode exactly its values.
import { readdirSync, readFileSync, statSync } from "node:fs"
import path from "node:path"
import { parse as parseYaml, stringify } from "yaml"
import { css, generate, tokensJson, variablesCss } from "../src/generate.ts"
import { GROUPS } from "../src/tokens.ts"
import { cascade } from "./cascade.ts"
import { MODES, loadPalettes, checkSet, resolver, resolveText, type Mode, type Palette } from "../src/tokens.ts"

const ROOT = path.join(import.meta.dirname, "..")
const palettes = loadPalettes(path.join(ROOT, "palettes"))

function files(dir: string, base = dir): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = path.join(dir, f)
    return statSync(p).isDirectory() ? files(p, base) : [path.relative(base, p)]
  })
}

describe("dist/ up to date", () => {
  it("dist/ has exactly the files that are generated, with the same bytes", () => {
    const expected = generate(palettes)
    expect(files(path.join(ROOT, "dist")).sort()).toEqual([...expected.keys()].sort())
    for (const [ruta, contenido] of expected)
      expect(readFileSync(path.join(ROOT, "dist", ruta), "utf8"), `dist/${ruta}: run npm run build`).toBe(contenido)
  })
})

describe("determinism", () => {
  it("two generations give the same bytes", () => {
    const a = generate(loadPalettes(path.join(ROOT, "palettes")))
    const b = generate(loadPalettes(path.join(ROOT, "palettes")))
    expect([...b]).toEqual([...a])
  })

  it("neither the order of keys in the YAML nor that of the palettes changes the output", () => {
    // Each file, rewritten with its keys reversed at every level. Lists (the emphasis
    // swatches) keep their order, which is the output order: only their maps are reversed.
    const reversed = (v: unknown): unknown =>
      Array.isArray(v)
        ? v.map(reversed)
        : v && typeof v === "object"
          ? Object.fromEntries(Object.entries(v).reverse().map(([k, x]) => [k, reversed(x)]))
          : v
    const others = palettes
      .map((p) => {
        const text = readFileSync(path.join(ROOT, "palettes", `${p.name}.yaml`), "utf8")
        return resolveText(p.name, stringify(reversed(parseYaml(text))))
      })
      .reverse()
    expect(Object.keys(reversed({ a: 1, b: 2 }) as object)).toEqual(["b", "a"])
    expect([...generate(checkSet(others))]).toEqual([...generate(palettes)])
  })
})

describe("palette.css", () => {
  const sheet = css(palettes)
  const pred = palettes.find((p) => p.isDefault)!
  const expected = (p: Palette, m: Mode) => new Map(variablesCss(p, m, "all"))
  const modeAttr = (m: Mode): Record<string, string> => (m === "dark" ? { "data-mode": "dark" } : {})

  it.each(MODES)("without data-theme (or with an unknown one), mode %s: the default palette", (m) => {
    expect(cascade(sheet, modeAttr(m))).toEqual(expected(pred, m))
    expect(cascade(sheet, { ...modeAttr(m), "data-theme": "other" })).toEqual(expected(pred, m))
  })

  for (const p of palettes)
    it.each(MODES)(`data-theme="${p.name}", mode %s: exactly its values`, (m) => {
      expect(cascade(sheet, { ...modeAttr(m), "data-theme": p.name })).toEqual(expected(p, m))
    })

  it("in palette.css, a role that references another of the same mode is var(); the rest, a colour", () => {
    // In every block of every mode, the default's :root ones too (an emphasis swatch's
    // blocks apart: they only carry their own variables).
    const blocks = [...sheet.matchAll(/^(:root[^{]*) \{\n([^}]*)\}/gm)].filter((b) => !b[1]!.includes("data-emphasis"))
    expect(blocks).toHaveLength(2 + 2 * palettes.length)
    for (const [, sel, cuerpo] of blocks)
      expect(cuerpo, sel).toContain(
        "  --diagram-ink: var(--ui-foreground);\n  --diagram-background: var(--ui-background);\n  --diagram-border: var(--ui-foreground);\n",
      )
    // The gallery is the same in both modes: a colour in every block, in light mode
    // too, where its role is {ui.background} (#ffffff is neutral's light background).
    expect(sheet).toMatch(/^:root \{[^}]*--gallery-title: #ffffff;/m)
    expect(sheet).toMatch(/:root\[data-mode="dark"\] \{[^}]*--gallery-title: #ffffff;/)
    // The graph carries the mode in its name: always a colour, even when its role is a reference.
    expect(sheet).not.toMatch(/--chord-[\w-]+: var\(/)
  })

  it("a reference to a role of the other mode is written as a colour, not var() (var() would take the current mode's value)", () => {
    const f = parseYaml(readFileSync(path.join(ROOT, "palettes", "neutral.yaml"), "utf8"))
    f.dark.ui.muted = "{light.ui.background}"
    const other = css([resolver("neutral", { ...f, isDefault: true })])
    expect(other).toMatch(/:root\[data-mode="dark"\] \{[^}]*\n {2}--ui-muted: #ffffff;\n/)
    expect(other).not.toMatch(/--ui-muted: var\(/)
  })

  it("--gallery-title is the same in both modes: in every block a colour, the same in light and dark", () => {
    const titles = [...sheet.matchAll(/^(:root[^{]*) \{\n[^}]*? {2}--gallery-title: ([^;]+);/gm)].map((m) => [m[1]!, m[2]!])
    expect(titles).toHaveLength(2 + 2 * palettes.length)
    for (const [sel, v] of titles) expect(v, sel).toMatch(/^#[0-9a-f]{6}$/)
    for (let i = 0; i < titles.length; i += 2) expect(titles[i + 1]![1], titles[i + 1]![0]).toBe(titles[i]![1])
  })

  it("the test's cascade tells palettes and modes apart (otherwise the above proves nothing)", () => {
    const [a, b] = palettes
    expect(expected(a!, "light")).not.toEqual(expected(b!, "light"))
    expect(expected(pred, "light")).not.toEqual(expected(pred, "dark"))
  })

  it("the variables are there, with these names", () => {
    const names = [...expected(pred, "light").keys()]
    for (const r of GROUPS.ui) expect(names).toContain(`--ui-${r}`)
    for (const r of ["ink", "accent", "neighbour", "visited", "edge"])
      for (const m of ["light", "dark"]) expect(names).toContain(`--chord-${r}-${m}`)
    for (const v of ["--diagram-ink", "--diagram-background", "--diagram-border"]) expect(names).toContain(v)
    for (const r of GROUPS.code) expect(names).toContain(`--code-${r}`)
  })
})

describe("the map roles", () => {
  const MAPS = GROUPS.diagram.filter((r) => !["ink", "background", "border"].includes(r))

  // The values are checked by the cascade above ("exactly its values").
  it("each palette.css block carries each --diagram-<role> once", () => {
    const blocks = [...css(palettes).matchAll(/^(:root[^{]*) \{\n([^}]*)\}/gm)].filter((b) => !b[1]!.includes("data-emphasis"))
    expect(blocks).toHaveLength(2 + 2 * palettes.length)
    expect(MAPS).toHaveLength(14)
    for (const [, sel, cuerpo] of blocks)
      for (const r of GROUPS.diagram) {
        const lines = cuerpo!.split("\n").filter((l) => l.startsWith(`  --diagram-${r}:`))
        expect(lines, `${sel} --diagram-${r}`).toHaveLength(1)
      }
  })

  it("tokens.json carries each role: its variable and its resolved value in every palette and mode", () => {
    const t = JSON.parse(tokensJson(palettes))
    for (const r of GROUPS.diagram) {
      expect(t.variables[`diagram.${r}`], r).toEqual({ light: `--diagram-${r}`, dark: `--diagram-${r}` })
      for (const p of palettes)
        for (const m of MODES) {
          expect(t.palettes[p.name][m].diagram[r], `${p.name}.${m}.${r}`).toMatch(/^#[0-9a-f]{6}$/)
          expect(t.palettes[p.name][m].diagram[r]).toBe(p[m].diagram[r])
          expect(t.palettes[p.name].css[m][`--diagram-${r}`]).toBe(p[m].diagram[r])
        }
    }
    expect(Object.keys(t.palettes.neutral.light.diagram)).toEqual([...GROUPS.diagram])
  })
})
