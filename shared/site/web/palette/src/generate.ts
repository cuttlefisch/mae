// SPDX-License-Identifier: GPL-3.0-or-later
// The generators: from the resolved palettes to the files in dist/. Pure functions
// (palettes to text), with no dates or anything machine-dependent: the same input
// gives the same bytes.
import {
  EMPHASIS_ATTRIBUTE,
  MODE_ATTRIBUTE,
  EMPHASIS_ROLES,
  GROUPS,
  SAME_IN_BOTH_MODES,
  MODES,
  MODE_INDEPENDENT,
  DARK_VALUE,
  variable,
  type Group,
  type Mode,
  type Swatch,
  type Palette,
  type EmphasisHues,
} from "./tokens.ts"

const WARNING = "Generated from palettes/"

/** The selector suffix that selects dark mode on :root. */
export const DARK = `[${MODE_ATTRIBUTE}="${DARK_VALUE}"]`

/**
 * A palette's CSS variables in one mode, in GROUPS's order. With `references`, a
 * role that is a reference to another role of the same mode in the file is written as
 * `var(--<that role>)`, not as its colour: the diagrams' ink is `var(--ui-foreground)`
 * and follows whichever `--ui-foreground` wins on the page. Written as a colour:
 * references to the other mode, the graph's roles (their name carries the mode) and
 * those pointing at the graph, and the roles that are the same in both modes (the
 * gallery).
 */
export function variablesCss(
  p: Palette,
  mode: Mode,
  which: "all" | "del-mode" | "fixed",
  references = false,
): [string, string][] {
  const out: [string, string][] = []
  for (const [group, roles] of Object.entries(GROUPS) as [Group, readonly string[]][]) {
    const fixed = MODE_INDEPENDENT.includes(group)
    if ((which === "del-mode" && fixed) || (which === "fixed" && !fixed)) continue
    for (const role of roles) {
      if (fixed) {
        // The fixed ones carry the mode in their name: both modes' go together.
        for (const m of MODES) out.push([variable(group, role, m), valueFor(p, m, group, role)])
        continue
      }
      const v = valueFor(p, mode, group, role)
      out.push([variable(group, role, mode), (references && asVar(p, mode, group, role)) || v])
    }
  }
  return out
}

function asVar(p: Palette, mode: Mode, group: Group, role: string): string | undefined {
  if (SAME_IN_BOTH_MODES.includes(group)) return undefined
  const target = p.references[mode][`${group}.${role}`]
  if (!target) return undefined
  const [aMode, toGroup, toRole] = target.split(".") as [Mode, Group, string]
  if (aMode !== mode || MODE_INDEPENDENT.includes(toGroup)) return undefined
  return `var(${variable(toGroup, toRole, mode)})`
}

function valueFor(p: Palette, mode: Mode, group: Group, role: string): string {
  return (p[mode][group] as Record<string, string>)[role]!
}

const block = (selector: string, vars: [string, string][]) =>
  `${selector} {\n${vars.map(([k, v]) => `  ${k}: ${v};\n`).join("")}}\n`

/**
 * palette.css. The default palette in `:root` and `:root[data-mode="dark"]`; every
 * palette, the default included, in `:root[data-theme="<name>"]` and
 * `…[data-mode="dark"]`. Specificity: :root (0,1,0) < :root[…] (0,2,0) <
 * :root[…][…] (0,3,0). The palette-and-dark-mode block carries every variable that
 * changes with the mode, so it beats both (0,2,0) ones whatever the order; the fixed
 * ones (the graph's) are only in the blocks without a mode.
 *
 * Emphasis: each swatch that is not the default, in
 * `:root[data-theme="<name>"][data-emphasis="<id>"]` (0,3,0) and `…[data-mode="dark"]`
 * (0,4,0), with only the variables each tone carries (EMPHASIS_ROLES). The dark one
 * beats both (0,3,0) ones in any order; the light one beats the palette's light block
 * (0,2,0) and, in dark mode, loses to its own (0,4,0). The default swatch has no block:
 * it is plain `[data-theme]`, and an unknown data-emphasis leaves it. A swatch's blocks
 * also carry the roles that do not follow the emphasis but are written as a var() of
 * one that does (pinnedEmphasis).
 */
export function css(palettes: Palette[]): string {
  const pred = palettes.find((p) => p.isDefault)!
  const parts = [
    `/* ${WARNING}: do not edit by hand.\n` +
      `   Default: ${pred.name}. Another palette: data-theme="<name>" on <html>;\n` +
      `   dark mode: ${MODE_ATTRIBUTE}="${DARK_VALUE}";\n` +
      `   another emphasis swatch of that palette: ${EMPHASIS_ATTRIBUTE}="<id>" (tokens.json). */\n`,
    block(":root", [...variablesCss(pred, "light", "del-mode", true), ...variablesCss(pred, "light", "fixed")]),
    block(`:root${DARK}`, variablesCss(pred, "dark", "del-mode", true)),
  ]
  for (const p of palettes) {
    const sel = `:root[data-theme="${p.name}"]`
    parts.push(block(sel, [...variablesCss(p, "light", "del-mode", true), ...variablesCss(p, "light", "fixed")]))
    parts.push(block(`${sel}${DARK}`, variablesCss(p, "dark", "del-mode", true)))
    const [light, dark] = [pinnedEmphasis(p, "light"), pinnedEmphasis(p, "dark")]
    for (const m of p.emphasis?.swatches ?? []) {
      if (m.id === p.emphasis!.defaultId) continue
      const selM = `${sel}[${EMPHASIS_ATTRIBUTE}="${m.id}"]`
      parts.push(block(selM, [...emphasisVariables(m, "light", false), ...light, ...emphasisVariables(m, "light", true)]))
      parts.push(block(`${selM}${DARK}`, [...emphasisVariables(m, "dark", false), ...dark]))
    }
  }
  return parts.join("\n")
}

/**
 * What a swatch's block pins in one mode, so that only what the emphasis carries
 * changes. A role that is a reference to another of the same mode in the file is
 * written as var(), and if the chain reaches a role the emphasis carries
 * (EMPHASIS_ROLES), it would follow it: in monokai, code.variable is {ui.accent} in
 * light mode. Those roles are written with their colour. And since a swatch's light
 * block (0,3,0) also holds in dark mode, after the palette's dark block (0,3,0), the
 * dark one also carries, with its value from the palette in dark mode, what the light
 * one pins and it does not. The Doom palettes have none.
 */
export function pinnedEmphasis(p: Palette, mode: Mode): [string, string][] {
  const carry = new Set(Object.values(EMPHASIS_ROLES).flatMap((ps) => ps.map(([g, q]) => `${g}.${q}`)))
  const pinned = (md: Mode): [string, string][] => {
    const out: [string, string][] = []
    for (const [group, roles] of Object.entries(GROUPS) as [Group, readonly string[]][])
      for (const role of roles) {
        if (carry.has(`${group}.${role}`)) continue
        // The var() chain: while the role is written as a var() of another (asVar).
        let [g, q] = [group, role]
        while (asVar(p, md, g, q)) {
          [g, q] = p.references[md][`${g}.${q}`]!.split(".").slice(1) as [Group, string]
          if (carry.has(`${g}.${q}`)) {
            out.push([variable(group, role, md), valueFor(p, md, group, role)])
            break
          }
        }
      }
    return out
  }
  const own = pinned(mode)
  if (mode === "light") return own
  const has = new Set(own.map(([k]) => k))
  const ofMode = new Map(variablesCss(p, "dark", "del-mode", true))
  const ofLight = pinned("light").filter(([k]) => !has.has(k)).map(([k]): [string, string] => [k, ofMode.get(k)!])
  return [...own, ...ofLight]
}

/**
 * An emphasis swatch's variables in one mode: those that change with the mode, or
 * (fixed) the graph's, which carry the mode in their name and go together.
 */
export function emphasisVariables(m: Swatch, mode: Mode, fixed: boolean): [string, string][] {
  const out: [string, string][] = []
  for (const [hue, roles] of Object.entries(EMPHASIS_ROLES) as [keyof EmphasisHues, readonly (readonly [Group, string])[]][])
    for (const [g, p] of roles) {
      if (MODE_INDEPENDENT.includes(g) !== fixed) continue
      for (const md of fixed ? MODES : [mode]) out.push([variable(g, p, md), m[md][hue]])
    }
  return out
}

/** The resolved tokens, for consumers that read neither CSS nor YAML. */
export function tokensJson(palettes: Palette[]): string {
  const variables: Record<string, Record<Mode, string>> = {}
  for (const [group, roles] of Object.entries(GROUPS) as [Group, readonly string[]][])
    for (const role of roles)
      variables[`${group}.${role}`] = { light: variable(group, role, "light"), dark: variable(group, role, "dark") }
  const data = {
    isDefault: palettes.find((p) => p.isDefault)!.name,
    variables,
    // Emphasis: the attribute that chooses it, and which roles each tone carries.
    emphasis: {
      attribute: EMPHASIS_ATTRIBUTE,
      roles: Object.fromEntries(Object.entries(EMPHASIS_ROLES).map(([t, ps]) => [t, ps.map(([g, p]) => `${g}.${p}`)])),
    },
    palettes: Object.fromEntries(
      palettes.map((p) => [
        p.name,
        {
          light: p.light,
          dark: p.dark,
          // Each mode's emphasis swatches, in the order they are offered (the same in
          // both), and the default; null if the palette has none.
          emphasis: p.emphasis && {
            defaultId: p.emphasis.defaultId,
            ...Object.fromEntries(
              MODES.map((md) => [md, p.emphasis!.swatches.map((m) => ({ id: m.id, name: m.name, ...m[md] }))]),
            ),
          },
          // The file's references (role -> mode.group.role), in GROUPS's order.
          references: Object.fromEntries(
            MODES.map((m) => [
              m,
              Object.fromEntries(
                Object.entries(GROUPS).flatMap(([g, ps]) =>
                  ps.flatMap((x) => (p.references[m][`${g}.${x}`] ? [[`${g}.${x}`, p.references[m][`${g}.${x}`]]] : [])),
                ),
              ),
            ]),
          ),
          // The resolved value of each palette.css variable in each mode with this palette.
          css: Object.fromEntries(MODES.map((m) => [m, Object.fromEntries(variablesCss(p, m, "all"))])),
        },
      ]),
    ),
  }
  return JSON.stringify(data, null, 2) + "\n"
}

/** All of dist/: relative path -> content. */
export function generate(palettes: Palette[]): Map<string, string> {
  const files = new Map<string, string>()
  files.set("palette.css", css(palettes))
  files.set("tokens.json", tokensJson(palettes))
  return files
}
