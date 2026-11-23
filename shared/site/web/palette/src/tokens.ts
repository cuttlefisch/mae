// SPDX-License-Identifier: GPL-3.0-or-later
// The colour tokens: which roles exist, how they are written in palettes/<name>.yaml
// and how references between them are resolved.
//
// One file per palette. Each mode (light = light, dark = dark) has the same groups
// and, in each group, the same roles, all required. A value is a lowercase "#rrggbb"
// or "#rrggbbaa" colour, or a reference to another role:
//   "{group.role}"          the role in the same mode;
//   "{light.group.role}"    the role in that mode (also "{dark.…}").
// So each colour is written once: the diagrams' ink is "{ui.foreground}", and
// changes with it.
import { readdirSync, readFileSync } from "node:fs"
import path from "node:path"
import { parse as parseYaml } from "yaml"
import { HEX } from "./color.ts"

export const MODES = ["light", "dark"] as const
export type Mode = (typeof MODES)[number]

// The order of each list is the output order (CSS, JSON): it does not depend on the
// order in which the palette files are written.
export const GROUPS = {
  // The page: the engine's own minimal role set, chosen from what contrast checking
  // needs (owner, 2026-11-19). ADR-119 D17 owns the full token set; this is only what
  // the checks and the emphasis rules use:
  // - background and panel: the two surfaces text sits on (the page, and raised panels
  //   such as inline code);
  // - foreground (body text), muted (secondary text: dates, tags, listings), link, and
  //   accent (emphasised text, the emphasis "text" tone): text colours;
  // - mark: the translucent background of marked text.
  ui: ["background", "panel", "foreground", "muted", "link", "accent", "mark"],
  // The title of gallery cards, on their fixed dark scrim over an image.
  gallery: ["title"],
  // Graphviz diagrams: text, background and outlines; and the roles of the
  // documentation maps (nodes, lines and zones).
  diagram: [
    "ink",
    "background",
    "border",
    "node",
    "node-soft",
    "node-control",
    "line",
    "outline",
    "highlight",
    "blocked",
    "zone-blue",
    "zone-green",
    "zone-amber",
    "zone-orange",
    "zone-violet",
    "zone-grey",
    "zone-brown",
  ],
  // The chord graph, with the names of its roles.
  graph: ["ink", "accent", "neighbour", "visited", "edge"],
  // Highlighted code blocks: the block's text and background, and the colour of each
  // class of token (org-code-<role>).
  code: [
    "text",
    "background",
    "comment",
    "string",
    "number",
    "constant",
    "keyword",
    "operator",
    "function",
    "type",
    "variable",
  ],
} as const

export type Group = keyof typeof GROUPS
export type Role<G extends Group = Group> = (typeof GROUPS)[G][number]
export type Values = { [G in Group]: Record<Role<G>, string> }

/** The three tones of an emphasis swatch in one mode. */
export interface EmphasisHues {
  /** Graphic: 3:1 on background and panel. */
  graphic: string
  /** Text: 4.5:1 on background and panel. */
  text: string
  /** The graph's accent: the graphic tone, inside chord-graph's band. */
  graph: string
}

/** An emphasis swatch: the Doom colour it comes from (id), its name and its tones. */
export interface Swatch {
  id: string
  name: string
  light: EmphasisHues
  dark: EmphasisHues
}

/** A palette's emphasis swatches, in the order they are offered, and the default one. */
export interface Emphasis {
  defaultId: string
  swatches: Swatch[]
}

/**
 * Which role each tone of a swatch carries. The default swatch's tones are the
 * palette's own (the resolver requires it); with data-emphasis="<id>", palette.css
 * swaps those variables for that swatch's tones. Nothing else follows the emphasis:
 * not the links, not the diagrams' blocking colour, not the code, not the graph's
 * neighbour.
 */
export const EMPHASIS_ROLES = {
  text: [["ui", "accent"]],
  graphic: [["diagram", "highlight"]],
  graph: [["graph", "accent"]],
} as const satisfies Record<keyof EmphasisHues, readonly (readonly [Group, string])[]>

/** The <html> attribute that chooses the emphasis swatch. */
export const EMPHASIS_ATTRIBUTE = "data-emphasis"

/**
 * The <html> attribute and value that select dark mode (engine-owned). Without it,
 * light mode applies.
 */
export const MODE_ATTRIBUTE = "data-mode"
export const DARK_VALUE = "dark"

export interface Palette {
  name: string
  isDefault: boolean
  light: Values
  dark: Values
  /**
   * The references as written, with their mode made explicit: in each mode,
   * "group.role" -> "mode.group.role" of the role it points at (directly, without
   * following the chain). Roles with their own colour are absent.
   */
  references: Record<Mode, Record<string, string>>
  /** The emphasis swatches; null if the palette has none (hand-written ones and nova). */
  emphasis: Emphasis | null
}

const NAME = /^[a-z][a-z0-9-]*$/
const REFERENCE = /^\{(?:(light|dark)\.)?([A-Za-z]+)\.([A-Za-z][A-Za-z0-9-]*)\}$/
const KEYS = ["isDefault", "light", "dark", "emphasis"]
const EMPHASIS_ID = /^[a-z][a-z-]*$/
const OPAQUE = /^#[0-9a-f]{6}$/

export class PaletteError extends Error {}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v)
}

/**
 * Checks the shape of a palette file already parsed, and resolves its references.
 * Fails with the value's path (`neutral.dark.ui.muted`) if a role is missing or
 * extra, if a value is neither a colour nor a reference, if a reference leads to no
 * role, or if there is a cycle.
 */
export function resolver(name: string, source: unknown): Palette {
  const failure = (where: string, what: string) => new PaletteError(`${where}: ${what}`)
  if (!NAME.test(name)) throw failure(name, "a palette's name is [a-z][a-z0-9-]*")
  if (!isObject(source)) throw failure(name, "the file is not a map")
  for (const k of Object.keys(source))
    if (!KEYS.includes(k)) throw failure(`${name}.${k}`, `unknown key (valid: ${KEYS.join(", ")})`)
  const { isDefault = false } = source
  if (typeof isDefault !== "boolean") throw failure(`${name}.isDefault`, "is true or false")

  // The shape: every mode, every group and every role, not one more or one fewer.
  const raw: Record<string, Record<string, Record<string, string>>> = {}
  for (const mode of MODES) {
    const m = source[mode]
    if (!isObject(m)) throw failure(`${name}.${mode}`, "the mode is missing")
    for (const g of Object.keys(m))
      if (!Object.hasOwn(GROUPS, g)) throw failure(`${name}.${mode}.${g}`, "unknown group")
    raw[mode] = {}
    for (const [group, roles] of Object.entries(GROUPS)) {
      const gr = m[group]
      if (!isObject(gr)) throw failure(`${name}.${mode}.${group}`, "the group is missing")
      for (const p of Object.keys(gr))
        if (!(roles as readonly string[]).includes(p))
          throw failure(`${name}.${mode}.${group}.${p}`, `unknown role (valid: ${roles.join(", ")})`)
      raw[mode][group] = {}
      for (const p of roles) {
        const v = gr[p]
        // Unquoted, YAML reads {ui.background} as a map: { "ui.background": null }.
        if (isObject(v))
          throw failure(`${name}.${mode}.${group}.${p}`, "is a map: is the reference missing its quotes?")
        if (typeof v !== "string") throw failure(`${name}.${mode}.${group}.${p}`, "the role is missing")
        raw[mode][group][p] = v
      }
    }
  }

  // The references, with cycle detection: `path` is the chain being followed.
  const done = new Map<string, string>()
  const references: Record<Mode, Record<string, string>> = { light: {}, dark: {} }
  const value = (mode: string, group: string, role: string, path: string[]): string => {
    const key = `${mode}.${group}.${role}`
    const ya = done.get(key)
    if (ya) return ya
    const where = `${name}.${key}`
    if (path.includes(key))
      throw failure(where, `circular reference: ${[...path, key].join(" -> ")}`)
    const v = raw[mode]![group]![role]!
    let r: string
    if (HEX.test(v)) {
      r = v
    } else {
      const m = REFERENCE.exec(v)
      if (!m) throw failure(where, `${JSON.stringify(v)} is neither a lowercase #rrggbb[aa] colour nor a {group.role} reference`)
      const [, aMode = mode, toGroup, toRole] = m as unknown as [string, string | undefined, string, string]
      // Object.hasOwn: "constructor", "toString" or "__proto__" are not groups.
      if (!Object.hasOwn(GROUPS, toGroup) || !(GROUPS[toGroup as Group] as readonly string[]).includes(toRole))
        throw failure(where, `${v} leads to no role`)
      references[mode as Mode][`${group}.${role}`] = `${aMode}.${toGroup}.${toRole}`
      r = value(aMode, toGroup, toRole, [...path, key])
    }
    done.set(key, r)
    return r
  }

  const resolved = {} as Record<Mode, Values>
  for (const mode of MODES) {
    const vs = {} as Record<string, Record<string, string>>
    for (const [group, roles] of Object.entries(GROUPS)) {
      vs[group] = {}
      for (const p of roles) vs[group][p] = value(mode, group, p, [])
    }
    resolved[mode] = vs as Values
  }

  // The roles that have the same value in both modes (the gallery title sits on a
  // fixed dark scrim).
  for (const g of SAME_IN_BOTH_MODES)
    for (const p of GROUPS[g] as readonly string[]) {
      const [c, o] = MODES.map((m) => (resolved[m][g] as Record<string, string>)[p])
      if (c !== o)
        throw failure(`${name}.dark.${g}.${p}`, `must be the same colour as in light (write {light.${g}.${p}})`)
    }

  const emphasis = source.emphasis === undefined ? null : readEmphasis(name, source.emphasis, resolved)
  return { name, isDefault, ...resolved, references, emphasis }
}

/**
 * The emphasis swatches: the shape (each swatch with an id, a name and its three
 * opaque tones in each mode; distinct ids; the default, one of them), and that the
 * default's tones are the roles they carry (EMPHASIS_ROLES).
 */
function readEmphasis(name: string, e: unknown, values: Record<Mode, Values>): Emphasis {
  const failure = (where: string, what: string) => new PaletteError(`${name}.emphasis${where}: ${what}`)
  const exact = (o: unknown, keys: readonly string[], where: string): Record<string, unknown> => {
    if (!isObject(o)) throw failure(where, "is not a map")
    const ks = Object.keys(o)
    for (const k of ks) if (!keys.includes(k)) throw failure(`${where}.${k}`, `unknown key (valid: ${keys.join(", ")})`)
    for (const k of keys) if (!ks.includes(k)) throw failure(`${where}.${k}`, "is missing")
    return o
  }
  const { defaultId, swatches } = exact(e, ["defaultId", "swatches"], "")
  if (!Array.isArray(swatches) || swatches.length === 0) throw failure(".swatches", "is a list with at least one swatch")
  const ids = new Set<string>()
  const read = swatches.map((m, i): Swatch => {
    const where = `.swatches[${i}]`
    const o = exact(m, ["id", "name", ...MODES], where)
    if (typeof o.id !== "string" || !EMPHASIS_ID.test(o.id)) throw failure(`${where}.id`, "is a name [a-z][a-z-]*")
    if (ids.has(o.id)) throw failure(`${where}.id`, `repeated: ${o.id}`)
    ids.add(o.id)
    if (typeof o.name !== "string" || o.name.trim() === "") throw failure(`${where}.name`, "is missing")
    const hues = {} as Record<Mode, EmphasisHues>
    for (const mode of MODES) {
      const t = exact(o[mode], Object.keys(EMPHASIS_ROLES), `${where}.${mode}`)
      for (const [k, v] of Object.entries(t))
        if (typeof v !== "string" || !OPAQUE.test(v)) throw failure(`${where}.${mode}.${k}`, "is an opaque lowercase #rrggbb colour")
      // In EMPHASIS_ROLES's order, not the file's: the output does not depend on it.
      hues[mode] = Object.fromEntries(Object.keys(EMPHASIS_ROLES).map((k) => [k, t[k]])) as unknown as EmphasisHues
    }
    return { id: o.id, name: o.name, ...hues }
  })
  if (typeof defaultId !== "string" || !ids.has(defaultId))
    throw failure(".defaultId", `is the id of one of the swatches (${[...ids].join(", ")})`)
  const pred = read.find((m) => m.id === defaultId)!
  for (const mode of MODES)
    for (const [hue, roles] of Object.entries(EMPHASIS_ROLES) as [keyof EmphasisHues, readonly (readonly [Group, string])[]][])
      for (const [g, p] of roles) {
        const v = (values[mode][g] as Record<string, string>)[p]
        if (v !== pred[mode][hue])
          throw failure(`.defaultId`, `${mode}.${g}.${p} (${v}) must be the ${hue} tone of ${defaultId} (${pred[mode][hue]})`)
      }
  return { defaultId, swatches: read }
}

/** Reads and resolves a directory's palettes: one <name>.yaml per palette, one default. */
export function loadPalettes(dir: string): Palette[] {
  const files = readdirSync(dir).sort()
  const palettes: Palette[] = []
  for (const f of files) {
    if (!f.endsWith(".yaml")) throw new PaletteError(`${dir}/${f}: a palette is a .yaml file`)
    palettes.push(resolveText(f.slice(0, -".yaml".length), readFileSync(path.join(dir, f), "utf8")))
  }
  return checkSet(palettes)
}

export function resolveText(name: string, text: string): Palette {
  return resolver(name, parseYaml(text, { uniqueKeys: true }))
}

/** The set: at least one palette, distinct names, exactly one default. Sorted by name. */
export function checkSet(palettes: Palette[]): Palette[] {
  if (palettes.length === 0) throw new PaletteError("there are no palettes")
  const names = palettes.map((p) => p.name)
  if (new Set(names).size !== names.length) throw new PaletteError(`repeated names: ${names.join(", ")}`)
  const pred = palettes.filter((p) => p.isDefault).map((p) => p.name)
  if (pred.length !== 1)
    throw new PaletteError(`there must be exactly one default palette; there are ${pred.length} (${pred.join(", ")})`)
  return [...palettes].sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))
}

/**
 * Palettes that were published on MAE's site and then removed, each mapped to the
 * palette that replaces it (P-98): a reader whose stored choice names one gets its
 * successor, without its accent. Empty: no palette has been removed since the site
 * began storing choices (the four removed before the import, in the README, were
 * never offered here).
 */
export const SUCCESSORS: Readonly<Record<string, string>> = {}

/**
 * The successor map, checked against the set and sorted by removed name: a removed
 * name is a valid palette name that is no longer a palette, and its successor is one.
 */
export function checkSuccessors(
  palettes: Palette[],
  successors: Readonly<Record<string, string>>,
): Record<string, string> {
  const names = new Set(palettes.map((p) => p.name))
  const out: Record<string, string> = {}
  const sorted = Object.entries(successors).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
  for (const [removed, successor] of sorted) {
    const where = `successors.${removed}`
    if (!NAME.test(removed)) throw new PaletteError(`${where}: a palette's name is [a-z][a-z0-9-]*`)
    if (names.has(removed)) throw new PaletteError(`${where}: is still a palette`)
    if (!names.has(successor)) throw new PaletteError(`${where}: ${successor} is not a palette`)
    out[removed] = successor
  }
  return out
}

/**
 * The CSS variable name of a role in a mode. The graph's carry the mode in their
 * name (as chord-graph writes them), so they hold in either mode; the others change
 * value with the mode.
 */
export function variable(group: Group, role: string, mode: Mode): string {
  switch (group) {
    case "ui":
      return `--ui-${role}`
    case "gallery":
      return `--gallery-${role}`
    case "diagram":
      return `--diagram-${role}`
    case "graph":
      return `--chord-${role}-${mode === "light" ? "light" : "dark"}`
    case "code":
      return `--code-${role}`
  }
}

export const MODE_INDEPENDENT: readonly Group[] = ["graph"]

/**
 * The roles that have the same value in both modes (the gallery title sits on a
 * fixed dark scrim; the resolver requires it). In palette.css they are always written
 * as a colour: a var(--ui-background) would follow the page, which does change with
 * the mode.
 */
export const SAME_IN_BOTH_MODES: readonly Group[] = ["gallery"]
