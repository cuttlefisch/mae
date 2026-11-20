// SPDX-License-Identifier: GPL-3.0-or-later
// Extracts the colours of the Doom Emacs themes (github.com/doomemacs/themes, MIT)
// that data/doom.json names («palettes» and «emphasisOnly») and writes them into the
// same file («themes»), with their provenance. What it writes is what
// scripts/from-doom.mjs reads.
//
//   node scripts/extract-doom.mjs <clone>/themes <commit>
//
// The clone is external: only its themes/*.el are read, as text. Nothing of it is
// loaded or run. This program reads def-doom-theme's bindings with a minimal
// expression reader and understands only these forms, which is what the chosen
// themes use for their colours:
//   '("#rrggbb" …)                the GUI value: the first element
//   other-name                     an earlier binding (as in let*)
//   (doom-blend a b x)             x·a + (1−x)·b, channel by channel in sRGB
//   (doom-darken a x)              (doom-blend a "#000000" (- 1 x))
//   (doom-lighten a x)             (doom-blend a "#ffffff" (- 1 x))
//   (if option a b)                by the default value of its defcustom
//   (cond ((equal option "v") a)   the clause whose string is the option's default
//         …                        (its defcustom: nil, t or a string), or the final
//         (t b))                   t; every clause tests the same option (since
//                                  2026-11-16, for doom-gruvbox and
//                                  doom-gruvbox-light: their bg and bg-alt)
//   `(,(car a) …) and (car a)      a
// Any other form is an error naming the theme and the variable.
import { readFileSync, writeFileSync } from "node:fs"
import path from "node:path"

export const VARIABLES = [
  "bg", "bg-alt", "base0", "base1", "base2", "base3", "base4", "base5", "base6", "base7", "base8", "fg", "fg-alt",
  "red", "orange", "green", "teal", "yellow", "blue", "dark-blue", "magenta", "violet", "cyan", "dark-cyan",
  // The "universal syntax classes" every Doom theme must define (doom-themes-base
  // uses them): each palette's `code` roles (scripts/doom.mjs).
  "comments", "strings", "keywords", "numbers", "constants", "variables", "functions", "type", "operators",
]

/** Reads an expression from text[i]; returns [expression, next index]. */
function read(text, i) {
  const skip = () => {
    for (;;) {
      while (i < text.length && /\s/.test(text[i])) i++
      if (text[i] === ";") {
        while (i < text.length && text[i] !== "\n") i++
        continue
      }
      return
    }
  }
  skip()
  const c = text[i]
  if (c === undefined) throw new Error("end of text inside an expression")
  if (c === "(") {
    const list = []
    i++
    for (;;) {
      skip()
      if (text[i] === ")") return [list, i + 1]
      const [e, j] = read(text, i)
      list.push(e)
      i = j
    }
  }
  if (c === ")") throw new Error(`unexpected closing parenthesis at ${i}`)
  if (c === "'" || c === "`" || c === ",") {
    const [e, j] = read(text, i + 1)
    return [{ prefix: c, e }, j]
  }
  if (c === '"') {
    let s = ""
    i++
    while (text[i] !== '"') {
      if (text[i] === "\\") i++
      if (i >= text.length) throw new Error("unterminated string")
      s += text[i++]
    }
    return [{ string: s }, i + 1]
  }
  let j = i
  while (j < text.length && !/[\s();"]/.test(text[j])) j++
  if (j === i) throw new Error(`unexpected character ${JSON.stringify(c)} at ${i}`)
  return [text.slice(i, j), j]
}

const rgb = (hex) => [1, 3, 5].map((k) => parseInt(hex.slice(k, k + 2), 16))
const hex = (c) => `#${c.map((x) => Math.round(x).toString(16).padStart(2, "0")).join("")}`
const mix = (a, b, x) => hex(rgb(a).map((v, k) => x * v + (1 - x) * rgb(b)[k]))

/**
 * A theme's colours, from the text of its .el file.
 * @param {string} name
 * @param {string} text
 * @returns {Record<string, string>}
 */
export function colors(name, text) {
  const defcustom = new Map(
    [...text.matchAll(/\(defcustom\s+([^\s()]+)\s+([^\s()]+)/g)].map((m) => [m[1], m[2]]),
  )
  const start = text.indexOf(`(def-doom-theme ${name}`)
  if (start < 0) throw new Error(`${name}: there is no (def-doom-theme ${name}`)
  // (def-doom-theme name "doc" :key value … (bindings) …): the bindings are the
  // first list after the name, the string and the keys.
  let i = start + 1
  let e
  ;[e, i] = read(text, i) // def-doom-theme
  ;[e, i] = read(text, i) // the name
  for (;;) {
    [e, i] = read(text, i)
    if (typeof e === "string" && e.startsWith(":")) {
      [e, i] = read(text, i)
      continue
    }
    if (Array.isArray(e)) break
  }
  const ligatures = new Map()
  const evaluate = (x, where) => {
    const fails = (por) => {
      throw new Error(`${name}.${where}: ${por}: ${JSON.stringify(x)}`)
    }
    if (typeof x === "string") {
      if (!ligatures.has(x)) fails("name with no earlier binding")
      return ligatures.get(x)
    }
    if (x.prefix === "'" && Array.isArray(x.e)) {
      const s = x.e[0]?.string
      if (!s || !/^#[0-9a-fA-F]{6}$/.test(s)) fails("the GUI value is not #rrggbb")
      return s.toLowerCase()
    }
    if (x.prefix === "`" && Array.isArray(x.e) && x.e[0]?.prefix === ",") return evaluate(x.e[0].e, where)
    if (!Array.isArray(x)) fails("unknown form")
    const [f, ...args] = x
    const num = (n) => {
      if (typeof n !== "string" || !/^-?[0-9.]+$/.test(n)) fails("a number was expected")
      return Number(n)
    }
    switch (f) {
      case "car":
        return evaluate(args[0], where)
      case "doom-blend":
        return mix(evaluate(args[0], where), evaluate(args[1], where), num(args[2]))
      case "doom-darken":
        return mix(evaluate(args[0], where), "#000000", 1 - num(args[1]))
      case "doom-lighten":
        return mix(evaluate(args[0], where), "#ffffff", 1 - num(args[1]))
      case "if": {
        const v = defcustom.get(args[0])
        if (v !== "nil" && v !== "t") fails(`the option ${args[0]} has no nil or t defcustom`)
        return evaluate(v === "t" ? args[1] : args[2], where)
      }
      case "cond": {
        // Only (cond ((equal option "v") a) … (t b)): any other form of cond fails.
        if (args.length < 2) fails("cond is ((equal option \"value\") …) … (t …)")
        const options = new Set()
        const clauses = args.map((c, k) => {
          if (!Array.isArray(c) || c.length !== 2) fails("each cond clause is (condition value)")
          const [p, v] = c
          if (k === args.length - 1) {
            if (p !== "t") fails("cond ends with a (t value) clause")
            return [undefined, v]
          }
          if (!Array.isArray(p) || p.length !== 3 || p[0] !== "equal" || typeof p[1] !== "string" || typeof p[2]?.string !== "string")
            fails("a cond clause is ((equal option \"value\") value), or the final t")
          options.add(p[1])
          return [p[2].string, v]
        })
        if (options.size !== 1) fails("the cond clauses test more than one option")
        const [option] = options
        const v = defcustom.get(option)
        const string = /^"[^"\\]*"$/.test(v ?? "") ? v.slice(1, -1) : undefined
        if (v !== "nil" && v !== "t" && string === undefined)
          fails(`the option ${option} has no nil, t or string defcustom`)
        const last = clauses.length - 1
        return evaluate(clauses.find(([s], k) => k === last || (string !== undefined && s === string))[1], where)
      }
      default:
        return fails("unknown function")
    }
  }
  for (const l of e) {
    if (!Array.isArray(l) || typeof l[0] !== "string") continue
    try {
      ligatures.set(l[0], evaluate(l[1], l[0]))
    } catch (err) {
      // A binding that is not a colour (a face, a list of options) does not matter
      // as long as none of VARIABLES uses it: then it fails there.
      ligatures.set(l[0], undefined)
      if (VARIABLES.includes(l[0])) throw err
    }
  }
  /** @type {Record<string, string>} */
  const output = {}
  for (const v of VARIABLES) {
    const c = ligatures.get(v)
    if (c === undefined) throw new Error(`${name}.${v}: missing, or not a colour`)
    output[v] = c
  }
  return output
}

const [themes, commit] = process.argv.slice(2)
if (import.meta.url === `file://${process.argv[1]}`) {
  if (!themes || !/^[0-9a-f]{40}$/.test(commit ?? "")) {
    console.error("usage: node scripts/extract-doom.mjs <clone>/themes <40-character commit>")
    process.exit(2)
  }
  const file = path.join(import.meta.dirname, "..", "data", "doom.json")
  const data = JSON.parse(readFileSync(file, "utf8"))
  // The themes of the Doom palettes, and those of the hand-written palettes that take
  // only their emphasis swatches from Doom («emphasisOnly»).
  const lists = [data.palettes, data.emphasisOnly ?? {}]
  const names = [...new Set(lists.flatMap((l) => Object.values(l).flatMap((p) => Object.values(p))))].sort()
  const output = {
    provenance: {
      repository: "https://github.com/doomemacs/themes",
      commit,
      files: names.map((n) => `themes/${n}-theme.el`),
      licence: "MIT",
      copyright: "Copyright (c) 2016-2026 Henrik Lissner.",
      note: "The GUI value of each variable, extracted by scripts/extract-doom.mjs; the full licence is in data/PROVENANCE.",
    },
    palettes: data.palettes,
    ...(data.emphasisOnly && { emphasisOnly: data.emphasisOnly }),
    themes: Object.fromEntries(
      names.map((n) => [n, colors(n, readFileSync(path.join(themes, `${n}-theme.el`), "utf8"))]),
    ),
  }
  writeFileSync(file, JSON.stringify(output, null, 2) + "\n")
  console.log(`${names.length} themes in ${path.relative(process.cwd(), file)}`
)
}
