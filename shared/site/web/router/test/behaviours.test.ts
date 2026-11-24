// SPDX-License-Identifier: GPL-3.0-or-later
// Behaviours (ADR-119 D18, A2; P-156) and the slot regions the router swaps
// (P-153), in a DOM (happy-dom), the page fetch stubbed at `fetch`.
import { Window } from "happy-dom";
import { describe, expect, it, vi } from "vitest";
import { HEAD_META_NAMES, start, type Behaviour, type Context } from "../src/router";

const ASSETS =
  '<link rel="stylesheet" href="/site-assets/base-1.css"><script type="module" src="/site-assets/router-1.js"></script>';

interface Page {
  id?: string | null;
  title: string;
  kb?: string;
  head?: string;
  left?: string;
  footer?: string;
  attrs?: boolean;
  lang?: string;
}

/** A site page as the daemon writes it for a KB with components. */
function page(p: Page): string {
  const kb = p.kb ?? "kb-1";
  const url = `/kb/${kb}/${p.id ?? "tags"}`;
  const id = p.id ? ` data-mae-page="${p.id}"` : "";
  const attrs =
    p.attrs === false ? "" : ` data-mae-kb="${kb}" data-mae-base="/kb/${kb}" data-mae-url="${url}"${id}`;
  const left = p.left === undefined ? "" : `<div id="mae-left-footer">${p.left}</div>`;
  const footer = p.footer === undefined ? "" : `<footer id="mae-footer">${p.footer}</footer>`;
  return (
    `<!doctype html><html lang="${p.lang ?? "en-US"}"><head><meta charset="utf-8">` +
    `<meta name="viewport" content="width=device-width">${ASSETS}${p.head ?? ""}<title>${p.title}</title></head>` +
    `<body><header id="mae-header"><a id="next" href="/kb/${kb}/b">next</a></header>` +
    `<nav id="mae-explorer" data-src="/x" data-version="1" hidden></nav>${left}` +
    `<main id="mae-main"${attrs}><article id="mae-article"><h1>${p.title}</h1></article></main>${footer}</body></html>`
  );
}

function setup(url: string, html: string, next: string) {
  const win = new Window({ url });
  win.document.write(html);
  (win as unknown as { fetch: unknown }).fetch = vi.fn(async () => ({
    type: "basic",
    status: 200,
    headers: { get: () => "text/html; charset=utf-8" },
    text: async () => next,
  }));
  Object.defineProperty(win.location, "assign", { value: vi.fn(), configurable: true });
  (win as unknown as { scrollTo: unknown }).scrollTo = vi.fn();
  const router = start(win as unknown as Parameters<typeof start>[0]);
  const go = async () => {
    const a = win.document.getElementById("next");
    a?.dispatchEvent(new win.MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
    await router.pending();
  };
  return { win, doc: win.document, router, go };
}

/** A behaviour that records every context it got and every cleanup. */
function recorder(id: string) {
  const seen: Context[] = [];
  const log: string[] = [];
  const b: Behaviour = {
    id,
    init(ctx) {
      seen.push(ctx);
      log.push(`init ${ctx.page?.id ?? "-"}`);
      ctx.signal.addEventListener("abort", () => log.push("abort"));
      return () => log.push(`cleanup aborted=${ctx.signal.aborted}`);
    },
  };
  return { b, seen, log };
}

describe("register({id, init})", () => {
  it("runs at once on the page on screen, then once per navigation, with the page's context", async () => {
    const A = page({ id: "a", title: "Alpha", lang: "fr-FR" });
    const B = page({ id: "b", title: "Beta", lang: "fr-FR" });
    const { doc, router, go } = setup("https://site.example/kb/kb-1/a", A, B);
    const r = recorder("probe");
    // Registered after start, as an operator's module script is.
    router.register(r.b);
    expect(r.log).toEqual(["init a"]);
    const first = r.seen[0]!;
    expect(first.root).toBe(doc);
    expect(first.page).toEqual({ id: "a", url: "/kb/kb-1/a", kb: "kb-1" });
    expect(first.site).toEqual({ kb: "kb-1", base: "/kb/kb-1", locale: "fr-FR" });
    expect(first.signal.aborted).toBe(false);
    await go();
    // The signal is aborted before the behaviour's own cleanup runs, exactly
    // once, and the next page gets a fresh context.
    expect(r.log).toEqual(["init a", "abort", "cleanup aborted=true", "init b"]);
    expect(first.signal.aborted).toBe(true);
    expect(r.seen[1]!.page).toEqual({ id: "b", url: "/kb/kb-1/b", kb: "kb-1" });
    expect(r.seen[1]!.signal.aborted).toBe(false);
    expect(r.seen[1]!.signal).not.toBe(first.signal);
  });

  it("gives a generated page no ID, and a page without the shell's data no page or site", async () => {
    const { router } = setup("https://site.example/kb/kb-1/tags", page({ title: "Tags" }), "");
    const r = recorder("probe");
    router.register(r.b);
    expect(r.seen[0]!.page).toEqual({ id: null, url: "/kb/kb-1/tags", kb: "kb-1" });
    const bare = setup("https://site.example/kb/kb-1/a", page({ id: "a", title: "A", attrs: false }), "");
    const r2 = recorder("probe");
    bare.router.register(r2.b);
    expect([r2.seen[0]!.page, r2.seen[0]!.site]).toEqual([null, null]);
  });

  it("replaces a behaviour registered again under the same id, cleaning the first up", () => {
    const { router } = setup("https://site.example/kb/kb-1/a", page({ id: "a", title: "A" }), "");
    const one = recorder("same");
    const two = recorder("same");
    router.register(one.b);
    router.register(two.b);
    expect(one.log).toEqual(["init a", "abort", "cleanup aborted=true"]);
    expect(two.log).toEqual(["init a"]);
  });

  it("keeps running every other behaviour when one throws, and refuses a malformed one", async () => {
    const A = page({ id: "a", title: "A" });
    const { win, router, go } = setup("https://site.example/kb/kb-1/a", A, page({ id: "b", title: "B" }));
    const error = vi.fn();
    (win as unknown as { console: unknown }).console = { error };
    router.register({
      id: "broken",
      init() {
        throw new Error("operator bug");
      },
    });
    const ok = recorder("ok");
    router.register(ok.b);
    await go();
    expect(ok.log).toEqual(["init a", "abort", "cleanup aborted=true", "init b"]);
    expect(error).toHaveBeenCalledTimes(2); // once per page
    for (const bad of [null, {}, { id: "", init() {} }, { id: "x" }, { id: 1, init() {} }]) {
      expect(() => router.register(bad as unknown as Behaviour)).toThrow(TypeError);
    }
  });
});

describe("slot regions", () => {
  it("swaps the left-panel footer, the page footer and the head's described metas with the page", async () => {
    const A = page({
      id: "a",
      title: "A",
      head: '<meta name="description" content="about a">',
      left: "<p>v1</p>",
    });
    const B = page({
      id: "b",
      title: "B",
      head: '<meta name="robots" content="noindex">',
      footer: "<p>foot b</p>",
    });
    const { doc, go } = setup("https://site.example/kb/kb-1/a", A, B);
    await go();
    expect(doc.getElementById("mae-left-footer")).toBeNull();
    expect(doc.getElementById("mae-footer")?.textContent).toBe("foot b");
    const metas = Array.from(doc.head.querySelectorAll("meta[name]")).map((m) => [
      m.getAttribute("name"),
      m.getAttribute("content"),
    ]);
    // The chrome's own viewport stays; the slot's description is replaced.
    expect(metas).toEqual([
      ["viewport", "width=device-width"],
      ["robots", "noindex"],
    ]);
    // Body order: header, explorer, (left footer), main, footer.
    const order = Array.from(doc.body.children)
      .map((e) => e.id)
      .filter((id) => id !== "mae-announcer");
    expect(order).toEqual(["mae-header", "mae-explorer", "mae-main", "mae-footer"]);
  });

  it("adds a left-panel footer after the explorer and before main", async () => {
    const A = page({ id: "a", title: "A" });
    const B = page({ id: "b", title: "B", left: "<p>v2</p>" });
    const { doc, go } = setup("https://site.example/kb/kb-1/a", A, B);
    await go();
    const order = Array.from(doc.body.children)
      .map((e) => e.id)
      .filter((id) => id !== "mae-announcer");
    expect(order).toEqual(["mae-header", "mae-explorer", "mae-left-footer", "mae-main"]);
    expect(doc.getElementById("mae-left-footer")?.textContent).toBe("v2");
  });

  it("names exactly the head metas the engine admits", () => {
    expect([...HEAD_META_NAMES]).toEqual([
      "application-name",
      "author",
      "description",
      "generator",
      "keywords",
      "robots",
    ]);
  });
});
