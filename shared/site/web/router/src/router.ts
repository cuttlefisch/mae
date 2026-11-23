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

  const announce = (text: string): void => {
    let live = doc.getElementById(ANNOUNCER_ID);
    if (live === null) {
      live = doc.createElement("div");
      live.id = ANNOUNCER_ID;
      live.setAttribute("aria-live", "polite");
      live.className = "mae-visually-hidden";
      doc.body.append(live);
    }
    live.textContent = text;
  };

  const swapIn = (next: Document): void => {
    unmountAll();
    doc.title = next.title;
    doc.documentElement.lang = next.documentElement.lang;
    const main = doc.getElementById(MAIN_ID);
    const nextMain = next.getElementById(MAIN_ID);
    if (main && nextMain) main.replaceWith(doc.adoptNode(nextMain));
    // The site's header before the main landmark and the side panels after
    // it: each replaced, removed or added to match the next page.
    const region = (id: string, place: "before" | "after"): void => {
      const mine = doc.getElementById(id);
      const theirs = next.getElementById(id);
      if (mine && theirs) mine.replaceWith(doc.adoptNode(theirs));
      else if (mine) mine.remove();
      else if (theirs) doc.getElementById(MAIN_ID)?.[place](doc.adoptNode(theirs));
    };
    region(HEADER_ID, "before");
    region(PANELS_ID, "after");
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
    announce(doc.title);
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
  }
}
