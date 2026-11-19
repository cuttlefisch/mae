// SPDX-License-Identifier: GPL-3.0-or-later
// Synthetic knowledge bases of different shapes, for testing and looking at the
// layouts. Deterministic (seeded), and no real organisation's data: titles are
// generic. Each returns a content index as a static site writes it (slug -> entry),
// with folder pages as "<folder>/index".
//
// The shapes are chosen to disagree with each other, because the site that
// prompted the hierarchical layouts is not representative of every site the
// package serves:
//
//   inventory  deep and lopsided; folder pages list their children, and those
//              listing links are almost all there is (measured on a
//              real site: 994 of 1003 links touch a folder page, 9 are real
//              cross-references, 4 stay inside one folder).
//   zettel     shallow, few folders, dense cross-folder links with hubs.
//   manual     balanced tree; links mostly between siblings and to the chapter.
//   decisions  one big flat folder of heavily linked notes, a folder of
//              decision records pointing into it, a status note everyone links.
//   small      a few dozen notes.
//   deep       a narrow tree six levels deep, links up, down and across.
import type { IndexEntry } from "../../src/geometry";

export type Index = Record<string, IndexEntry>;

/** mulberry32: small, fast, good enough for fixtures, and stable across runtimes. */
export function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

class Builder {
  readonly index: Index = {};
  add(slug: string, title: string, links: string[] = []): string {
    const e = (this.index[slug] ??= { title, links: [] });
    e.title = title;
    for (const l of links) if (!e.links!.includes(l)) e.links!.push(l);
    return slug;
  }
  link(from: string, to: string): void {
    if (from === to) return;
    const e = this.index[from];
    if (!e) throw new Error(`no note ${from}`);
    if (!e.links!.includes(to)) e.links!.push(to);
  }
  /** A folder page that lists its children, as generated index pages do. */
  folder(path: string, title: string, children: string[]): string {
    return this.add(`${path}/index`, title, children);
  }
}

const pick = <T>(r: () => number, xs: T[]): T => xs[Math.floor(r() * xs.length)]!;
const pad = (n: number, w = 3) => String(n).padStart(w, "0");

export function inventory(seed = 1): Index {
  const r = rng(seed);
  const b = new Builder();
  // Sizes in the spirit of the measured site: one leaf folder of 161, one of 138...
  const lists = [161, 138, 109, 58, 48, 42, 39, 33, 27, 22, 16, 13, 7, 7, 5, 5, 2];
  const listPages: string[] = [];
  const allItems: string[] = [];
  lists.forEach((n, i) => {
    const path = `systems/inventory/list-${pad(i, 2)}`;
    const items: string[] = [];
    for (let k = 0; k < n; k++) items.push(b.add(`${path}/item-${pad(k)}`, `Item ${i}-${pad(k)}`));
    allItems.push(...items);
    listPages.push(b.folder(path, `List ${pad(i, 2)}`, items));
  });
  const invLoose: string[] = [];
  for (let k = 0; k < 16; k++)
    invLoose.push(b.add(`systems/inventory/sheet-${pad(k, 2)}`, `Sheet ${k}`));
  b.folder("systems/inventory", "Inventory", [...listPages, ...invLoose]);
  const sys = b.folder("systems", "Systems", [
    "systems/inventory/index",
    b.add("systems/map", "Systems map"),
  ]);
  const parts = ["annexes", "articles", "operational", "organisational", "protection"];
  const partSizes = [12, 41, 33, 4, 36];
  const partPages = parts.map((p, i) => {
    const notes: string[] = [];
    for (let k = 0; k < partSizes[i]!; k++)
      notes.push(b.add(`standard/${p}/n-${pad(k, 2)}`, `${p} ${k}`));
    return b.folder(`standard/${p}`, p, notes);
  });
  const std = b.folder("standard", "Standard", partPages);
  const prog = ["programme/plan", "programme/risks", "programme/roles"].map((s) =>
    b.add(s, s.split("/")[1]!),
  );
  b.folder("programme", "Programme", prog);
  const ref = ["reference/glossary", "reference/sources"].map((s) => b.add(s, s.split("/")[1]!));
  b.folder("reference", "Reference", ref);
  b.add("index", "Home", [sys, std, "programme/index", "reference/index"]);
  for (const s of ["about", "changes", "help"]) b.add(s, s);
  // The few real cross-references.
  b.link("programme/plan", "programme/risks");
  b.link("programme/plan", "standard/operational/n-03");
  b.link("programme/risks", "reference/glossary");
  b.link("programme/roles", "systems/map");
  b.link("standard/annexes/n-01", "programme/plan");
  b.link("reference/sources", "standard/articles/n-07");
  b.link("systems/map", pick(r, allItems));
  b.link("programme/plan", "programme/roles");
  b.link("reference/glossary", "reference/sources");
  return b.index;
}

export function zettel(seed = 2, size = 400): Index {
  const r = rng(seed);
  const b = new Builder();
  const folders = ["inbox", "literature", "permanent", "projects", "people", "journal"];
  const weights = [0.08, 0.22, 0.45, 0.1, 0.05, 0.1];
  const notes: string[] = [];
  for (let k = 0; k < size; k++) {
    let x = r();
    let f = folders.length - 1;
    for (let i = 0; i < weights.length; i++) {
      if ((x -= weights[i]!) < 0) {
        f = i;
        break;
      }
    }
    notes.push(b.add(`${folders[f]}/z-${pad(k)}`, `Note ${pad(k)}`));
  }
  // Preferential attachment: early notes become hubs. Mean out-degree ~4.
  const deg = notes.map(() => 1);
  notes.forEach((n, i) => {
    const m = 1 + Math.floor(r() * 7);
    for (let j = 0; j < m && i > 0; j++) {
      const total = deg.slice(0, i).reduce((a, c) => a + c, 0);
      let x = r() * total;
      let t = 0;
      while ((x -= deg[t]!) > 0) t++;
      b.link(n, notes[t]!);
      deg[t]! += 1;
      deg[i]! += 1;
    }
  });
  b.add("index", "Home", notes.slice(0, 5));
  return b.index;
}

export function manual(seed = 3): Index {
  const r = rng(seed);
  const b = new Builder();
  const sections: string[] = [];
  for (let s = 0; s < 4; s++) {
    const chapters: string[] = [];
    for (let c = 0; c < 4; c++) {
      const path = `part-${s}/chapter-${c}`;
      const pages: string[] = [];
      const n = 5 + Math.floor(r() * 8);
      for (let p = 0; p < n; p++)
        pages.push(b.add(`${path}/page-${pad(p, 2)}`, `${s}.${c}.${p} Page`));
      // Prev/next between siblings, each page back to its chapter.
      pages.forEach((p, i) => {
        if (i > 0) b.link(p, pages[i - 1]!);
        b.link(p, `${path}/index`);
      });
      // One wide title, next to the current page in the browser tests: where
      // a width per character fell short of the real text, a curved label too
      // long for its arc lost glyphs. CJK: 132 px at 11 px against 74
      // estimated (a wide all-caps Latin title, 137 against 124, left a band
      // too narrow for the zoom steps to land in, and the check passed with
      // the estimate back).
      const title = s === 0 && c === 1 ? "情報セキュリティ管理規程" : `Chapter ${s}.${c}`;
      chapters.push(b.folder(path, title, pages));
    }
    sections.push(b.folder(`part-${s}`, `Part ${s}`, chapters));
  }
  b.add("index", "Manual", sections);
  // A few "see also" links across parts.
  const all = Object.keys(b.index).filter((k) => k.includes("/page-"));
  for (let k = 0; k < 25; k++) b.link(pick(r, all), pick(r, all));
  return b.index;
}

export function decisions(seed = 4): Index {
  const r = rng(seed);
  const b = new Builder();
  const kb: string[] = [];
  for (let k = 0; k < 300; k++) kb.push(b.add(`kb/note-${pad(k)}`, `Note ${pad(k)}`));
  const status = b.add("kb/status", "Current status");
  kb.forEach((n, i) => {
    const m = Math.floor(r() * 5);
    for (let j = 0; j < m; j++) b.link(n, kb[Math.floor(r() * kb.length)]!);
    if (r() < 0.3) b.link(n, status);
    if (i > 0 && r() < 0.5) b.link(n, kb[i - 1]!);
  });
  const adrs: string[] = [];
  for (let k = 1; k <= 40; k++) {
    const a = b.add(`adrs/${pad(k, 4)}-decision`, `ADR ${k}`);
    for (let j = 0; j < 3; j++) b.link(a, pick(r, kb));
    if (k > 1 && r() < 0.4) b.link(a, `adrs/${pad(k - 1, 4)}-decision`);
    adrs.push(a);
  }
  // Not "templates": a static-site generator may ignore a folder of that name by
  // default, and six of this KB's links vanished from a built site.
  for (const f of ["daily", "drafts", "snippets"]) {
    for (let k = 0; k < 6; k++) b.add(`${f}/${f}-${k}`, `${f} ${k}`, [pick(r, kb)]);
  }
  b.add("index", "Home", [status, adrs[0]!]);
  return b.index;
}

export function small(seed = 5): Index {
  const r = rng(seed);
  const b = new Builder();
  const all: string[] = [];
  for (const f of ["", "guides", "notes", "ops"]) {
    for (let k = 0; k < 9; k++) all.push(b.add(`${f ? f + "/" : ""}s-${k}`, `${f || "root"} ${k}`));
  }
  for (let k = 0; k < 50; k++) b.link(pick(r, all), pick(r, all));
  return b.index;
}

export function deep(seed = 6): Index {
  const r = rng(seed);
  const b = new Builder();
  const all: string[] = [];
  const grow = (path: string, depth: number): string[] => {
    const here: string[] = [];
    const n = 2 + Math.floor(r() * 4);
    for (let k = 0; k < n; k++)
      here.push(b.add(`${path}/leaf-${k}`, `${path.split("/").pop()} ${k}`));
    all.push(...here);
    if (depth < 6) {
      for (let c = 0; c < 2; c++) {
        const child = `${path}/d${depth}-${c}`;
        grow(child, depth + 1);
        here.push(`${child}/index`);
      }
    }
    b.folder(path, path.split("/").pop()!, here);
    return here;
  };
  grow("root", 1);
  for (let k = 0; k < 60; k++) b.link(pick(r, all), pick(r, all));
  b.add("index", "Home", ["root/index"]);
  return b.index;
}

export const SHAPES: Record<string, (seed?: number) => Index> = {
  inventory,
  zettel,
  manual,
  decisions,
  small,
  deep,
};
