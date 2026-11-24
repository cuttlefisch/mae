// SPDX-License-Identifier: GPL-3.0-or-later
//
// The live KB site's in-page router (ADR-119 D16, A6). Navigation between the
// site's pages never reloads: the router fetches the next page through the
// browser client's backend, swaps its title, main landmark and side panels
// into this document, and keeps history, focus, scroll and the components'
// lifecycle right. Anything it cannot do safely becomes a full navigation.
//
// @ai-caution: [security] This is the only code on the page that turns a
// string into HTML (DOMParser, under the Trusted Types policy `mae-router`,
// which the page's CSP names as the only one allowed). The HTML it parses is
// the daemon's own page, already sanitised by mae-site (D19); the router adds
// nothing to it and never parses anything else. Where a browser has no
// Trusted Types the directive is ignored, so this must stay the only writer
// by construction, not by enforcement: add no other `innerHTML`,
// `insertAdjacentHTML`, `document.write` or parser call.
//
// Rules from the browser client's backend, which every fetch goes
// through: GET only; no custom request headers (they are dropped); a missing
// session answers with a cross-origin redirect to the login, so redirects are
// never followed here; the backend's own pages (its fixed 404, its 502) are
// not the site's and carry no `main#mae-main`.

/** Where the site's pages live: every live route is under `/kb/`. */
export const SITE_PREFIX = "/kb/";

/** Ids the core's chrome uses (`mae-` is reserved: no content anchor has it). */
export const MAIN_ID = "mae-main";
export const PANELS_ID = "mae-panels";
export const HEADER_ID = "mae-header";
export const ANNOUNCER_ID = "mae-announcer";
export const EXPLORER_ID = "mae-explorer";

/** What a click asks for, as far as routing is concerned. */
export interface ClickFacts {
  button: number;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  defaultPrevented: boolean;
}

/** The link attributes that decide whether the router may take a click. */
export interface LinkFacts {
  /** `href`, or an SVG link's `xlink:href` (P-26). */
  href: string | null;
  target: string | null;
  download: boolean;
}

/**
 * The URL the router should fetch for a click on a link, or `null` when the
 * browser should handle it: a modified or non-primary click, a new window, a
 * download, another origin, a path outside the site, or a jump within the
 * current page.
 */
export function routableClick(click: ClickFacts, link: LinkFacts, here: URL): URL | null {
  if (click.defaultPrevented || click.button !== 0) return null;
  if (click.metaKey || click.ctrlKey || click.shiftKey || click.altKey) return null;
  if (link.href === null || link.download) return null;
  if (link.target !== null && link.target !== "" && link.target !== "_self") return null;
  let url: URL;
  try {
    url = new URL(link.href, here);
  } catch {
    return null;
  }
  if (url.origin !== here.origin || !url.pathname.startsWith(SITE_PREFIX)) return null;
  // Same page, another fragment: the browser scrolls, history records it.
  if (url.pathname === here.pathname && url.search === here.search && url.hash !== "") {
    return null;
  }
  return url;
}

/**
 * The element id a URL fragment names, decoded; the raw text when it is not
 * valid percent-encoding (`#%E0`), which then simply names nothing. Never
 * throws: it runs after the swap, when the URL has already changed.
 */
export function fragmentId(hash: string): string {
  const raw = hash.startsWith("#") ? hash.slice(1) : hash;
  try {
    return decodeURIComponent(raw);
  } catch {
    return raw;
  }
}

/** What the response to a page fetch allows. */
export type Outcome = "swap" | "full";

/** The parts of a fetch response the router decides on. */
export interface ResponseFacts {
  type: string;
  status: number;
  contentType: string | null;
}

/**
 * `swap` only for a plain 200 HTML page. A redirect (the login, on an expired
 * session: `opaqueredirect` under `redirect: "manual"`), an error status or
 * any other body is handed to the browser as a full navigation, which then
 * does whatever that answer means.
 */
export function outcome(r: ResponseFacts): Outcome {
  if (r.type === "opaqueredirect" || r.status !== 200) return "full";
  const ct = (r.contentType ?? "").toLowerCase();
  return ct.startsWith("text/html") ? "swap" : "full";
}

/** The stylesheets and scripts a document loads, in order. */
export function assetsOf(doc: Document): string[] {
  const out: string[] = [];
  for (const el of Array.from(doc.querySelectorAll("link[rel=stylesheet][href], script[src]"))) {
    out.push(el.getAttribute("href") ?? el.getAttribute("src") ?? "");
  }
  return out;
}

/**
 * Whether `next` can be swapped into `current`: it is one of the site's pages
 * (it has the main landmark), and it loads exactly the assets this document
 * already has. A page from a newer daemon (other asset names) or from the
 * backend (no landmark) is a full navigation instead.
 */
export function swappable(current: Document, next: Document): boolean {
  if (next.getElementById(MAIN_ID) === null) return false;
  const a = assetsOf(current);
  const b = assetsOf(next);
  return a.length === b.length && a.every((x, i) => x === b[i]);
}

/**
 * Say `text` through the page's one live region, which the router creates on
 * first use: navigations and components announce through the same element.
 */
export function announce(doc: Document, text: string): void {
  let live = doc.getElementById(ANNOUNCER_ID);
  if (live === null) {
    live = doc.createElement("div");
    live.id = ANNOUNCER_ID;
    live.setAttribute("aria-live", "polite");
    live.className = "mae-visually-hidden";
    doc.body.append(live);
  }
  live.textContent = text;
}

/** A component: mounted on every page, cleaned up before the next (P-25, P-156). */
export interface Behaviour {
  mount(root: Document): void | (() => void);
}

/** What `start` returns, for tests and for components. */
export interface Router {
  register(name: string, b: Behaviour): void;
  navigate(url: URL, push: boolean): Promise<void>;
  /** The navigation in flight, if any (tests wait on it). */
  pending(): Promise<void> | null;
}

interface TrustedTypesLike {
  createPolicy(
    name: string,
    rules: { createHTML(s: string): string },
  ): { createHTML(s: string): unknown };
}

/** Install the router on `win`: click and history handling, components. */
export function start(win: Window & typeof globalThis): Router {
  const doc = win.document;
  const tt = (win as unknown as { trustedTypes?: TrustedTypesLike }).trustedTypes;
  // The only HTML sink on the page; see the file header.
  const policy = tt?.createPolicy("mae-router", { createHTML: (s) => s });
  const parse = (html: string): Document =>
    new win.DOMParser().parseFromString(
      (policy ? policy.createHTML(html) : html) as string,
      "text/html",
    );

  const behaviours = new Map<string, Behaviour>();
  let cleanups: Array<() => void> = [];
  const mountAll = (): void => {
    for (const b of behaviours.values()) {
      const c = b.mount(doc);
      if (typeof c === "function") cleanups.push(c);
    }
  };
  const unmountAll = (): void => {
    for (const c of cleanups) c();
    cleanups = [];
  };

  win.history.scrollRestoration = "manual";
  // The page this document shows, by path and query: a history move within it
  // (a fragment) is the browser's, not a navigation.
  const pageOf = (u: URL): string => u.pathname + u.search;
  let shown = pageOf(new URL(win.location.href));
  let inFlight: AbortController | null = null;
  let current: Promise<void> | null = null;

  const swapIn = (next: Document): void => {
    unmountAll();
    doc.title = next.title;
    doc.documentElement.lang = next.documentElement.lang;
    const main = doc.getElementById(MAIN_ID);
    const nextMain = next.getElementById(MAIN_ID);
    if (main && nextMain) main.replaceWith(doc.adoptNode(nextMain));
    // The site's header, then the explorer, before the main landmark, and the
    // side panels after it: each replaced, removed or added to match the next
    // page; the explorer kept instead when `keep` says it is the same list.
    const region = (
      id: string,
      place: "before" | "after",
      at: () => Element | null,
      keep?: (mine: Element, theirs: Element) => boolean,
    ): void => {
      const mine = doc.getElementById(id);
      const theirs = next.getElementById(id);
      if (mine && theirs) {
        if (!keep?.(mine, theirs)) mine.replaceWith(doc.adoptNode(theirs));
      } else if (mine) mine.remove();
      else if (theirs) at()?.[place](doc.adoptNode(theirs));
    };
    const mainOf = (): Element | null => doc.getElementById(MAIN_ID);
    region(EXPLORER_ID, "before", mainOf, keepExplorer);
    region(HEADER_ID, "before", () => doc.getElementById(EXPLORER_ID) ?? mainOf());
    region(PANELS_ID, "after", mainOf);
    mountAll();
  };

  const settle = (url: URL, scrollY: number | null): void => {
    const target = url.hash ? doc.getElementById(fragmentId(url.hash)) : null;
    if (scrollY !== null) win.scrollTo(0, scrollY);
    else if (target) target.scrollIntoView();
    else win.scrollTo(0, 0);
    const focus = target ?? doc.querySelector(`#${MAIN_ID} h1`) ?? doc.getElementById(MAIN_ID);
    if (focus !== null && focus instanceof win.HTMLElement) {
      if (!focus.hasAttribute("tabindex")) focus.setAttribute("tabindex", "-1");
      focus.focus({ preventScroll: true });
    }
    announce(doc, doc.title);
  };

  const run = async (url: URL, push: boolean, scrollY: number | null): Promise<void> => {
    inFlight?.abort();
    const ctl = new AbortController();
    inFlight = ctl;
    let res: Response;
    try {
      res = await win.fetch(url.href, {
        method: "GET",
        credentials: "same-origin",
        redirect: "manual",
        signal: ctl.signal,
      });
    } catch {
      if (ctl.signal.aborted) return; // a later navigation won
      win.location.assign(url.href);
      return;
    }
    if (ctl.signal.aborted) return;
    const facts = {
      type: res.type,
      status: res.status,
      contentType: res.headers.get("content-type"),
    };
    if (outcome(facts) === "full") {
      win.location.assign(url.href);
      return;
    }
    const next = parse(await res.text());
    if (ctl.signal.aborted) return;
    if (!swappable(doc, next)) {
      win.location.assign(url.href);
      return;
    }
    if (push) {
      win.history.replaceState({ ...(win.history.state ?? {}), scrollY: win.scrollY }, "");
      win.history.pushState({ scrollY: null }, "", url.href);
    }
    swapIn(next);
    shown = pageOf(url);
    settle(url, scrollY);
    if (inFlight === ctl) inFlight = null;
  };

  const navigate = (url: URL, push: boolean, scrollY: number | null = null): Promise<void> => {
    current = run(url, push, scrollY);
    return current;
  };

  doc.addEventListener("click", (ev) => {
    const anchor = (ev.target as Element | null)?.closest?.("a");
    if (!anchor) return;
    const link: LinkFacts = {
      href: anchor.getAttribute("href") ?? anchor.getAttribute("xlink:href"),
      target: anchor.getAttribute("target"),
      download: anchor.hasAttribute("download"),
    };
    const url = routableClick(ev as MouseEvent, link, new URL(win.location.href));
    if (url === null) return;
    ev.preventDefault();
    void navigate(url, true);
  });

  win.addEventListener("popstate", (ev) => {
    const url = new URL(win.location.href);
    if (pageOf(url) === shown) {
      // Back to the page on screen: whatever was loading is no longer wanted.
      inFlight?.abort();
      return;
    }
    const state = (ev as PopStateEvent).state as { scrollY?: number | null } | null;
    void navigate(url, false, state?.scrollY ?? null);
  });

  return {
    register(name, b) {
      behaviours.set(name, b);
      const c = b.mount(doc);
      if (typeof c === "function") cleanups.push(c);
    },
    navigate: (url, push) => navigate(url, push),
    pending: () => current,
  };
}

// ---------------------------------------------------------------------------
// Mode and palette (ADR-119 D17; P-91 to P-99). The reader's choice lives in
// three attributes on <html> (data-mode, data-theme, data-emphasis), which
// palette.css and base.css read. The pre-paint script sets them before first
// paint from storage; the controls below change them without a reload. Under
// the router <html> survives navigation, so a choice holds for the session
// even when storage throws (P-97).
//
// @ai-caution: [build] The functions from `storageKey` to `prepaint` are also
// the pre-paint script: scripts/prepaint.mjs copies their source text
// (Function.prototype.toString) into dist/prepaint.js, a classic script. Each
// must stay a self-contained function declaration that calls only the others
// in that list: no module-level constant, no import. test/prepaint.test.ts
// runs the generated file.

/** A colour mode. */
export type Mode = "light" | "dark";

/** One palette as the palette build publishes it (palette/dist/themes.json). */
export interface PaletteData {
  /** Its display name: a proper name, never translated (A4). */
  name: string;
  /** The menu's swatches in each mode. */
  swatches: Record<Mode, string[]>;
  /** Its accents, the default first; null when it offers none. */
  accents: { default: string; items: Record<string, Record<Mode, string>> } | null;
}

/** What the site offers: palette/dist/themes.json. */
export interface ThemeData {
  default: string;
  /** Removed palette -> the palette that replaces it (P-98). */
  successors: Record<string, string>;
  palettes: Record<string, PaletteData>;
}

/** A reader's choice, validated. `accent` null: the palette's default. */
export interface Choice {
  /** null: follow the system. */
  mode: Mode | null;
  palette: string;
  accent: string | null;
}

/** The storage key of each part of a choice (MAE's own names). */
export function storageKey(part: "mode" | "palette" | "accent", palette?: string): string {
  return part === "accent" ? "mae.accent." + palette : "mae." + part;
}

/** A stored string; null when absent, or when storage itself throws (P-97). */
export function readStored(storage: () => Pick<Storage, "getItem">, key: string): string | null {
  try {
    const v = storage().getItem(key);
    return typeof v === "string" ? v : null;
  } catch {
    return null;
  }
}

/**
 * The accent `stored` of `palette` if it is one of that palette's and not its
 * default; null otherwise (the default, P-96). Checked with Object.hasOwn: a
 * stored string is never used as a key before it is known to be one.
 */
export function validAccent(data: ThemeData, palette: string, stored: string | null): string | null {
  if (stored === null || !Object.hasOwn(data.palettes, palette)) return null;
  const accents = data.palettes[palette]?.accents ?? null;
  if (accents === null || !Object.hasOwn(accents.items, stored)) return null;
  return stored === accents.default ? null : stored;
}

/**
 * The stored choice, validated against the published lists: an unknown mode
 * follows the system, an unknown palette is the default, an accent not valid
 * for the palette is its default, and a removed palette becomes its successor
 * without an accent (P-98).
 */
export function resolveChoice(data: ThemeData, read: (key: string) => string | null): Choice {
  const m = read(storageKey("mode"));
  const mode = m === "light" || m === "dark" ? m : null;
  const stored = read(storageKey("palette"));
  if (stored !== null && !Object.hasOwn(data.palettes, stored)) {
    const next = Object.hasOwn(data.successors, stored) ? data.successors[stored] : undefined;
    if (next !== undefined && Object.hasOwn(data.palettes, next)) {
      return { mode, palette: next, accent: null };
    }
  }
  const palette = stored !== null && Object.hasOwn(data.palettes, stored) ? stored : data.default;
  return { mode, palette, accent: validAccent(data, palette, read(storageKey("accent", palette))) };
}

/** Whether the system asks for dark; false where it cannot say. */
export function systemDark(win: Pick<Window, "matchMedia">): boolean {
  try {
    return win.matchMedia("(prefers-color-scheme: dark)").matches;
  } catch {
    return false;
  }
}

/** Writes a choice on <html> (P-92): base.css sets color-scheme from data-mode. */
export function applyChoice(root: Element, choice: Choice, mode: Mode): void {
  root.setAttribute("data-mode", mode);
  root.setAttribute("data-theme", choice.palette);
  if (choice.accent === null) root.removeAttribute("data-emphasis");
  else root.setAttribute("data-emphasis", choice.accent);
}

/**
 * The pre-paint script: apply the stored choice before first paint (P-91,
 * P-93, P-96), and publish the offer for the controls (`mae.theme`).
 */
export function prepaint(win: Window, data: ThemeData): void {
  const choice = resolveChoice(data, (k) => readStored(() => win.localStorage, k));
  const mode = choice.mode ?? (systemDark(win) ? "dark" : "light");
  applyChoice(win.document.documentElement, choice, mode);
  const w = win as unknown as { mae?: Record<string, unknown> };
  w.mae = { ...(w.mae ?? {}), theme: data };
}

/**
 * Storage for the controls: what this page wrote wins, so a choice holds for
 * the session when storage throws (P-97); every write also tries storage.
 */
export function sessionStore(storage: () => Pick<Storage, "getItem" | "setItem" | "removeItem">): {
  get(key: string): string | null;
  set(key: string, value: string | null): void;
} {
  const written = new Map<string, string | null>();
  return {
    get(key) {
      if (written.has(key)) return written.get(key) ?? null;
      return readStored(storage, key);
    },
    set(key, value) {
      written.set(key, value);
      try {
        if (value === null) storage().removeItem(key);
        else storage().setItem(key, value);
      } catch {
        // Kept for the session only.
      }
    },
  };
}

/** What a key does inside the menu (WAI-ARIA APG menu button). */
export type MenuAction =
  | "next"
  | "prev"
  | "first"
  | "last"
  | "choose"
  | "open-sub"
  | "close-sub"
  | "close"
  | "leave";

/** The action of `key` on a palette (`inSubmenu` false) or on an accent. */
export function menuAction(key: string, inSubmenu: boolean): MenuAction | null {
  switch (key) {
    case "ArrowDown":
      return "next";
    case "ArrowUp":
      return "prev";
    case "Home":
      return "first";
    case "End":
      return "last";
    case "Enter":
    case " ":
      return "choose";
    case "ArrowRight":
      return inSubmenu ? null : "open-sub";
    case "ArrowLeft":
      return inSubmenu ? "close-sub" : null;
    case "Escape":
      return "close";
    case "Tab":
      return "leave";
    default:
      return null;
  }
}

/** The item focus moves to: wrapping at both ends; -1 for an empty list. */
export function moveFocus(action: MenuAction, current: number, count: number): number {
  if (count <= 0) return -1;
  switch (action) {
    case "first":
      return 0;
    case "last":
      return count - 1;
    case "next":
      return (current + 1) % count;
    case "prev":
      return (current - 1 + count) % count;
    default:
      return current;
  }
}

/** A box on screen, in CSS pixels. */
export interface Box {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** Where a menu goes: its corner and the height it may take before scrolling. */
export interface Placement {
  left: number;
  top: number;
  maxHeight: number;
}

/** The gap kept between a menu and the window's edge, in CSS pixels. */
export const EDGE = 8;

const clamp = (v: number, lo: number, hi: number): number => Math.max(lo, Math.min(v, hi));

/**
 * The palette menu under its button, its right edge on the button's, inside
 * the window. Above the button only when there is more room there; either way
 * it scrolls inside itself when the list is longer than the room (P-99).
 */
export function placeMenu(button: Box, width: number, height: number, vw: number, vh: number): Placement {
  const w = Math.min(width, vw - 2 * EDGE);
  const left = clamp(button.right - w, EDGE, Math.max(EDGE, vw - EDGE - w));
  const below = vh - EDGE - button.bottom;
  const above = button.top - EDGE;
  if (height > below && above > below) {
    const maxHeight = Math.max(0, above);
    return { left, top: button.top - Math.min(height, maxHeight), maxHeight };
  }
  return { left, top: button.bottom, maxHeight: Math.max(0, below) };
}

/**
 * An accent submenu beside its palette's menu: to the right when it fits,
 * else to the left, else over the list, aligned with its item (P-99).
 */
export function placeSubmenu(item: Box, menu: Box, width: number, height: number, vw: number, vh: number): Placement {
  const w = Math.min(width, vw - 2 * EDGE);
  let left: number;
  if (menu.right + w <= vw - EDGE) left = menu.right;
  else if (menu.left - w >= EDGE) left = menu.left - w;
  else left = clamp(menu.left, EDGE, Math.max(EDGE, vw - EDGE - w));
  const maxHeight = Math.max(0, vh - 2 * EDGE);
  const top = clamp(item.top, EDGE, Math.max(EDGE, vh - EDGE - Math.min(height, maxHeight)));
  return { left, top, maxHeight };
}

/**
 * How long the pointer rests on a palette before its submenu opens, in ms
 * (P-99: crossing other items on the way to a submenu opens none of them).
 * Between GTK's menu popup delay (225 ms) and Windows' MenuShowDelay
 * (400 ms): long enough that a diagonal move across one or two items opens
 * nothing, short enough to feel immediate when the pointer stops.
 */
export const HOVER_INTENT_MS = 300;

/** Ids of the controls (core chrome: `mae-` is reserved, A2). */
export const THEME_ID = "mae-theme";
export const MODE_TOGGLE_ID = "mae-mode-toggle";
export const PALETTE_BUTTON_ID = "mae-palette-button";
export const PALETTE_MENU_ID = "mae-palette-menu";
export const ACCENT_MENU_PREFIX = "mae-accent-menu-";

/** The words the page's locale gives the menu, read from the daemon's markup. */
interface Labels {
  menu: string;
  defaultAccent: string;
  accent(id: string): string;
}

/** One palette's entry, as built. */
interface Entry {
  id: string;
  item: HTMLElement;
  sub: HTMLElement | null;
  accents: HTMLElement[];
  swatches: HTMLElement[];
}

function el(doc: Document, tag: string, attrs: Record<string, string>): HTMLElement {
  const e = doc.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  return e;
}

function swatchEl(doc: Document): HTMLElement {
  return el(doc, "span", { class: "mae-swatch" });
}

/** A menuitemradio: a check mark and swatches (both hidden from AT), then its name. */
function radioItem(doc: Document, attrs: Record<string, string>, name: string, swatches: number) {
  const item = el(doc, "div", { role: "menuitemradio", tabindex: "-1", "aria-checked": "false", ...attrs });
  item.className = "mae-menu-item";
  const deco = el(doc, "span", { "aria-hidden": "true", class: "mae-menu-deco" });
  deco.append(el(doc, "span", { class: "mae-check" }));
  const swatchEls: HTMLElement[] = [];
  for (let i = 0; i < swatches; i++) swatchEls.push(swatchEl(doc));
  deco.append(...swatchEls);
  const label = el(doc, "span", { class: "mae-menu-label" });
  label.textContent = name;
  item.append(deco, label);
  return { item, swatchEls };
}

function buildEntry(doc: Document, id: string, p: PaletteData, labels: Labels): { wrap: HTMLElement; entry: Entry } {
  const wrap = el(doc, "div", { role: "none" });
  const subId = ACCENT_MENU_PREFIX + id;
  const attrs: Record<string, string> = { "data-palette": id };
  if (p.accents !== null) {
    Object.assign(attrs, { "aria-haspopup": "menu", "aria-expanded": "false", "aria-controls": subId });
  }
  const { item, swatchEls } = radioItem(doc, attrs, p.name, p.swatches.light.length);
  if (p.accents !== null) item.append(el(doc, "span", { "aria-hidden": "true", class: "mae-more" }));
  wrap.append(item);
  const entry: Entry = { id, item, sub: null, accents: [], swatches: swatchEls };
  if (p.accents !== null) {
    const sub = el(doc, "div", { role: "menu", id: subId, "aria-label": p.name, class: "mae-menu mae-submenu" });
    sub.hidden = true;
    for (const a of Object.keys(p.accents.items)) {
      const name = a === p.accents.default ? labels.defaultAccent : labels.accent(a);
      const r = radioItem(doc, { "data-accent": a }, name, 1);
      entry.accents.push(r.item);
      entry.swatches.push(...r.swatchEls);
      sub.append(r.item);
    }
    entry.sub = sub;
    wrap.append(sub);
  }
  return { wrap, entry };
}

/** The palette menu, closed: one entry per palette, in the published order. */
export function buildMenu(doc: Document, data: ThemeData, labels: Labels): { menu: HTMLElement; entries: Entry[] } {
  const menu = el(doc, "div", { role: "menu", id: PALETTE_MENU_ID, "aria-label": labels.menu, class: "mae-menu" });
  menu.hidden = true;
  const entries: Entry[] = [];
  for (const [id, p] of Object.entries(data.palettes)) {
    const { wrap, entry } = buildEntry(doc, id, p, labels);
    menu.append(wrap);
    entries.push(entry);
  }
  return { menu, entries };
}

/** The labels: the button's own name, and the words the daemon put on the box. */
function labelsOf(box: HTMLElement, button: HTMLElement): Labels {
  return {
    menu: (button.textContent ?? "").trim(),
    defaultAccent: box.getAttribute("data-default-label") ?? "",
    accent: (id) => box.getAttribute("data-accent-" + id) ?? id,
  };
}

function setBox(e: HTMLElement, p: Placement): void {
  e.style.left = `${p.left}px`;
  e.style.top = `${p.top}px`;
  e.style.maxHeight = `${p.maxHeight}px`;
}

/**
 * The mode toggle and the palette menu (P-91 to P-99), as a behaviour: mounted
 * on every page into the daemon's `#mae-theme` box in the header, which ships
 * hidden so that without JavaScript the controls are absent, never broken.
 * Built with createElement and textContent only (see the file header).
 */
export function themeControls(win: Window & typeof globalThis): Behaviour {
  const store = sessionStore(() => win.localStorage);
  const root = win.document.documentElement;
  let refresh: (() => void) | null = null;
  const chosenMode = (): boolean => {
    const m = store.get(storageKey("mode"));
    return m === "light" || m === "dark";
  };
  // Follow the system, live, until the reader chooses (P-91).
  try {
    win.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", (ev) => {
      if (chosenMode()) return;
      root.setAttribute("data-mode", ev.matches ? "dark" : "light");
      refresh?.();
    });
  } catch {
    // No media queries: the mode stays as painted.
  }
  return {
    mount(doc) {
      const data = (win as unknown as { mae?: { theme?: ThemeData } }).mae?.theme;
      const box = doc.getElementById(THEME_ID);
      const toggle = doc.getElementById(MODE_TOGGLE_ID);
      const button = doc.getElementById(PALETTE_BUTTON_ID);
      if (!data || !box || !toggle || !button) return;
      const ctl = mountControls(win, doc, data, store, { box, toggle, button });
      refresh = ctl.refresh;
      return () => {
        ctl.cleanup();
        refresh = null;
      };
    },
  };
}

interface Parts {
  box: HTMLElement;
  toggle: HTMLElement;
  button: HTMLElement;
}

/** The state of one page's controls, and what changes it. */
function mountControls(
  win: Window & typeof globalThis,
  doc: Document,
  data: ThemeData,
  store: ReturnType<typeof sessionStore>,
  { box, toggle, button }: Parts,
): { refresh(): void; cleanup(): void } {
  const root = doc.documentElement;
  doc.getElementById(PALETTE_MENU_ID)?.remove();
  const { menu, entries } = buildMenu(doc, data, labelsOf(box, button));
  button.after(menu);
  const ac = new AbortController();
  // Every listener goes with the page (cleanup aborts them all).
  const on = <E extends Event>(t: EventTarget, type: string, f: (ev: E) => void): void =>
    t.addEventListener(type, f as unknown as EventListener, { signal: ac.signal });
  let openSub: Entry | null = null;
  let hover: ReturnType<typeof setTimeout> | undefined;

  const current = (): Choice => {
    const t = root.getAttribute("data-theme");
    const palette = t !== null && Object.hasOwn(data.palettes, t) ? t : data.default;
    return { mode: null, palette, accent: validAccent(data, palette, root.getAttribute("data-emphasis")) };
  };
  const mode = (): Mode => (root.getAttribute("data-mode") === "dark" ? "dark" : "light");
  // The accent checked in a palette's submenu: the current one for the active
  // palette, the remembered one for the others.
  const accentOf = (id: string): string | null => {
    const c = current();
    return c.palette === id ? c.accent : validAccent(data, id, store.get(storageKey("accent", id)));
  };

  const refresh = (): void => {
    const c = current();
    const m = mode();
    toggle.setAttribute("aria-pressed", m === "dark" ? "true" : "false");
    for (const e of entries) {
      const p = data.palettes[e.id]!;
      e.item.setAttribute("aria-checked", e.id === c.palette ? "true" : "false");
      const checked = accentOf(e.id) ?? p.accents?.default;
      const colours = [...p.swatches[m], ...Object.values(p.accents?.items ?? {}).map((a) => a[m])];
      e.swatches.forEach((s, i) => (s.style.backgroundColor = colours[i] ?? ""));
      for (const a of e.accents) {
        a.setAttribute("aria-checked", a.getAttribute("data-accent") === checked ? "true" : "false");
      }
    }
  };

  const viewport = (): [number, number] => [doc.documentElement.clientWidth || win.innerWidth, win.innerHeight];
  const place = (): void => {
    if (menu.hidden) return;
    const [vw, vh] = viewport();
    setBox(menu, placeMenu(button.getBoundingClientRect(), menu.offsetWidth, menu.scrollHeight, vw, vh));
    if (openSub?.sub) {
      const s = openSub.sub;
      const at = placeSubmenu(openSub.item.getBoundingClientRect(), menu.getBoundingClientRect(), s.offsetWidth, s.scrollHeight, vw, vh);
      setBox(s, at);
    }
  };
  const focusOn = (e: HTMLElement | undefined): void => {
    if (!e) return;
    e.focus();
    e.scrollIntoView?.({ block: "nearest" });
  };
  const closeSub = (): void => {
    if (openSub?.sub) {
      openSub.sub.hidden = true;
      openSub.item.setAttribute("aria-expanded", "false");
    }
    openSub = null;
  };
  const openSubOf = (e: Entry, focus: boolean): void => {
    if (!e.sub) return;
    if (openSub !== e) closeSub();
    openSub = e;
    e.sub.hidden = false;
    e.item.setAttribute("aria-expanded", "true");
    place();
    if (focus) focusOn(e.accents.find((a) => a.getAttribute("aria-checked") === "true") ?? e.accents[0]);
  };
  const open = (): void => {
    refresh();
    menu.hidden = false;
    button.setAttribute("aria-expanded", "true");
    place();
    focusOn(entries.find((e) => e.id === current().palette)?.item ?? entries[0]?.item);
  };
  const close = (refocus: boolean): void => {
    clearTimeout(hover);
    closeSub();
    menu.hidden = true;
    button.setAttribute("aria-expanded", "false");
    if (refocus) button.focus();
  };

  const choose = (palette: string, accent: string | null): void => {
    const p = data.palettes[palette]!;
    store.set(storageKey("palette"), palette === data.default ? null : palette);
    if (p.accents !== null) store.set(storageKey("accent", palette), accent);
    applyChoice(root, { mode: null, palette, accent }, mode());
    refresh();
    close(true);
  };
  const choosePalette = (e: Entry): void => choose(e.id, accentOf(e.id));
  const chooseAccent = (e: Entry, a: HTMLElement): void =>
    choose(e.id, validAccent(data, e.id, a.getAttribute("data-accent")));

  const entryOf = (t: EventTarget | null): Entry | undefined => {
    const item = (t as Element | null)?.closest?.("[data-palette]");
    return entries.find((e) => e.item === item);
  };
  const accentAt = (t: EventTarget | null): [Entry, HTMLElement] | undefined => {
    const a = (t as Element | null)?.closest?.("[data-accent]");
    const e = entries.find((x) => x.accents.some((y) => y === a));
    return e && a ? [e, a as HTMLElement] : undefined;
  };

  on(toggle, "click", () => {
    const next: Mode = mode() === "dark" ? "light" : "dark";
    store.set(storageKey("mode"), next);
    root.setAttribute("data-mode", next);
    refresh();
  });
  on(button, "click", () => (menu.hidden ? open() : close(false)));
  on(button, "keydown", (ev: KeyboardEvent) => {
    if (ev.key !== "ArrowDown" && ev.key !== "ArrowUp") return;
    ev.preventDefault();
    if (menu.hidden) open();
  });
  on(menu, "keydown", (ev: KeyboardEvent) => {
    const acc = accentAt(ev.target);
    const action = menuAction(ev.key, acc !== undefined);
    if (action === null) return;
    if (action === "leave") return close(false);
    ev.preventDefault();
    const e = acc?.[0] ?? entryOf(ev.target);
    if (!e) return;
    const list = acc ? e.accents : entries.map((x) => x.item);
    const at = list.indexOf(acc ? acc[1] : e.item);
    switch (action) {
      case "choose":
        return acc ? chooseAccent(e, acc[1]) : choosePalette(e);
      case "open-sub":
        return openSubOf(e, true);
      case "close-sub":
        closeSub();
        return focusOn(e.item);
      case "close":
        return close(true);
      default:
        return focusOn(list[moveFocus(action, at, list.length)]);
    }
  });
  on(menu, "click", (ev: MouseEvent) => {
    const acc = accentAt(ev.target);
    if (acc) return chooseAccent(acc[0], acc[1]);
    const e = entryOf(ev.target);
    if (!e) return;
    // P-99: the selected palette opens its submenu; another is selected.
    if (e.sub && e.id === current().palette) {
      if (openSub === e) closeSub();
      else openSubOf(e, true);
      return;
    }
    choosePalette(e);
  });
  // Hover intent (mouse only; touch has no hover): resting on a palette opens
  // its submenu, or closes another's; passing over does nothing.
  on(menu, "pointerover", (ev: PointerEvent) => {
    if (ev.pointerType !== "mouse") return;
    clearTimeout(hover);
    if (accentAt(ev.target)) return;
    const e = entryOf(ev.target);
    if (!e || e === openSub) return;
    hover = setTimeout(() => (e.sub ? openSubOf(e, false) : closeSub()), HOVER_INTENT_MS);
  });
  on(menu, "pointerleave", () => clearTimeout(hover));
  on(doc, "pointerdown", (ev: PointerEvent) => {
    if (!menu.hidden && !box.contains(ev.target as Node)) close(false);
  });
  on(box, "focusout", (ev: FocusEvent) => {
    const to = ev.relatedTarget as Node | null;
    if (!menu.hidden && to !== null && !box.contains(to)) close(false);
  });
  on(win, "resize", place);
  on(win, "scroll", place);
  on(menu, "scroll", place);

  refresh();
  box.hidden = false;
  return {
    refresh,
    cleanup() {
      clearTimeout(hover);
      ac.abort();
    },
  };
}

// ---------------------------------------------------------------------------
// The explorer (ADR-119 L1a; P-39 to P-45): the KB's pages by tag, in a left
// column. The daemon ships an empty, hidden `<nav id="mae-explorer">` naming
// the list's URL (`data-src`) and its version (`data-version`), and only to a
// reader allowed the list; this behaviour fetches the list, builds it with
// createElement and textContent only (see the file header), and shows it. The
// router keeps the nav across navigations while its `data-src` holds, so the
// list's scroll position and the reader's open groups survive (P-45); a new
// version is fetched again, an unchanged one never. Anything unexpected
// leaves the nav hidden: no error is shown, as for a reader without the list.

/** The nav attributes that follow the page when the router keeps the node. */
export const EXPLORER_ATTRS = [
  "data-version",
  "aria-label",
  "data-untagged",
  "lang",
] as const;

/** One page in the list: its title as raw text, and its path on the site. */
export interface ExplorerPage {
  title: string;
  href: string;
}

/** The list as the daemon serves it, in display order (never re-sorted here). */
export interface ExplorerData {
  groups: Array<{ tag: string; pages: ExplorerPage[] }>;
  untagged: ExplorerPage[];
}

/**
 * Whether the router keeps the explorer on screen (`mine`) for the next page
 * (`theirs`): when both name the same list. Kept, it takes the next page's
 * version, label, untagged label and language; otherwise it is replaced.
 */
export function keepExplorer(mine: Element, theirs: Element): boolean {
  const src = theirs.getAttribute("data-src");
  if (src === null || mine.getAttribute("data-src") !== src) return false;
  for (const name of EXPLORER_ATTRS) {
    const v = theirs.getAttribute(name);
    if (v === null) mine.removeAttribute(name);
    else mine.setAttribute(name, v);
  }
  return true;
}

/** `href` as a URL of one of the site's pages on this origin; null otherwise. */
export function sitePage(href: string, here: URL): URL | null {
  let url: URL;
  try {
    url = new URL(href, here);
  } catch {
    return null;
  }
  return url.origin === here.origin && url.pathname.startsWith(SITE_PREFIX)
    ? url
    : null;
}

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

function pagesOf(v: unknown, here: URL): ExplorerPage[] | null {
  if (!Array.isArray(v)) return null;
  const out: ExplorerPage[] = [];
  for (const p of v) {
    if (
      !isRecord(p) ||
      typeof p.title !== "string" ||
      typeof p.href !== "string"
    )
      return null;
    if (sitePage(p.href, here) === null) return null;
    out.push({ title: p.title, href: p.href });
  }
  return out;
}

/**
 * The list, if `value` (parsed JSON) has the daemon's shape; null otherwise,
 * including when any link leaves the site: a list that is not entirely the
 * site's own pages is not shown at all (fail closed).
 */
export function parseExplorer(value: unknown, here: URL): ExplorerData | null {
  if (!isRecord(value) || !Array.isArray(value.groups)) return null;
  const untagged = pagesOf(value.untagged, here);
  if (untagged === null) return null;
  const groups: ExplorerData["groups"] = [];
  for (const g of value.groups) {
    if (!isRecord(g) || typeof g.tag !== "string") return null;
    const pages = pagesOf(g.pages, here);
    if (pages === null) return null;
    groups.push({ tag: g.tag, pages });
  }
  return { groups, untagged };
}

/** A vertical extent on screen, in CSS pixels. */
export interface Span {
  top: number;
  bottom: number;
}

/**
 * Sub-pixel slack in "fully visible": layout rounds fractional boxes, and an
 * entry flush with the list's edge must not count as hidden by a rounding
 * error (it would nudge the list on every navigation). Half a pixel can never
 * hide a visible part of a line.
 */
export const VISIBLE_SLACK = 0.5;

/**
 * The part of the list a reader can see: its scroll box (inside its borders),
 * cut to the window. A sticky list below the header reaches past the window's
 * bottom until the page scrolls; an entry there is not on screen.
 */
export function listView(
  nav: Span,
  clientTop: number,
  clientHeight: number,
  windowHeight: number,
): Span {
  const top = nav.top + clientTop;
  return {
    top: Math.max(top, 0),
    bottom: Math.min(top + clientHeight, windowHeight),
  };
}

/** Whether `entry` lies wholly inside `view`. */
export function fullyVisible(entry: Span, view: Span): boolean {
  return (
    entry.top >= view.top - VISIBLE_SLACK &&
    entry.bottom <= view.bottom + VISIBLE_SLACK
  );
}

/**
 * The list's `scrollTop` that shows `entry` (P-44, P-45): unchanged when it is
 * already fully visible; otherwise the least move that shows it, its top
 * first when it is taller than the view. Only the list scrolls, never the
 * window, so this is applied to the list's own `scrollTop`.
 */
export function revealScrollTop(
  entry: Span,
  view: Span,
  scrollTop: number,
): number {
  if (fullyVisible(entry, view)) return scrollTop;
  const fits = entry.bottom - entry.top <= view.bottom - view.top;
  const delta =
    entry.top < view.top || !fits
      ? entry.top - view.top
      : entry.bottom - view.bottom;
  return Math.max(0, scrollTop + delta);
}

/** One occurrence of the current page in the list (a page is listed under each tag). */
export interface Occurrence {
  open: boolean;
  visible: boolean;
}

/**
 * Which occurrence of the current page is marked (P-39): the one the reader
 * clicked; else one in an open group that the reader can see; else the first
 * (whose group is then opened). -1 when the page is not listed.
 */
export function chooseOccurrence(
  occurrences: readonly Occurrence[],
  clicked: number | null,
): number {
  if (occurrences.length === 0) return -1;
  if (clicked !== null && clicked >= 0 && clicked < occurrences.length)
    return clicked;
  const seen = occurrences.findIndex((o) => o.open && o.visible);
  return seen >= 0 ? seen : 0;
}

/** A built entry: its link, its group, and the group's key (null: untagged). */
interface ExplorerEntry {
  a: HTMLAnchorElement;
  group: HTMLDetailsElement;
  key: string | null;
}

/** The list as built into one nav, at one version. */
interface BuiltExplorer {
  nav: HTMLElement;
  version: string;
  groups: Map<string | null, HTMLDetailsElement>;
  entries: ExplorerEntry[];
  /** The untagged group's summary text, in the nav's words. */
  untagged: HTMLElement | null;
  /** Elements whose text is content (tags, titles), in the page's language. */
  content: HTMLElement[];
}

/** The groups, closed, in the given order; the untagged group last, if any. */
function buildExplorer(
  doc: Document,
  nav: HTMLElement,
  version: string,
  data: ExplorerData,
): BuiltExplorer {
  const built: BuiltExplorer = {
    nav,
    version,
    groups: new Map(),
    entries: [],
    untagged: null,
    content: [],
  };
  const group = (
    key: string | null,
    pages: ExplorerPage[],
  ): HTMLDetailsElement => {
    const details = doc.createElement("details");
    const summary = doc.createElement("summary");
    const label = doc.createElement("span");
    if (key === null) built.untagged = label;
    else {
      label.textContent = key;
      built.content.push(label);
    }
    summary.append(label);
    const list = doc.createElement("ul");
    for (const p of pages) {
      const li = doc.createElement("li");
      const a = doc.createElement("a");
      a.setAttribute("href", p.href);
      a.textContent = p.title;
      built.content.push(a);
      built.entries.push({ a, group: details, key });
      li.append(a);
      list.append(li);
    }
    details.append(summary, list);
    built.groups.set(key, details);
    return details;
  };
  const nodes = data.groups.map((g) => group(g.tag, g.pages));
  if (data.untagged.length > 0) nodes.push(group(null, data.untagged));
  nav.replaceChildren(...nodes);
  return built;
}

/**
 * The nav's words and languages: the untagged group's name from
 * `data-untagged`; and when the nav's words fell back to another language
 * (`lang` on the nav), the tags and titles carry the page's language back.
 */
function labelExplorer(built: BuiltExplorer, doc: Document): void {
  const nav = built.nav;
  if (built.untagged)
    built.untagged.textContent = nav.getAttribute("data-untagged") ?? "";
  const navLang = nav.getAttribute("lang");
  const pageLang = doc.documentElement.getAttribute("lang");
  const back =
    navLang !== null &&
    pageLang !== null &&
    pageLang !== "" &&
    navLang !== pageLang;
  for (const e of built.content) {
    if (back) e.setAttribute("lang", pageLang);
    else e.removeAttribute("lang");
  }
}

/** The list at `src`, if it is a 200 JSON answer of the daemon's shape; else null. */
async function fetchExplorer(
  win: Window & typeof globalThis,
  src: string,
  here: URL,
  signal: AbortSignal,
): Promise<ExplorerData | null> {
  const url = sitePage(src, here);
  if (url === null) return null;
  try {
    // The router's rules: GET, no custom headers, redirects (the login) not followed.
    const res = await win.fetch(url.href, {
      method: "GET",
      credentials: "same-origin",
      redirect: "manual",
      signal,
    });
    if (res.type === "opaqueredirect" || res.status !== 200) return null;
    const ct = (res.headers.get("content-type") ?? "").toLowerCase();
    if (!ct.startsWith("application/json")) return null;
    return parseExplorer(JSON.parse(await res.text()), here);
  } catch {
    return null;
  }
}

/** The explorer behaviour, and for tests the list fetch in flight. */
export interface ExplorerBehaviour extends Behaviour {
  idle(): Promise<void> | null;
}

/**
 * The explorer as a behaviour (P-156): on every page, mark the current page in
 * the list on screen, and fetch the list only when its version changed.
 */
export function explorer(win: Window & typeof globalThis): ExplorerBehaviour {
  let built: BuiltExplorer | null = null;
  // The occurrence the reader clicked, for the page that click navigates to.
  let clicked: { key: string | null; path: string } | null = null;
  let loading: Promise<void> | null = null;

  const rect = (e: Element): Span => {
    const r = e.getBoundingClientRect();
    return { top: r.top, bottom: r.bottom };
  };

  /** aria-current on exactly one occurrence of this page, shown in the list. */
  const mark = (b: BuiltExplorer, click: typeof clicked): void => {
    const here = win.location.href;
    const path = new URL(here).pathname;
    const current: ExplorerEntry[] = [];
    for (const e of b.entries) {
      e.a.removeAttribute("aria-current");
      if (new URL(e.a.getAttribute("href") ?? "", here).pathname === path)
        current.push(e);
    }
    const nav = b.nav;
    const view = (): Span =>
      listView(rect(nav), nav.clientTop, nav.clientHeight, win.innerHeight);
    const v = view();
    const at =
      click && click.path === path
        ? current.findIndex((e) => e.key === click.key)
        : -1;
    const chosen =
      current[
        chooseOccurrence(
          current.map((e) => ({
            open: e.group.hasAttribute("open"),
            visible: fullyVisible(rect(e.a), v),
          })),
          at >= 0 ? at : null,
        )
      ];
    if (!chosen) return;
    chosen.group.setAttribute("open", "");
    chosen.a.setAttribute("aria-current", "page");
    const top = revealScrollTop(rect(chosen.a), view(), nav.scrollTop);
    if (top !== nav.scrollTop) nav.scrollTop = top;
  };

  const load = async (
    doc: Document,
    nav: HTMLElement,
    src: string,
    version: string,
    signal: AbortSignal,
    click: typeof clicked,
  ): Promise<void> => {
    const data = await fetchExplorer(
      win,
      src,
      new URL(win.location.href),
      signal,
    );
    if (signal.aborted) return;
    const before = built?.nav === nav ? built : null;
    if (data === null) {
      // Fail closed: no list rather than a stale or partial one.
      nav.hidden = true;
      nav.replaceChildren();
      built = null;
      return;
    }
    const scrollTop = nav.scrollTop;
    const b = buildExplorer(doc, nav, version, data);
    // The reader's open groups survive a new version; on first render only
    // the current page's group opens (mark).
    if (before)
      for (const [key, g] of before.groups)
        if (g.hasAttribute("open")) b.groups.get(key)?.setAttribute("open", "");
    labelExplorer(b, doc);
    built = b;
    nav.hidden = false;
    nav.scrollTop = scrollTop;
    mark(b, click);
  };

  return {
    idle: () => loading,
    mount(doc) {
      loading = null;
      const click = clicked;
      clicked = null;
      const nav = doc.getElementById(EXPLORER_ID);
      const src = nav?.getAttribute("data-src");
      if (!nav || src === null || src === undefined) return;
      if (built !== null && built.nav !== nav) built = null;
      const ac = new AbortController();
      nav.addEventListener(
        "click",
        (ev) => {
          const a = (ev.target as Element | null)?.closest?.("a");
          const entry = built?.entries.find((e) => e.a === a);
          if (!entry) return;
          const link: LinkFacts = {
            href: entry.a.getAttribute("href"),
            target: null,
            download: false,
          };
          const url = routableClick(
            ev as MouseEvent,
            link,
            new URL(win.location.href),
          );
          if (url !== null) clicked = { key: entry.key, path: url.pathname };
        },
        { signal: ac.signal },
      );
      const version = nav.getAttribute("data-version") ?? "";
      if (built !== null) {
        labelExplorer(built, doc);
        mark(built, click);
      }
      if (built === null || built.version !== version) {
        loading = load(doc, nav, src, version, ac.signal, click);
      }
      return () => ac.abort();
    },
  };
}

// ---------------------------------------------------------------------------
// Copying a source block (ADR-119 D12; P-109). The daemon's article carries
// the control's words (`data-copy-*`, from the locale catalogue); this
// behaviour puts one button before each `pre.org-src`, built with
// createElement and textContent only (see the file header), so without
// JavaScript there is none. A click copies the block's code exactly as its
// text content holds it, consecutive blank lines included, through the async
// clipboard API; where there is no clipboard, or it refuses, the block's text
// is selected for the reader to copy instead. Either outcome is announced.

export const ARTICLE_ID = "mae-article";
/** The copy button's class (`mae-`: chrome, which content cannot wear). */
export const COPY_CLASS = "mae-copy";

/** The copy control's words, as the article carries them. */
export interface CopyWords {
  /** The button's text and accessible name. */
  label: string;
  /** Announced when the clipboard took the text. */
  copied: string;
  /** Announced when the text was selected instead. */
  selected: string;
  /** The catalogue's locale when it is a fallback (the button is marked). */
  lang: string | null;
}

/** The words on `article`; null when any is missing (then no control). */
export function copyWords(article: Element): CopyWords | null {
  const label = article.getAttribute("data-copy-label");
  const copied = article.getAttribute("data-copy-done");
  const selected = article.getAttribute("data-copy-selected");
  if (!label || !copied || !selected) return null;
  return { label, copied, selected, lang: article.getAttribute("data-copy-lang") };
}

/** Select everything inside `el`, for the reader to copy by hand. */
function selectContents(win: Window, doc: Document, el: Element): void {
  const selection = win.getSelection();
  if (selection === null) return;
  const range = doc.createRange();
  range.selectNodeContents(el);
  selection.removeAllRanges();
  selection.addRange(range);
}

/**
 * Copy `code`'s text: to the clipboard when there is one that accepts it,
 * otherwise by selecting it. Announces and returns which happened.
 */
export async function copyBlock(
  win: Window,
  doc: Document,
  code: Element,
  words: CopyWords,
): Promise<"copied" | "selected"> {
  const text = code.textContent ?? "";
  const clipboard = (win.navigator as Navigator | undefined)?.clipboard;
  if (clipboard && typeof clipboard.writeText === "function") {
    try {
      await clipboard.writeText(text);
      announce(doc, words.copied);
      return "copied";
    } catch {
      // Refused (no permission, document not focused): select instead.
    }
  }
  selectContents(win, doc, code);
  announce(doc, words.selected);
  return "selected";
}

/** The copy control as a behaviour (P-156): buttons on mount, none after. */
export function codeCopy(win: Window & typeof globalThis): Behaviour {
  return {
    mount(doc) {
      const article = doc.getElementById(ARTICLE_ID);
      const words = article ? copyWords(article) : null;
      if (!article || !words) return;
      const ac = new AbortController();
      const buttons: HTMLElement[] = [];
      for (const pre of Array.from(article.querySelectorAll("pre.org-src"))) {
        const code = Array.from(pre.children).find((c) => c.localName === "code");
        if (!code) continue;
        const button = doc.createElement("button");
        button.type = "button";
        button.className = COPY_CLASS;
        button.textContent = words.label;
        if (words.lang) button.setAttribute("lang", words.lang);
        button.addEventListener(
          "click",
          () => {
            void copyBlock(win, doc, code, words);
          },
          { signal: ac.signal },
        );
        pre.before(button);
        buttons.push(button);
      }
      return () => {
        ac.abort();
        for (const b of buttons) b.remove();
      };
    },
  };
}

// On one of the site's pages: start once, and publish the component registry
// (D18). A document without the main landmark is not the site's.
if (
  typeof window !== "undefined" &&
  typeof document !== "undefined" &&
  document.getElementById(MAIN_ID) !== null
) {
  const w = window as unknown as { mae?: { site?: Router } };
  if (!w.mae?.site) {
    const router = start(window);
    w.mae = { ...(w.mae ?? {}), site: router };
    router.register("theme", themeControls(window));
    router.register("explorer", explorer(window));
    router.register("copy", codeCopy(window));
  }
}
