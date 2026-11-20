// SPDX-License-Identifier: GPL-3.0-or-later
// The Doom Emacs palettes: palettes/ is what comes out of data/doom.json through
// scripts/doom.mjs (nobody edits them by hand), the modes Doom provides are used as
// they are, and scripts/extract-doom.mjs's reader understands the forms it says and
// only those. What each palette needs to work is required by test/contrast.test.ts.
import { readFileSync, readdirSync } from "node:fs"
import path from "node:path"
import { MARK, EMPHASIS_MARK, ROOT, generate, generateByHand, readData, partByHand, hueH } from "../scripts/doom.mjs"
import { contrast, opaque } from "../src/color.ts"
import { VARIABLES, colors } from "../scripts/extract-doom.mjs"
import { loadPalettes, resolveText } from "../src/tokens.ts"

const data = readData()
const DIR = path.join(ROOT, "palettes")
const generated = readdirSync(DIR).filter((f) => readFileSync(path.join(DIR, f), "utf8").includes(MARK))

describe("Doom palettes", () => {
  it("palettes/ has exactly those of data/doom.json, and each is what the generator produces", () => {
    const expected = generate(data)
    expect(generated.map((f) => f.replace(/\.yaml$/, "")).sort()).toEqual([...expected.keys()])
    for (const [n, text] of expected) expect(readFileSync(path.join(DIR, `${n}.yaml`), "utf8"), n).toBe(text)
  })

  it("gruvbox and monokai (emphasisOnly): their hand-written part, as is, and at the end the emphasis block the generator produces", () => {
    const read = (n: string) => readFileSync(path.join(DIR, `${n}.yaml`), "utf8")
    const expected = generateByHand(data, read, resolveText)
    expect([...expected.keys()]).toEqual(["gruvbox", "monokai"])
    for (const [n, text] of expected) {
      expect(read(n), n).toBe(text)
      expect(text.split(EMPHASIS_MARK).length, n).toBe(2)
      expect(text).not.toContain(MARK)
    }
    // The hand-written ones not in emphasisOnly carry no block.
    for (const f of readdirSync(DIR).filter((f) => !generated.includes(f) && !expected.has(f.replace(/\.yaml$/, ""))))
      expect(readFileSync(path.join(DIR, f), "utf8"), f).not.toContain(EMPHASIS_MARK)
    // The hand-written part is what comes before the block, and without a block, the whole text.
    expect(partByHand("a: 1\n")).toBe("a: 1\n")
    expect(partByHand(`a: 1\n\n${EMPHASIS_MARK} x\nemphasis: {}\n`)).toBe("a: 1\n")
  })

  it("the modes Doom provides are used as they are: page and body; the links, its blue, with its hue", () => {
    const palettes = new Map(loadPalettes(DIR).map((p) => [p.name, p]))
    let moved = 0
    for (const [n, origin] of Object.entries(data.palettes) as [string, Record<string, string>][])
      for (const [mode, theme] of Object.entries(origin)) {
        const t = data.themes[theme]
        const b = palettes.get(n)![mode as "light" | "dark"].ui
        expect([b.background, b.foreground], `${n}.${mode}`).toEqual([t.bg, t.fg])
        // The blue as is if it reaches 4.5:1 on the page and the panels; if not, moved only in
        // lightness: the same OKLCH hue (as far as 8 bits and chroma clipping allow).
        const reaches = [b.background, b.panel].every((f) => contrast(opaque(t.blue), opaque(f)) >= 4.5)
        if (reaches) expect(b.link, `${n}.${mode}`).toBe(t.blue)
        else {
          moved++
          expect(b.link, `${n}.${mode}`).not.toBe(t.blue)
          expect(Math.abs(hueH(b.link) - hueH(t.blue)), `${n}.${mode}`).toBeLessThan(2)
        }
      }
    // Some themes' blue does not reach it (otherwise the branch above would go untested).
    expect(moved).toBeGreaterThan(0)
  })

  it("the data: every named theme is present, with all its variables as #rrggbb, and the provenance has a commit", () => {
    expect(data.provenance.commit).toMatch(/^[0-9a-f]{40}$/)
    expect(data.provenance.repository).toBe("https://github.com/doomemacs/themes")
    const named = new Set(
      [data.palettes, data.emphasisOnly].flatMap((l) => Object.values(l).flatMap((p) => Object.values(p as object))),
    )
    expect([...named].sort()).toEqual(Object.keys(data.themes).sort())
    for (const [theme, vars] of Object.entries(data.themes) as [string, Record<string, string>][]) {
      expect(Object.keys(vars), theme).toEqual(VARIABLES)
      for (const v of VARIABLES) expect(vars[v], `${theme}.${v}`).toMatch(/^#[0-9a-f]{6}$/)
    }
    for (const n of Object.keys(data.palettes)) expect(n).toMatch(/^[a-z][a-z0-9-]*$/)
    // A palette is Doom's or hand-written with Doom swatches, not both.
    for (const n of Object.keys(data.emphasisOnly)) expect(Object.keys(data.palettes)).not.toContain(n)
  })
})

describe("the Doom theme reader", () => {
  const theme = (ligatures: string, extra = "") => `;;; a theme
${extra}
(def-doom-theme doom-x
  "Documentation with \\"quotes\\" and (parentheses)."
  :family 'doom-x
  :background-mode 'dark
  ;; name      gui       256       16
  ((c1         '("#102030" "black" "black"))
   (c2         '("#FFFFFF" nil nil))
   ${ligatures}
   (sel        (doom-darken c1 0.1)))
  ((cara :foreground red)))`
  const all = (spare: Record<string, string>) =>
    VARIABLES.map((v) => `(${v} ${spare[v] ?? "c1"})`).join("\n   ")

  it("GUI value (lowercased), earlier name, doom-darken, doom-lighten, doom-blend, if and car", () => {
    const c = colors(
      "doom-x",
      theme(
        all({
          bg: "'(\"#ABCDEF\" \"#000000\" \"black\")",
          fg: "c2",
          red: "(doom-darken c2 0.25)",
          green: "(doom-lighten c1 0.25)",
          blue: "(doom-blend c1 c2 0.25)",
          yellow: "(if doom-x-bright c2 c1)",
          orange: "(if doom-x-other c2 c1)",
          teal: "`(,(car c2) nil nil)",
        }),
        "(defcustom doom-x-bright nil \"\")\n(defcustom doom-x-other t \"\")",
      ),
    )
    expect(c.bg).toBe("#abcdef")
    expect(c.fg).toBe("#ffffff")
    expect(c.red).toBe("#bfbfbf") // 0.75·white + 0.25·black
    expect(c.green).toBe("#4c5864") // 0.75·#102030 + 0.25·white
    expect(c.blue).toBe("#c3c7cb") // 0.25·#102030 + 0.75·white
    expect(c.yellow).toBe("#102030") // defcustom nil: the else branch
    expect(c.orange).toBe("#ffffff") // defcustom t: the then branch
    expect(c.teal).toBe("#ffffff")
    expect(c.base0).toBe("#102030")
  })

  // cond (since 2026-11-16: doom-gruvbox's and doom-gruvbox-light's bg and bg-alt), only with
  // ((equal option "value") x) clauses on a single option and a final t; the one that applies is
  // the clause for the option's defcustom string, or the final t if it is nil, t or another string.
  const cond = (option: string, ...clauses: string[]) => theme(all({ bg: `(cond ${clauses.join("\n         ")})` }), option)
  const THREE = ['((equal doom-x-v "hard") c2)', '((equal doom-x-v "soft") (doom-darken c2 0.25))', "(t '(\"#282828\" nil nil))"]

  it("cond: the defcustom string's clause, or the final t if it is nil, t or another string", () => {
    expect(colors("doom-x", cond('(defcustom doom-x-v nil "")', ...THREE)).bg).toBe("#282828")
    expect(colors("doom-x", cond('(defcustom doom-x-v t "")', ...THREE)).bg).toBe("#282828")
    expect(colors("doom-x", cond('(defcustom doom-x-v "hard" "")', ...THREE)).bg).toBe("#ffffff")
    expect(colors("doom-x", cond('(defcustom doom-x-v "soft" "")', ...THREE)).bg).toBe("#bfbfbf")
    expect(colors("doom-x", cond('(defcustom doom-x-v "other" "")', ...THREE)).bg).toBe("#282828")
    // As in Doom: with comments between the clauses.
    expect(colors("doom-x", cond('(defcustom doom-x-v nil "")', `${THREE[0]} ; bg0_h\n`, `${THREE[1]} ; bg0_s\n`, THREE[2]!)).bg).toBe("#282828")
  })

  it.each([
    ["without the final t", ['((equal doom-x-v "hard") c2)', '((equal doom-x-v "soft") c1)'], /cond ends with a \(t value\) clause/],
    ["the t is not the last", ["(t c1)", '((equal doom-x-v "hard") c2)'], /a cond clause is .*, or the final t/],
    ["only the t", ["(t c1)"], /cond is/],
    ["a condition other than equal", ['((eq doom-x-v "hard") c2)', "(t c1)"], /a cond clause is/],
    ["equal with its arguments reversed", ['((equal "hard" doom-x-v) c2)', "(t c1)"], /a cond clause is/],
    ["equal with a symbol, not a string", ["((equal doom-x-v hard) c2)", "(t c1)"], /a cond clause is/],
    ["equal between two strings", ['((equal "hard" "soft") c2)', "(t c1)"], /a cond clause is/],
    ["equal with three arguments", ['((equal doom-x-v "hard" "soft") c2)', "(t c1)"], /a cond clause is/],
    ["a condition that is a bare option", ["(doom-x-v c2)", "(t c1)"], /a cond clause is/],
    ["a clause with two values", ['((equal doom-x-v "hard") c1 c2)', "(t c1)"], /each cond clause is/],
    ["two options", ['((equal doom-x-v "hard") c2)', '((equal doom-x-w "soft") c2)', "(t c1)"], /the cond clauses test more than one option/],
  ])("cond: %s fails, naming the theme and the variable", (_q, clauses, error) => {
    const extra = '(defcustom doom-x-v nil "")\n(defcustom doom-x-w nil "")'
    expect(() => colors("doom-x", cond(extra, ...(clauses as string[])))).toThrow(new RegExp(`doom-x\\.bg: ${(error as RegExp).source}`))
  })

  it("cond: an option without a defcustom, or with a default that is not nil, t or a string, fails", () => {
    expect(() => colors("doom-x", cond("", ...THREE))).toThrow(/doom-x\.bg: the option doom-x-v has no nil, t or string defcustom/)
    expect(() => colors("doom-x", cond('(defcustom doom-x-v other-option "")', ...THREE))).toThrow(/doom-x\.bg: the option doom-x-v/)
    expect(() => colors("doom-x", cond('(defcustom doom-x-v 3 "")', ...THREE))).toThrow(/doom-x\.bg: the option doom-x-v/)
  })

  it("an unknown form, a name with no binding or a missing variable fail, naming the theme and the variable", () => {
    expect(() => colors("doom-x", theme(all({ red: "(doom-saturate c1 0.5)" })))).toThrow(/doom-x\.red: unknown function/)
    expect(() => colors("doom-x", theme(all({ red: "nothing" })))).toThrow(/doom-x\.red: name with no earlier binding/)
    expect(() => colors("doom-x", theme(all({ red: "(if doom-x-none c1 c2)" })))).toThrow(/doom-x\.red: the option/)
    expect(() => colors("doom-x", theme(all({}).replace(/\(cyan c1\)/, "")))).toThrow(/doom-x\.cyan: missing/)
    expect(() => colors("doom-y", theme(all({})))).toThrow(/there is no \(def-doom-theme doom-y/)
  })

})
