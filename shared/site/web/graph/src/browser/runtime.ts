// SPDX-License-Identifier: GPL-3.0-or-later
// What the engine owns of a mounted graph's lifetime: the cleanups that a page
// change (ChordGraphHandle.update) or the end of the mount (dispose) runs. Each
// mount has its own list, keyed by its root element (the `.chord-graph` element
// that mountChordGraph builds), so that one mount's page change never stops what
// another mount drew.

export const ROOT_CLASS = "chord-graph";

const pending = new WeakMap<Element, Array<() => void>>();

/** The root a cleanup belongs to: the mounted graph that holds `owner`. */
function rootOf(owner: Element): Element {
  return owner.closest(`.${ROOT_CLASS}`) ?? owner;
}

/**
 * Runs `fn` at the next page change or at the end of the mount that holds `owner`,
 * whichever comes first. Once: a drained cleanup is forgotten.
 */
export function onCleanup(owner: Element, fn: () => void): void {
  const root = rootOf(owner);
  const list = pending.get(root);
  if (list) list.push(fn);
  else pending.set(root, [fn]);
}

/** Runs and forgets every cleanup registered under `root`, in registration order. */
export function runCleanups(root: Element): void {
  const list = pending.get(root);
  pending.delete(root);
  for (const fn of list ?? []) fn();
}
