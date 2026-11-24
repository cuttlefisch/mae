// SPDX-License-Identifier: GPL-3.0-or-later
// The example copy-link component's behaviour (ADR-119 H1, P-162), from the
// fixture component directory, registered through `mae.site.register` as an
// operator's module script is: it upgrades the plain link the template wrote
// into a copy button, and undoes that before the next page.
import path from "node:path";
import { pathToFileURL } from "node:url";
import { Window } from "happy-dom";
import { describe, expect, it, vi } from "vitest";
import { start } from "../src/router";

const FIXTURE = path.resolve(
  import.meta.dirname,
  "../../../../../docs/verification/fixtures/components/slots/copy-link.js",
);

/** A page whose header holds the copy-link slot as the engine renders it. */
function page(id: string): string {
  return (
    `<!doctype html><html lang="en-US"><head><title>${id}</title></head><body>` +
    `<header id="mae-header"><span class="site-copy-link">` +
    `<a class="site-copy-link-anchor" href="/kb/demo/${id}">Link to this page</a>` +
    `<span class="site-copy-link-status" role="status"></span></span>` +
    `<a id="next" href="/kb/demo/next">next</a></header>` +
    `<main id="mae-main" data-mae-kb="demo" data-mae-base="/kb/demo" data-mae-url="/kb/demo/${id}">` +
    `<article id="mae-article"><h1>${id}</h1><h2 id="sec-1">One</h2><h2 id="mae-toc-h">TOC</h2>` +
    `</article></main></body></html>`
  );
}

describe("the copy-link component", () => {
  it("registers itself on import and upgrades the link, then undoes it before the next page", async () => {
    const win = new Window({ url: "https://site.example/kb/demo/guide" });
    win.document.write(page("guide"));
    (win as unknown as { fetch: unknown }).fetch = vi.fn(async () => ({
      type: "basic",
      status: 200,
      headers: { get: () => "text/html; charset=utf-8" },
      text: async () => page("next"),
    }));
    (win as unknown as { scrollTo: unknown }).scrollTo = vi.fn();
    const writeText = vi.fn(async () => {});
    Object.defineProperty(win.navigator, "clipboard", { value: { writeText }, configurable: true });
    const router = start(win as unknown as Parameters<typeof start>[0]);
    const doc = win.document;

    // As the browser runs it: after the router, with `mae.site` defined.
    (globalThis as unknown as { mae?: unknown }).mae = { site: router };
    try {
      await import(pathToFileURL(FIXTURE).href);
    } finally {
      delete (globalThis as unknown as { mae?: unknown }).mae;
    }

    const el = (sel: string) => doc.querySelector(sel) as unknown as HTMLElement | null;
    const anchor = el("a.site-copy-link-anchor");
    const button = el("button.site-copy-link-button");
    expect(anchor?.hidden).toBe(true);
    expect(button?.textContent).toBe("Copy link");
    // One section button, for the content heading only (not the chrome's).
    const sections = doc.querySelectorAll("button.site-copy-link-section");
    expect(sections.length).toBe(1);
    expect(sections[0]?.parentElement?.id).toBe("sec-1");

    button?.click();
    await vi.waitFor(() => expect(writeText).toHaveBeenCalled());
    expect(writeText).toHaveBeenCalledWith("https://site.example/kb/demo/guide");
    expect(doc.querySelector(".site-copy-link-status")?.textContent).toBe("Link copied.");
    (sections[0] as unknown as HTMLElement).click();
    await vi.waitFor(() => expect(writeText).toHaveBeenCalledTimes(2));
    expect(writeText).toHaveBeenLastCalledWith("https://site.example/kb/demo/guide#sec-1");

    // A stale button is inert once the page is left (the signal is aborted).
    const stale = button;
    doc.getElementById("next")?.dispatchEvent(
      new win.MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }),
    );
    await router.pending();
    stale?.click();
    await Promise.resolve();
    expect(writeText).toHaveBeenCalledTimes(2);
    // And the next page is upgraded in turn, with its own address.
    expect(doc.querySelector("a.site-copy-link-anchor")?.getAttribute("href")).toBe("/kb/demo/next");
    expect(doc.querySelectorAll("button.site-copy-link-button").length).toBe(1);
    el("button.site-copy-link-button")?.click();
    await vi.waitFor(() => expect(writeText).toHaveBeenCalledTimes(3));
    expect(writeText).toHaveBeenLastCalledWith("https://site.example/kb/demo/next");
  });
});
