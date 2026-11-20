// SPDX-License-Identifier: GPL-3.0-or-later
/**
 * Pure geometry and data shaping for the chord graph. No DOM access here, so
 * everything is unit-tested in Node; the browser script only draws what these
 * functions return.
 *
 * The design: every note sits on one ring, in an equal angular slot, drawn as
 * a rounded annular wedge ("petal"). Notes are ordered by top-level folder and
 * then title, so each folder forms a contiguous arc. Links are drawn as chords:
 * quadratic curves from wedge to wedge, pulled toward the centre.
 */

export interface IndexEntry {
  title?: string;
  links?: string[];
  tags?: string[];
}

export interface RingNode {
  slug: string;
  /** The slug as the content index has it, for building the page's URL. */
  href: string;
  title: string;
  /** Top-level folder ("" for notes at the root). */
  group: string;
  /** Index of `group` among all groups, in ring order. */
  groupIndex: number;
  /** Angle of the wedge's centre, in radians; the first node sits at the top. */
  angle: number;
  /** Number of distinct notes this note links to or is linked from. */
  degree: number;
}

export interface RingEdge {
  source: string;
  target: string;
}

export interface RingGroup {
  name: string;
  /** Angles where the group's arc starts and ends, in radians. */
  start: number;
  end: number;
  size: number;
}

export interface Ring {
  nodes: RingNode[];
  edges: RingEdge[];
  groups: RingGroup[];
  /** Angular size of one slot, in radians. */
  slot: number;
}

export interface RingOptions {
  /** Slugs to leave off the ring entirely, e.g. tag listing pages. */
  exclude?: (slug: string) => boolean;
  /** Locale for ordering titles; defaults to the runtime's. */
  locale?: string;
  /**
   * Empty slots left between consecutive groups, so each folder reads as its
   * own arc. 0 packs the ring with no gaps.
   */
  groupGap?: number;
}

/**
 * "folder/index", "folder/index/" and "folder/" all name the folder's own page.
 * Trailing slashes go first: stripping "/index" first left "folder/index/" as
 * "folder/index", a second note for the same page (found by the property tests).
 */
/** A page's URL path as the site writes its own links: a folder's index page is the
 * folder ("systems/index" -> "systems/", "index" -> ""), so a wedge links to the
 * canonical URL. The slug that identifies the note (current-note detection) is kept
 * apart and does not change. */
export function pageHref(rawSlug: string): string {
  return rawSlug.replace(/^\/+/, "").replace(/(^|\/)index$/, "$1");
}

export function normaliseSlug(slug: string): string {
  let s = (slug.replace(/^\/+/, "").split("#")[0] ?? "").replace(/\/+$/, "");
  if (s.endsWith("/index")) s = s.slice(0, -"/index".length);
  return s === "" ? "index" : s;
}

export function groupOf(slug: string): string {
  const s = normaliseSlug(slug);
  const i = s.indexOf("/");
  return i === -1 ? "" : s.slice(0, i);
}

/**
 * The collator's order, then plain code-unit order: a collator with
 * sensitivity "base" and numeric collation calls "Foo" and "foo", "cafe" and
 * "café", "n1" and "n01" equal, and a sort that stops there keeps whatever
 * order the index happened to have (and can interleave two such folders' arcs).
 */
export function compareText(collator: Intl.Collator, a: string, b: string): number {
  return collator.compare(a, b) || (a < b ? -1 : a > b ? 1 : 0);
}

/**
 * Lay the content index out on a ring. Deterministic: the same index always
 * gives the same order and angles. Root notes (group "") come first, so hubs
 * kept at the root sit together at the top of the ring.
 */
export function buildRing(index: Record<string, IndexEntry>, opts: RingOptions = {}): Ring {
  const collator = new Intl.Collator(opts.locale, { numeric: true, sensitivity: "base" });
  // Several index entries can name the same note ("kb/" and "kb/index"). They
  // are ONE note: its links are the union of theirs, and its title and URL come
  // from the lexicographically smallest raw slug, so the ring does not depend on
  // the order the index happens to list them in.
  const seen = new Map<
    string,
    { title: string; group: string; links: string[]; href: string; raw: string }
  >();
  for (const [rawSlug, entry] of Object.entries(index)) {
    const slug = normaliseSlug(rawSlug);
    if (opts.exclude?.(slug)) continue;
    const prev = seen.get(slug);
    const links = [...new Set([...(prev?.links ?? []), ...(entry.links ?? [])])];
    if (prev && prev.raw <= rawSlug) {
      prev.links = links;
      continue;
    }
    seen.set(slug, {
      title: entry.title || slug,
      group: groupOf(slug),
      links,
      href: pageHref(rawSlug),
      raw: rawSlug,
    });
  }

  const ordered = [...seen.entries()].sort(([sa, a], [sb, b]) => {
    if (a.group !== b.group) {
      if (a.group === "") return -1;
      if (b.group === "") return 1;
      return compareText(collator, a.group, b.group);
    }
    return collator.compare(a.title, b.title) || compareText(collator, sa, sb);
  });

  // One undirected edge per linked pair; self-links and links to notes that
  // are not on the ring are dropped.
  const edgeKeys = new Set<string>();
  const edges: RingEdge[] = [];
  for (const [slug, { links }] of ordered) {
    for (const raw of links) {
      const target = normaliseSlug(raw);
      if (target === slug || !seen.has(target)) continue;
      const key = slug < target ? `${slug}\u0000${target}` : `${target}\u0000${slug}`;
      if (edgeKeys.has(key)) continue;
      edgeKeys.add(key);
      edges.push({ source: slug, target });
    }
  }

  return layoutRing(
    ordered.map(([slug, v]) => ({ slug, title: v.title, group: v.group, href: v.href })),
    edges,
    opts.groupGap ?? 0,
  );
}

/** One wedge to lay out: a note (buildRing's internal step, kept separate for its tests). */
export interface RingEntry {
  slug: string;
  title: string;
  group: string;
  href: string;
}

/**
 * Angles, folder arcs and gaps for entries already in ring order (each group
 * contiguous).
 */
export function layoutRing(entries: RingEntry[], edges: RingEdge[], groupGap = 0): Ring {
  const ordered = entries.map((e) => [e.slug, e] as const);
  const degree = new Map<string, number>();
  for (const e of edges) {
    degree.set(e.source, (degree.get(e.source) ?? 0) + 1);
    degree.set(e.target, (degree.get(e.target) ?? 0) + 1);
  }

  const gap = Math.max(0, groupGap);
  const groupCount = new Set(ordered.map(([, v]) => v.group)).size;
  const n = ordered.length;
  const totalSlots = n + (groupCount > 1 ? gap * groupCount : 0);
  const slot = totalSlots > 0 ? (2 * Math.PI) / totalSlots : 0;
  const groups: RingGroup[] = [];
  let position = 0;
  const nodes = ordered.map(([slug, { title, group, href }], i) => {
    if (groups[groups.length - 1]?.name !== group) {
      if (i > 0 && groupCount > 1) position += gap;
      groups.push({ name: group, start: 0, end: 0, size: 0 });
    }
    const angle = -Math.PI / 2 + position * slot;
    position += 1;
    const g = groups[groups.length - 1]!;
    if (g.size === 0) g.start = angle - slot / 2;
    g.end = angle + slot / 2;
    g.size += 1;
    return {
      slug,
      href,
      title,
      group,
      groupIndex: groups.length - 1,
      angle,
      degree: degree.get(slug) ?? 0,
    };
  });
  return { nodes, edges, groups, slot };
}

/** Notes linked to `slug` in either direction. */
export function neighbours(ring: Ring, slug: string): Set<string> {
  const out = new Set<string>();
  for (const e of ring.edges) {
    if (e.source === slug) out.add(e.target);
    else if (e.target === slug) out.add(e.source);
  }
  return out;
}

export function polar(cx: number, cy: number, r: number, a: number): [number, number] {
  return [cx + r * Math.cos(a), cy + r * Math.sin(a)];
}

const f = (x: number) => Number(x.toFixed(3));
const pt = ([x, y]: [number, number]) => `${f(x)} ${f(y)}`;

/**
 * An annular sector from angle a0 to a1 (a1 > a0) between innerR and outerR.
 * Its two inner corners are rounded by up to `corner` and its two outer corners
 * by up to `outerCorner` (the same as `corner` unless given): the visited band
 * shares the wedge's inner corners and keeps a flush outer edge. Each radius is
 * clamped so opposite fillets never meet, however thin or short the wedge: a
 * tiny wedge degrades to a less rounded one, never a self-intersecting one.
 */
export function wedgePath(
  cx: number,
  cy: number,
  innerR: number,
  outerR: number,
  a0: number,
  a1: number,
  corner = 0,
  outerCorner = corner,
): string {
  const thickness = outerR - innerR;
  const clamp = (c: number, r: number) =>
    Math.max(0, Math.min(c, thickness / 2 - 0.01, ((a1 - a0) * Math.max(r, 1)) / 2 - 0.01));
  const ci = clamp(corner, innerR);
  const co = clamp(outerCorner, innerR);
  const largeArc = a1 - a0 > Math.PI ? 1 : 0;
  if (ci <= 0 && co <= 0) {
    return [
      `M ${pt(polar(cx, cy, outerR, a0))}`,
      `A ${f(outerR)} ${f(outerR)} 0 ${largeArc} 1 ${pt(polar(cx, cy, outerR, a1))}`,
      `L ${pt(polar(cx, cy, innerR, a1))}`,
      `A ${f(innerR)} ${f(innerR)} 0 ${largeArc} 0 ${pt(polar(cx, cy, innerR, a0))}`,
      "Z",
    ].join(" ");
  }
  const dO = co / outerR;
  const dI = ci / Math.max(innerR, 1);
  return [
    `M ${pt(polar(cx, cy, outerR, a0 + dO))}`,
    `A ${f(outerR)} ${f(outerR)} 0 ${largeArc} 1 ${pt(polar(cx, cy, outerR, a1 - dO))}`,
    `A ${f(co)} ${f(co)} 0 0 1 ${pt(polar(cx, cy, outerR - co, a1))}`,
    `L ${pt(polar(cx, cy, innerR + ci, a1))}`,
    `A ${f(ci)} ${f(ci)} 0 0 1 ${pt(polar(cx, cy, innerR, a1 - dI))}`,
    `A ${f(innerR)} ${f(innerR)} 0 ${largeArc} 0 ${pt(polar(cx, cy, innerR, a0 + dI))}`,
    `A ${f(ci)} ${f(ci)} 0 0 1 ${pt(polar(cx, cy, innerR + ci, a0))}`,
    `L ${pt(polar(cx, cy, outerR - co, a0))}`,
    `A ${f(co)} ${f(co)} 0 0 1 ${pt(polar(cx, cy, outerR, a0 + dO))}`,
    "Z",
  ].join(" ");
}

/**
 * A chord between two points on a circle of radius r: a quadratic curve whose
 * control point is the chord's midpoint pulled toward the centre by `pull`
 * (0 = straight line, 1 = through the centre).
 */
export function chordPath(cx: number, cy: number, r: number, a0: number, a1: number, pull = 0.55) {
  const [x0, y0] = polar(cx, cy, r, a0);
  const [x1, y1] = polar(cx, cy, r, a1);
  const mx = (x0 + x1) / 2;
  const my = (y0 + y1) / 2;
  const qx = mx + (cx - mx) * pull;
  const qy = my + (cy - my) * pull;
  return `M ${f(x0)} ${f(y0)} Q ${f(qx)} ${f(qy)} ${f(x1)} ${f(y1)}`;
}

/**
 * Wedge thickness (in viewBox units) that keeps every wedge a usable target:
 * at least `minPx` on screen for a diagram `renderedPx` wide drawn in a viewBox
 * `viewBoxSize` wide, and never more than `max`.
 */
export function wedgeThickness(renderedPx: number, viewBoxSize: number, minPx = 12, max = 14) {
  const perPx = viewBoxSize / Math.max(renderedPx, 1);
  return Math.min(max, Math.max(minPx * perPx, 1));
}

/** The visited band's share of a wedge's thickness, measured from its inner edge. */
export const VISITED_BAND = 0.4;

/**
 * A wedge's corner radius for a given thickness: about a third of it, at most
 * 2, and never more than the visited band can carry. wedgePath clamps a corner
 * to half the thickness it is drawn on, so a larger radius would round the
 * thin band less than its wedge and the band would poke out of the wedge's
 * corners (in the dialog, where wedges are thin in viewBox units).
 */
export function wedgeCorner(thickness: number, band = VISITED_BAND) {
  return Math.max(0, Math.min(thickness / 3, 2, (thickness * band) / 2 - 0.01));
}
