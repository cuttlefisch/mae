// SPDX-License-Identifier: GPL-3.0-or-later
// «You are here» over the graph: a ring on the current note, a pin stuck into
// it and a label with the page's title, in a free space of the frame
// (../here.ts decides where). Hidden (`shown: false`), it draws nothing. It is drawn in an svg of its own,
// over the drawing and in screen px, so it is the same in the three views, in the
// sidebar and in the dialog, and does not scale with the zoom. What is taken is measured from what
// is drawn at that moment (wedges, links, names, controls): the
// label never covers a name nor sits under a control. Text only through
// textContent.
import { GAP, HALO, PIN_H, makeGrid, placePin, type Placement } from "../here";
import { truncate } from "../here";
import { overlayBoxes } from "./controls";
import { onCleanup } from "./runtime";

const SVG_NS = "http://www.w3.org/2000/svg";
const CELL = 3; // px per cell of the occupancy grid
const PAD_X = 8;
const PAD_Y = 5;
const LINE1_PX = 11; // «You are here»
const LINE2_PX = 12; // the title
const LINE_GAP = 2;
const GLYPH_W = 9; // the small pin before «You are here»
/** The title's maximum width: in the sidebar, and in the dialog. */
export const TITLE_MAX = { sidebar: 140, dialog: 240 };

function el<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string> = {}) {
  const e = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  return e;
}

let measurer: CanvasRenderingContext2D | null | undefined;
function textPx(text: string, font: string): number {
  if (measurer === undefined) measurer = document.createElement("canvas").getContext("2d");
  if (!measurer) return text.length * 7;
  measurer.font = font;
  return measurer.measureText(text).width;
}

// The Path2D of what is drawn, per element and its `d` (a lifted wedge changes its d).
const paths = new WeakMap<Element, [string, Path2D]>();
function pathOf(e: Element): Path2D | null {
  const d = e.getAttribute("d");
  if (!d) return null;
  const known = paths.get(e);
  if (known && known[0] === d) return known[1];
  const p = new Path2D(d);
  paths.set(e, [d, p]);
  return p;
}

/** A circle in drawing units (the svg): what is inside a ring counts as taken. */
export type Disc = [number, number, number];

/**
 * The frame's occupancy grid: what is drawn in `svg` (wedges and arcs filled,
 * links and leaders 3 px thick, names and dots by their box), the discs
 * (the inside of the rings, where the links run) and the controls.
 */
function occupancy(svg: SVGSVGElement, w: number, h: number, discs: Disc[]) {
  const cols = Math.max(1, Math.ceil(w / CELL));
  const rows = Math.max(1, Math.ceil(h / CELL));
  const canvas = document.createElement("canvas");
  canvas.width = cols;
  canvas.height = rows;
  // A canvas that is read back (getImageData), drawn on the CPU: on the GPU, reading
  // it waited for everything to be drawn there and copied back (2026-11-19, headless
  // Chromium, 2,600 notes and 10,000 links: 35 ms on the rings, 676 ms on the
  // balloons, every time a zoom stopped).
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) return makeGrid(w, h, CELL, new Uint8Array(cols * rows));
  const s = svg.getBoundingClientRect();
  const put = (m: DOMMatrix | null) => {
    if (!m) return 0;
    ctx.setTransform(
      m.a / CELL,
      m.b / CELL,
      m.c / CELL,
      m.d / CELL,
      (m.e - s.left) / CELL,
      (m.f - s.top) / CELL,
    );
    return Math.hypot(m.a, m.b);
  };
  ctx.fillStyle = "#000";
  ctx.strokeStyle = "#000";
  const root = svg.getScreenCTM();
  const scale = put(root);
  for (const [x, y, r] of discs) {
    ctx.beginPath();
    ctx.arc(x, y, r, 0, Math.PI * 2);
    ctx.fill();
  }
  for (const e of svg.querySelectorAll(".chord-graph-shape, .chord-graph-group-arc")) {
    const p = pathOf(e);
    if (!p) continue;
    put((e as SVGGraphicsElement).getScreenCTM());
    ctx.fill(p);
  }
  put(root);
  ctx.lineWidth = scale > 0 ? 3 / scale : 1;
  for (const e of svg.querySelectorAll(
    ".chord-graph-edge, .chord-graph-tree, .chord-graph-leader:not(.hover)",
  )) {
    const p = pathOf(e);
    if (p) ctx.stroke(p);
  }
  ctx.setTransform(1 / CELL, 0, 0, 1 / CELL, 0, 0);
  const rects: DOMRect[] = [];
  for (const e of svg.querySelectorAll(
    ".chord-graph-label, .chord-graph-group-label, .chord-graph-callout:not(.hover), .chord-graph-dot:not(.hover)",
  ))
    rects.push(e.getBoundingClientRect());
  for (const r of rects) ctx.fillRect(r.left - s.left, r.top - s.top, r.width, r.height);
  for (const b of overlayBoxes(svg)) ctx.fillRect(b.x, b.y, b.w, b.h);
  const data = ctx.getImageData(0, 0, cols, rows).data;
  const occ = new Uint8Array(cols * rows);
  for (let i = 0; i < occ.length; i++) occ[i] = data[i * 4 + 3]! > 24 ? 1 : 0;
  return makeGrid(w, h, CELL, occ);
}

export interface Here {
  /** The label's width and height, in px (to reserve room for it when framing: reserve). */
  size: [number, number];
  /**
   * Places the marker: `anchor` is the current note's outer edge, in drawing
   * units; `ring`, in those units, the radius of what is marked when it is a
   * whole ring (a folder's page in the balloons); `discs`, what is taken
   * that is not a stroke.
   */
  update(anchor: [number, number], discs: Disc[], ring?: number): void;
  /**
   * While the camera moves: the marker follows the note, without measuring again.
   * `anchor` is in px of the `w` x `h` frame (whoever moves the camera knows where
   * it lands: viewport.ts, onFrame), read from nothing in the document, which has
   * just changed.
   */
  follow(anchor: [number, number], w: number, h: number): void;
  /** No current note drawn: nothing. */
  clear(): void;
  /**
   * The reader shows or hides it (the expanded view's switch). Hidden, it is
   * removed; shown, the caller places it (update). Nothing else changes.
   */
  setShown(shown: boolean): void;
}

export interface HereArgs {
  svg: SVGSVGElement;
  /** Drawn or not (hereMarker "none", or the reader has hidden it in the dialog). */
  shown: boolean;
  title: string;
  /** «You are here», in the site's language. */
  label: string;
  large: boolean;
  signal: AbortSignal;
}

export function mountHere(a: HereArgs): Here {
  const { svg } = a;
  const frame = svg.parentElement!;
  let layer = frame.querySelector<SVGSVGElement>(":scope > .chord-graph-marker");
  if (!layer) {
    layer = el("svg", { class: "chord-graph-marker", "aria-hidden": "true" });
    svg.after(layer);
  }
  const overlay = layer;
  let shown = a.shown;
  overlay.replaceChildren();
  overlay.dataset.shown = String(shown);
  delete overlay.dataset.where;
  delete overlay.dataset.box;
  // A redraw, or the engine's cleanup at a page change: the marker goes with its drawing.
  a.signal.addEventListener("abort", () => overlay.replaceChildren());
  onCleanup(svg, () => overlay.replaceChildren());

  const family = getComputedStyle(svg).fontFamily || "system-ui";
  const font1 = `700 ${LINE1_PX}px ${family}`;
  const font2 = `600 ${LINE2_PX}px ${family}`;
  const title = truncate(a.title, a.large ? TITLE_MAX.dialog : TITLE_MAX.sidebar, (t) =>
    textPx(t, font2),
  );
  const w1 = GLYPH_W + 4 + textPx(a.label, font1);
  const size: [number, number] = [
    Math.ceil(Math.max(w1, textPx(title, font2)) + 2 * PAD_X),
    Math.ceil(2 * PAD_Y + LINE1_PX + LINE_GAP + LINE2_PX + 2),
  ];

  const project = ([x, y]: [number, number]): [number, number] => {
    const m = svg.getScreenCTM();
    const s = svg.getBoundingClientRect();
    if (!m) return [x, y];
    return [m.a * x + m.c * y + m.e - s.left, m.b * x + m.d * y + m.f - s.top];
  };
  const scale = () => {
    const m = svg.getScreenCTM();
    return m ? Math.hypot(m.a, m.b) : 1;
  };

  let last: { p: Placement; anchor: [number, number]; halo: number; off: boolean } | null = null;

  const draw = (p: Placement, anchor: [number, number], halo: number, off: boolean) => {
    overlay.replaceChildren();
    const f = (n: number) => n.toFixed(1);
    const g = el("g", { class: "chord-graph-marker-mark" });
    // The ring, with a page-coloured border under it: it shows on any
    // fill and over the links. Outside the frame there is no note to ring.
    if (!off) {
      g.append(
        el("circle", {
          class: "chord-graph-marker-ring-backdrop",
          cx: f(anchor[0]),
          cy: f(anchor[1]),
          r: f(halo),
        }),
        el("circle", {
          class: "chord-graph-marker-ring",
          cx: f(anchor[0]),
          cy: f(anchor[1]),
          r: f(halo),
        }),
      );
    }
    if (p.leader.length > 1) {
      const d = p.leader.map(([x, y], i) => `${i ? "L" : "M"}${f(x)},${f(y)}`).join("");
      g.append(
        el("path", { class: "chord-graph-marker-leader-backdrop", d }),
        el("path", { class: "chord-graph-marker-leader", d }),
      );
    }
    if (!off) {
      // The pin: its point on the note, its head PIN_H px away (downward if the
      // label goes below).
      const s = p.pin === "down" ? -1 : 1;
      const [x, y] = anchor;
      const r = 6.5;
      const cy = y - s * (PIN_H - r);
      g.append(
        el("path", {
          class: "chord-graph-marker-pin",
          d:
            `M${f(x)},${f(y)}` +
            `C${f(x - 1.5)},${f(y - s * 7)} ${f(x - r)},${f(cy + s * 6)} ${f(x - r)},${f(cy)}` +
            `A${r},${r} 0 1,${s > 0 ? 1 : 0} ${f(x + r)},${f(cy)}` +
            `C${f(x + r)},${f(cy + s * 6)} ${f(x + 1.5)},${f(y - s * 7)} ${f(x)},${f(y)}Z`,
        }),
        el("circle", { class: "chord-graph-marker-pin-eye", cx: f(x), cy: f(cy), r: "2.4" }),
      );
    }
    // The label: «You are here» with a small pin before it, and the title.
    const b = p.box;
    g.append(
      el("rect", {
        class: "chord-graph-marker-box",
        x: f(b.x),
        y: f(b.y),
        width: f(b.w),
        height: f(b.h),
        rx: "6",
      }),
    );
    const gx = b.x + PAD_X + GLYPH_W / 2;
    const gy = b.y + PAD_Y + 1;
    g.append(
      el("path", {
        class: "chord-graph-marker-glyph",
        d: `M${f(gx)},${f(gy + 11)}L${f(gx - 4)},${f(gy + 5)}A4.5,4.5 0 1,1 ${f(gx + 4)},${f(gy + 5)}Z`,
      }),
    );
    const t1 = el("text", {
      class: "chord-graph-marker-text",
      x: f(b.x + PAD_X + GLYPH_W + 4),
      y: f(b.y + PAD_Y),
      "dominant-baseline": "hanging",
    });
    t1.textContent = a.label;
    const t2 = el("text", {
      class: "chord-graph-marker-title",
      x: f(b.x + PAD_X),
      y: f(b.y + PAD_Y + LINE1_PX + LINE_GAP + 1),
      "dominant-baseline": "hanging",
    });
    t2.textContent = title;
    g.append(t1, t2);
    overlay.append(g);
    overlay.dataset.where = p.where;
    overlay.dataset.box = [b.x, b.y, b.w, b.h].map((n) => Math.round(n)).join(",");
  };

  // A note outside the frame (the reader has moved away from it): the label goes to
  // the nearest edge and the leader points toward it.
  const clamp = ([x, y]: [number, number], w: number, h: number): [[number, number], boolean] => {
    const off = x < 0 || y < 0 || x > w || y > h;
    return [[Math.min(w - 2, Math.max(2, x)), Math.min(h - 2, Math.max(2, y))], off];
  };

  return {
    size,
    update(anchorU, discs, ring) {
      if (!shown) return;
      const s = svg.getBoundingClientRect();
      if (s.width <= 0 || s.height <= 0) return;
      const [anchor, off] = clamp(project(anchorU), s.width, s.height);
      const halo = ring !== undefined ? Math.max(HALO, ring * scale() + 4) : HALO;
      const grid = occupancy(svg, s.width, s.height, discs);
      const p = placePin({
        grid,
        anchor,
        size,
        halo: off ? GAP : halo,
        controls: overlayBoxes(svg),
      });
      last = { p, anchor, halo, off };
      draw(p, anchor, halo, off);
    },
    follow(at, w, h) {
      if (!last) return;
      const [anchor, off] = clamp(at, w, h);
      const dx = anchor[0] - last.anchor[0];
      const dy = anchor[1] - last.anchor[1];
      const p = last.p;
      // The label and its leader go with the note.
      const box = { ...p.box, x: p.box.x + dx, y: p.box.y + dy };
      const leader = p.leader.map(([x, y]) => [x + dx, y + dy] as [number, number]);
      draw({ ...p, box, leader }, anchor, last.halo, off);
    },
    clear() {
      last = null;
      overlay.replaceChildren();
      delete overlay.dataset.where;
      delete overlay.dataset.box;
    },
    setShown(s) {
      shown = s;
      overlay.dataset.shown = String(s);
      if (!s) this.clear();
    },
  };
}
