// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment happy-dom
// The engine-owned interface (src/mount.ts): what it builds, what it intercepts,
// and what it leaves behind. Run in a DOM (happy-dom), through the real browser
// scripts: nothing here stubs the drawing.
import { readdirSync, readFileSync } from "node:fs";
import { URL as NodeURL, fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mountChordGraph, type ChordGraphHandle, type MountOptions } from "../src/mount";
import { onCleanup } from "../src/browser/runtime";
import type { GraphData } from "../src/types";

const DATA: GraphData = {
  index: { title: "Home", links: ["a/one", "b/two"] },
  "a/one": { title: "One", links: ["b/two"], content: "the first note" },
  "b/two": { title: "Two", links: [] },
};

const mounted: ChordGraphHandle[] = [];
afterEach(() => {
  for (const h of mounted.splice(0)) h.dispose();
  document.body.replaceChildren();
  vi.restoreAllMocks();
  try {
    localStorage.clear();
  } catch {
    // No storage: nothing to clear.
  }
});

function mount(
  options: Partial<MountOptions> = {},
  data: GraphData | Promise<GraphData> = DATA,
): { container: HTMLElement; root: HTMLElement; handle: ChordGraphHandle } {
  const container = document.createElement("div");
  document.body.append(container);
  const handle = mountChordGraph(container, data, { current: "index", ...options });
  mounted.push(handle);
  return { container, root: container.firstElementChild as HTMLElement, handle };
}

// The data is a resolved promise and the draw awaits it: a few turns of the loop.
const settle = async () => {
  for (let i = 0; i < 3; i++) await new Promise((r) => setTimeout(r, 0));
};

const sidebarWedge = (root: HTMLElement, slug: string) =>
  root.querySelector<SVGAElement>(
    `:scope > .chord-graph-view .chord-graph-wedge[data-slug="${slug}"]`,
  );

const click = () => new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });

// ---- the contract: every selector the browser scripts query exists ----

// Every `<receiver>.querySelector(…)` / `querySelectorAll(…)` in src/browser, read
// from the source, so a new query cannot be missed by this test.
// Node's URL: under happy-dom the global one is the window's.
const BROWSER = fileURLToPath(new NodeURL("../src/browser/", import.meta.url));
const QUERY =
  /\b([A-Za-z_$][\w$]*)\s*\??\.\s*querySelector(?:All)?\s*(?:<[^>]*>)?\s*\(\s*([^)]*)\)/g;
const queries = readdirSync(BROWSER)
  .filter((f) => f.endsWith(".ts"))
  .flatMap((f) =>
    [...readFileSync(BROWSER + f, "utf8").matchAll(QUERY)].map((m) => ({
      file: f,
      receiver: m[1]!,
      arg: m[2]!.trim(),
    })),
  );
// Receivers that are the mount's own elements (the root, a view, the dialog, the
// find box): what they query must be built by mountChordGraph.
const ON_THE_MOUNT = new Set(["container", "view", "dialog", "box", "root"]);
// Receivers that hold what the scripts draw themselves: the svg's layers, and the
// frame's «You are here» overlay (browser/here.ts makes it on first use).
const DRAWN = new Set(["svg", "nodeLayer", "calloutLayer", "frame"]);

describe("mountChordGraph: the element tree the browser scripts expect", () => {
  it("finds the scripts' queries in the source (the test is not vacuous)", () => {
    const own = queries.filter((q) => ON_THE_MOUNT.has(q.receiver));
    expect(own.length).toBeGreaterThanOrEqual(20);
    expect([...new Set(queries.map((q) => q.file))]).toEqual(
      expect.arrayContaining(["chord.ts", "find.ts", "here.ts", "hier.ts"]),
    );
  });

  it("every receiver is known: a new one must be classified here", () => {
    for (const q of queries)
      expect(
        ON_THE_MOUNT.has(q.receiver) || DRAWN.has(q.receiver),
        `${q.file}: ${q.receiver}`,
      ).toBe(true);
  });

  it("every selector queried on the mount's elements exists in what it builds", () => {
    const { root } = mount(
      {
        layouts: ["chord", "sunburst", "balloon"],
        orientations: ["top-inside", "leaves-inside"],
        hereToggle: true,
      },
      new Promise<GraphData>(() => {}), // never drawn: the tree as built
    );
    const own = queries.filter((q) => ON_THE_MOUNT.has(q.receiver));
    for (const q of own) {
      const literal = /^(["'`])([^"'`$]*)\1$/.exec(q.arg);
      expect(
        literal,
        `${q.file}: ${q.receiver}.querySelector(${q.arg}) is not a fixed selector`,
      ).not.toBeNull();
      const selector = literal![2]!;
      expect(root.querySelector(selector), `${q.file}: ${selector}`).not.toBeNull();
    }
  });

  it("the optional controls only when asked for", () => {
    const { root } = mount({}, new Promise<GraphData>(() => {}));
    expect(root.classList.contains("chord-graph")).toBe(true);
    for (const absent of [".chord-graph-layouts", ".chord-graph-flip", ".chord-graph-here"])
      expect(root.querySelector(absent), absent).toBeNull();
    expect(JSON.parse(root.dataset.cfg ?? "")).toMatchObject({
      here: { shown: true, toggle: false },
      markVisited: true,
    });
    // The engine's own options stay out of the page.
    expect(root.dataset.cfg).not.toContain("current");
  });

  it("a removed «You are here» setting throws, and mounts nothing", () => {
    const container = document.createElement("div");
    expect(() =>
      mountChordGraph(container, DATA, { current: "index", hereMarker: "callout" as never }),
    ).toThrow(/was removed/);
    expect(container.childNodes.length).toBe(0);
  });

  it("a title with markup is text, never markup", async () => {
    const evil = '<img src=x onerror="alert(1)">';
    const { root } = mount({}, { index: { title: evil, links: [] } });
    await settle();
    expect(root.querySelector("img")).toBeNull();
    expect(root.querySelector(":scope > .chord-graph-view .chord-graph-caption")?.textContent).toBe(
      evil,
    );
  });
});

// ---- clicks: navigate or the browser ----

describe("a click on a note", () => {
  it("navigate absent: not intercepted (the browser follows the link)", async () => {
    const { root } = mount();
    await settle();
    const wedge = sidebarWedge(root, "a/one");
    expect(wedge).not.toBeNull();
    let reached: boolean | null = null;
    // Past the graph's own listener, as the browser's default action would be.
    root.addEventListener("click", (ev) => {
      reached = ev.defaultPrevented;
      ev.preventDefault(); // no real navigation in the test's window
    });
    wedge!.firstElementChild!.dispatchEvent(click());
    expect(reached).toBe(false);
  });

  it("navigate given: the same click opens the page through it, and goes no further", async () => {
    const navigate = vi.fn();
    const { root } = mount({ navigate, basePath: "/kb" });
    await settle();
    const ev = click();
    sidebarWedge(root, "a/one")!.firstElementChild!.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(true);
    expect(navigate).toHaveBeenCalledTimes(1);
    expect((navigate.mock.calls[0]![0] as URL).pathname).toBe("/kb/a/one");
  });
});

// ---- the lifetime: update and dispose ----

describe("update and dispose", () => {
  it("update runs the pending cleanups, marks the page visited and draws it as current", async () => {
    const { root, handle } = mount();
    await settle();
    const ran = vi.fn();
    onCleanup(root.querySelector(".chord-graph-svg")!, ran);
    handle.update("b/two");
    expect(ran).toHaveBeenCalledTimes(1);
    expect(JSON.parse(localStorage.getItem("chord-graph-visited") ?? "[]")).toContain("b/two");
    await settle();
    expect(sidebarWedge(root, "b/two")?.getAttribute("aria-current")).toBe("page");
    expect(sidebarWedge(root, "index")?.getAttribute("aria-current")).toBeNull();
  });

  it("markVisited false: nothing is stored", async () => {
    const { handle } = mount({ markVisited: false });
    handle.update("b/two");
    await settle();
    expect(localStorage.getItem("chord-graph-visited")).toBeNull();
  });

  it("dispose runs the registered cleanups, stops the drawing's listeners, and empties the container", async () => {
    const navigate = vi.fn();
    const { container, root, handle } = mount({ navigate });
    await settle();
    const wedge = sidebarWedge(root, "a/one")!.firstElementChild!;
    wedge.dispatchEvent(click());
    expect(navigate).toHaveBeenCalledTimes(1); // live before
    const ran = vi.fn();
    onCleanup(root.querySelector(".chord-graph-svg")!, ran);
    handle.dispose();
    expect(ran).toHaveBeenCalledTimes(1);
    expect(container.childNodes.length).toBe(0);
    wedge.dispatchEvent(click());
    expect(navigate).toHaveBeenCalledTimes(1); // and dead after
    handle.dispose(); // twice is harmless
    expect(ran).toHaveBeenCalledTimes(1);
  });

  it("disposed while the data is on its way: nothing is wired up when it arrives", async () => {
    let arrive: (d: GraphData) => void = () => {};
    const { root, handle } = mount({}, new Promise<GraphData>((r) => (arrive = r)));
    const input = root.querySelector<HTMLInputElement>(".chord-graph-find-input")!;
    handle.dispose();
    arrive(DATA);
    await settle();
    expect(input.hasAttribute("aria-controls")).toBe(false);
    expect(root.querySelector(".chord-graph-wedge")).toBeNull();
  });
});

// ---- the page body's links ----

describe("bodyLinks: the page body's links peek at their notes", () => {
  const listenerTypes = (spy: { mock: { calls: unknown[][] } }) =>
    spy.mock.calls.map((c) => c[0]).filter((t) => t === "pointerover" || t === "pointerout");

  it("absent: no listener on the document", async () => {
    const add = vi.spyOn(document, "addEventListener");
    mount();
    await settle();
    expect(listenerTypes(add)).toEqual([]);
  });

  it("given: one pair of listeners per document however many mounts, removed with the last", async () => {
    const add = vi.spyOn(document, "addEventListener");
    const remove = vi.spyOn(document, "removeEventListener");
    const a = mount({ bodyLinks: "article a.internal" });
    const b = mount({ bodyLinks: "article a.internal" });
    await settle();
    expect(listenerTypes(add)).toEqual(["pointerover", "pointerout"]);
    a.handle.dispose();
    expect(listenerTypes(remove)).toEqual([]);
    b.handle.dispose();
    expect(listenerTypes(remove).sort()).toEqual(["pointerout", "pointerover"]);
  });

  it("a body link under the selector and inside the base path peeks; others do not", async () => {
    const { root } = mount({ bodyLinks: "article a.internal", basePath: "/kb" });
    await settle();
    const article = document.createElement("article");
    const link = (href: string, cls = "internal") => {
      const a = document.createElement("a");
      a.className = cls;
      a.href = href;
      a.textContent = href;
      article.append(a);
      return a;
    };
    const inside = link("/kb/a/one");
    const outside = link("/a/one"); // outside the base path: no note
    const plain = link("/kb/a/one", "external"); // not under the selector
    document.body.append(article);
    const peeked = () => sidebarWedge(root, "a/one")!.classList.contains("peek");

    for (const a of [outside, plain]) {
      a.dispatchEvent(new MouseEvent("pointerover", { bubbles: true }));
      expect(peeked(), a.className + " " + a.getAttribute("href")).toBe(false);
    }
    inside.dispatchEvent(new MouseEvent("pointerover", { bubbles: true }));
    expect(peeked()).toBe(true);
    inside.dispatchEvent(new MouseEvent("pointerout", { bubbles: true }));
    expect(peeked()).toBe(false);
  });
});
