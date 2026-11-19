// SPDX-License-Identifier: GPL-3.0-or-later
// The shape of palette files and the resolution of references: every role present,
// none extra, every reference leads to a role and there are no cycles. On a minimal
// palette built here (every role one colour) that each test changes one value of.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import { stringify } from "yaml"
import {
  PaletteError,
  GROUPS,
  MODES,
  loadPalettes,
  checkSet,
  resolver,
  resolveText,
  type Palette,
} from "../src/tokens.ts"

type Source = Record<string, unknown> & Record<"light" | "dark", Record<string, Record<string, string>>>

function minimum(): Source {
  const mode = () =>
    Object.fromEntries(
      Object.entries(GROUPS).map(([g, ps]) => [g, Object.fromEntries(ps.map((p) => [p, "#123456"]))]),
    )
  return { light: mode(), dark: mode() }
}

const fails = (f: () => unknown, message: RegExp) => expect(f).toThrow(message)

describe("references", () => {
  it("a same-mode reference takes that role's value", () => {
    const f = minimum()
    f.light.ui!.foreground = "#242424"
    f.light.diagram!.ink = "{ui.foreground}"
    f.dark.ui!.foreground = "#e6e6e6"
    f.dark.diagram!.ink = "{ui.foreground}"
    const p = resolver("x", f)
    expect(p.light.diagram.ink).toBe("#242424")
    expect(p.dark.diagram.ink).toBe("#e6e6e6")
  })

  it("a reference with a mode takes that mode's value", () => {
    const f = minimum()
    f.light.ui!.background = "#fbf1c7"
    f.dark.ui!.muted = "{light.ui.background}"
    expect(resolver("x", f).dark.ui.muted).toBe("#fbf1c7")
  })

  it("chains of references resolve down to the colour", () => {
    const f = minimum()
    f.light.ui!.link = "#0b5cad"
    f.light.code!.number = "{ui.link}"
    f.light.code!.constant = "{code.number}"
    f.light.code!.operator = "{code.constant}"
    expect(resolver("x", f).light.code.operator).toBe("#0b5cad")
  })

  it("a role naming itself is a cycle", () => {
    const f = minimum()
    f.light.ui!.muted = "{ui.muted}"
    fails(() => resolver("x", f), /x\.light\.ui\.muted: circular reference: light\.ui\.muted -> light\.ui\.muted/)
  })

  it("a long cycle, across modes too, is detected and named in full", () => {
    const f = minimum()
    f.light.ui!.muted = "{diagram.border}"
    f.light.diagram!.border = "{dark.graph.ink}"
    f.dark.graph!.ink = "{light.ui.muted}"
    fails(
      () => resolver("x", f),
      /circular reference: light\.ui\.muted -> light\.diagram\.border -> dark\.graph\.ink -> light\.ui\.muted/,
    )
  })

  it("a reference to a hyphenated role (diagram.node-soft) resolves, with a mode too", () => {
    const f = minimum()
    f.light.diagram!["node-soft"] = "#fafafa"
    f.light.diagram!["zone-grey"] = "{diagram.node-soft}"
    f.dark.diagram!["zone-grey"] = "{light.diagram.node-soft}"
    const p = resolver("x", f)
    expect(p.light.diagram["zone-grey"]).toBe("#fafafa")
    expect(p.dark.diagram["zone-grey"]).toBe("#fafafa")
    expect(p.light.diagram["node-soft"]).toBe("#fafafa")
  })

  it("a reference to a role or group that does not exist fails", () => {
    for (const bad of ["{ui.nothing}", "{nothing.background}", "{light.nothing.background}", "{dark.ui.nothing}"]) {
      const f = minimum()
      f.light.diagram!.ink = bad
      fails(() => resolver("x", f), /x\.light\.diagram\.ink: .* leads to no role/)
    }
  })

  it("a value that is neither a lowercase colour nor a reference fails", () => {
    for (const bad of ["#FFFFFF", "#fff", "red", "ui.background", "{ui.background", "{middle.ui.background}", "#12345678 "]) {
      const f = minimum()
      f.dark.ui!.background = bad
      fails(() => resolver("x", f), /x\.dark\.ui\.background: .* is neither a lowercase/)
    }
  })
})

describe("shape", () => {
  it("the minimal palette is valid", () => {
    expect(resolver("x", minimum()).name).toBe("x")
  })

  it("a missing role, group or mode", () => {
    let f = minimum()
    delete f.dark.graph!.edge
    fails(() => resolver("x", f), /x\.dark\.graph\.edge: the role is missing/)
    f = minimum()
    delete f.light.gallery
    fails(() => resolver("x", f), /x\.light\.gallery: the group is missing/)
    f = minimum()
    delete (f as Partial<Source>).dark
    fails(() => resolver("x", f), /x\.dark: the mode is missing/)
  })

  it("an extra role, group or key (a typo) fails instead of being ignored", () => {
    let f = minimum()
    f.light.ui!.backgruond = "#ffffff"
    fails(() => resolver("x", f), /x\.light\.ui\.backgruond: unknown role/)
    f = minimum()
    f.light.extra = {}
    fails(() => resolver("x", f), /x\.light\.extra: unknown group/)
    f = minimum()
    f.colors = {}
    fails(() => resolver("x", f), /x\.colors: unknown key/)
  })

  it("isDefault is a boolean", () => {
    const f = minimum()
    f.isDefault = "yes"
    fails(() => resolver("x", f), /x\.isDefault/)
  })

  it("a palette's name goes into a CSS selector and a file name: only [a-z][a-z0-9-]*", () => {
    for (const bad of ['a"]', "A", "1a", "a b", "a/b", ""]) fails(() => resolver(bad, minimum()), /a palette's name/)
  })

  it("the gallery title is the same in both modes", () => {
    const f = minimum()
    f.dark.gallery!.title = "#000000"
    fails(() => resolver("x", f), /x\.dark\.gallery\.title: must be the same colour as in light/)
  })

  it("a repeated key in the YAML fails (it would lose a value)", () => {
    const text = stringify(minimum()).replace("  ui:\n", '  ui:\n    background: "#000000"\n')
    fails(() => resolveText("x", text), /unique|duplicate/i)
  })

  it("Object.prototype's names are not groups and lead to no role", () => {
    for (const g of ["constructor", "toString", "hasOwnProperty", "__proto__"]) {
      // JSON.parse creates "__proto__" as an own key, as a YAML would.
      const f = JSON.parse(JSON.stringify(minimum()))
      Object.defineProperty(f.light, g, { value: {}, enumerable: true, configurable: true, writable: true })
      fails(() => resolver("x", f), new RegExp(`x\\.light\\.${g}: unknown group`))
    }
    for (const ref of ["{constructor.name}", "{toString.length}", "{hasOwnProperty.name}", "{light.constructor.name}"]) {
      const f = minimum()
      f.light.diagram!.ink = ref
      expect(() => resolver("x", f), ref).toThrow(PaletteError)
      fails(() => resolver("x", f), /x\.light\.diagram\.ink: .* leads to no role/)
    }
  })

  it("an unquoted reference (YAML reads it as a map) says the quotes are missing", () => {
    const text = stringify(minimum()).replace('    ink: "#123456"', "    ink: {ui.foreground}")
    expect(text).toContain("ink: {ui.foreground}")
    fails(() => resolveText("x", text), /x\.light\.diagram\.ink: is a map: is the reference missing its quotes\?/)
  })

  it("the errors are PaletteError", () => {
    expect(() => resolver("x", 3)).toThrow(PaletteError)
  })
})

describe("emphasis", () => {
  // A minimal palette with two swatches; the default, b, is its roles' one.
  const withEmphasis = () => {
    const f = minimum()
    const hues = (x: string) => ({ graphic: x, text: x, graph: x })
    f.emphasis = {
      defaultId: "b",
      swatches: [
        { id: "a", name: "a", light: hues("#aaaaaa"), dark: hues("#aaaaaa") },
        { id: "b", name: "b", light: hues("#123456"), dark: hues("#123456") },
      ],
    }
    return f as Source & { emphasis: { defaultId: string; swatches: Record<string, unknown>[] } }
  }

  it("without emphasis the palette has no swatches (null); with them, in their order and with the default", () => {
    expect(resolver("x", minimum()).emphasis).toBeNull()
    const e = resolver("x", withEmphasis()).emphasis!
    expect(e.defaultId).toBe("b")
    expect(e.swatches.map((m) => m.id)).toEqual(["a", "b"])
    expect(e.swatches[1]!.dark).toEqual({ graphic: "#123456", text: "#123456", graph: "#123456" })
  })

  it("the default's tones are ui.accent (text), diagram.highlight (graphic) and graph.accent (graph)", () => {
    for (const [hue, role] of [
      ["text", "ui.accent"],
      ["graphic", "diagram.highlight"],
      ["graph", "graph.accent"],
    ] as const) {
      const f = withEmphasis()
      ;(f.emphasis.swatches[1]!.dark as Record<string, string>)[hue] = "#654321"
      fails(() => resolver("x", f), new RegExp(`x\\.emphasis\\.defaultId: dark\\.${role.replace(".", "\\.")} \\(#123456\\) must be the ${hue} tone of b`))
    }
  })

  it("the default is one of the swatches; ids distinct; tones opaque; not one key more or fewer", () => {
    let f = withEmphasis()
    f.emphasis.defaultId = "c"
    fails(() => resolver("x", f), /x\.emphasis\.defaultId: is the id of one of the swatches \(a, b\)/)
    f = withEmphasis()
    f.emphasis.swatches[0]!.id = "b"
    fails(() => resolver("x", f), /x\.emphasis\.swatches\[1\]\.id: repeated: b/)
    f = withEmphasis()
    ;(f.emphasis.swatches[0]!.light as Record<string, string>).text = "#aaaaaa80"
    fails(() => resolver("x", f), /x\.emphasis\.swatches\[0\]\.light\.text: is an opaque/)
    f = withEmphasis()
    delete (f.emphasis.swatches[0]!.light as Record<string, string>).graph
    fails(() => resolver("x", f), /x\.emphasis\.swatches\[0\]\.light\.graph: is missing/)
    f = withEmphasis()
    f.emphasis.swatches[0]!.color = "#aaaaaa"
    fails(() => resolver("x", f), /x\.emphasis\.swatches\[0\]\.color: unknown key/)
    f = withEmphasis()
    f.emphasis.swatches = []
    fails(() => resolver("x", f), /x\.emphasis\.swatches: is a list with at least one swatch/)
    f = withEmphasis()
    f.emphasis.swatches[0]!.id = "Green"
    fails(() => resolver("x", f), /x\.emphasis\.swatches\[0\]\.id: is a name/)
  })
})

describe("the set of palettes", () => {
  const p = (name: string, isDefault: boolean): Palette => ({ ...resolver(name, minimum()), isDefault })

  it("exactly one default", () => {
    fails(() => checkSet([p("a", false), p("b", false)]), /exactly one default palette; there are 0/)
    fails(() => checkSet([p("a", true), p("b", true)]), /there are 2 \(a, b\)/)
    expect(checkSet([p("b", false), p("a", true)]).map((x) => x.name)).toEqual(["a", "b"])
  })

  it("in the directory, each palette is a .yaml and there is nothing else", () => {
    // tmpdir() is $TMPDIR if set.
    const dir = mkdtempSync(path.join(tmpdir(), "palettes-"))
    try {
      writeFileSync(path.join(dir, "a.yaml"), stringify({ ...minimum(), isDefault: true }))
      expect(loadPalettes(dir).map((x) => x.name)).toEqual(["a"])
      writeFileSync(path.join(dir, "b.yml"), stringify(minimum()))
      fails(() => loadPalettes(dir), /b\.yml: a palette is a \.yaml file/)
      rmSync(path.join(dir, "b.yml"))
      mkdirSync(path.join(dir, "sub"))
      fails(() => loadPalettes(dir), /sub: a palette is a \.yaml file/)
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })

  it("the repository's palettes: neutral is the default, and every role of every mode has a colour", () => {
    const all = loadPalettes(path.join(import.meta.dirname, "..", "palettes"))
    expect(all.map((x) => [x.name, x.isDefault])).toEqual([
      ["acario", false],
      ["ayu", false],
      ["bluloco", false],
      ["challenger-deep", false],
      ["dark-plus", false],
      ["dracula", false],
      ["earl-grey", false],
      ["flatwhite", false],
      ["gruvbox", false],
      ["henna", false],
      ["horizon", false],
      ["material", false],
      ["monokai", false],
      ["neutral", true],
      ["nord", false],
      ["nova", false],
      ["oceanic-next", false],
      ["one", false],
      ["peacock", false],
      ["rouge", false],
      ["solarized", false],
      ["sourcerer", false],
      ["spacegrey", false],
      ["tokyo-night", false],
      ["tomorrow", false],
      ["vibrant", false],
      ["xcode", false],
      ["zenburn", false],
    ])
    for (const x of all)
      for (const m of MODES)
        for (const [g, ps] of Object.entries(GROUPS))
          for (const role of ps)
            expect((x[m] as Record<string, Record<string, string>>)[g]![role], `${x.name}.${m}.${g}.${role}`).toMatch(
              /^#[0-9a-f]{6}([0-9a-f]{2})?$/,
            )
  })
})
