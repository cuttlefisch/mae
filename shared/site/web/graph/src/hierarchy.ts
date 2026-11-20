// SPDX-License-Identifier: GPL-3.0-or-later
// The folder tree of a site, shared by the hierarchical layouts (sunburst and
// constellation). Pure: no DOM.
//
// A note whose slug is also a folder path is that folder's OWN page and is
// represented by the folder itself; the site's "index" is the
// root's own page. Every note is represented by exactly one node.
//
// Links are classified. A link between a folder's own page and anything inside
// that folder (at any depth) is STRUCTURAL: it says "this is in here", which a
// hierarchical layout already shows by position. Generated folder pages that
// list their children are the common case (on a real site, 994 of 1003
// links). Everything else is a REAL link: a cross-reference the reader cannot
// see from the tree.
import { compareText, normaliseSlug, pageHref, type IndexEntry } from "./geometry";

export type HKind = "folder" | "note";

export interface HNode {
  /** Unique. Notes: their slug. Folders: NUL + "folder:" + path, which no slug can be. */
  id: string;
  kind: HKind;
  /** Folder: its path ("" for the root). Note: its slug. */
  path: string;
  title: string;
  /** Note: its URL path. Folder: its own page's URL path, if it has one. */
  href?: string;
  /** Folder: the slug of its own page, if it has one. */
  own?: string;
  /** The root is 0; top-level folders and root notes are 1. */
  depth: number;
  parent?: HNode;
  /** Folders first, then notes; each by title, then path. */
  children: HNode[];
  /** Notes this node stands for: itself (note), or its own page and everything below (folder). */
  size: number;
}

export interface HEdge {
  /** Node ids, source < target. */
  source: string;
  target: string;
  /** Between a folder's own page and something inside that folder. */
  structural: boolean;
}

export interface Hierarchy {
  root: HNode;
  nodes: Map<string, HNode>;
  /** Note slug -> the id of the node that represents it. */
  nodeOf: Map<string, string>;
  edges: HEdge[];
  /** Depth of the deepest node. */
  maxDepth: number;
}

interface Note {
  slug: string;
  title: string;
  href: string;
  links: string[];
}

/** The site's notes, merged as buildRing merges them ("x/" = "x/index" = "x"). */
export function collectNotes(
  index: Record<string, IndexEntry>,
  exclude?: (slug: string) => boolean,
): Map<string, Note> {
  const seen = new Map<string, Note & { raw: string }>();
  for (const [rawSlug, entry] of Object.entries(index)) {
    const slug = normaliseSlug(rawSlug);
    if (exclude?.(slug)) continue;
    const prev = seen.get(slug);
    const links = [...new Set([...(prev?.links ?? []), ...(entry.links ?? [])])];
    if (prev && prev.raw <= rawSlug) {
      prev.links = links;
      continue;
    }
    seen.set(slug, {
      slug,
      title: entry.title || slug,
      href: pageHref(rawSlug),
      links,
      raw: rawSlug,
    });
  }
  return seen;
}

export const folderId = (path: string) => `\u0000folder:${path}`;

export function buildHierarchy(
  index: Record<string, IndexEntry>,
  opts: { exclude?: (slug: string) => boolean; locale?: string } = {},
): Hierarchy {
  const collator = new Intl.Collator(opts.locale, { numeric: true, sensitivity: "base" });
  const notes = collectNotes(index, opts.exclude);
  const nodes = new Map<string, HNode>();
  const nodeOf = new Map<string, string>();
  const parentPath = (p: string) => (p.includes("/") ? p.slice(0, p.lastIndexOf("/")) : "");

  const folder = (path: string): HNode => {
    const id = folderId(path);
    let f = nodes.get(id);
    if (!f) {
      f = {
        id,
        kind: "folder",
        path,
        title: path === "" ? "" : path.slice(path.lastIndexOf("/") + 1),
        depth: path === "" ? 0 : path.split("/").length,
        children: [],
        size: 0,
      };
      nodes.set(id, f);
      if (path !== "") {
        const p = folder(parentPath(path));
        f.parent = p;
        p.children.push(f);
      }
    }
    return f;
  };
  const root = folder("");
  for (const slug of notes.keys()) if (slug.includes("/")) folder(parentPath(slug));

  for (const n of notes.values()) {
    const asFolder = n.slug === "index" ? root : nodes.get(folderId(n.slug));
    if (asFolder) {
      asFolder.own = n.slug;
      asFolder.href = n.href;
      if (n.title) asFolder.title = n.title;
      nodeOf.set(n.slug, asFolder.id);
      continue;
    }
    const p = folder(parentPath(n.slug));
    const node: HNode = {
      id: n.slug,
      kind: "note",
      path: n.slug,
      title: n.title,
      href: n.href,
      depth: p.depth + 1,
      parent: p,
      children: [],
      size: 1,
    };
    nodes.set(node.id, node);
    p.children.push(node);
    nodeOf.set(n.slug, node.id);
  }

  let maxDepth = 0;
  const finish = (f: HNode): number => {
    maxDepth = Math.max(maxDepth, f.depth);
    if (f.kind === "note") return f.size;
    f.children.sort(
      (a, b) =>
        Number(a.kind === "note") - Number(b.kind === "note") ||
        collator.compare(a.title, b.title) ||
        compareText(collator, a.path, b.path),
    );
    f.size = (f.own ? 1 : 0) + f.children.reduce((s, c) => s + finish(c), 0);
    return f.size;
  };
  finish(root);

  const isAncestor = (a: HNode, b: HNode) => {
    for (let p = b.parent; p; p = p.parent) if (p === a) return true;
    return false;
  };
  const seen = new Set<string>();
  const edges: HEdge[] = [];
  for (const n of notes.values()) {
    const a = nodeOf.get(n.slug)!;
    for (const raw of n.links) {
      const t = normaliseSlug(raw);
      const b = nodeOf.get(t);
      if (b === undefined || b === a) continue;
      const [s, d] = a < b ? [a, b] : [b, a];
      const k = `${s}\u0000${d}`;
      if (seen.has(k)) continue;
      seen.add(k);
      const A = nodes.get(s)!;
      const B = nodes.get(d)!;
      edges.push({ source: s, target: d, structural: isAncestor(A, B) || isAncestor(B, A) });
    }
  }
  edges.sort((x, y) =>
    x.source < y.source ? -1 : x.source > y.source ? 1 : x.target < y.target ? -1 : 1,
  );
  return { root, nodes, nodeOf, edges, maxDepth };
}

/** The path of nodes from `a` up to (and including) the lowest common ancestor, then down to `b`. */
export function treePath(a: HNode, b: HNode): HNode[] {
  const up = (n: HNode) => {
    const out: HNode[] = [];
    for (let p: HNode | undefined = n; p; p = p.parent) out.push(p);
    return out;
  };
  const ua = up(a);
  const ub = up(b);
  const inA = new Set(ua);
  const lca = ub.find((n) => inA.has(n))!;
  return [...ua.slice(0, ua.indexOf(lca) + 1), ...ub.slice(0, ub.indexOf(lca)).reverse()];
}
