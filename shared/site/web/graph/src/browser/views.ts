// SPDX-License-Identifier: GPL-3.0-or-later
// What each drawn view (the sidebar box or the expanded view) offers to code that
// did not draw it: chord.ts changes it without drawing it again. Drawing again
// puts the camera where the first framing leaves it, and the reader saw the graph
// leave the screen and come back (owner, 2026-11-17, the «You are here» switch).
import { onCleanup } from "./runtime";

export interface ViewHandle {
  /**
   * Shows or hides «You are here» (here.ts): the marker is removed or placed again,
   * and nothing else; neither the camera nor the drawing is touched.
   */
  setHere(shown: boolean): void;
  /**
   * Highlights the note `slug` and the current page's link to it (a body link under
   * the pointer); null removes it. A note the view does not draw, or the current
   * page, highlights nothing.
   */
  peek(slug: string | null): void;
}

const handles = new Map<HTMLElement, ViewHandle>();

/** The view `view` answers with `handle` until its drawing ends (`signal`). */
export function register(view: HTMLElement, handle: ViewHandle, signal: AbortSignal): void {
  if (signal.aborted) return;
  handles.set(view, handle);
  const drop = () => {
    if (handles.get(view) === handle) handles.delete(view);
  };
  signal.addEventListener("abort", drop);
  // And at the engine's next page change or the end of the mount, whichever comes first.
  onCleanup(view, drop);
}

/** To every drawn view: the sidebar one and the expanded one at once. */
export function peekAll(slug: string | null): void {
  for (const h of handles.values()) h.peek(slug);
}

/** The handle of `view`, if it is drawn (if not, the caller draws it). */
export function handleOf(view: HTMLElement): ViewHandle | undefined {
  return handles.get(view);
}
