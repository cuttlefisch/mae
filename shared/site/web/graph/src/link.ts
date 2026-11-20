// SPDX-License-Identifier: GPL-3.0-or-later
// From a link on the page to the graph's note (owner, 2026-11-17: hovering an
// internal link of the body highlights its note in the graph and the link to it). By
// the same rule that gives each note of the graph its slug (geometry.ts, normaliseSlug):
// the link's path, without the site's base, without «/index» or the trailing slash.
import { normaliseSlug } from "./geometry";

/**
 * The slug of the note `href` leads to, read from the page `page` (an absolute
 * URL), on a site served under `base` ("" or "/something"). null if it leads off the
 * site: to another origin or outside the base.
 */
export function slugOfHref(href: string, page: string, base = ""): string | null {
  let url: URL;
  try {
    url = new URL(href, page);
  } catch {
    return null;
  }
  if (url.origin !== new URL(page).origin) return null;
  const root = base.replace(/\/+$/, "");
  let path = url.pathname;
  if (root !== "") {
    if (path !== root && !path.startsWith(`${root}/`)) return null;
    path = path.slice(root.length);
  }
  try {
    path = decodeURIComponent(path);
  } catch {
    // A stray % is not an escape: the path as it came.
  }
  return normaliseSlug(path.replace(/\.html$/, ""));
}
