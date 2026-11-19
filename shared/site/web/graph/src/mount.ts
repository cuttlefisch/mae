// SPDX-License-Identifier: GPL-3.0-or-later
// The engine-owned interface: mounts the chord graph in an element the engine
// gives, draws it for the current page, and redraws it on every page change the
// engine reports (update). Nothing here reads page-wide globals: the data, the
// site's base path and the way a page opens in place all come in as arguments.
// The element tree is built with createElement, setAttribute and textContent
// only, never innerHTML, so no label or title can inject markup.
import { i18n } from "./i18n";
import { defaultOptions, type ChordGraphOptions, type GraphData } from "./types";
import { cssVariables, resolveColors } from "./themes";
import { resolveLayouts } from "./layouts";
import { resolveHere } from "./here";
import { normaliseSlug } from "./geometry";
import { NAMES_MODES } from "./browser/names";
import {
  attach,
  closeDialogs,
  detach,
  markVisited,
  render,
  unwatchBodyLinks,
  watchBodyLinks,
} from "./browser/chord";
import { ROOT_CLASS, runCleanups } from "./browser/runtime";

export type { GraphData, GraphEntry } from "./types";

export interface MountOptions extends ChordGraphOptions {
  /** The current page's slug. */
  current: string;
  /** The language of the graph's labels ("en-US", "es-ES"; default "en-US"). */
  locale?: string;
  /** "" (the default) or "/something": the path the site is served under. */
  basePath?: string;
  /**
   * Opens a page in place. When absent, a click on a note is not intercepted:
   * the browser follows the link.
   */
  navigate?: (url: URL) => void;
  /**
   * A CSS selector for the page body's internal links: hovering one highlights its
   * note in the graph. When absent, no listener is installed for them.
   */
  bodyLinks?: string;
}

export interface ChordGraphHandle {
  /**
   * The page changed to `current`: runs the pending cleanups, closes the expanded
   * view, marks the page visited (if asked) and draws the graph again.
   */
  update(current: string): void;
  /** Runs the cleanups, closes the expanded view and removes everything the mount added. */
  dispose(): void;
}

const SVG_NS = "http://www.w3.org/2000/svg";

/**
 * Mounts the chord graph inside `container`: builds its elements, then draws it
 * for `options.current` once `data` is there. Throws if `options` names a removed
 * or unknown «You are here» setting (here.ts, resolveHere).
 */
export function mountChordGraph(
  container: HTMLElement,
  data: GraphData | Promise<GraphData>,
  options: MountOptions,
): ChordGraphHandle {
  const { current, locale, basePath = "", navigate, bodyLinks, ...graphOptions } = options;
  const opts = { ...defaultOptions, ...graphOptions };
  const doc = container.ownerDocument;
  const t = i18n(locale ?? "en-US").components.chordGraph;
  // The theme's colours, as CSS custom properties on the root (light and dark);
  // the stylesheet picks the mode from the page's own toggle.
  const themeStyle = cssVariables(resolveColors(opts.theme, opts.colors));
  // Which views the in-page switch offers (validated: a typo falls back).
  const { offered, orientations } = resolveLayouts(opts);
  // «You are here»: the pin or nothing, and the reader's switch. Checked here, when
  // the graph is mounted: a removed or unknown value throws an error that says
  // what to write instead (callout and radial are gone).
  const here = resolveHere(opts);

  const el = <K extends keyof HTMLElementTagNameMap>(
    tag: K,
    attrs: Record<string, string> = {},
    ...children: Array<Node | string>
  ): HTMLElementTagNameMap[K] => {
    const e = doc.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
    for (const c of children) e.append(typeof c === "string" ? doc.createTextNode(c) : c);
    return e;
  };
  // A button whose name is spoken (the hidden text) and whose glyph is seen.
  const iconButton = (
    cls: string,
    glyph: string,
    label: string,
    attrs: Record<string, string> = {},
  ) =>
    el(
      "button",
      { type: "button", class: cls, title: label, ...attrs },
      el("span", { "aria-hidden": "true" }, glyph),
      el("span", { class: "chord-graph-sr" }, label),
    );
  const svg = () => {
    const s = doc.createElementNS(SVG_NS, "svg");
    s.setAttribute("class", "chord-graph-svg");
    s.setAttribute("role", "group");
    s.setAttribute("aria-label", t.title);
    return s;
  };
  // Zoom controls (hierarchical views only; shown by the script), and the
  // sunburst's flip (only when the site offers it; shown by the script in the
  // sunburst alone). A toggle: pressed = top level outside.
  const zoom = () =>
    el(
      "div",
      { class: "chord-graph-zoom", hidden: "" },
      iconButton("chord-graph-zoom-in", "+", t.zoomIn),
      iconButton("chord-graph-zoom-out", "−", t.zoomOut),
      iconButton("chord-graph-fit", "◎", t.fit),
      ...(orientations.length > 1
        ? [iconButton("chord-graph-flip", "⇅", t.flip, { "aria-pressed": "false", hidden: "" })]
        : []),
    );
  // The switch between views (only when the site offers more than one).
  const layouts = (): Node[] =>
    offered.length > 1
      ? [
          el(
            "div",
            { class: "chord-graph-layouts", role: "group", "aria-label": t.layout },
            ...offered.map((l) =>
              el(
                "button",
                { type: "button", "data-layout": l, "aria-pressed": "false" },
                t.layouts[l],
              ),
            ),
          ),
        ]
      : [];
  // Which pages the expanded view names (browser/names.ts): a segmented control
  // like the views', in the dialog only (the sidebar box names the current page).
  const names = () =>
    el(
      "div",
      { class: "chord-graph-names", role: "group", "aria-label": t.names },
      ...NAMES_MODES.map((m) =>
        el("button", { type: "button", "data-names": m, "aria-pressed": "false" }, t.namesModes[m]),
      ),
    );
  // Show or hide «You are here» (here.ts): a switch, as in the WAI-ARIA APG
  // «Switch» pattern (a button with role="switch" and aria-checked: on = shown),
  // beside «Names», in the dialog only; only with hereToggle (owner,
  // 2026-11-17: a pressed button did not read as something that toggles). Its
  // name is its label, «You are here»; the track, the thumb and the words on and
  // off are what the eye reads, hidden from assistive technology, which hears
  // the role and the state instead. The state shows by more than colour: the
  // thumb moves and the word changes.
  const hereSwitch = (): Node[] =>
    here.toggle
      ? [
          el(
            "div",
            { class: "chord-graph-here" },
            el(
              "button",
              {
                type: "button",
                role: "switch",
                "aria-checked": "true",
                class: "chord-graph-switch",
              },
              el("span", { class: "chord-graph-switch-label" }, t.here),
              el(
                "span",
                { class: "chord-graph-switch-track", "aria-hidden": "true" },
                el("span", { class: "chord-graph-switch-thumb" }),
              ),
              el("span", { class: "chord-graph-switch-on", "aria-hidden": "true" }, t.on),
              el("span", { class: "chord-graph-switch-off", "aria-hidden": "true" }, t.off),
            ),
          ),
        ]
      : [];

  // Two views share one drawing routine: the sidebar box, and a native
  // <dialog> opened with showModal(). The dialog renders in the browser's top
  // layer, above any page layout, makes the page behind it inert, closes on
  // Escape, and returns focus to the button that opened it. It takes the whole
  // page, on an opaque background (browser/chord.scss).
  // The browser script reads data-cfg and needs neither the theme nor the raw
  // «You are here» settings: the colours are in the style, validated, and the
  // marker's settings are `here`, checked. Raw options stay out of the page.
  const cfg = JSON.stringify({
    ...opts,
    theme: undefined,
    colors: undefined,
    hereMarker: undefined,
    hereToggle: undefined,
    here,
  });
  const root = el(
    "div",
    {
      class: ROOT_CLASS,
      style: themeStyle,
      "data-cfg": cfg,
      "data-label-empty": t.empty,
      "data-label-root": t.root,
      "data-label-count": t.count,
      "data-label-one": t.one,
      "data-label-here": t.here,
    },
    el("h3", {}, t.title),
    el(
      "div",
      { class: "chord-graph-view" },
      el(
        "div",
        { class: "chord-graph-frame" },
        svg(),
        zoom(),
        iconButton("chord-graph-expand", "⤢", t.expand, { "aria-haspopup": "dialog" }),
      ),
      ...layouts(),
      el("p", { class: "chord-graph-caption", "aria-live": "polite" }),
    ),
    el(
      "dialog",
      { class: "chord-graph-dialog", "aria-label": t.title },
      el(
        "div",
        { class: "chord-graph-view" },
        el(
          "div",
          { class: "chord-graph-frame" },
          svg(),
          // Finding a note: a combobox (WAI-ARIA APG, list autocomplete), in the
          // expanded view only (browser/find.ts).
          el(
            "div",
            { class: "chord-graph-find", "data-label-none": t.findNone },
            el("input", {
              type: "search",
              class: "chord-graph-find-input",
              role: "combobox",
              "aria-autocomplete": "list",
              "aria-expanded": "false",
              "aria-label": t.find,
              placeholder: t.find,
              autocomplete: "off",
              spellcheck: "false",
            }),
            el("ul", {
              class: "chord-graph-find-list",
              role: "listbox",
              "aria-label": t.find,
              hidden: "",
            }),
            el("p", { class: "chord-graph-sr chord-graph-find-status", "aria-live": "polite" }),
          ),
          zoom(),
          iconButton("chord-graph-close", "✕", t.close),
        ),
        el("div", { class: "chord-graph-controls" }, ...layouts(), names(), ...hereSwitch()),
        el("p", { class: "chord-graph-caption", "aria-live": "polite" }),
      ),
    ),
    el("p", { class: "chord-graph-sr chord-graph-hint" }, t.hint),
    el("p", { class: "chord-graph-sr chord-graph-hint-hier" }, t.hintHier),
  );
  container.append(root);

  attach(root, { data: Promise.resolve(data), basePath, navigate });
  if (bodyLinks !== undefined) watchBodyLinks(root, bodyLinks, basePath);

  let disposed = false;
  const draw = (page: string) => {
    const slug = normaliseSlug(page);
    if (opts.markVisited) markVisited(slug);
    void render(root, slug);
  };
  draw(current);

  return {
    update(page: string) {
      if (disposed) return;
      runCleanups(root);
      closeDialogs(root);
      draw(page);
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      runCleanups(root);
      closeDialogs(root);
      detach(root);
      unwatchBodyLinks(root);
      root.remove();
    },
  };
}
