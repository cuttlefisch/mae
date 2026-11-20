// SPDX-License-Identifier: GPL-3.0-or-later
// Test helper: a minimal cascade of :root blocks, with var() resolved.

// A minimal cascade, only for palette.css's shape: blocks `selector { --x: v; }`
// with `:root` selectors plus attributes. The highest specificity wins (each
// attribute counts one) and, when equal, the last. Then each var(--y) is replaced
// with the value --y won, as the browser does on the same element.
export function cascade(sheet: string, attributes: Record<string, string>): Map<string, string> {
  const rules = [...sheet.matchAll(/^(:root[^{]*) \{\n([^}]*)\}/gm)].map((m, order) => {
    const attrs = [...m[1]!.matchAll(/\[([\w-]+)="([^"]*)"\]/g)].map((a) => [a[1]!, a[2]!] as const)
    expect(m[1]!.replace(/\[[\w-]+="[^"]*"\]/g, "")).toBe(":root")
    return {
      applies: attrs.every(([k, v]) => attributes[k] === v),
      specificity: 1 + attrs.length,
      order,
      decl: [...m[2]!.matchAll(/^ {2}(--[\w-]+): ([^;]+);$/gm)].map((d) => [d[1]!, d[2]!] as const),
    }
  })
  const wins = new Map<string, { v: string; e: number; o: number }>()
  for (const r of rules.filter((x) => x.applies))
    for (const [k, v] of r.decl) {
      const ya = wins.get(k)
      if (!ya || r.specificity > ya.e || (r.specificity === ya.e && r.order > ya.o))
        wins.set(k, { v, e: r.specificity, o: r.order })
    }
  const values = new Map([...wins].map(([k, x]) => [k, x.v]))
  const replace = (v: string, path: string[]): string =>
    v.replace(/var\((--[\w-]+)\)/g, (_, n: string) => {
      if (path.includes(n)) throw new Error(`circular var(): ${[...path, n].join(" -> ")}`)
      const x = values.get(n)
      if (x === undefined) throw new Error(`var(${n}) has no value`)
      return replace(x, [...path, n])
    })
  return new Map([...values].map(([k, v]) => [k, replace(v, [k])]))
}
