// SPDX-License-Identifier: GPL-3.0-or-later
// The router's decisions as pure functions, then the whole flow in a DOM
// (happy-dom): a fresh window per test, the page fetch stubbed at `fetch`.
import { Window } from "happy-dom";
import { describe, expect, it, vi } from "vitest";
import {
  fragmentId,
  outcome,
  routableClick,
  start,
  swappable,
  type ClickFacts,
  type LinkFacts,
} from "../src/router";

const HERE = new URL("https://site.example/kb/k/a");
const click = (o: Partial<ClickFacts> = {}): ClickFacts => ({
  button: 0,
  metaKey: false,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  defaultPrevented: false,
  ...o,
});
const link = (href: string | null, o: Partial<LinkFacts> = {}): LinkFacts => ({
  href,
  target: null,
  download: false,
  ...o,
});

describe("routableClick", () => {
  it("takes a plain primary click on another page of the site", () => {
    expect(routableClick(click(), link("/kb/k/b"), HERE)?.href).toBe("https://site.example/kb/k/b");
    expect(routableClick(click(), link("b"), HERE)?.pathname).toBe("/kb/k/b");
    expect(routableClick(click(), link("/kb/k/a?x=1"), HERE)?.search).toBe("?x=1");
    expect(routableClick(click(), link("/kb/k/b#s"), HERE)?.hash).toBe("#s");
    expect(routableClick(click(), link("/kb/k/a", { target: "_self" }), HERE)).not.toBeNull();
  });

  it("leaves every other click to the browser", () => {
    const cases: Array<[ClickFacts, LinkFacts]> = [
      [click({ button: 1 }), link("/kb/k/b")],
      [click({ metaKey: true }), link("/kb/k/b")],
      [click({ ctrlKey: true }), link("/kb/k/b")],
      [click({ shiftKey: true }), link("/kb/k/b")],
      [click({ altKey: true }), link("/kb/k/b")],
      [click({ defaultPrevented: true }), link("/kb/k/b")],
      [click(), link(null)],
      [click(), link("/kb/k/b", { download: true })],
      [click(), link("/kb/k/b", { target: "_blank" })],
      [click(), link("https://other.example/kb/k/b")],
      [click(), link("/site-assets/palette-0.css")],
      [click(), link("/_/salir")],
      [click(), link("mailto:a@b.example")],
      [click(), link("#section")],
      [click(), link("/kb/k/a#section")],
      [click(), link("http://[bad")],
    ];
    for (const [c, l] of cases) expect(routableClick(c, l, HERE), JSON.stringify([c, l])).toBeNull();
  });
});

describe("fragmentId", () => {
  it("decodes a fragment and never throws on a malformed one", () => {
    expect(fragmentId("#sec%20one")).toBe("sec one");
    expect(fragmentId("#cid-1")).toBe("cid-1");
    expect(fragmentId("#%E0")).toBe("%E0");
    expect(fragmentId("")).toBe("");
  });
});

describe("outcome", () => {
  it("swaps only a plain 200 HTML page", () => {
    expect(outcome({ type: "basic", status: 200, contentType: "text/html; charset=utf-8" })).toBe(
      "swap",
    );
    for (const r of [
      { type: "opaqueredirect", status: 0, contentType: null },
      { type: "basic", status: 404, contentType: "text/html" },
      { type: "basic", status: 502, contentType: "text/html" },
      { type: "basic", status: 302, contentType: "text/html" },
      { type: "basic", status: 200, contentType: "application/json" },
      { type: "basic", status: 200, contentType: null },
    ]) {
      expect(outcome(r), JSON.stringify(r)).toBe("full");
    }
  });
});

const ASSETS =
  '<link rel="stylesheet" href="/site-assets/palette-1.css"><link rel="stylesheet" href="/site-assets/base-1.css"><script type="module" src="/site-assets/router-1.js"></script>';

/** A site page as the daemon writes it. */
function page(o: {
  title: string;
  body: string;
  panels?: string;
  header?: string;
  assets?: string;
}): string {
  const aside = o.panels === undefined ? "" : `<aside id="mae-panels">${o.panels}</aside>`;
  const header = o.header === undefined ? "" : `<header id="mae-header">${o.header}</header>`;
  return `<!doctype html><html lang="en-US"><head><meta charset="utf-8">${o.assets ?? ASSETS}<title>${o.title}</title></head><body>${header}<main id="mae-main"><article id="mae-article"><h1>${o.title}</h1>${o.body}</article></main>${aside}</body></html>`;
}

interface Served {
  status?: number;
  type?: string;
  contentType?: string;
  body?: string;
  reject?: boolean;
  delay?: Promise<void>;
}

/** A window showing `html` at `url`, its fetch answering from `serve`. */
function setup(url: string, html: string, serve: (u: string) => Served) {
  const win = new Window({ url });
  win.document.write(html);
  const requests: Array<{ url: string; init: RequestInit }> = [];
  const fetch = vi.fn(async (u: string, init: RequestInit) => {
    requests.push({ url: u, init });
    const s = serve(u);
    if (s.delay) await s.delay;
    if (s.reject) throw new TypeError("network");
    return {
      type: s.type ?? "basic",
      status: s.status ?? 200,
      headers: { get: () => s.contentType ?? "text/html; charset=utf-8" },
      text: async () => s.body ?? "",
    };
  });
  (win as unknown as { fetch: unknown }).fetch = fetch;
  const assign = vi.fn();
  Object.defineProperty(win.location, "assign", { value: assign, configurable: true });
  const scrollTo = vi.fn();
  (win as unknown as { scrollTo: unknown }).scrollTo = scrollTo;
  const router = start(win as unknown as Parameters<typeof start>[0]);
  return { win, doc: win.document, router, requests, assign, scrollTo };
}

const A = page({ title: "Alpha", body: '<p><a id="to-b" href="/kb/k/b">B</a></p>', panels: "<nav id=\"mae-toc\">A toc</nav>" });
const B = page({ title: "Beta", body: "<p>beta body</p>", panels: '<nav id="mae-backlinks">B links</nav>' });

/** A primary click on the element, bubbling, as a browser dispatches it. */
function clickOn(doc: Window["document"], id: string): void {
  const el = doc.getElementById(id);
  if (!el) throw new Error(`no #${id}`);
  const win = doc.defaultView as unknown as Window;
  el.dispatchEvent(new win.MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
}

describe("navigation", () => {
  it("fetches the next page as GET, same-origin, unfollowed redirects, no headers, then swaps it in", async () => {
    const { doc, router, requests, assign, win } = setup("https://site.example/kb/k/a", A, () => ({ body: B }));
    clickOn(doc, "to-b");
    await router.pending();
    expect(requests).toHaveLength(1);
    const init = requests[0]!.init;
    expect(requests[0]!.url).toBe("https://site.example/kb/k/b");
    expect(init.method).toBe("GET");
    expect(init.credentials).toBe("same-origin");
    expect(init.redirect).toBe("manual");
    expect(init.headers).toBeUndefined();
    expect(assign).not.toHaveBeenCalled();
    expect(doc.title).toBe("Beta");
    expect(doc.getElementById("mae-main")?.textContent).toContain("beta body");
    expect(doc.getElementById("mae-panels")?.textContent).toBe("B links");
    expect(doc.querySelectorAll("main").length).toBe(1);
    expect(win.location.pathname).toBe("/kb/k/b");
    // Focus and announcement (P-22a).
    expect(doc.activeElement?.tagName).toBe("H1");
    expect(doc.getElementById("mae-announcer")?.textContent).toBe("Beta");
    expect(doc.getElementById("mae-announcer")?.getAttribute("aria-live")).toBe("polite");
  });

  it("falls back to a full navigation for a redirect, an error, a foreign page, new assets or a network failure", async () => {
    const cases: Served[] = [
      { type: "opaqueredirect", status: 0 },
      { status: 404, body: B },
      { status: 502, body: "<html><body>bad gateway</body></html>" },
      { body: "<html><body><p>the backend's own page</p></body></html>" },
      { body: page({ title: "Beta", body: "x", assets: ASSETS.replace("base-1", "base-2") }) },
      { reject: true },
    ];
    for (const served of cases) {
      const { doc, router, assign } = setup("https://site.example/kb/k/a", A, () => served);
      clickOn(doc, "to-b");
      await router.pending();
      expect(assign, JSON.stringify(served)).toHaveBeenCalledWith("https://site.example/kb/k/b");
      expect(doc.title, JSON.stringify(served)).toBe("Alpha");
    }
  });

  it("removes or adds the side panels with the page", async () => {
    const bare = page({ title: "Bare", body: '<a id="to-p" href="/kb/k/p">p</a>' });
    const { doc, router } = setup("https://site.example/kb/k/a", A, (u) => ({
      body: u.endsWith("/p") ? B : bare,
    }));
    void router.navigate(new URL("https://site.example/kb/k/bare"), true);
    await router.pending();
    expect(doc.getElementById("mae-panels")).toBeNull();
    clickOn(doc, "to-p");
    await router.pending();
    expect(doc.getElementById("mae-panels")?.textContent).toBe("B links");
    expect(doc.getElementById("mae-main")?.nextElementSibling?.id).toBe("mae-panels");
  });

  it("replaces, removes or adds the site header before main, with the page", async () => {
    const withHeader = (site: string, body = "") =>
      page({ title: site, body, header: `<a href="/kb/${site}/">${site}</a>` });
    const { doc, router } = setup(
      "https://site.example/kb/one/a",
      withHeader("one", '<a id="go" href="/kb/two/x">x</a>'),
      (u) => ({ body: u.includes("/two/") ? withHeader("two") : page({ title: "none", body: "" }) }),
    );
    clickOn(doc, "go");
    await router.pending();
    expect(doc.getElementById("mae-header")?.textContent).toBe("two");
    expect(doc.querySelectorAll("header").length).toBe(1);
    void router.navigate(new URL("https://site.example/kb/three/y"), true);
    await router.pending();
    expect(doc.getElementById("mae-header")).toBeNull();
    void router.navigate(new URL("https://site.example/kb/two/z"), true);
    await router.pending();
    expect(doc.getElementById("mae-main")?.previousElementSibling?.id).toBe("mae-header");
  });

  it("lets only the latest of overlapping navigations land", async () => {
    let release!: () => void;
    const slow = new Promise<void>((r) => (release = r));
    const { doc, router } = setup("https://site.example/kb/k/a", A, (u) =>
      u.endsWith("/slow") ? { body: page({ title: "Slow", body: "" }), delay: slow } : { body: B },
    );
    const first = router.navigate(new URL("https://site.example/kb/k/slow"), true);
    await router.navigate(new URL("https://site.example/kb/k/b"), true);
    release();
    await first;
    expect(doc.title).toBe("Beta");
  });

  it("mounts each component once per page and cleans it up before the next (P-25, P-156)", async () => {
    const { doc, router } = setup("https://site.example/kb/k/a", A, () => ({ body: B }));
    let mounts = 0;
    let cleanups = 0;
    router.register({
      id: "counter",
      init: () => {
        mounts += 1;
        return () => (cleanups += 1);
      },
    });
    expect([mounts, cleanups]).toEqual([1, 0]);
    clickOn(doc, "to-b");
    await router.pending();
    expect([mounts, cleanups]).toEqual([2, 1]);
    void router.navigate(new URL("https://site.example/kb/k/a"), true);
    await router.pending();
    expect([mounts, cleanups]).toEqual([3, 2]);
  });

  it("restores the scroll position on a history move, after the swap (P-23)", async () => {
    const { win, router, scrollTo, requests } = setup("https://site.example/kb/k/b", B, () => ({ body: A }));
    win.history.replaceState(null, "", "/kb/k/a");
    win.dispatchEvent(new win.PopStateEvent("popstate", { state: { scrollY: 120 } }));
    await router.pending();
    expect(requests).toHaveLength(1);
    expect(win.document.title).toBe("Alpha");
    expect(scrollTo).toHaveBeenLastCalledWith(0, 120);
  });

  it("lands a page whose link has a malformed fragment, focus and all", async () => {
    const bad = page({ title: "Alpha", body: '<a id="bad" href="/kb/k/b#%E0">b</a>' });
    const { doc, router } = setup("https://site.example/kb/k/a", bad, () => ({ body: B }));
    clickOn(doc, "bad");
    await router.pending();
    expect(doc.title).toBe("Beta");
    expect(doc.activeElement?.tagName).toBe("H1");
  });

  it("drops a navigation in flight when history returns to the shown page", async () => {
    let release!: () => void;
    const slow = new Promise<void>((r) => (release = r));
    const { win, doc, router } = setup("https://site.example/kb/k/a", A, () => ({
      body: B,
      delay: slow,
    }));
    const going = router.navigate(new URL("https://site.example/kb/k/b"), true);
    win.dispatchEvent(new win.PopStateEvent("popstate", { state: null }));
    release();
    await going;
    expect(doc.title).toBe("Alpha");
    expect(win.location.pathname).toBe("/kb/k/a");
  });

  it("leaves a history move within the shown page to the browser", async () => {
    const { win, router, requests } = setup("https://site.example/kb/k/a", A, () => ({ body: B }));
    win.history.pushState(null, "", "/kb/k/a#later");
    win.dispatchEvent(new win.PopStateEvent("popstate", { state: null }));
    expect(router.pending()).toBeNull();
    expect(requests).toHaveLength(0);
  });

  it("follows a link inside inline SVG, href or xlink:href (P-26)", async () => {
    const svg = page({
      title: "Diagram",
      body: '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><a id="s1" href="/kb/k/b"><text>b</text></a><a id="s2" xlink:href="/kb/k/b"><text>b</text></a></svg>',
    });
    for (const id of ["s1", "s2"]) {
      const { doc, router, requests } = setup("https://site.example/kb/k/a", svg, () => ({ body: B }));
      clickOn(doc, id);
      await router.pending();
      expect(requests.map((r) => r.url), id).toEqual(["https://site.example/kb/k/b"]);
      expect(doc.title, id).toBe("Beta");
    }
  });
});

describe("swappable", () => {
  it("is false without the main landmark or with other assets", () => {
    const win = new Window();
    const parse = (s: string) => new win.DOMParser().parseFromString(s, "text/html");
    const a = parse(A) as unknown as Document;
    expect(swappable(a, parse(B) as unknown as Document)).toBe(true);
    expect(swappable(a, parse("<html><body>x</body></html>") as unknown as Document)).toBe(false);
    const other = page({ title: "B", body: "", assets: ASSETS.replace("router-1", "router-2") });
    expect(swappable(a, parse(other) as unknown as Document)).toBe(false);
  });
  it("is false across KBs, even with the same assets (a shared component directory)", () => {
    const win = new Window();
    const parse = (s: string) => new win.DOMParser().parseFromString(s, "text/html") as unknown as Document;
    const inKb = (html: string, kb: string | null) => {
      const d = parse(html);
      if (kb !== null) d.getElementById("mae-main")!.setAttribute("data-mae-kb", kb);
      return d;
    };
    expect(swappable(inKb(A, "one"), inKb(B, "one"))).toBe(true);
    expect(swappable(inKb(A, "one"), inKb(B, "two"))).toBe(false);
    expect(swappable(inKb(A, "one"), inKb(B, null))).toBe(false);
    expect(swappable(inKb(A, null), inKb(B, "two"))).toBe(false);
  });
});
