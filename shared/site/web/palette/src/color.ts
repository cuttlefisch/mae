// SPDX-License-Identifier: GPL-3.0-or-later
// Colour arithmetic: WCAG 2 luminance and contrast, compositing a translucent
// colour over an opaque one, and OKLab. The same formulas as the chord graph's
// test/themes.test.ts, so that a colour that passes here passes there.

export type Rgb = [number, number, number]

/** Lowercase "#rrggbb" or "#rrggbbaa": any other form is an error. */
export const HEX = /^#[0-9a-f]{6}([0-9a-f]{2})?$/

export function parse(hex: string): { rgb: Rgb; alpha: number } {
  if (!HEX.test(hex)) throw new Error(`not a lowercase #rrggbb or #rrggbbaa colour: ${hex}`)
  const n = (i: number) => parseInt(hex.slice(i, i + 2), 16)
  return { rgb: [n(1), n(3), n(5)], alpha: hex.length === 9 ? n(7) / 255 : 1 }
}

export function opaque(hex: string): Rgb {
  const c = parse(hex)
  if (c.alpha !== 1) throw new Error(`${hex} must be opaque`)
  return c.rgb
}

export function toHex(rgb: Rgb): string {
  return `#${rgb.map((c) => c.toString(16).padStart(2, "0")).join("")}`
}

/** A translucent colour as seen over an opaque one (rounded to 8 bits). */
export function on(up: string, down: string): Rgb {
  const t = parse(up)
  const b = parse(down).rgb
  return t.rgb.map((c, i) => Math.round(t.alpha * c + (1 - t.alpha) * b[i]!)) as Rgb
}

const linear = (c: number) => {
  const s = c / 255
  return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4
}

export function luminance([r, g, b]: Rgb): number {
  return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

export function contrast(a: Rgb, b: Rgb): number {
  const [alto, bajo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number]
  return (alto + 0.05) / (bajo + 0.05)
}

/** OKLab of an opaque colour: [L, a, b]. */
export function oklab(hex: string): [number, number, number] {
  const [r, g, b] = opaque(hex).map(linear) as Rgb
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b)
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b)
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b)
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ]
}

/** OKLab distance ×100 (the scale of the palette validator chord-graph uses). */
export function deltaE(a: string, b: string): number {
  const [p, q] = [oklab(a), oklab(b)]
  return 100 * Math.hypot(p[0] - q[0], p[1] - q[1], p[2] - q[2])
}

/** CIELAB (D65, 2° observer) of an opaque colour: [L, a, b]. */
export function lab(hex: string): [number, number, number] {
  const [r, g, b] = opaque(hex).map(linear) as Rgb
  const f = (t: number) => (t > (6 / 29) ** 3 ? Math.cbrt(t) : t / (3 * (6 / 29) ** 2) + 4 / 29)
  const x = f((0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047)
  const y = f(0.2126729 * r + 0.7151522 * g + 0.072175 * b)
  const z = f((0.0193339 * r + 0.119192 * g + 0.9503041 * b) / 1.08883)
  return [116 * y - 16, 500 * (x - y), 200 * (y - z)]
}

/**
 * CIEDE2000 between two CIELAB colours (Sharma, Wu and Dalal, 2005; kL = kC = kH = 1).
 * It is the distance the emphasis rules ask for (ΔE00), not deltaE's (OKLab).
 */
export function deltaE00Lab([L1, a1, b1]: number[], [L2, a2, b2]: number[]): number {
  const rad = Math.PI / 180
  const C1 = Math.hypot(a1!, b1!)
  const C2 = Math.hypot(a2!, b2!)
  const Cm = (C1 + C2) / 2
  const G = 0.5 * (1 - Math.sqrt(Cm ** 7 / (Cm ** 7 + 25 ** 7)))
  const [ap1, ap2] = [(1 + G) * a1!, (1 + G) * a2!]
  const [Cp1, Cp2] = [Math.hypot(ap1, b1!), Math.hypot(ap2, b2!)]
  const hue = (b: number, ap: number) => (b === 0 && ap === 0 ? 0 : (Math.atan2(b, ap) / rad + 360) % 360)
  const [hp1, hp2] = [hue(b1!, ap1), hue(b2!, ap2)]
  const dL = L2! - L1!
  const dC = Cp2 - Cp1
  let dh = 0
  if (Cp1 * Cp2 !== 0) {
    dh = hp2 - hp1
    if (dh > 180) dh -= 360
    else if (dh < -180) dh += 360
  }
  const dH = 2 * Math.sqrt(Cp1 * Cp2) * Math.sin((dh / 2) * rad)
  const Lm = (L1! + L2!) / 2
  const Cpm = (Cp1 + Cp2) / 2
  let hm = hp1 + hp2
  if (Cp1 * Cp2 !== 0) {
    if (Math.abs(hp1 - hp2) <= 180) hm = (hp1 + hp2) / 2
    else hm = hp1 + hp2 < 360 ? (hp1 + hp2 + 360) / 2 : (hp1 + hp2 - 360) / 2
  }
  const T =
    1 -
    0.17 * Math.cos((hm - 30) * rad) +
    0.24 * Math.cos(2 * hm * rad) +
    0.32 * Math.cos((3 * hm + 6) * rad) -
    0.2 * Math.cos((4 * hm - 63) * rad)
  const dTheta = 30 * Math.exp(-(((hm - 275) / 25) ** 2))
  const RC = 2 * Math.sqrt(Cpm ** 7 / (Cpm ** 7 + 25 ** 7))
  const SL = 1 + (0.015 * (Lm - 50) ** 2) / Math.sqrt(20 + (Lm - 50) ** 2)
  const SC = 1 + 0.045 * Cpm
  const SH = 1 + 0.015 * Cpm * T
  const RT = -Math.sin(2 * dTheta * rad) * RC
  return Math.sqrt((dL / SL) ** 2 + (dC / SC) ** 2 + (dH / SH) ** 2 + RT * (dC / SC) * (dH / SH))
}

/** CIEDE2000 between two opaque colours. */
export function deltaE00(a: string, b: string): number {
  return deltaE00Lab(lab(a), lab(b))
}
