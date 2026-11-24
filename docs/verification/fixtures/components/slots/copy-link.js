// P-162: copy a link to the page, and to each addressable section heading.
//
// Progressive enhancement (ADR-119 A2): the template writes a plain link to
// the page, which works and can be copied without JavaScript. This behaviour
// keeps that link's address, hides the link and puts a copy button in its
// place, and adds one button per addressable section heading. It registers
// with the router (D18: `mae.site.register({id, init(ctx) -> cleanup})`),
// runs once per navigation and is cleaned up before the next (P-156). It
// builds DOM and never writes HTML, as the router's Trusted Types policy
// requires.
//
// The router is loaded before this script (module scripts run in document
// order), so `mae.site` exists when it runs; without the router (no
// JavaScript site at all) the link stays a link.

const COPY_LABEL = "Copy link";
const STATUS_COPIED = "Link copied.";
const STATUS_MANUAL = "Copy this link: ";
const SECTION_LABEL = "Copy a link to this section";

async function copy(win, url, status) {
  try {
    if (!win.navigator.clipboard) throw new Error("no clipboard");
    await win.navigator.clipboard.writeText(url);
    status.textContent = STATUS_COPIED;
  } catch {
    // No clipboard (an insecure origin, or permission refused): show the
    // address as text the reader can select.
    status.textContent = STATUS_MANUAL + url;
  }
}

function button(doc, className, text, label) {
  const b = doc.createElement("button");
  b.type = "button";
  b.className = className;
  b.textContent = text;
  if (label) b.setAttribute("aria-label", label);
  return b;
}

export function init(ctx) {
  const doc = ctx.root;
  const win = doc.defaultView;
  const control = doc.querySelector(".site-copy-link");
  const anchor = control?.querySelector("a.site-copy-link-anchor[href]");
  const status = control?.querySelector(".site-copy-link-status");
  if (!win || !anchor || !status) return undefined;
  const page = new URL(anchor.getAttribute("href"), win.location.href).href;
  const main = button(doc, "site-copy-link-button", COPY_LABEL);
  main.addEventListener("click", () => copy(win, page, status), { signal: ctx.signal });
  anchor.hidden = true;
  anchor.after(main);

  const added = [main];
  for (const h of doc.querySelectorAll("h2[id], h3[id], h4[id], h5[id], h6[id]")) {
    if (h.id.startsWith("mae-")) continue; // the chrome's, not a section
    const b = button(doc, "site-copy-link-section", "#", SECTION_LABEL);
    const at = page + "#" + encodeURIComponent(h.id);
    b.addEventListener("click", () => copy(win, at, status), { signal: ctx.signal });
    h.append(b);
    added.push(b);
  }
  return () => {
    for (const b of added) b.remove();
    anchor.hidden = false;
    status.textContent = "";
  };
}

globalThis.mae?.site?.register({ id: "copy-link", init });
