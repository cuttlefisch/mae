// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import fc from "fast-check";
import { buildHierarchy, collectNotes, folderId, treePath, type HNode } from "../src/hierarchy";
import { indexArb, params, shuffled } from "./arbitraries";

const idx = {
  index: { title: "Home", links: ["a/index", "b/x"] },
  "a/index": { title: "A", links: ["a/one", "a/sub/deep"] },
  "a/one": { title: "One", links: ["a/two", "b/index", "a/index"] },
  "a/two": { title: "Two", links: [] },
  "a/sub/deep": { title: "Deep", links: ["b/x"] },
  "b/x": { title: "X", links: ["a/index"] },
};

describe("buildHierarchy", () => {
  const h = buildHierarchy(idx);

  it("represents a folder's own page by the folder, and the site index by the root", () => {
    expect(h.nodeOf.get("a")).toBe(folderId("a"));
    expect(h.nodeOf.get("index")).toBe(folderId(""));
    expect(h.nodes.get(folderId("a"))!.title).toBe("A");
    // b has no own page: it keeps its name.
    expect(h.nodes.get(folderId("b"))!.title).toBe("b");
  });

  it("counts each note once in sizes: own page included", () => {
    expect(h.root.size).toBe(6);
    expect(h.nodes.get(folderId("a"))!.size).toBe(4); // A, One, Two, Deep
    expect(h.nodes.get(folderId("b"))!.size).toBe(1);
  });

  it("orders folders before notes", () => {
    expect(h.nodes.get(folderId("a"))!.children.map((c) => c.kind)).toEqual([
      "folder",
      "note",
      "note",
    ]);
  });

  it("classifies links: a folder page and its contents, at any depth, is structural", () => {
    const kind = (a: string, b: string) => {
      const [s, t] = [h.nodeOf.get(a)!, h.nodeOf.get(b)!].sort();
      return h.edges.find((e) => e.source === s && e.target === t)?.structural;
    };
    expect(kind("index", "a")).toBe(true); // the root contains everything
    expect(kind("index", "b/x")).toBe(true);
    expect(kind("a", "a/one")).toBe(true);
    expect(kind("a", "a/sub/deep")).toBe(true); // a grandchild still
    expect(kind("a/one", "a/two")).toBe(false); // siblings
    expect(kind("b/x", "a")).toBe(false); // another folder's page
    // b has no page of its own, so a link to "b/index" leads nowhere: no edge.
    expect(kind("a/one", "b")).toBeUndefined();
    expect(kind("a/sub/deep", "b/x")).toBe(false);
  });

  it("keeps one edge per pair, whichever side links", () => {
    // a/one -> a/index and a/index -> a/one are one edge.
    const pairs = h.edges.map((e) => `${e.source}|${e.target}`);
    expect(new Set(pairs).size).toBe(pairs.length);
  });

  it("treePath runs up to the lowest common ancestor and down", () => {
    const deep = h.nodes.get("a/sub/deep")!;
    const x = h.nodes.get("b/x")!;
    expect(treePath(deep, x).map((n) => n.path)).toEqual([
      "a/sub/deep",
      "a/sub",
      "a",
      "",
      "b",
      "b/x",
    ]);
    const one = h.nodes.get("a/one")!;
    const two = h.nodes.get("a/two")!;
    expect(treePath(one, two).map((n) => n.path)).toEqual(["a/one", "a", "a/two"]);
  });
});

describe("collectNotes", () => {
  it("merges the entries that name one note, its links their union; the smallest raw slug gives title and URL", () => {
    const notes = collectNotes(
      {
        "kb/index": { title: "From index", links: ["a"] },
        "kb/": { title: "From slash", links: ["b"] },
        a: { title: "A", links: [] },
        "tags/x": { title: "tag", links: [] },
      },
      (s) => s.startsWith("tags/"),
    );
    expect([...notes.keys()].sort()).toEqual(["a", "kb"]);
    const kb = notes.get("kb")!;
    expect(kb.links.sort()).toEqual(["a", "b"]);
    expect(kb.title).toBe("From slash"); // "kb/" < "kb/index"
    expect(kb.href).toBe("kb/");
  });
});

describe("buildHierarchy: invariants for any index", () => {
  it("every note is represented exactly once, and sizes add up", () => {
    fc.assert(
      fc.property(indexArb, (ix) => {
        const h = buildHierarchy(ix);
        const notes = collectNotes(ix);
        expect(h.nodeOf.size).toBe(notes.size);
        for (const slug of notes.keys()) expect(h.nodes.has(h.nodeOf.get(slug)!)).toBe(true);
        // Distinct notes map to distinct nodes.
        expect(new Set(h.nodeOf.values()).size).toBe(notes.size);
        expect(h.root.size).toBe(notes.size);
        const check = (n: HNode): number => {
          if (n.kind === "note") return 1;
          const s = (n.own ? 1 : 0) + n.children.reduce((a, c) => a + check(c), 0);
          expect(n.size).toBe(s);
          for (const c of n.children) {
            expect(c.parent).toBe(n);
            expect(c.depth).toBe(n.depth + 1);
          }
          return s;
        };
        check(h.root);
      }),
      params,
    );
  });

  it("structural exactly when one end is the other's ancestor", () => {
    fc.assert(
      fc.property(indexArb, (ix) => {
        const h = buildHierarchy(ix);
        const anc = (a: HNode, b: HNode) => {
          for (let p = b.parent; p; p = p.parent) if (p === a) return true;
          return false;
        };
        for (const e of h.edges) {
          const A = h.nodes.get(e.source)!;
          const B = h.nodes.get(e.target)!;
          expect(e.source < e.target).toBe(true);
          expect(e.structural).toBe(anc(A, B) || anc(B, A));
        }
      }),
      params,
    );
  });

  it("does not depend on the order of the index", () => {
    fc.assert(
      fc.property(indexArb, fc.nat(1000), (ix, by) => {
        const a = buildHierarchy(ix);
        const b = buildHierarchy(shuffled(ix, by));
        const flat = (n: HNode): unknown => [n.id, n.title, n.size, n.children.map(flat)];
        expect(flat(b.root)).toEqual(flat(a.root));
        expect(b.edges).toEqual(a.edges);
      }),
      params,
    );
  });
});
