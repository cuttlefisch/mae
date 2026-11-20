// SPDX-License-Identifier: GPL-3.0-or-later
// Which pages the expanded graph names (owner, 2026-11-13: the expanded view drew
// every label that fit and crowded the drawing). Level-of-detail labelling behind a
// display setting, as in other graph tools: the current page only (the default, as
// the sidebar box), or all: the neighbours' names and every label that fits. The page under
// the pointer or with the focus is always named. The reader's choice is kept in
// this browser, like the view.

export const NAMES_KEY = "chord-graph-names";
export const NAMES_MODES = ["current", "all"] as const;
export type NamesMode = (typeof NAMES_MODES)[number];

/**
 * The stored choice if it is one, else the default: the current page only. A third
 * option, «with neighbours», drew the same names as «all» at the scale the view opens
 * at (owner, 2026-11-13: redundant); a browser that kept it gets «all».
 */
export function chooseNames(stored: string | null): NamesMode {
  if (stored === "neighbours") return "all";
  return (NAMES_MODES as readonly string[]).includes(stored ?? "")
    ? (stored as NamesMode)
    : "current";
}
