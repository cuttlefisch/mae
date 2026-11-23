// SPDX-License-Identifier: GPL-3.0-or-later
// Test helper: the normalised accessibility tree of the component fixtures
// (docs/verification/fixtures/components/). A deliberately small model, the same
// on both sides of the contract:
// - a node is an element with a role: its `role` attribute, else the implicit role
//   of <header> outside main/article (banner), <button> (button), <a href> (link);
// - role none/presentation and elements without a role pass their children up;
// - hidden (the `hidden` attribute) and aria-hidden="true" subtrees are absent;
// - name: aria-label, else, for a role named from content (button, link and the
//   menu items), the text of the subtree without its hidden parts, whitespace
//   collapsed and trimmed; omitted when empty;
// - states, when present: pressed, expanded, haspopup, checked (aria-*, "true" and
//   "false" as booleans), and focused (the document's active element);
// - children in document order, omitted when none.

export interface A11yNode {
  role: string;
  name?: string;
  pressed?: boolean;
  expanded?: boolean;
  haspopup?: string;
  checked?: boolean;
  focused?: boolean;
  children?: A11yNode[];
}

/** The roles this model names from their content (WAI-ARIA "name from content"). */
const NAME_FROM_CONTENT = new Set(["button", "link", "menuitem", "menuitemradio", "menuitemcheckbox"]);

const hiddenEl =(e: Element): boolean => e.hasAttribute("hidden") || e.getAttribute("aria-hidden") === "true";

function roleOf(e: Element): string | null {
  const explicit = e.getAttribute("role");
  if (explicit) return explicit === "presentation" ? "none" : explicit;
  const tag = e.tagName.toLowerCase();
  if (tag === "button") return "button";
  if (tag === "a" && e.hasAttribute("href")) return "link";
  if (tag === "header" && !e.closest("main, article")) return "banner";
  return null;
}

function textOf(n: Node): string {
  if (n.nodeType === 3) return n.textContent ?? "";
  if (n.nodeType !== 1 || hiddenEl(n as Element)) return "";
  return Array.from(n.childNodes, textOf).join("");
}

const bool = (v: string | null): boolean | undefined => (v === "true" ? true : v === "false" ? false : undefined);

function nodesOf(e: Element, doc: Document): A11yNode[] {
  if (hiddenEl(e)) return [];
  const kids = Array.from(e.children).flatMap((c) => nodesOf(c, doc));
  const role = roleOf(e);
  if (role === null || role === "none") return kids;
  const node: A11yNode = { role };
  const fromContent = NAME_FROM_CONTENT.has(role) ? textOf(e) : "";
  const name = (e.getAttribute("aria-label") ?? fromContent).replace(/\s+/g, " ").trim();
  if (name !== "") node.name = name;
  const states: Array<[keyof A11yNode, string]> = [
    ["pressed", "aria-pressed"],
    ["expanded", "aria-expanded"],
    ["checked", "aria-checked"],
  ];
  for (const [k, attr] of states) {
    const v = bool(e.getAttribute(attr));
    if (v !== undefined) (node as unknown as Record<string, boolean>)[k] = v;
  }
  const popup = e.getAttribute("aria-haspopup");
  if (popup !== null && popup !== "false") node.haspopup = popup;
  if (doc.activeElement === e) node.focused = true;
  if (kids.length > 0) node.children = kids;
  return [node];
}

/** The normalised tree of the first element with `role` (explicit or implicit). */
export function treeOf(doc: Document, role: string): A11yNode {
  const all = Array.from(doc.querySelectorAll("*"));
  const el = all.find((e) => roleOf(e) === role);
  if (!el) throw new Error(`no ${role}`);
  const [node] = nodesOf(el, doc);
  if (!node) throw new Error(`${role} is hidden`);
  return node;
}
