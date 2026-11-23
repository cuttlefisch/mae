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
/**
 * The URL the router should fetch for a click on a link, or `null` when the
 * browser should handle it: a modified or non-primary click, a new window, a
 * download, another origin, a path outside the site, or a jump within the
 * current page.
 */
export function routableClick(click, link, here) {
    if (click.defaultPrevented || click.button !== 0)
        return null;
    if (click.metaKey || click.ctrlKey || click.shiftKey || click.altKey)
        return null;
    if (link.href === null || link.download)
        return null;
    if (link.target !== null && link.target !== "" && link.target !== "_self")
        return null;
    let url;
    try {
        url = new URL(link.href, here);
    }
    catch {
        return null;
    }
    if (url.origin !== here.origin || !url.pathname.startsWith(SITE_PREFIX))
        return null;
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
export function fragmentId(hash) {
    const raw = hash.startsWith("#") ? hash.slice(1) : hash;
    try {
        return decodeURIComponent(raw);
    }
    catch {
        return raw;
    }
}
/**
 * `swap` only for a plain 200 HTML page. A redirect (the login, on an expired
 * session: `opaqueredirect` under `redirect: "manual"`), an error status or
 * any other body is handed to the browser as a full navigation, which then
 * does whatever that answer means.
 */
export function outcome(r) {
    if (r.type === "opaqueredirect" || r.status !== 200)
        return "full";
    const ct = (r.contentType ?? "").toLowerCase();
    return ct.startsWith("text/html") ? "swap" : "full";
}
/** The stylesheets and scripts a document loads, in order. */
export function assetsOf(doc) {
    const out = [];
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
export function swappable(current, next) {
    if (next.getElementById(MAIN_ID) === null)
        return false;
    const a = assetsOf(current);
    const b = assetsOf(next);
    return a.length === b.length && a.every((x, i) => x === b[i]);
}
/** Install the router on `win`: click and history handling, components. */
export function start(win) {
    const doc = win.document;
    const tt = win.trustedTypes;
    // The only HTML sink on the page; see the file header.
    const policy = tt?.createPolicy("mae-router", { createHTML: (s) => s });
    const parse = (html) => new win.DOMParser().parseFromString((policy ? policy.createHTML(html) : html), "text/html");
    const behaviours = new Map();
    let cleanups = [];
    const mountAll = () => {
        for (const b of behaviours.values()) {
            const c = b.mount(doc);
            if (typeof c === "function")
                cleanups.push(c);
        }
    };
    const unmountAll = () => {
        for (const c of cleanups)
            c();
        cleanups = [];
    };
    win.history.scrollRestoration = "manual";
    // The page this document shows, by path and query: a history move within it
    // (a fragment) is the browser's, not a navigation.
    const pageOf = (u) => u.pathname + u.search;
    let shown = pageOf(new URL(win.location.href));
    let inFlight = null;
    let current = null;
    const announce = (text) => {
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
    const swapIn = (next) => {
        unmountAll();
        doc.title = next.title;
        doc.documentElement.lang = next.documentElement.lang;
        const main = doc.getElementById(MAIN_ID);
        const nextMain = next.getElementById(MAIN_ID);
        if (main && nextMain)
            main.replaceWith(doc.adoptNode(nextMain));
        // The site's header before the main landmark and the side panels after
        // it: each replaced, removed or added to match the next page.
        const region = (id, place) => {
            const mine = doc.getElementById(id);
            const theirs = next.getElementById(id);
            if (mine && theirs)
                mine.replaceWith(doc.adoptNode(theirs));
            else if (mine)
                mine.remove();
            else if (theirs)
                doc.getElementById(MAIN_ID)?.[place](doc.adoptNode(theirs));
        };
        region(HEADER_ID, "before");
        region(PANELS_ID, "after");
        mountAll();
    };
    const settle = (url, scrollY) => {
        const target = url.hash ? doc.getElementById(fragmentId(url.hash)) : null;
        if (scrollY !== null)
            win.scrollTo(0, scrollY);
        else if (target)
            target.scrollIntoView();
        else
            win.scrollTo(0, 0);
        const focus = target ?? doc.querySelector(`#${MAIN_ID} h1`) ?? doc.getElementById(MAIN_ID);
        if (focus !== null && focus instanceof win.HTMLElement) {
            if (!focus.hasAttribute("tabindex"))
                focus.setAttribute("tabindex", "-1");
            focus.focus({ preventScroll: true });
        }
        announce(doc.title);
    };
    const run = async (url, push, scrollY) => {
        inFlight?.abort();
        const ctl = new AbortController();
        inFlight = ctl;
        let res;
        try {
            res = await win.fetch(url.href, {
                method: "GET",
                credentials: "same-origin",
                redirect: "manual",
                signal: ctl.signal,
            });
        }
        catch {
            if (ctl.signal.aborted)
                return; // a later navigation won
            win.location.assign(url.href);
            return;
        }
        if (ctl.signal.aborted)
            return;
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
        if (ctl.signal.aborted)
            return;
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
        if (inFlight === ctl)
            inFlight = null;
    };
    const navigate = (url, push, scrollY = null) => {
        current = run(url, push, scrollY);
        return current;
    };
    doc.addEventListener("click", (ev) => {
        const anchor = ev.target?.closest?.("a");
        if (!anchor)
            return;
        const link = {
            href: anchor.getAttribute("href") ?? anchor.getAttribute("xlink:href"),
            target: anchor.getAttribute("target"),
            download: anchor.hasAttribute("download"),
        };
        const url = routableClick(ev, link, new URL(win.location.href));
        if (url === null)
            return;
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
        const state = ev.state;
        void navigate(url, false, state?.scrollY ?? null);
    });
    return {
        register(name, b) {
            behaviours.set(name, b);
            const c = b.mount(doc);
            if (typeof c === "function")
                cleanups.push(c);
        },
        navigate: (url, push) => navigate(url, push),
        pending: () => current,
    };
}
/** The storage key of each part of a choice (MAE's own names). */
export function storageKey(part, palette) {
    return part === "accent" ? "mae.accent." + palette : "mae." + part;
}
/** A stored string; null when absent, or when storage itself throws (P-97). */
export function readStored(storage, key) {
    try {
        const v = storage().getItem(key);
        return typeof v === "string" ? v : null;
    }
    catch {
        return null;
    }
}
/**
 * The accent `stored` of `palette` if it is one of that palette's and not its
 * default; null otherwise (the default, P-96). Checked with Object.hasOwn: a
 * stored string is never used as a key before it is known to be one.
 */
export function validAccent(data, palette, stored) {
    if (stored === null || !Object.hasOwn(data.palettes, palette))
        return null;
    const accents = data.palettes[palette]?.accents ?? null;
    if (accents === null || !Object.hasOwn(accents.items, stored))
        return null;
    return stored === accents.default ? null : stored;
}
/**
 * The stored choice, validated against the published lists: an unknown mode
 * follows the system, an unknown palette is the default, an accent not valid
 * for the palette is its default, and a removed palette becomes its successor
 * without an accent (P-98).
 */
export function resolveChoice(data, read) {
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
export function systemDark(win) {
    try {
        return win.matchMedia("(prefers-color-scheme: dark)").matches;
    }
    catch {
        return false;
    }
}
/** Writes a choice on <html> (P-92): base.css sets color-scheme from data-mode. */
export function applyChoice(root, choice, mode) {
    root.setAttribute("data-mode", mode);
    root.setAttribute("data-theme", choice.palette);
    if (choice.accent === null)
        root.removeAttribute("data-emphasis");
    else
        root.setAttribute("data-emphasis", choice.accent);
}
/**
 * The pre-paint script: apply the stored choice before first paint (P-91,
 * P-93, P-96), and publish the offer for the controls (`mae.theme`).
 */
export function prepaint(win, data) {
    const choice = resolveChoice(data, (k) => readStored(() => win.localStorage, k));
    const mode = choice.mode ?? (systemDark(win) ? "dark" : "light");
    applyChoice(win.document.documentElement, choice, mode);
    const w = win;
    w.mae = { ...(w.mae ?? {}), theme: data };
}
/**
 * Storage for the controls: what this page wrote wins, so a choice holds for
 * the session when storage throws (P-97); every write also tries storage.
 */
export function sessionStore(storage) {
    const written = new Map();
    return {
        get(key) {
            if (written.has(key))
                return written.get(key) ?? null;
            return readStored(storage, key);
        },
        set(key, value) {
            written.set(key, value);
            try {
                if (value === null)
                    storage().removeItem(key);
                else
                    storage().setItem(key, value);
            }
            catch {
                // Kept for the session only.
            }
        },
    };
}
/** The action of `key` on a palette (`inSubmenu` false) or on an accent. */
export function menuAction(key, inSubmenu) {
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
export function moveFocus(action, current, count) {
    if (count <= 0)
        return -1;
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
/** The gap kept between a menu and the window's edge, in CSS pixels. */
export const EDGE = 8;
const clamp = (v, lo, hi) => Math.max(lo, Math.min(v, hi));
/**
 * The palette menu under its button, its right edge on the button's, inside
 * the window. Above the button only when there is more room there; either way
 * it scrolls inside itself when the list is longer than the room (P-99).
 */
export function placeMenu(button, width, height, vw, vh) {
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
export function placeSubmenu(item, menu, width, height, vw, vh) {
    const w = Math.min(width, vw - 2 * EDGE);
    let left;
    if (menu.right + w <= vw - EDGE)
        left = menu.right;
    else if (menu.left - w >= EDGE)
        left = menu.left - w;
    else
        left = clamp(menu.left, EDGE, Math.max(EDGE, vw - EDGE - w));
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
function el(doc, tag, attrs) {
    const e = doc.createElement(tag);
    for (const [k, v] of Object.entries(attrs))
        e.setAttribute(k, v);
    return e;
}
function swatchEl(doc) {
    return el(doc, "span", { class: "mae-swatch" });
}
/** A menuitemradio: a check mark and swatches (both hidden from AT), then its name. */
function radioItem(doc, attrs, name, swatches) {
    const item = el(doc, "div", { role: "menuitemradio", tabindex: "-1", "aria-checked": "false", ...attrs });
    item.className = "mae-menu-item";
    const deco = el(doc, "span", { "aria-hidden": "true", class: "mae-menu-deco" });
    deco.append(el(doc, "span", { class: "mae-check" }));
    const swatchEls = [];
    for (let i = 0; i < swatches; i++)
        swatchEls.push(swatchEl(doc));
    deco.append(...swatchEls);
    const label = el(doc, "span", { class: "mae-menu-label" });
    label.textContent = name;
    item.append(deco, label);
    return { item, swatchEls };
}
function buildEntry(doc, id, p, labels) {
    const wrap = el(doc, "div", { role: "none" });
    const subId = ACCENT_MENU_PREFIX + id;
    const attrs = { "data-palette": id };
    if (p.accents !== null) {
        Object.assign(attrs, { "aria-haspopup": "menu", "aria-expanded": "false", "aria-controls": subId });
    }
    const { item, swatchEls } = radioItem(doc, attrs, p.name, p.swatches.light.length);
    if (p.accents !== null)
        item.append(el(doc, "span", { "aria-hidden": "true", class: "mae-more" }));
    wrap.append(item);
    const entry = { id, item, sub: null, accents: [], swatches: swatchEls };
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
export function buildMenu(doc, data, labels) {
    const menu = el(doc, "div", { role: "menu", id: PALETTE_MENU_ID, "aria-label": labels.menu, class: "mae-menu" });
    menu.hidden = true;
    const entries = [];
    for (const [id, p] of Object.entries(data.palettes)) {
        const { wrap, entry } = buildEntry(doc, id, p, labels);
        menu.append(wrap);
        entries.push(entry);
    }
    return { menu, entries };
}
/** The labels: the button's own name, and the words the daemon put on the box. */
function labelsOf(box, button) {
    return {
        menu: (button.textContent ?? "").trim(),
        defaultAccent: box.getAttribute("data-default-label") ?? "",
        accent: (id) => box.getAttribute("data-accent-" + id) ?? id,
    };
}
function setBox(e, p) {
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
export function themeControls(win) {
    const store = sessionStore(() => win.localStorage);
    const root = win.document.documentElement;
    let refresh = null;
    const chosenMode = () => {
        const m = store.get(storageKey("mode"));
        return m === "light" || m === "dark";
    };
    // Follow the system, live, until the reader chooses (P-91).
    try {
        win.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", (ev) => {
            if (chosenMode())
                return;
            root.setAttribute("data-mode", ev.matches ? "dark" : "light");
            refresh?.();
        });
    }
    catch {
        // No media queries: the mode stays as painted.
    }
    return {
        mount(doc) {
            const data = win.mae?.theme;
            const box = doc.getElementById(THEME_ID);
            const toggle = doc.getElementById(MODE_TOGGLE_ID);
            const button = doc.getElementById(PALETTE_BUTTON_ID);
            if (!data || !box || !toggle || !button)
                return;
            const ctl = mountControls(win, doc, data, store, { box, toggle, button });
            refresh = ctl.refresh;
            return () => {
                ctl.cleanup();
                refresh = null;
            };
        },
    };
}
/** The state of one page's controls, and what changes it. */
function mountControls(win, doc, data, store, { box, toggle, button }) {
    const root = doc.documentElement;
    doc.getElementById(PALETTE_MENU_ID)?.remove();
    const { menu, entries } = buildMenu(doc, data, labelsOf(box, button));
    button.after(menu);
    const ac = new AbortController();
    // Every listener goes with the page (cleanup aborts them all).
    const on = (t, type, f) => t.addEventListener(type, f, { signal: ac.signal });
    let openSub = null;
    let hover;
    const current = () => {
        const t = root.getAttribute("data-theme");
        const palette = t !== null && Object.hasOwn(data.palettes, t) ? t : data.default;
        return { mode: null, palette, accent: validAccent(data, palette, root.getAttribute("data-emphasis")) };
    };
    const mode = () => (root.getAttribute("data-mode") === "dark" ? "dark" : "light");
    // The accent checked in a palette's submenu: the current one for the active
    // palette, the remembered one for the others.
    const accentOf = (id) => {
        const c = current();
        return c.palette === id ? c.accent : validAccent(data, id, store.get(storageKey("accent", id)));
    };
    const refresh = () => {
        const c = current();
        const m = mode();
        toggle.setAttribute("aria-pressed", m === "dark" ? "true" : "false");
        for (const e of entries) {
            const p = data.palettes[e.id];
            e.item.setAttribute("aria-checked", e.id === c.palette ? "true" : "false");
            const checked = accentOf(e.id) ?? p.accents?.default;
            const colours = [...p.swatches[m], ...Object.values(p.accents?.items ?? {}).map((a) => a[m])];
            e.swatches.forEach((s, i) => (s.style.backgroundColor = colours[i] ?? ""));
            for (const a of e.accents) {
                a.setAttribute("aria-checked", a.getAttribute("data-accent") === checked ? "true" : "false");
            }
        }
    };
    const viewport = () => [doc.documentElement.clientWidth || win.innerWidth, win.innerHeight];
    const place = () => {
        if (menu.hidden)
            return;
        const [vw, vh] = viewport();
        setBox(menu, placeMenu(button.getBoundingClientRect(), menu.offsetWidth, menu.scrollHeight, vw, vh));
        if (openSub?.sub) {
            const s = openSub.sub;
            const at = placeSubmenu(openSub.item.getBoundingClientRect(), menu.getBoundingClientRect(), s.offsetWidth, s.scrollHeight, vw, vh);
            setBox(s, at);
        }
    };
    const focusOn = (e) => {
        if (!e)
            return;
        e.focus();
        e.scrollIntoView?.({ block: "nearest" });
    };
    const closeSub = () => {
        if (openSub?.sub) {
            openSub.sub.hidden = true;
            openSub.item.setAttribute("aria-expanded", "false");
        }
        openSub = null;
    };
    const openSubOf = (e, focus) => {
        if (!e.sub)
            return;
        if (openSub !== e)
            closeSub();
        openSub = e;
        e.sub.hidden = false;
        e.item.setAttribute("aria-expanded", "true");
        place();
        if (focus)
            focusOn(e.accents.find((a) => a.getAttribute("aria-checked") === "true") ?? e.accents[0]);
    };
    const open = () => {
        refresh();
        menu.hidden = false;
        button.setAttribute("aria-expanded", "true");
        place();
        focusOn(entries.find((e) => e.id === current().palette)?.item ?? entries[0]?.item);
    };
    const close = (refocus) => {
        clearTimeout(hover);
        closeSub();
        menu.hidden = true;
        button.setAttribute("aria-expanded", "false");
        if (refocus)
            button.focus();
    };
    const choose = (palette, accent) => {
        const p = data.palettes[palette];
        store.set(storageKey("palette"), palette === data.default ? null : palette);
        if (p.accents !== null)
            store.set(storageKey("accent", palette), accent);
        applyChoice(root, { mode: null, palette, accent }, mode());
        refresh();
        close(true);
    };
    const choosePalette = (e) => choose(e.id, accentOf(e.id));
    const chooseAccent = (e, a) => choose(e.id, validAccent(data, e.id, a.getAttribute("data-accent")));
    const entryOf = (t) => {
        const item = t?.closest?.("[data-palette]");
        return entries.find((e) => e.item === item);
    };
    const accentAt = (t) => {
        const a = t?.closest?.("[data-accent]");
        const e = entries.find((x) => x.accents.some((y) => y === a));
        return e && a ? [e, a] : undefined;
    };
    on(toggle, "click", () => {
        const next = mode() === "dark" ? "light" : "dark";
        store.set(storageKey("mode"), next);
        root.setAttribute("data-mode", next);
        refresh();
    });
    on(button, "click", () => (menu.hidden ? open() : close(false)));
    on(button, "keydown", (ev) => {
        if (ev.key !== "ArrowDown" && ev.key !== "ArrowUp")
            return;
        ev.preventDefault();
        if (menu.hidden)
            open();
    });
    on(menu, "keydown", (ev) => {
        const acc = accentAt(ev.target);
        const action = menuAction(ev.key, acc !== undefined);
        if (action === null)
            return;
        if (action === "leave")
            return close(false);
        ev.preventDefault();
        const e = acc?.[0] ?? entryOf(ev.target);
        if (!e)
            return;
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
    on(menu, "click", (ev) => {
        const acc = accentAt(ev.target);
        if (acc)
            return chooseAccent(acc[0], acc[1]);
        const e = entryOf(ev.target);
        if (!e)
            return;
        // P-99: the selected palette opens its submenu; another is selected.
        if (e.sub && e.id === current().palette) {
            if (openSub === e)
                closeSub();
            else
                openSubOf(e, true);
            return;
        }
        choosePalette(e);
    });
    // Hover intent (mouse only; touch has no hover): resting on a palette opens
    // its submenu, or closes another's; passing over does nothing.
    on(menu, "pointerover", (ev) => {
        if (ev.pointerType !== "mouse")
            return;
        clearTimeout(hover);
        if (accentAt(ev.target))
            return;
        const e = entryOf(ev.target);
        if (!e || e === openSub)
            return;
        hover = setTimeout(() => (e.sub ? openSubOf(e, false) : closeSub()), HOVER_INTENT_MS);
    });
    on(menu, "pointerleave", () => clearTimeout(hover));
    on(doc, "pointerdown", (ev) => {
        if (!menu.hidden && !box.contains(ev.target))
            close(false);
    });
    on(box, "focusout", (ev) => {
        const to = ev.relatedTarget;
        if (!menu.hidden && to !== null && !box.contains(to))
            close(false);
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
// On one of the site's pages: start once, and publish the component registry
// (D18). A document without the main landmark is not the site's.
if (typeof window !== "undefined" &&
    typeof document !== "undefined" &&
    document.getElementById(MAIN_ID) !== null) {
    const w = window;
    if (!w.mae?.site) {
        const router = start(window);
        w.mae = { ...(w.mae ?? {}), site: router };
        router.register("theme", themeControls(window));
    }
}
