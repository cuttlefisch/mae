// SPDX-License-Identifier: GPL-3.0-or-later
// The order in which links are painted (owner, 2026-11-17): in an SVG what comes
// later is painted on top. The current page's links, on top of all the others;
// while a note is hovered, its own on top of those; when the pointer leaves, the
// current page's again. They are moved to the end of their layer, redrawing nothing.

/**
 * Puts the elements of each group, in order, at the end of `layer` (on top): each
 * group over the earlier ones. Whatever is not in `layer` (culled, out of the
 * view) is left as it is.
 */
export function raise(layer: Element, groups: Iterable<Element>[]): void {
  for (const g of groups) for (const e of g) if (e.parentNode === layer) layer.appendChild(e);
}
