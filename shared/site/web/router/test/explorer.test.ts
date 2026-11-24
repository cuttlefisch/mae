// SPDX-License-Identifier: GPL-3.0-or-later
// The explorer (P-39 to P-45, P-156): its decisions as pure functions over
// varied inputs, then the behaviour and the router's handling of its region
// in a DOM (happy-dom), the page and the list fetches stubbed at `fetch`.
import { readFileSync } from "node:fs";
import path from "node:path";
import { Window } from "happy-dom";
import { describe, expect, it, vi } from "vitest";
import {
  EXPLORER_ID,
  VISIBLE_SLACK,
  chooseOccurrence,
  explorer,
  fullyVisible,
  keepExplorer,
  listView,
  parseExplorer,
  revealScrollTop,
  start,
  type ExplorerData,
  type Occurrence,
  type Span,
} from "../src/router";

const ORIGIN = "https://site.example";
const HERE = new URL(`${ORIGIN}/kb/k/a`);

/** A deterministic generator (mulberry32): varied inputs, reproducible failures. */
function rng(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// ---------------------------------------------------------------------------
// Pure functions.

describe("parseExplorer", () => {
  const valid = {
    groups: [
      { tag: "zeta", pages: [{ title: "B", href: "/kb/k/b" }] },
      {
        tag: "alpha",
        pages: [
          { title: "A", href: "/kb/k/a" },
          { title: "<b>", href: "/kb/k/c" },
        ],
      },
    ],
    untagged: [{ title: "U", href: "/kb/k/u" }],
  };

  it("keeps the daemon's order exactly, never re-sorting", () => {
    const next = rng(7);
    for (let round = 0; round < 20; round++) {
      const tags = Array.from(
        { length: 1 + Math.floor(next() * 6) },
        (_, i) => `t${Math.floor(next() * 100)}-${i}`,
      );
      const data = {
        groups: tags.map((tag) => ({
          tag,
          pages: Array.from({ length: Math.floor(next() * 4) }, (_, i) => ({
            title: `p${Math.floor(next() * 1000)}`,
            href: `/kb/k/${tag}-${i}`,
          })),
        })),
        untagged: [],
      };
      expect(parseExplorer(JSON.parse(JSON.stringify(data)), HERE)).toEqual(
        data,
      );
    }
    expect(parseExplorer(valid, HERE)).toEqual(valid);
  });

  it("refuses anything not of the daemon's shape, or any link off the site", () => {
    const page = (href: unknown, title: unknown = "t") => ({
      groups: [{ tag: "x", pages: [{ title, href }] }],
      untagged: [],
    });
    const bad: unknown[] = [
      null,
      [],
      "x",
      42,
      {},
      { groups: [] },
      { untagged: [] },
      { groups: {}, untagged: [] },
      { groups: [], untagged: {} },
      { groups: [null], untagged: [] },
      { groups: [{ tag: 3, pages: [] }], untagged: [] },
      { groups: [{ pages: [] }], untagged: [] },
      { groups: [{ tag: "x" }], untagged: [] },
      { groups: [], untagged: [{ title: "t" }] },
      { groups: [], untagged: [{ href: "/kb/k/a" }] },
      page("/kb/k/a", 5),
      page("/kb/k/a", null),
      page(7),
      page("javascript:alert(1)"),
      page("data:text/html,x"),
      page("https://other.example/kb/k/a"),
      page("//other.example/kb/k/a"),
      page("/site-assets/x.css"),
      page("/kb/../_/salir"),
      page("http://[bad"),
    ];
    for (const v of bad)
      expect(parseExplorer(v, HERE), JSON.stringify(v)).toBeNull();
  });
});

describe("chooseOccurrence (P-39)", () => {
  /** Every combination of open/visible for n occurrences. */
  function* all(n: number): Generator<Occurrence[]> {
    for (let bits = 0; bits < 1 << (2 * n); bits++) {
      yield Array.from({ length: n }, (_, i) => ({
        open: !!(bits & (1 << (2 * i))),
        visible: !!(bits & (1 << (2 * i + 1))),
      }));
    }
  }

  it("none listed: none chosen", () => {
    expect(chooseOccurrence([], null)).toBe(-1);
    expect(chooseOccurrence([], 0)).toBe(-1);
  });

  it("the clicked one; else the first open and visible; else the first", () => {
    for (let n = 1; n <= 4; n++) {
      for (const occ of all(n)) {
        const seen = occ.findIndex((o) => o.open && o.visible);
        const fallback = seen >= 0 ? seen : 0;
        expect(chooseOccurrence(occ, null), JSON.stringify(occ)).toBe(fallback);
        for (let c = 0; c < n; c++) expect(chooseOccurrence(occ, c)).toBe(c);
        // A click on an occurrence that is no longer listed counts for nothing.
        for (const c of [-1, n, n + 3])
          expect(chooseOccurrence(occ, c)).toBe(fallback);
      }
    }
  });
});

describe("the list's scroll (P-44, P-45)", () => {
  it("listView is the scroll box inside the borders, cut to the window", () => {
    expect(listView({ top: 40, bottom: 840 }, 1, 798, 600)).toEqual({
      top: 41,
      bottom: 600,
    });
    expect(listView({ top: -30, bottom: 300 }, 0, 330, 600)).toEqual({
      top: 0,
      bottom: 300,
    });
    expect(listView({ top: 10, bottom: 210 }, 2, 196, 900)).toEqual({
      top: 12,
      bottom: 208,
    });
  });

  it("fullyVisible tolerates sub-pixel rounding only", () => {
    const view = { top: 100, bottom: 300 };
    expect(fullyVisible({ top: 100, bottom: 300 }, view)).toBe(true);
    expect(
      fullyVisible(
        { top: 100 - VISIBLE_SLACK, bottom: 300 + VISIBLE_SLACK },
        view,
      ),
    ).toBe(true);
    expect(fullyVisible({ top: 99, bottom: 120 }, view)).toBe(false);
    expect(fullyVisible({ top: 280, bottom: 301 }, view)).toBe(false);
  });

  it("leaves a visible entry where it is and otherwise moves the least that shows it", () => {
    const next = rng(42);
    let unchanged = 0;
    let moved = 0;
    for (let i = 0; i < 2000; i++) {
      // A list of `content` px scrolled to `scrollTop`, its view at `viewTop`
      // on screen and `viewH` tall; an entry at `at` in the list, `h` tall.
      const viewH = 20 + Math.floor(next() * 600);
      const content = viewH + Math.floor(next() * 3000);
      const scrollTop = Math.floor(next() * (content - viewH + 1));
      const h = 1 + Math.floor(next() * (next() < 0.1 ? viewH * 2 : 60));
      const at = Math.floor(next() * Math.max(1, content - h));
      const viewTop = Math.floor(next() * 200) - 50;
      const view: Span = { top: viewTop, bottom: viewTop + viewH };
      const onScreen = (st: number): Span => ({
        top: viewTop + at - st,
        bottom: viewTop + at + h - st,
      });
      const before = onScreen(scrollTop);
      const out = revealScrollTop(before, view, scrollTop);
      const ctx = JSON.stringify({
        viewH,
        content,
        scrollTop,
        h,
        at,
        viewTop,
        out,
      });
      expect(out, ctx).toBeGreaterThanOrEqual(0);
      if (fullyVisible(before, view)) {
        expect(out, ctx).toBe(scrollTop);
        unchanged++;
        continue;
      }
      moved++;
      const after = onScreen(out);
      if (h <= viewH) {
        expect(fullyVisible(after, view), ctx).toBe(true);
        // The least move: the entry lands on the edge it came in from.
        if (before.top < view.top) expect(after.top, ctx).toBe(view.top);
        else expect(after.bottom, ctx).toBe(view.bottom);
      } else {
        // Taller than the view: its top is shown.
        expect(after.top, ctx).toBe(view.top);
      }
    }
    // The generator exercised both outcomes, not one.
    expect(unchanged).toBeGreaterThan(100);
    expect(moved).toBeGreaterThan(100);
  });
});

describe("keepExplorer", () => {
  const win = new Window();
  const doc = win.document;
  const nav = (attrs: Record<string, string>) => {
    const e = doc.createElement("nav");
    for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
    return e as unknown as Element;
  };

  it("keeps the same list and takes the next page's words, version and language", () => {
    const mine = nav({
      "data-src": "/kb/k/explorer.json",
      "data-version": "a".repeat(16),
      "aria-label": "Pages",
      "data-untagged": "Untagged",
      lang: "en-US",
    });
    const theirs = nav({
      "data-src": "/kb/k/explorer.json",
      "data-version": "b".repeat(16),
      "aria-label": "Páginas",
      "data-untagged": "Sin etiqueta",
    });
    expect(keepExplorer(mine, theirs)).toBe(true);
    expect(mine.getAttribute("data-version")).toBe("b".repeat(16));
    expect(mine.getAttribute("aria-label")).toBe("Páginas");
    expect(mine.getAttribute("data-untagged")).toBe("Sin etiqueta");
    expect(mine.hasAttribute("lang")).toBe(false);
    expect(
      keepExplorer(
        mine,
        nav({ "data-src": "/kb/k/explorer.json", lang: "en-US" }),
      ),
    ).toBe(true);
    expect(mine.getAttribute("lang")).toBe("en-US");
  });

  it("replaces another list, and never matches on a missing src", () => {
    const mine = nav({
      "data-src": "/kb/one/explorer.json",
      "data-version": "1".repeat(16),
    });
    expect(
      keepExplorer(
        mine,
        nav({
          "data-src": "/kb/two/explorer.json",
          "data-version": "2".repeat(16),
        }),
      ),
    ).toBe(false);
    expect(mine.getAttribute("data-version")).toBe("1".repeat(16));
    expect(keepExplorer(nav({}), nav({}))).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// In a DOM.

const ASSETS =
  '<link rel="stylesheet" href="/site-assets/base-1.css"><script type="module" src="/site-assets/router-1.js"></script>';
const V1 = "0123456789abcdef";
const V2 = "fedcba9876543210";

interface NavOpts {
  src?: string;
  version?: string;
  lang?: string;
  label?: string;
  untagged?: string;
}

function navHtml(o: NavOpts = {}): string {
  const lang = o.lang ? ` lang="${o.lang}"` : "";
  return `<nav id="mae-explorer" class="mae-explorer" aria-label="${o.label ?? "Pages"}" data-src="${o.src ?? "/kb/k/explorer.json"}" data-version="${o.version ?? V1}" data-untagged="${o.untagged ?? "Untagged"}"${lang} hidden></nav>`;
}

/** A site page as the daemon writes it: header, explorer, main, panels. */
function page(o: {
  title: string;
  body?: string;
  nav?: NavOpts | null;
  header?: boolean;
  htmlLang?: string;
}): string {
  const header =
    o.header === false
      ? ""
      : `<header id="mae-header"><a href="/kb/k/">k</a></header>`;
  const nav = o.nav === null ? "" : navHtml(o.nav);
  return `<!doctype html><html lang="${o.htmlLang ?? "en-US"}"><head><meta charset="utf-8">${ASSETS}<title>${o.title}</title></head><body>${header}${nav}<main id="mae-main"><article><h1>${o.title}</h1>${o.body ?? ""}</article></main><aside id="mae-panels">p</aside></body></html>`;
}

/** The fixture list: page `a` under three tags, `b` under one, `u` untagged. */
const LIST: ExplorerData = {
  groups: [
    {
      tag: "one",
      pages: [
        { title: "Alpha", href: "/kb/k/a" },
        { title: "Beta", href: "/kb/k/b" },
      ],
    },
    { tag: "two", pages: [{ title: "Alpha", href: "/kb/k/a" }] },
    {
      tag: "three",
      pages: [
        { title: "Beta", href: "/kb/k/b" },
        { title: "Alpha", href: "/kb/k/a" },
      ],
    },
  ],
  untagged: [{ title: "Untitled <b>bold</b> & co", href: "/kb/k/u" }],
};

interface Served {
  status?: number;
  type?: string;
  contentType?: string;
  body?: string;
  reject?: boolean;
  delay?: Promise<void>;
}

const json = (data: unknown): Served => ({
  contentType: "application/json",
  body: JSON.stringify(data),
});

/** A window at `url` showing `html`; `serve` answers every fetch. */
function setup(url: string, html: string, serve: (u: string) => Served) {
  const win = new Window({ url });
  win.document.write(html);
  const requests: Array<{ url: string; init: RequestInit }> = [];
  const fetch = vi.fn(async (u: string, init: RequestInit) => {
    requests.push({ url: u, init });
    const s = serve(u);
    if (s.delay) await s.delay;
    if (init.signal?.aborted) throw new DOMException("aborted", "AbortError");
    if (s.reject) throw new TypeError("network");
    return {
      type: s.type ?? "basic",
      status: s.status ?? 200,
      headers: { get: () => s.contentType ?? "text/html; charset=utf-8" },
      text: async () => s.body ?? "",
    };
  });
  (win as unknown as { fetch: unknown }).fetch = fetch;
  Object.defineProperty(win.location, "assign", {
    value: vi.fn(),
    configurable: true,
  });
  const scrollTo = vi.fn();
  (win as unknown as { scrollTo: unknown }).scrollTo = scrollTo;
  const scrollIntoView = vi.spyOn(win.Element.prototype, "scrollIntoView");
  const dom = win as unknown as Window & typeof globalThis;
  const router = start(dom as unknown as Parameters<typeof start>[0]);
  const ex = explorer(dom as unknown as Parameters<typeof explorer>[0]);
  router.register("explorer", ex);
  const lists = () => requests.filter((r) => r.url.endsWith("/explorer.json"));
  const settled = async () => {
    await router.pending();
    await ex.idle();
  };
  return {
    win,
    doc: win.document as unknown as Document,
    router,
    ex,
    requests,
    lists,
    scrollTo,
    scrollIntoView,
    settled,
  };
}

/** Pages a, b, u of KB k, each with the explorer; the list served from LIST. */
function site(
  o: {
    version?: (u: string) => string;
    list?: (u: string) => Served;
    navLang?: string;
    htmlLang?: string;
  } = {},
) {
  return (u: string): Served => {
    if (u.endsWith("/explorer.json")) return o.list ? o.list(u) : json(LIST);
    const id = new URL(u).pathname.split("/").pop() ?? "";
    return {
      body: page({
        title: id,
        body: links,
        nav: { version: o.version?.(u) ?? V1, lang: o.navLang },
        htmlLang: o.htmlLang,
      }),
    };
  };
}
const links =
  '<a id="go-a" href="/kb/k/a">a</a><a id="go-b" href="/kb/k/b">b</a><a id="go-u" href="/kb/k/u">u</a>';
const start_ = (
  url: string,
  serve = site(),
  o: { navLang?: string; htmlLang?: string } = {},
) =>
  setup(
    url,
    page({
      title: "start",
      body: links,
      nav: { lang: o.navLang },
      htmlLang: o.htmlLang,
    }),
    serve,
  );

type Doc = Document;
const nav = (doc: Doc) => doc.getElementById(EXPLORER_ID)! as HTMLElement;
const groups = (doc: Doc) => Array.from(nav(doc).querySelectorAll("details"));
const current = (doc: Doc) =>
  Array.from(nav(doc).querySelectorAll('[aria-current="page"]'));
/** Which group (by summary text) holds the marked entry. */
const markedIn = (doc: Doc) =>
  current(doc).map(
    (a) => a.closest("details")!.querySelector("summary")!.textContent,
  );
const openGroups = (doc: Doc) =>
  groups(doc)
    .filter((d) => d.hasAttribute("open"))
    .map((d) => d.querySelector("summary")!.textContent);

function clickOn(el: Element): void {
  const win = el.ownerDocument.defaultView!;
  el.dispatchEvent(
    new win.MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }),
  );
}

describe("the explorer's list", () => {
  it("is built from the JSON with DOM nodes only, in the daemon's order, then shown", async () => {
    const t = start_(`${ORIGIN}/kb/k/b`);
    expect(nav(t.doc).hidden).toBe(true);
    await t.settled();
    const n = nav(t.doc);
    expect(n.hidden).toBe(false);
    expect(t.lists()).toHaveLength(1);
    const init = t.lists()[0]!.init;
    expect([
      init.method,
      init.credentials,
      init.redirect,
      init.headers,
    ]).toEqual(["GET", "same-origin", "manual", undefined]);
    expect(Array.from(n.children).map((c) => c.tagName)).toEqual([
      "DETAILS",
      "DETAILS",
      "DETAILS",
      "DETAILS",
    ]);
    const shape = groups(t.doc).map((d) => ({
      tag: d.querySelector("summary")!.textContent,
      pages: Array.from(d.querySelectorAll("ul > li > a")).map((a) => ({
        title: a.textContent,
        href: a.getAttribute("href"),
      })),
    }));
    expect(shape).toEqual([
      ...LIST.groups,
      { tag: "Untagged", pages: LIST.untagged },
    ]);
    // A title is text, never markup.
    expect(n.querySelector("b")).toBeNull();
  });

  it("renders no untagged group when there is no untagged page", async () => {
    const t = start_(
      `${ORIGIN}/kb/k/a`,
      site({ list: () => json({ ...LIST, untagged: [] }) }),
    );
    await t.settled();
    expect(
      groups(t.doc).map((d) => d.querySelector("summary")!.textContent),
    ).toEqual(["one", "two", "three"]);
  });

  it("stays hidden and empty on any failure (fail closed)", async () => {
    const failures: Served[] = [
      {
        status: 404,
        contentType: "application/json",
        body: JSON.stringify(LIST),
      },
      {
        status: 500,
        contentType: "application/json",
        body: JSON.stringify(LIST),
      },
      { type: "opaqueredirect", status: 0 },
      { contentType: "text/html", body: JSON.stringify(LIST) },
      { contentType: "application/json", body: "{not json" },
      {
        contentType: "application/json",
        body: JSON.stringify({ groups: LIST.groups }),
      },
      json({
        groups: [
          {
            tag: "x",
            pages: [{ title: "x", href: "https://evil.example/kb/k/x" }],
          },
        ],
        untagged: [],
      }),
      { reject: true },
    ];
    for (const f of failures) {
      const t = start_(`${ORIGIN}/kb/k/a`, site({ list: () => f }));
      await t.settled();
      expect(nav(t.doc).hidden, JSON.stringify(f)).toBe(true);
      expect(nav(t.doc).children.length, JSON.stringify(f)).toBe(0);
    }
  });

  it("never fetches a list off the site", async () => {
    for (const src of [
      "https://evil.example/kb/k/explorer.json",
      "/elsewhere/explorer.json",
      "javascript:1",
    ]) {
      const t = setup(
        `${ORIGIN}/kb/k/a`,
        page({ title: "a", nav: { src } }),
        () => json(LIST),
      );
      await t.settled();
      expect(t.requests, src).toHaveLength(0);
      expect(nav(t.doc).hidden).toBe(true);
    }
  });

  it("hides a shown list whose new version fails to load", async () => {
    let served = 0;
    const t = start_(
      `${ORIGIN}/kb/k/a`,
      site({
        version: () => V2,
        list: () => (++served > 1 ? { status: 500 } : json(LIST)),
      }),
    );
    await t.settled();
    expect(nav(t.doc).hidden).toBe(false);
    clickOn(t.doc.getElementById("go-b")!);
    await t.settled();
    expect(t.lists()).toHaveLength(2);
    expect(nav(t.doc).hidden).toBe(true);
    expect(nav(t.doc).children.length).toBe(0);
  });

  it("fetches again only when the version changes, keeping the reader's open groups", async () => {
    let version = V1;
    const t = start_(`${ORIGIN}/kb/k/u`, site({ version: () => version }));
    await t.settled();
    const before = nav(t.doc);
    expect(openGroups(t.doc)).toEqual(["Untagged"]);
    groups(t.doc)[1]!.setAttribute("open", "");
    for (const id of ["go-b", "go-u", "go-b"]) {
      clickOn(t.doc.getElementById(id)!);
      await t.settled();
    }
    expect(t.lists()).toHaveLength(1);
    expect(nav(t.doc)).toBe(before);
    expect(groups(t.doc)).toHaveLength(4);
    version = V2;
    clickOn(t.doc.getElementById("go-u")!);
    await t.settled();
    expect(t.lists()).toHaveLength(2);
    expect(nav(t.doc).getAttribute("data-version")).toBe(V2);
    // Rebuilt, not appended, with the groups the reader had open still open.
    expect(groups(t.doc)).toHaveLength(4);
    expect(openGroups(t.doc)).toEqual(["one", "two", "Untagged"]);
  });

  it("aborts the list fetch in flight when the page goes, and never renders it", async () => {
    let release!: () => void;
    const slow = new Promise<void>((r) => (release = r));
    const t = start_(
      `${ORIGIN}/kb/k/a`,
      site({ list: () => ({ ...json(LIST), delay: slow }) }),
    );
    const loading = t.ex.idle();
    const signal = t.lists()[0]!.init.signal!;
    expect(signal.aborted).toBe(false);
    // The next page names the same list: the nav is kept, the fetch begun again.
    void t.router.navigate(new URL(`${ORIGIN}/kb/k/b`), true);
    await t.router.pending();
    expect(signal.aborted).toBe(true);
    release();
    await loading;
    await t.ex.idle();
    expect(current(t.doc)).toHaveLength(1);
    expect(groups(t.doc)).toHaveLength(4);
  });

  it("an unmount alone aborts the fetch, and the list stays hidden", async () => {
    let release!: () => void;
    const slow = new Promise<void>((r) => (release = r));
    const win = new Window({ url: `${ORIGIN}/kb/k/a` });
    win.document.write(page({ title: "a" }));
    let signal: AbortSignal | undefined;
    (win as unknown as { fetch: unknown }).fetch = async (
      _u: string,
      init: RequestInit,
    ) => {
      signal = init.signal ?? undefined;
      await slow;
      return {
        type: "basic",
        status: 200,
        headers: { get: () => "application/json" },
        text: async () => JSON.stringify(LIST),
      };
    };
    const ex = explorer(win as unknown as Parameters<typeof explorer>[0]);
    const cleanup = ex.mount(win.document as unknown as Document);
    const loading = ex.idle();
    expect(typeof cleanup).toBe("function");
    (cleanup as () => void)();
    expect(signal?.aborted).toBe(true);
    release();
    await loading;
    expect(nav(win.document as unknown as Document).hidden).toBe(true);
    expect(nav(win.document as unknown as Document).children.length).toBe(0);
  });

  it("gives tags and titles the page's language when the nav's words fell back", async () => {
    const t = start_(
      `${ORIGIN}/kb/k/a`,
      site({ navLang: "en-US", htmlLang: "es-ES" }),
      { navLang: "en-US", htmlLang: "es-ES" },
    );
    await t.settled();
    const n = nav(t.doc);
    const summaries = Array.from(n.querySelectorAll("summary > span"));
    expect(summaries.map((s) => s.getAttribute("lang"))).toEqual([
      "es-ES",
      "es-ES",
      "es-ES",
      null,
    ]);
    expect(
      Array.from(n.querySelectorAll("a")).every(
        (a) => a.getAttribute("lang") === "es-ES",
      ),
    ).toBe(true);
    const plain = start_(`${ORIGIN}/kb/k/a`, site({ htmlLang: "es-ES" }), {
      htmlLang: "es-ES",
    });
    await plain.settled();
    expect(nav(plain.doc).querySelectorAll("[lang]")).toHaveLength(0);
  });
});

describe("the current page (P-39): exactly one occurrence marked", () => {
  it("first render: the first occurrence, and only its group opens", async () => {
    const t = start_(`${ORIGIN}/kb/k/a`);
    await t.settled();
    expect(current(t.doc)).toHaveLength(1);
    expect(markedIn(t.doc)).toEqual(["one"]);
    expect(openGroups(t.doc)).toEqual(["one"]);
    expect(current(t.doc)[0]!.getAttribute("href")).toBe("/kb/k/a");
  });

  it("the occurrence the reader clicked, in any of its groups", async () => {
    for (const [group, want] of [
      [2, "three"],
      [1, "two"],
      [0, "one"],
    ] as const) {
      const t = start_(`${ORIGIN}/kb/k/b`);
      await t.settled();
      const g = groups(t.doc)[group]!;
      g.setAttribute("open", "");
      const entry = Array.from(g.querySelectorAll("a")).find(
        (a) => a.getAttribute("href") === "/kb/k/a",
      )!;
      clickOn(entry);
      await t.settled();
      expect(t.win.location.pathname).toBe("/kb/k/a");
      expect(current(t.doc), want).toHaveLength(1);
      expect(markedIn(t.doc)).toEqual([want]);
    }
  });

  it("from elsewhere: an occurrence in an open group the reader can see; else the first, opened", async () => {
    const t = start_(`${ORIGIN}/kb/k/u`);
    await t.settled();
    // Groups two and three open, one closed: the first visible is in two.
    groups(t.doc)[1]!.setAttribute("open", "");
    groups(t.doc)[2]!.setAttribute("open", "");
    clickOn(t.doc.getElementById("go-a")!);
    await t.settled();
    expect(markedIn(t.doc)).toEqual(["two"]);
    expect(openGroups(t.doc)).toEqual(["two", "three", "Untagged"]);
    // All closed: the first occurrence, its group opened, nothing else.
    for (const d of groups(t.doc)) d.removeAttribute("open");
    clickOn(t.doc.getElementById("go-b")!);
    await t.settled();
    expect(markedIn(t.doc)).toEqual(["one"]);
    expect(openGroups(t.doc)).toEqual(["one"]);
    clickOn(t.doc.getElementById("go-u")!);
    await t.settled();
    expect(current(t.doc)).toHaveLength(1);
    expect(markedIn(t.doc)).toEqual(["Untagged"]);
  });

  it("an open group whose occurrence is scrolled out of the list's view does not count", async () => {
    const t = start_(`${ORIGIN}/kb/k/u`);
    await t.settled();
    const n = nav(t.doc);
    const layout = fakeLayout(n, 100);
    for (const d of groups(t.doc)) d.setAttribute("open", "");
    layout.scrollTop = 0;
    // Entries 30 px apart, in order: one/Alpha 0, one/Beta 30, two/Alpha 60,
    // three/Beta 90, three/Alpha 120, Untagged/u 150; the view shows 0..100.
    clickOn(t.doc.getElementById("go-b")!);
    await t.settled();
    expect(markedIn(t.doc)).toEqual(["one"]);
    // Scrolled so that only three's entries are in view: three's Alpha is chosen.
    layout.scrollTop = 95;
    clickOn(t.doc.getElementById("go-a")!);
    await t.settled();
    expect(markedIn(t.doc)).toEqual(["three"]);
    expect(layout.scrollTop).toBe(95);
  });
});

/**
 * A layout for the list: a scroll box `height` px tall at the top of the
 * window, every link 20 px tall and 30 px below the previous one, so a link's
 * place on screen follows the list's scrollTop.
 */
function fakeLayout(n: HTMLElement, height: number) {
  const state = { scrollTop: 0 };
  Object.defineProperty(n, "scrollTop", {
    get: () => state.scrollTop,
    set: (v: number) => (state.scrollTop = v),
    configurable: true,
  });
  Object.defineProperty(n, "clientTop", { value: 0, configurable: true });
  Object.defineProperty(n, "clientHeight", {
    value: height,
    configurable: true,
  });
  Object.defineProperty(n, "getBoundingClientRect", {
    value: () => ({
      top: 0,
      bottom: height,
      left: 0,
      right: 200,
      width: 200,
      height,
    }),
    configurable: true,
  });
  const place = () =>
    Array.from(n.querySelectorAll("a")).forEach((a, i) =>
      Object.defineProperty(a, "getBoundingClientRect", {
        value: () => ({
          top: i * 30 - state.scrollTop,
          bottom: i * 30 + 20 - state.scrollTop,
          left: 0,
          right: 200,
          width: 200,
          height: 20,
        }),
        configurable: true,
      }),
    );
  place();
  return state;
}

describe("the list's scroll on navigation (P-44, P-45)", () => {
  it("scrolls only the list, to an entry out of its view, and leaves a visible one", async () => {
    const t = start_(`${ORIGIN}/kb/k/u`);
    await t.settled();
    const n = nav(t.doc);
    const layout = fakeLayout(n, 100);
    for (const d of groups(t.doc)) d.setAttribute("open", "");
    layout.scrollTop = 40;
    // The view shows 40..140. b's first occurrence, one/Beta (30-50), is cut;
    // three/Beta (90-110) is open and visible: marked, and nothing scrolls.
    clickOn(t.doc.getElementById("go-b")!);
    await t.settled();
    expect(markedIn(t.doc)).toEqual(["three"]);
    expect(layout.scrollTop).toBe(40);
    // u (Untagged, 150-170) is out of view: the list scrolls to show its bottom.
    clickOn(t.doc.getElementById("go-u")!);
    await t.settled();
    expect(markedIn(t.doc)).toEqual(["Untagged"]);
    expect(layout.scrollTop).toBe(70);
    expect(t.scrollIntoView).not.toHaveBeenCalled();
    // The window moved only as the router moves it on any navigation (to the top).
    for (const call of t.scrollTo.mock.calls) expect(call).toEqual([0, 0]);
  });
});

describe("the explorer's region, under the router", () => {
  it("is kept with the same list, replaced with another, removed and inserted with the page", async () => {
    const serve = (u: string): Served => {
      if (u.endsWith("/explorer.json")) return json(LIST);
      const p = new URL(u).pathname;
      if (p === "/kb/other/x")
        return {
          body: page({ title: "x", nav: { src: "/kb/other/explorer.json" } }),
        };
      if (p === "/kb/k/q")
        return { body: page({ title: "q", nav: null, header: false }) };
      return {
        body: page({
          title: "b",
          nav: {
            label: "Páginas",
            untagged: "Sin etiqueta",
            version: V1,
            lang: "en-US",
          },
        }),
      };
    };
    const t = setup(
      `${ORIGIN}/kb/k/a`,
      page({ title: "a", nav: { lang: undefined } }),
      serve,
    );
    await t.settled();
    const first = nav(t.doc);
    void t.router.navigate(new URL(`${ORIGIN}/kb/k/b`), true);
    await t.settled();
    expect(nav(t.doc)).toBe(first);
    expect(first.getAttribute("aria-label")).toBe("Páginas");
    expect(first.getAttribute("lang")).toBe("en-US");
    expect(first.hidden).toBe(false);
    // The untagged group's name follows the page's words without a refetch.
    expect(groups(t.doc).at(-1)!.querySelector("summary")!.textContent).toBe(
      "Sin etiqueta",
    );
    expect(t.lists()).toHaveLength(1);
    void t.router.navigate(new URL(`${ORIGIN}/kb/other/x`), true);
    await t.settled();
    expect(nav(t.doc)).not.toBe(first);
    expect(nav(t.doc).getAttribute("data-src")).toBe("/kb/other/explorer.json");
    expect(t.lists()).toHaveLength(2);
    void t.router.navigate(new URL(`${ORIGIN}/kb/k/q`), true);
    await t.settled();
    expect(t.doc.getElementById(EXPLORER_ID)).toBeNull();
    expect(t.doc.getElementById("mae-header")).toBeNull();
    void t.router.navigate(new URL(`${ORIGIN}/kb/k/b`), true);
    await t.settled();
    // Inserted in the daemon's order: header, explorer, main.
    const order = Array.from(t.doc.body.children)
      .map((c) => c.id)
      .filter((id) => id !== "mae-announcer");
    expect(order).toEqual([
      "mae-header",
      EXPLORER_ID,
      "mae-main",
      "mae-panels",
    ]);
    expect(t.doc.querySelectorAll(`#${EXPLORER_ID}`)).toHaveLength(1);
    expect(nav(t.doc).hidden).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// The stylesheet's decisions the DOM tests cannot see (the browser harness
// measures the rest).

describe("the explorer's rules in base.css", () => {
  const css = readFileSync(
    path.join(import.meta.dirname, "..", "..", "base", "base.css"),
    "utf8",
  ).replace(/\/\*[\s\S]*?\*\//g, "");
  /** The bodies of every rule for exactly `selector`, in order. */
  const rules = (selector: string): string[] => {
    const out: string[] = [];
    for (
      let at = css.indexOf(`${selector} {`);
      at >= 0;
      at = css.indexOf(`${selector} {`, at + 1)
    ) {
      if (at > 0 && !/\s/.test(css[at - 1]!)) continue;
      out.push(css.slice(css.indexOf("{", at) + 1, css.indexOf("}", at)));
    }
    return out;
  };
  const rule = (selector: string): string => {
    const found = rules(selector);
    expect(found.length, selector).toBeGreaterThan(0);
    return found[0]!;
  };

  it("keeps the nav hidden until the behaviour shows it", () => {
    expect(rule(".mae-explorer[hidden]")).toMatch(/display:\s*none/);
  });

  it("puts the content before the explorer on a narrow window", () => {
    const narrow = rules("body").find((r) =>
      r.includes("grid-template-areas"),
    )!;
    const areas = narrow
      .match(/grid-template-areas:([^;]*);/)![1]!
      .match(/"[^"]*"/g)!;
    expect(areas.indexOf('"main"')).toBeLessThan(areas.indexOf('"explorer"'));
  });

  /** WCAG relative luminance and contrast ratio, from #rrggbb. */
  const lum = (hex: string): number => {
    const c = [1, 3, 5]
      .map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
      .map((v) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
    return 0.2126 * c[0]! + 0.7152 * c[1]! + 0.0722 * c[2]!;
  };
  const ratio = (a: string, b: string): number => {
    const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
    return (x! + 0.05) / (y! + 0.05);
  };

  it("draws the current entry's bar in a token at 3:1 on the page in every palette and mode", () => {
    const bar = rule('.mae-explorer a[aria-current="page"]');
    const token = bar.match(
      /border-inline-start-color:\s*var\(--ui-([a-z]+)\)/,
    )![1]!;
    const tokens = JSON.parse(
      readFileSync(
        path.join(
          import.meta.dirname,
          "..",
          "..",
          "palette",
          "dist",
          "tokens.json",
        ),
        "utf8",
      ),
    ) as {
      palettes: Record<
        string,
        Record<"light" | "dark", { ui: Record<string, string> }>
      >;
    };
    const low: string[] = [];
    for (const [name, p] of Object.entries(tokens.palettes)) {
      for (const mode of ["light", "dark"] as const) {
        const r = ratio(p[mode].ui[token]!, p[mode].ui.background!);
        if (!(r >= 3)) low.push(`${name}/${mode}: ${r.toFixed(2)}`);
      }
    }
    expect(Object.keys(tokens.palettes).length).toBeGreaterThan(10);
    expect(low).toEqual([]);
  });
});
