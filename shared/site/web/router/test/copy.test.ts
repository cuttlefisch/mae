// SPDX-License-Identifier: GPL-3.0-or-later
// The copy control (P-109): in a DOM (happy-dom), with the clipboard stubbed
// on the window's navigator. Every block's text is generated (seeded), so
// the exactness checks are not tuned to one hand-picked string.
import { Window } from "happy-dom";
import { describe, expect, it, vi } from "vitest";
import {
  ANNOUNCER_ID,
  COPY_CLASS,
  codeCopy,
  copyBlock,
  copyWords,
} from "../src/router";

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

/** Pieces a block's text is made of: what copying has dropped or mangled. */
const PIECES = [
  "\t",
  "  ",
  "\n",
  "\n\n",
  "\n\n\n",
  "x = 1;",
  "<b>&amp;</b>",
  "]]",
  "&",
  "<",
  "\u{1F600}",
  "é",
  "trailing   ",
  "a".repeat(2100),
];

/** A block's text, and the same text cut into highlighted spans. */
function block(next: () => number): { text: string; html: string } {
  let text = "";
  let html = "";
  const n = 1 + Math.floor(next() * 12);
  for (let i = 0; i < n; i++) {
    const piece = PIECES[Math.floor(next() * PIECES.length)] ?? "";
    text += piece;
    const escaped = piece.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    html += next() < 0.5 ? escaped : `<span class="org-code-keyword">${escaped}</span>`;
  }
  return { text, html };
}

const WORDS =
  'data-copy-label="Copy code" data-copy-done="Code copied" data-copy-selected="Code selected"';

function page(blocks: string[], attrs: string = WORDS): string {
  const pres = blocks
    .map((b) => `<pre class="org-src"><code class="language-sh">${b}</code></pre>`)
    .join("<p>between</p>");
  return `<!doctype html><html lang="en-US"><head><title>t</title></head><body><main id="mae-main"><article id="mae-article" ${attrs}><h1>t</h1>${pres}<pre class="org-example"><code>not a source block</code></pre></article></main></body></html>`;
}

function setup(html: string, clipboard: "ok" | "missing" | "refuses" = "ok") {
  const win = new Window({ url: "https://site.example/kb/k/a" });
  win.document.write(html);
  const written: string[] = [];
  const writeText = vi.fn(async (t: string) => {
    if (clipboard === "refuses") throw new DOMException("denied", "NotAllowedError");
    written.push(t);
  });
  Object.defineProperty(win.navigator, "clipboard", {
    value: clipboard === "missing" ? undefined : { writeText },
    configurable: true,
  });
  const dom = win as unknown as Parameters<typeof codeCopy>[0];
  const doc = win.document as unknown as Document;
  return { win, dom, doc, written, writeText };
}

const announced = (doc: Document): string | null =>
  doc.getElementById(ANNOUNCER_ID)?.textContent ?? null;

describe("copyWords", () => {
  it("reads the article's words, and none when any is missing", () => {
    const { doc } = setup(page([]));
    const article = doc.getElementById("mae-article");
    expect(article && copyWords(article)).toEqual({
      label: "Copy code",
      copied: "Code copied",
      selected: "Code selected",
      lang: null,
    });
    for (const drop of ["data-copy-label", "data-copy-done", "data-copy-selected"]) {
      article?.removeAttribute(drop);
      expect(article && copyWords(article)).toBeNull();
    }
  });
});

describe("the copy control", () => {
  it("copies every block's exact text, blank lines and markup included", async () => {
    const next = rng(109);
    for (let round = 0; round < 25; round++) {
      const blocks = Array.from({ length: 3 }, () => block(next));
      const { dom, doc, written } = setup(page(blocks.map((b) => b.html)));
      codeCopy(dom).mount(doc);
      const buttons = Array.from(doc.querySelectorAll(`.${COPY_CLASS}`));
      // One per source block, never for an example block.
      expect(buttons).toHaveLength(3);
      for (const [i, b] of buttons.entries()) {
        (b as HTMLElement).click();
        await vi.waitFor(() => expect(written).toHaveLength(i + 1));
        expect(written[i]).toBe(blocks[i]?.text);
      }
      expect(announced(doc)).toBe("Code copied");
    }
  });

  it("keeps consecutive blank lines (the old site's control dropped one)", async () => {
    const text = "a\n\n\nb\n\n";
    const { dom, doc, written } = setup(page([text]));
    codeCopy(dom).mount(doc);
    (doc.querySelector(`.${COPY_CLASS}`) as HTMLElement).click();
    await vi.waitFor(() => expect(written).toEqual([text]));
  });

  for (const clipboard of ["missing", "refuses"] as const) {
    it(`selects the block's text when the clipboard is ${clipboard}`, async () => {
      const { text, html } = block(rng(7));
      const { win, doc, written } = setup(page([html]), clipboard);
      const code = doc.querySelector("pre.org-src > code");
      if (!code) throw new Error("no block");
      const words = copyWords(doc.getElementById("mae-article") as Element);
      if (!words) throw new Error("no words");
      const how = await copyBlock(
        win as unknown as Parameters<typeof copyBlock>[0],
        doc,
        code,
        words,
      );
      expect(how).toBe("selected");
      expect(written).toEqual([]);
      expect(win.getSelection()?.toString()).toBe(text);
      expect(announced(doc)).toBe("Code selected");
    });
  }

  it("marks its buttons when the words are a fallback", () => {
    const { dom, doc } = setup(page(["x"], `${WORDS} data-copy-lang="en-US"`));
    codeCopy(dom).mount(doc);
    expect(doc.querySelector(`.${COPY_CLASS}`)?.getAttribute("lang")).toBe("en-US");
  });

  it("adds nothing without its words, or without an article", () => {
    for (const html of [page(["x"], ""), page(["x"]).replace('id="mae-article"', "")]) {
      const { dom, doc } = setup(html);
      expect(codeCopy(dom).mount(doc)).toBeUndefined();
      expect(doc.querySelectorAll(`.${COPY_CLASS}`)).toHaveLength(0);
    }
  });

  it("is gone after unmount, and a click on a stale button copies nothing", async () => {
    const { dom, doc, writeText } = setup(page(["one", "two"]));
    const cleanup = codeCopy(dom).mount(doc);
    const stale = doc.querySelector(`.${COPY_CLASS}`) as HTMLElement;
    expect(typeof cleanup).toBe("function");
    if (typeof cleanup === "function") cleanup();
    expect(doc.querySelectorAll(`.${COPY_CLASS}`)).toHaveLength(0);
    stale.click();
    await Promise.resolve();
    expect(writeText).not.toHaveBeenCalled();
    // Mounted again (the next page): one button per block, not two.
    codeCopy(dom).mount(doc);
    expect(doc.querySelectorAll(`.${COPY_CLASS}`)).toHaveLength(2);
  });

  it("builds its buttons with DOM APIs only, never an HTML sink", () => {
    const { win, dom, doc } = setup(page(["<b>x</b>"]));
    const proto = win.Element.prototype as unknown as Record<string, unknown>;
    const inner = vi.spyOn(win.Element.prototype, "innerHTML", "set");
    const outer = vi.spyOn(win.Element.prototype, "outerHTML", "set");
    const adjacent = vi.spyOn(proto as { insertAdjacentHTML(): void }, "insertAdjacentHTML");
    codeCopy(dom).mount(doc);
    const b = doc.querySelector(`.${COPY_CLASS}`);
    expect(b?.textContent).toBe("Copy code");
    expect(b?.children).toHaveLength(0);
    expect(inner).not.toHaveBeenCalled();
    expect(outer).not.toHaveBeenCalled();
    expect(adjacent).not.toHaveBeenCalled();
  });
});
