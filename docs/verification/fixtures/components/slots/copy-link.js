// P-162: copy a link to the page, and to each addressable section heading.
//
// A behaviour in the router's shape (shared/site/web/router: `Behaviour`,
// `mount(root) -> cleanup`): mounted once per navigation and cleaned up
// before the next (P-156). It builds DOM and never writes HTML, as the
// router's Trusted Types policy requires.
//
// The page's address comes from the button the template wrote
// (data-site-href: base path + /<kb-id>/<id>); this adds the origin. A
// section's address adds the heading's id, which is mae-org's anchor.

export const name = "copy-link";

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

export function mount(doc) {
  const win = doc.defaultView;
  const control = doc.querySelector(".site-copy-link");
  const button = control?.querySelector("button[data-site-href]");
  const status = control?.querySelector(".site-copy-link-status");
  if (!win || !button || !status) return undefined;
  const ac = new AbortController();
  const page = win.location.origin + button.dataset.siteHref;
  button.addEventListener("click", () => copy(win, page, status), { signal: ac.signal });

  const added = [];
  for (const h of doc.querySelectorAll("h2[id], h3[id], h4[id], h5[id], h6[id]")) {
    if (h.id.startsWith("mae-")) continue; // the chrome's, not a section
    const b = doc.createElement("button");
    b.type = "button";
    b.className = "site-copy-link-section";
    b.setAttribute("aria-label", SECTION_LABEL);
    b.textContent = "#";
    b.addEventListener("click", () => copy(win, page + "#" + encodeURIComponent(h.id), status), {
      signal: ac.signal,
    });
    h.append(b);
    added.push(b);
  }
  return () => {
    ac.abort();
    for (const b of added) b.remove();
    status.textContent = "";
  };
}

export default { mount };
