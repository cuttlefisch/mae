// SPDX-License-Identifier: GPL-3.0-or-later
// What the controls floating over the drawing cover (zoom, fit, expand, close,
// find), so that framing leaves nothing under them (owner, 2026-11-13: in the
// sidebar, after «fit», part of the balloons sat behind the + and − buttons).
// Measured at the moment of framing, not a constant: the sidebar and the dialog
// have different controls, the font size enlarges them, and in the chord view the
// zoom is hidden.
import type { Box } from "../viewport";

/**
 * The boxes, in px relative to the svg, of everything the graph's frame puts over
 * the drawing: every child of the frame that is not the svg itself nor the «You are here» marker, whatever control it is
 * (a new one counts without touching this). Whatever is hidden measures nothing and does not count. The search's
 * suggestion list sits inside its box, but is closed before
 * framing (find.ts, choose).
 */
export function overlayBoxes(svg: SVGSVGElement): Box[] {
  const frame = svg.parentElement;
  if (!frame) return [];
  const s = svg.getBoundingClientRect();
  const out: Box[] = [];
  for (const el of frame.children) {
    // The «You are here» marker (here.ts) sits over the drawing but is not a control:
    // it covers the whole frame and lets the pointer through.
    if (el === svg || el.classList.contains("chord-graph-marker")) continue;
    const b = el.getBoundingClientRect();
    if (b.width <= 0 || b.height <= 0) continue;
    out.push({ x: b.left - s.left, y: b.top - s.top, w: b.width, h: b.height });
  }
  return out;
}
