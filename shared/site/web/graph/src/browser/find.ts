// SPDX-License-Identifier: GPL-3.0-or-later
// Finding a note in the expanded graph (owner, 2026-11-12). A combobox with a
// list of suggestions (WAI-ARIA APG, "list autocomplete"): typing lists the notes
// that match (src/search.ts), the arrows move through them, Enter or a click
// chooses one, Escape closes the list and then clears the box (never the dialog
// while there is something to close). The chosen note goes to the view as a
// `chord-graph-find` event with its slug, and an emptied box as one with null.
// Choosing only shows where the note is (owner, 2026-11-17): the view centres
// it, marks it and its links, and gives it the focus (hier.ts, chord.ts);
// its page opens only from the note itself, a click on it or Enter on it, as any
// note's does.
import { findNotes, type Findable, type Found } from "../search";
import { onCleanup } from "./runtime";

export const FIND_EVENT = "chord-graph-find";

let ids = 0;

export function mountFind(
  box: HTMLElement,
  view: HTMLElement,
  notes: readonly Findable[],
  labels: { count: string; one: string },
  outer: AbortSignal,
): void {
  const input = box.querySelector<HTMLInputElement>(".chord-graph-find-input");
  const list = box.querySelector<HTMLUListElement>(".chord-graph-find-list");
  const status = box.querySelector<HTMLElement>(".chord-graph-find-status");
  if (!input || !list || !status) return;
  // Stops with the caller's draw and with the engine's cleanup at a page change.
  const ac = new AbortController();
  if (outer.aborted) return;
  outer.addEventListener("abort", () => ac.abort(), { once: true });
  onCleanup(box, () => ac.abort());
  const signal = ac.signal;
  const n = ++ids;
  list.id ||= `chord-graph-find-list-${n}`;
  input.setAttribute("aria-controls", list.id);

  let shown: Findable[] = [];
  let active = -1;

  const optionId = (i: number) => `${list.id}-${i}`;
  const setActive = (i: number) => {
    active = i;
    for (const [k, li] of [...list.children].entries()) {
      li.setAttribute("aria-selected", String(k === i));
      if (k === i) li.scrollIntoView?.({ block: "nearest" });
    }
    if (i >= 0) input.setAttribute("aria-activedescendant", optionId(i));
    else input.removeAttribute("aria-activedescendant");
  };
  const close = () => {
    list.hidden = true;
    input.setAttribute("aria-expanded", "false");
    setActive(-1);
  };
  const show = () => {
    shown = findNotes(notes, input.value);
    list.replaceChildren(
      ...shown.map((note, i) => {
        const li = document.createElement("li");
        li.id = optionId(i);
        li.setAttribute("role", "option");
        li.setAttribute("aria-selected", "false");
        li.dataset.slug = note.id;
        const title = document.createElement("span");
        title.className = "chord-graph-find-title";
        title.textContent = note.title;
        const path = document.createElement("span");
        path.className = "chord-graph-find-path";
        path.textContent = note.id;
        li.append(title, path);
        // Found in its text: a few words around the mention, so the reader sees why.
        if ((note as Found).snippet) {
          const s = document.createElement("span");
          s.className = "chord-graph-find-snippet";
          s.textContent = (note as Found).snippet!;
          li.append(s);
        }
        return li;
      }),
    );
    const typed = input.value.trim() !== "";
    status.textContent = !typed
      ? ""
      : shown.length === 0
        ? (box.dataset.labelNone ?? "")
        : shown.length === 1
          ? labels.one
          : labels.count.replace("{n}", String(shown.length));
    if (shown.length === 0) {
      close();
      return;
    }
    list.hidden = false;
    input.setAttribute("aria-expanded", "true");
    setActive(-1);
  };
  // The box emptied: nothing found any more.
  const clear = () => view.dispatchEvent(new CustomEvent(FIND_EVENT, { detail: null }));
  const choose = (i: number) => {
    const note = shown[i];
    if (!note) return;
    input.value = note.title;
    close();
    status.textContent = "";
    view.dispatchEvent(new CustomEvent(FIND_EVENT, { detail: note.id }));
  };

  input.addEventListener(
    "input",
    () => {
      show();
      if (input.value.trim() === "") clear();
    },
    { signal },
  );
  input.addEventListener(
    "keydown",
    (ev) => {
      const open = !list.hidden && shown.length > 0;
      if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
        if (!open) {
          if (input.value.trim() !== "") show();
          ev.preventDefault();
          return;
        }
        const d = ev.key === "ArrowDown" ? 1 : -1;
        setActive((active + d + shown.length + (active < 0 && d < 0 ? 1 : 0)) % shown.length);
        ev.preventDefault();
      } else if (ev.key === "Enter") {
        if (open) {
          choose(active >= 0 ? active : 0);
          ev.preventDefault();
        }
      } else if (ev.key === "Escape") {
        // First the list, then the text; only an empty, closed box lets Escape
        // close the dialog.
        if (open) close();
        else if (input.value !== "") {
          input.value = "";
          status.textContent = "";
          clear();
        } else return;
        ev.preventDefault();
        ev.stopPropagation();
      }
    },
    { signal },
  );
  // A press on an option keeps the focus in the box (the click then chooses).
  list.addEventListener("pointerdown", (ev) => ev.preventDefault(), { signal });
  list.addEventListener(
    "click",
    (ev) => {
      const li = (ev.target as Element | null)?.closest?.("li");
      if (li) choose([...list.children].indexOf(li));
    },
    { signal },
  );
  input.addEventListener("blur", close, { signal });
}
