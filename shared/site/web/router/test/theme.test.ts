// SPDX-License-Identifier: GPL-3.0-or-later
// Mode and palette (P-91 to P-99): the choice functions as pure functions, then
// the generated pre-paint script and the controls in a DOM (happy-dom), with
// storage that works, storage injected with hostile values, and storage that throws.
import { readFileSync } from "node:fs";
import path from "node:path";
import { Window as HappyWindow } from "happy-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  EDGE,
  HOVER_INTENT_MS,
  menuAction,
  moveFocus,
  placeMenu,
  placeSubmenu,
  prepaint,
  readStored,
  resolveChoice,
  sessionStore,
  start,
  storageKey,
  systemDark,
  themeControls,
  validAccent,
  type Box,
  type MenuAction,
  type ThemeData,
} from "../src/router";

const ROOT = path.join(import.meta.dirname, "..");
const PREPAINT = readFileSync(path.join(ROOT, "dist", "prepaint.js"), "utf8");
const THEMES: ThemeData = JSON.parse(
  readFileSync(path.join(ROOT, "..", "palette", "dist", "themes.json"), "utf8"),
);

/** A small offer: a default without accents, one with, and a removed palette. */
const DATA: ThemeData = {
  default: "plain",
  successors: { old: "rich", lost: "nowhere" },
  palettes: {
    plain: { name: "Plain", swatches: { light: ["#ffffff"], dark: ["#000000"] }, accents: null },
    rich: {
      name: "Rich",
      swatches: { light: ["#eeeeee"], dark: ["#111111"] },
      accents: {
        default: "violet",
        items: {
          violet: { light: "#550055", dark: "#dd88dd" },
          orange: { light: "#883300", dark: "#ff9944" },
        },
      },
    },
  },
};

const reader =
  (stored: Record<string, string>) =>
  (k: string): string | null =>
    Object.hasOwn(stored, k) ? stored[k]! : null;

describe("the stored choice, validated (P-91, P-96, P-98)", () => {
  it("storage keys are MAE's own, the accent's per palette", () => {
    expect(storageKey("mode")).toBe("mae.mode");
    expect(storageKey("palette")).toBe("mae.palette");
    expect(storageKey("accent", "rich")).toBe("mae.accent.rich");
  });

  it("a valid choice is kept", () => {
    expect(
      resolveChoice(DATA, reader({ "mae.mode": "dark", "mae.palette": "rich", "mae.accent.rich": "orange" })),
    ).toEqual({ mode: "dark", palette: "rich", accent: "orange" });
    expect(resolveChoice(DATA, reader({ "mae.mode": "light" })).mode).toBe("light");
  });

  it("nothing stored: the system's mode, the default palette, its default accent", () => {
    expect(resolveChoice(DATA, reader({}))).toEqual({ mode: null, palette: "plain", accent: null });
  });

  it("an unknown or hostile value is the default, never a key", () => {
    for (const bad of ["__proto__", "constructor", "toString", "hasOwnProperty", "Rich", " rich", ""]) {
      const c = resolveChoice(DATA, reader({ "mae.mode": bad, "mae.palette": bad }));
      expect(c, bad).toEqual({ mode: null, palette: "plain", accent: null });
      expect(validAccent(DATA, "rich", bad), bad).toBeNull();
      expect(validAccent(DATA, bad, "orange"), bad).toBeNull();
    }
  });

  it("an accent is per palette: another palette's, or the default, is no accent (P-96)", () => {
    expect(validAccent(DATA, "rich", "orange")).toBe("orange");
    expect(validAccent(DATA, "rich", "violet")).toBeNull(); // its default
    expect(validAccent(DATA, "plain", "orange")).toBeNull(); // offers none
    expect(validAccent(DATA, "rich", null)).toBeNull();
    const c = resolveChoice(DATA, reader({ "mae.palette": "rich", "mae.accent.plain": "orange" }));
    expect(c.accent).toBeNull();
  });

  it("a removed palette becomes its successor and drops its accent (P-98)", () => {
    expect(
      resolveChoice(DATA, reader({ "mae.palette": "old", "mae.accent.rich": "orange", "mae.accent.old": "orange" })),
    ).toEqual({ mode: null, palette: "rich", accent: null });
    // A successor that is not a palette is no successor: the default.
    expect(resolveChoice(DATA, reader({ "mae.palette": "lost" })).palette).toBe("plain");
    // A palette that exists is never mapped, even if named as removed.
    const both = { ...DATA, successors: { rich: "plain" } };
    expect(resolveChoice(both, reader({ "mae.palette": "rich" })).palette).toBe("rich");
  });

  it("storage that throws, or returns no string, reads as absent (P-97)", () => {
    const throws = () => {
      throw new DOMException("denied", "SecurityError");
    };
    expect(readStored(throws, "mae.mode")).toBeNull();
    expect(readStored(() => ({ getItem: throws }), "mae.mode")).toBeNull();
    expect(readStored(() => ({ getItem: () => 3 as unknown as string }), "k")).toBeNull();
    expect(readStored(() => ({ getItem: () => "dark" }), "k")).toBe("dark");
    expect(systemDark({ matchMedia: throws } as unknown as Window)).toBe(false);
  });

  it("the session store keeps what this page wrote when storage throws", () => {
    const throws = (): Storage => {
      throw new DOMException("denied", "SecurityError");
    };
    const s = sessionStore(throws);
    expect(s.get("mae.mode")).toBeNull();
    s.set("mae.mode", "dark");
    expect(s.get("mae.mode")).toBe("dark");
    s.set("mae.mode", null);
    expect(s.get("mae.mode")).toBeNull();
    // With working storage, writes reach it, and a removal removes.
    const real = new Map<string, string>([["mae.palette", "rich"]]);
    const storage = {
      getItem: (k: string) => real.get(k) ?? null,
      setItem: (k: string, v: string) => void real.set(k, v),
      removeItem: (k: string) => void real.delete(k),
    };
    const t = sessionStore(() => storage);
    expect(t.get("mae.palette")).toBe("rich");
    t.set("mae.accent.rich", "orange");
    expect(real.get("mae.accent.rich")).toBe("orange");
    t.set("mae.palette", null);
    expect(real.has("mae.palette")).toBe(false);
  });
});

describe("keyboard map and roving focus (APG menu button)", () => {
  it("maps each key on a palette and on an accent", () => {
    const table: Array<[string, MenuAction | null, MenuAction | null]> = [
      ["ArrowDown", "next", "next"],
      ["ArrowUp", "prev", "prev"],
      ["Home", "first", "first"],
      ["End", "last", "last"],
      ["Enter", "choose", "choose"],
      [" ", "choose", "choose"],
      ["ArrowRight", "open-sub", null],
      ["ArrowLeft", null, "close-sub"],
      ["Escape", "close", "close"],
      ["Tab", "leave", "leave"],
      ["a", null, null],
      ["Spacebar", null, null],
    ];
    for (const [key, onPalette, onAccent] of table) {
      expect(menuAction(key, false), key).toBe(onPalette);
      expect(menuAction(key, true), key).toBe(onAccent);
    }
  });

  it("moves with wrap-around at both ends, and nowhere in an empty list", () => {
    expect(moveFocus("next", 0, 3)).toBe(1);
    expect(moveFocus("next", 2, 3)).toBe(0);
    expect(moveFocus("prev", 0, 3)).toBe(2);
    expect(moveFocus("prev", 2, 3)).toBe(1);
    expect(moveFocus("first", 2, 3)).toBe(0);
    expect(moveFocus("last", 0, 3)).toBe(2);
    expect(moveFocus("choose", 1, 3)).toBe(1);
    expect(moveFocus("next", 0, 0)).toBe(-1);
    expect(moveFocus("next", 0, 1)).toBe(0);
  });
});

describe("placement inside the window (P-99)", () => {
  const box = (left: number, top: number, w: number, h: number): Box => ({
    left,
    top,
    right: left + w,
    bottom: top + h,
  });
  const inside = (left: number, width: number, vw: number) => {
    expect(left).toBeGreaterThanOrEqual(EDGE);
    expect(left + width).toBeLessThanOrEqual(vw - EDGE);
  };

  // The button at the header's right end, at desktop, phone and 400% zoom widths.
  for (const [vw, vh] of [
    [1440, 900],
    [390, 844],
    [320, 568],
  ] as const) {
    it(`at ${vw}x${vh}: menu and submenu inside the window, a long list scrolls`, () => {
      const button = box(vw - 140, 8, 130, 32);
      const m = placeMenu(button, 260, 2000, vw, vh);
      inside(m.left, Math.min(260, vw - 2 * EDGE), vw);
      expect(m.top).toBe(button.bottom);
      expect(m.top + m.maxHeight).toBe(vh - EDGE);
      const menu = box(m.left, m.top, Math.min(260, vw - 2 * EDGE), m.maxHeight);
      const item = box(menu.left, menu.top + 300, menu.right - menu.left, 32);
      const s = placeSubmenu(item, menu, 200, 180, vw, vh);
      inside(s.left, 200, vw);
      expect(s.top).toBeGreaterThanOrEqual(EDGE);
      expect(s.top + Math.min(180, s.maxHeight)).toBeLessThanOrEqual(vh - EDGE);
      if (vw === 1440) expect(s.left + 200).toBeLessThanOrEqual(menu.left); // beside it
    });
  }

  it("the submenu goes right when it fits, else left, else over the list", () => {
    const item = box(100, 100, 200, 32);
    expect(placeSubmenu(item, box(100, 40, 200, 400), 150, 100, 1000, 800).left).toBe(300);
    expect(placeSubmenu(item, box(700, 40, 200, 400), 150, 100, 1000, 800).left).toBe(550);
    const over = placeSubmenu(item, box(20, 40, 340, 400), 300, 100, 390, 800);
    expect(over.left).toBe(20); // covers the list, inside the window
    // A tall submenu low in the window moves up, never off the bottom.
    const low = placeSubmenu(box(100, 780, 200, 32), box(100, 40, 200, 760), 150, 300, 1000, 800);
    expect(low.top).toBe(800 - EDGE - 300);
  });

  it("opens above the button only when there is more room there", () => {
    const low = placeMenu(box(500, 700, 100, 30), 200, 400, 1000, 800);
    expect(low.top + 400).toBe(700);
    expect(low.maxHeight).toBe(700 - EDGE);
    const fits = placeMenu(box(500, 300, 100, 30), 200, 400, 1000, 800);
    expect(fits.top).toBe(330);
  });
});

/** The header as the daemon writes it (daemon/src/site.rs `theme_controls`). */
const HEADER =
  '<header id="mae-header"><a href="/kb/k/">Site</a><div id="mae-theme" class="mae-theme" hidden data-default-label="Default" data-accent-orange="Orange" data-accent-violet="Violet"><button type="button" id="mae-mode-toggle" aria-pressed="false"><span class="mae-mode-icon" aria-hidden="true"></span>Dark mode</button><button type="button" id="mae-palette-button" aria-haspopup="menu" aria-expanded="false" aria-controls="mae-palette-menu">Color palette</button></div></header>';

const PAGE = (header = HEADER) =>
  `<!doctype html><html lang="en-US"><head><title>A</title></head><body>${header}<main id="mae-main"><h1>A</h1><p><a id="to-b" href="/kb/k/b">B</a></p></main><button id="after">after</button></body></html>`;

type StorageMode = "ok" | "throws";
/** A happy-dom window, typed as the DOM window the code under test takes. */
type Win = Window & typeof globalThis;

/** A window with the page, the system's mode, and storage working or throwing. */
function browser(o: { stored?: Record<string, string>; storage?: StorageMode; dark?: boolean; html?: string } = {}) {
  const win = new HappyWindow({
    url: "https://site.example/kb/k/a",
    settings: { disableJavaScriptFileLoading: true, disableCSSFileLoading: true, handleDisabledFileLoadingAsSuccess: true },
  }) as unknown as Win;
  win.document.write(o.html ?? PAGE());
  const listeners: Array<(e: { matches: boolean }) => void> = [];
  const mq = {
    matches: o.dark ?? false,
    addEventListener: (_: string, f: (e: { matches: boolean }) => void) => listeners.push(f),
  };
  Object.defineProperty(win, "matchMedia", { value: () => mq, configurable: true });
  if (o.storage === "throws") {
    Object.defineProperty(win, "localStorage", {
      get() {
        throw new DOMException("denied", "SecurityError");
      },
      configurable: true,
    });
  } else {
    for (const [k, v] of Object.entries(o.stored ?? {})) win.localStorage.setItem(k, v);
  }
  const errors = vi.fn();
  (win.console as unknown as { error: unknown }).error = errors;
  const doc = win.document;
  const root = doc.documentElement;
  const system = (dark: boolean) => listeners.forEach((f) => f({ matches: dark }));
  return { win, doc, root, errors, system };
}

/** Runs the generated pre-paint script, as the browser runs a classic script. */
function runPrepaint(win: Win): void {
  new Function("window", PREPAINT)(win);
}

function mountControls(win: Win) {
  runPrepaint(win);
  const b = themeControls(win);
  const cleanup = b.mount(win.document);
  return { cleanup: typeof cleanup === "function" ? cleanup : () => undefined };
}

const key = (win: Win, k: string) => {
  const t = win.document.activeElement ?? win.document.body;
  t.dispatchEvent(new win.KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
};
const click = (win: Win, el: Element) =>
  el.dispatchEvent(new win.MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
const byId = (win: Win, id: string): HTMLElement => {
  const e = win.document.getElementById(id);
  if (!e) throw new Error(`no #${id}`);
  return e;
};
const active = (win: Win) => win.document.activeElement;

afterEach(() => vi.useRealTimers());

describe("the generated pre-paint script (P-91, P-93, P-96, P-97, P-98)", () => {
  it("carries the palette build's offer, exactly", () => {
    const m = PREPAINT.match(/^var data = (.*);$/m);
    expect(JSON.parse(m![1]!)).toEqual(THEMES);
    expect(PREPAINT.startsWith("// SPDX-License-Identifier: GPL-3.0-or-later\n")).toBe(true);
    expect(PREPAINT).not.toMatch(/\b(import|export)\b/);
  });

  it("nothing stored: the system's mode and the default palette, no accent", () => {
    for (const dark of [false, true]) {
      const { win, root } = browser({ dark });
      runPrepaint(win);
      expect(root.getAttribute("data-mode")).toBe(dark ? "dark" : "light");
      expect(root.getAttribute("data-theme")).toBe(THEMES.default);
      expect(root.hasAttribute("data-emphasis")).toBe(false);
    }
  });

  it("a stored choice is on <html> before anything else runs", () => {
    const { win, root } = browser({
      stored: { "mae.mode": "dark", "mae.palette": "gruvbox", "mae.accent.gruvbox": "yellow" },
    });
    runPrepaint(win);
    expect([root.getAttribute("data-mode"), root.getAttribute("data-theme"), root.getAttribute("data-emphasis")]).toEqual([
      "dark",
      "gruvbox",
      "yellow",
    ]);
    // The offer is published for the controls.
    expect((win as unknown as { mae: { theme: ThemeData } }).mae.theme.default).toBe(THEMES.default);
  });

  it("injected values not valid for the palette are ignored (P-96)", () => {
    const { win, root } = browser({
      stored: { "mae.mode": "dim", "mae.palette": "gruvbox", "mae.accent.gruvbox": "teal" },
    });
    runPrepaint(win);
    expect(root.getAttribute("data-mode")).toBe("light");
    expect(root.getAttribute("data-theme")).toBe("gruvbox");
    expect(root.hasAttribute("data-emphasis")).toBe(false);
  });

  it("storage that throws: the defaults, and no error (P-97)", () => {
    const { win, root, errors } = browser({ storage: "throws", dark: true });
    expect(() => runPrepaint(win)).not.toThrow();
    expect(root.getAttribute("data-mode")).toBe("dark");
    expect(root.getAttribute("data-theme")).toBe(THEMES.default);
    expect(errors).not.toHaveBeenCalled();
  });

  it("the source and the generated script agree on every stored case", () => {
    const cases: Array<Record<string, string>> = [
      {},
      { "mae.mode": "dark" },
      { "mae.mode": "light", "mae.palette": "nord", "mae.accent.nord": "yellow" },
      { "mae.palette": "gruvbox", "mae.accent.gruvbox": "orange" },
      { "mae.palette": "gruvbox", "mae.accent.gruvbox": "nope" },
      { "mae.palette": "constructor", "mae.mode": "__proto__" },
    ];
    const attrs = (root: Element) => ["data-mode", "data-theme", "data-emphasis"].map((a) => root.getAttribute(a));
    for (const stored of cases)
      for (const dark of [false, true]) {
        const a = browser({ stored, dark });
        runPrepaint(a.win);
        const b = browser({ stored, dark });
        prepaint(b.win, THEMES);
        expect(attrs(b.root), JSON.stringify([stored, dark])).toEqual(attrs(a.root));
      }
  });
});

describe("the controls (P-91, P-93, P-94, P-99)", () => {
  it("absent without the offer: the box stays hidden, nothing is built", () => {
    const { win, doc } = browser();
    themeControls(win).mount(doc);
    expect(byId(win, "mae-theme").hidden).toBe(true);
    expect(doc.getElementById("mae-palette-menu")).toBeNull();
  });

  it("the toggle: a constant name, aria-pressed while dark, applied and stored without reload", () => {
    const { win, root } = browser({ dark: false });
    mountControls(win);
    const toggle = byId(win, "mae-mode-toggle");
    expect(byId(win, "mae-theme").hidden).toBe(false);
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
    click(win, toggle);
    expect(root.getAttribute("data-mode")).toBe("dark");
    expect(toggle.getAttribute("aria-pressed")).toBe("true");
    expect(toggle.textContent).toBe("Dark mode");
    expect(toggle.hasAttribute("title")).toBe(false);
    expect(win.localStorage.getItem("mae.mode")).toBe("dark");
    click(win, toggle);
    expect(root.getAttribute("data-mode")).toBe("light");
    expect(win.localStorage.getItem("mae.mode")).toBe("light");
  });

  it("follows the system live until the reader chooses (P-91)", () => {
    const { win, root, system } = browser({ dark: false });
    mountControls(win);
    system(true);
    expect(root.getAttribute("data-mode")).toBe("dark");
    expect(byId(win, "mae-mode-toggle").getAttribute("aria-pressed")).toBe("true");
    click(win, byId(win, "mae-mode-toggle"));
    system(true);
    expect(root.getAttribute("data-mode")).toBe("light");
  });

  it("the menu's markup: one radio per palette, accents in a submenu, the default first", () => {
    const { win, doc } = browser();
    mountControls(win);
    const menu = byId(win, "mae-palette-menu");
    expect(menu.getAttribute("role")).toBe("menu");
    expect(menu.getAttribute("aria-label")).toBe("Color palette");
    expect(menu.hidden).toBe(true);
    const items = [...menu.querySelectorAll("[data-palette]")];
    expect(items.map((i) => i.getAttribute("data-palette"))).toEqual(Object.keys(THEMES.palettes));
    for (const i of items) {
      const id = i.getAttribute("data-palette")!;
      const p = THEMES.palettes[id]!;
      expect(i.getAttribute("role")).toBe("menuitemradio");
      expect(i.getAttribute("tabindex")).toBe("-1");
      expect(i.textContent).toBe(p.name);
      if (p.accents === null) {
        expect(i.hasAttribute("aria-haspopup")).toBe(false);
        expect(doc.getElementById(`mae-accent-menu-${id}`)).toBeNull();
        continue;
      }
      expect(i.getAttribute("aria-haspopup")).toBe("menu");
      expect(i.getAttribute("aria-controls")).toBe(`mae-accent-menu-${id}`);
      const sub = byId(win, `mae-accent-menu-${id}`);
      expect(sub.getAttribute("aria-label")).toBe(p.name);
      expect([...sub.querySelectorAll("[data-accent]")].map((a) => a.getAttribute("data-accent"))).toEqual(
        Object.keys(p.accents.items),
      );
      expect(sub.querySelector("[data-accent]")!.textContent).toBe("Default");
    }
    // The locale's accent names; an accent the markup does not name keeps its id.
    const gruvbox = byId(win, "mae-accent-menu-gruvbox");
    expect([...gruvbox.querySelectorAll("[role=menuitemradio]")].map((a) => a.textContent)).toEqual([
      "Default",
      "yellow",
      "green",
      "Violet",
    ]);
  });

  // ADR-119 A4: on a locale with no catalogue the daemon marks the box
  // lang="en-US"; the menu built from the box's words is in that language.
  it("the menu is built inside the box, so a fallback box's lang covers it", () => {
    const fallback = HEADER.replace('class="mae-theme" hidden', 'class="mae-theme" hidden lang="en-US"');
    const { win } = browser({ html: PAGE(fallback).replace('<html lang="en-US">', '<html lang="xx-YY">') });
    mountControls(win);
    const menu = byId(win, "mae-palette-menu");
    const accent = byId(win, "mae-accent-menu-gruvbox").querySelector("[data-accent]")!;
    for (const e of [menu, accent]) expect(e.closest("[lang]")?.getAttribute("lang")).toBe("en-US");
  });

  it("keyboard: open on the checked palette, move, choose with its remembered accent, focus back", () => {
    const { win, root } = browser({ stored: { "mae.palette": "nord", "mae.accent.gruvbox": "yellow" } });
    mountControls(win);
    const button = byId(win, "mae-palette-button");
    button.focus();
    click(win, button); // Enter on a button is a click
    expect(button.getAttribute("aria-expanded")).toBe("true");
    expect(active(win)?.getAttribute("data-palette")).toBe("nord");
    expect(active(win)?.getAttribute("aria-checked")).toBe("true");
    key(win, "Home");
    expect(active(win)?.getAttribute("data-palette")).toBe("acario");
    key(win, "ArrowUp");
    expect(active(win)?.getAttribute("data-palette")).toBe("zenburn");
    key(win, "End");
    key(win, "ArrowDown");
    expect(active(win)?.getAttribute("data-palette")).toBe("acario");
    while (active(win)?.getAttribute("data-palette") !== "gruvbox") key(win, "ArrowDown");
    key(win, "Enter");
    expect(root.getAttribute("data-theme")).toBe("gruvbox");
    expect(root.getAttribute("data-emphasis")).toBe("yellow");
    expect(byId(win, "mae-palette-menu").hidden).toBe(true);
    expect(button.getAttribute("aria-expanded")).toBe("false");
    expect(active(win)).toBe(button);
    expect(win.localStorage.getItem("mae.palette")).toBe("gruvbox");
  });

  it("keyboard: → opens the submenu on the checked accent, Enter chooses it, ← and Escape go back", () => {
    const { win, root } = browser({ stored: { "mae.palette": "gruvbox" } });
    mountControls(win);
    const button = byId(win, "mae-palette-button");
    button.focus();
    key(win, "ArrowDown");
    expect(active(win)?.getAttribute("data-palette")).toBe("gruvbox");
    key(win, "ArrowRight");
    const sub = byId(win, "mae-accent-menu-gruvbox");
    expect(sub.hidden).toBe(false);
    expect(active(win)?.getAttribute("data-palette")).toBeNull();
    expect(active(win)?.getAttribute("data-accent")).toBe("orange"); // the default, checked
    expect(active(win)?.getAttribute("aria-checked")).toBe("true");
    key(win, "ArrowLeft");
    expect(sub.hidden).toBe(true);
    expect(active(win)?.getAttribute("data-palette")).toBe("gruvbox");
    key(win, "ArrowRight");
    key(win, "ArrowDown");
    key(win, "ArrowDown");
    expect(active(win)?.getAttribute("data-accent")).toBe("green");
    key(win, "Enter");
    expect(root.getAttribute("data-emphasis")).toBe("green");
    expect(win.localStorage.getItem("mae.accent.gruvbox")).toBe("green");
    expect(sub.hidden).toBe(true);
    expect(byId(win, "mae-palette-menu").hidden).toBe(true);
    expect(active(win)).toBe(button);
    // "Default" clears the stored accent (P-96).
    key(win, "ArrowDown");
    key(win, "ArrowRight");
    expect(active(win)?.getAttribute("data-accent")).toBe("green");
    key(win, "Home");
    key(win, " ");
    expect(root.hasAttribute("data-emphasis")).toBe(false);
    expect(win.localStorage.getItem("mae.accent.gruvbox")).toBeNull();
    // Escape from a submenu closes everything.
    key(win, "ArrowDown");
    key(win, "ArrowRight");
    key(win, "Escape");
    expect(sub.hidden).toBe(true);
    expect(byId(win, "mae-palette-menu").hidden).toBe(true);
    expect(active(win)).toBe(button);
  });

  it("keyboard: → on a palette without accents does nothing; Tab closes and moves on", () => {
    const { win } = browser();
    mountControls(win);
    byId(win, "mae-palette-button").focus();
    key(win, "ArrowDown");
    expect(active(win)?.getAttribute("data-palette")).toBe("neutral");
    key(win, "ArrowRight");
    expect(active(win)?.getAttribute("data-palette")).toBe("neutral");
    const tab = new win.KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    active(win)!.dispatchEvent(tab);
    expect(tab.defaultPrevented).toBe(false);
    expect(byId(win, "mae-palette-menu").hidden).toBe(true);
  });

  it("choosing the site default removes the stored palette (P-93)", () => {
    const { win, root } = browser({ stored: { "mae.palette": "nord" } });
    mountControls(win);
    click(win, byId(win, "mae-palette-button"));
    click(win, win.document.querySelector('[data-palette="neutral"]')!);
    expect(root.getAttribute("data-theme")).toBe("neutral");
    expect(win.localStorage.getItem("mae.palette")).toBeNull();
  });

  it("mouse: hover intent opens a submenu; crossing items opens none", () => {
    vi.useFakeTimers();
    const { win } = browser({ stored: { "mae.palette": "gruvbox" } });
    mountControls(win);
    click(win, byId(win, "mae-palette-button"));
    const over = (el: Element, pointerType = "mouse") =>
      el.dispatchEvent(new win.PointerEvent("pointerover", { bubbles: true, pointerType }));
    const item = (id: string) => win.document.querySelector(`[data-palette="${id}"]`)!;
    over(item("dracula"));
    vi.advanceTimersByTime(HOVER_INTENT_MS - 50);
    over(item("earl-grey"));
    vi.advanceTimersByTime(HOVER_INTENT_MS - 50);
    over(item("gruvbox"));
    vi.advanceTimersByTime(HOVER_INTENT_MS - 1);
    expect(byId(win, "mae-accent-menu-dracula").hidden).toBe(true);
    expect(byId(win, "mae-accent-menu-earl-grey").hidden).toBe(true);
    expect(byId(win, "mae-accent-menu-gruvbox").hidden).toBe(true);
    vi.advanceTimersByTime(1);
    expect(byId(win, "mae-accent-menu-gruvbox").hidden).toBe(false);
    expect(item("gruvbox").getAttribute("aria-expanded")).toBe("true");
    // Resting on a palette without accents closes it; touch has no hover.
    over(item("neutral"));
    vi.advanceTimersByTime(HOVER_INTENT_MS);
    expect(byId(win, "mae-accent-menu-gruvbox").hidden).toBe(true);
    over(item("nord"), "touch");
    vi.advanceTimersByTime(HOVER_INTENT_MS * 2);
    expect(byId(win, "mae-accent-menu-nord").hidden).toBe(true);
  });

  it("click or tap: the selected palette opens its submenu, another is selected; outside closes", () => {
    const { win, root } = browser({ stored: { "mae.palette": "gruvbox" } });
    mountControls(win);
    const button = byId(win, "mae-palette-button");
    click(win, button);
    click(win, win.document.querySelector('[data-palette="gruvbox"]')!);
    expect(byId(win, "mae-accent-menu-gruvbox").hidden).toBe(false);
    click(win, win.document.querySelector('#mae-accent-menu-gruvbox [data-accent="violet"]')!);
    expect(root.getAttribute("data-emphasis")).toBe("violet");
    expect(byId(win, "mae-palette-menu").hidden).toBe(true);
    click(win, button);
    click(win, win.document.querySelector('[data-palette="nord"]')!);
    expect(root.getAttribute("data-theme")).toBe("nord");
    expect(byId(win, "mae-accent-menu-nord").hidden).toBe(true);
    click(win, button);
    expect(byId(win, "mae-palette-menu").hidden).toBe(false);
    byId(win, "to-b").dispatchEvent(new win.PointerEvent("pointerdown", { bubbles: true }));
    expect(byId(win, "mae-palette-menu").hidden).toBe(true);
    // The button toggles: a second click closes.
    click(win, button);
    click(win, button);
    expect(byId(win, "mae-palette-menu").hidden).toBe(true);
  });

  it("storage that throws: no error, and toggle and menu work for the session (P-97)", () => {
    const { win, root, errors } = browser({ storage: "throws" });
    mountControls(win);
    click(win, byId(win, "mae-mode-toggle"));
    expect(root.getAttribute("data-mode")).toBe("dark");
    click(win, byId(win, "mae-palette-button"));
    click(win, win.document.querySelector('[data-palette="gruvbox"]')!);
    click(win, byId(win, "mae-palette-button"));
    click(win, win.document.querySelector('[data-palette="gruvbox"]')!);
    click(win, win.document.querySelector('#mae-accent-menu-gruvbox [data-accent="green"]')!);
    expect(root.getAttribute("data-theme")).toBe("gruvbox");
    expect(root.getAttribute("data-emphasis")).toBe("green");
    // Switching away and back restores the session's accent (P-96).
    click(win, byId(win, "mae-palette-button"));
    click(win, win.document.querySelector('[data-palette="nord"]')!);
    expect(root.hasAttribute("data-emphasis")).toBe(false);
    click(win, byId(win, "mae-palette-button"));
    key(win, "Home");
    while (active(win)?.getAttribute("data-palette") !== "gruvbox") key(win, "ArrowDown");
    key(win, "Enter");
    expect(root.getAttribute("data-emphasis")).toBe("green");
    expect(errors).not.toHaveBeenCalled();
  });

  it("swatches show the palette's colours in the mode in effect", () => {
    const { win } = browser();
    mountControls(win);
    const sw = () =>
      [...win.document.querySelectorAll('[data-palette="gruvbox"] .mae-swatch')].map(
        (s) => (s as HTMLElement).style.backgroundColor,
      );
    const light = sw();
    expect(light).toHaveLength(THEMES.palettes.gruvbox!.swatches.light.length);
    click(win, byId(win, "mae-mode-toggle"));
    expect(sw()).not.toEqual(light);
    for (const s of win.document.querySelectorAll(".mae-menu-deco")) expect(s.getAttribute("aria-hidden")).toBe("true");
  });
});

describe("under the router", () => {
  it("each page's header gets working controls; the choice survives navigation", async () => {
    const { win, root } = browser({ stored: { "mae.palette": "gruvbox" } });
    runPrepaint(win);
    const fetch = vi.fn(async () => ({
      type: "basic",
      status: 200,
      headers: { get: () => "text/html; charset=utf-8" },
      text: async () => PAGE().replace("<title>A</title>", "<title>B</title>"),
    }));
    (win as unknown as { fetch: unknown }).fetch = fetch;
    (win as unknown as { scrollTo: unknown }).scrollTo = vi.fn();
    // Every listener the controls add goes with their page: record the signals.
    const signals: AbortSignal[] = [];
    const add = win.addEventListener.bind(win);
    win.addEventListener = ((type: string, f: EventListener, o?: AddEventListenerOptions) => {
      if (o?.signal) signals.push(o.signal);
      add(type, f, o);
    }) as typeof win.addEventListener;
    const router = start(win as unknown as Parameters<typeof start>[0]);
    router.register("theme", themeControls(win));
    click(win, byId(win, "mae-mode-toggle"));
    const mode = root.getAttribute("data-mode");
    const old = byId(win, "mae-palette-button");
    const first = [...signals];
    expect(first.length).toBeGreaterThan(0);
    expect(first.some((x) => x.aborted)).toBe(false);
    click(win, byId(win, "to-b"));
    await router.pending();
    expect(first.every((x) => x.aborted)).toBe(true);
    expect(win.document.title).toBe("B");
    const button = byId(win, "mae-palette-button");
    expect(button).not.toBe(old);
    expect(byId(win, "mae-theme").hidden).toBe(false);
    expect(win.document.querySelectorAll("#mae-palette-menu")).toHaveLength(1);
    expect(root.getAttribute("data-mode")).toBe(mode);
    expect(root.getAttribute("data-theme")).toBe("gruvbox");
    expect(byId(win, "mae-mode-toggle").getAttribute("aria-pressed")).toBe(mode === "dark" ? "true" : "false");
    click(win, button);
    expect(byId(win, "mae-palette-menu").hidden).toBe(false);
    // The old page's listeners are gone: a click on the detached button does nothing.
    click(win, old);
    expect(byId(win, "mae-palette-menu").hidden).toBe(false);
  });
});
