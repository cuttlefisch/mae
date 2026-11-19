// SPDX-License-Identifier: GPL-3.0-or-later
// Finding a note in the expanded graph (owner, 2026-11-12): the reader types,
// the graph lists the notes that match, and choosing one opens that page with the
// graph on it. Pure: no DOM. The script (browser/find.ts) draws the
// box and the list.
//
// What matches, as a site's own search bar (FlexSearch with
// "forward" tokenizing): every word typed is the start of a word of the note's
// title, its path or its text ("acc" finds "acceso"; "de" no longer matches the
// middle of "modelo", which buried the right pages under noise: owner,
// 2026-11-12, "control de acceso"). Unlike that bar, case and accents never
// count ("ano" finds "Año"): readers type without them, and on a site written in
// a language with accents (Spanish, say) that is most of what they type.
//
// How they are ordered (lower rank first):
//   0  the title starts with what was typed
//   1  the title holds it as a phrase
//   2  the title has every word
//   3  the text holds it as a phrase
//   4  the text has every word
//   5  only the path has them (with the title)
// Within a rank: in the text, more mentions first; then the shorter title (closer
// to what was typed); then alphabetical.
//
// A phrase is the words typed, in order, whatever separates them, in what was
// typed or in the title or text: «0.1.0 page» holds the phrase 0 1 0 page.
// Looked for as the words joined by spaces, a title written with dots never held
// it, and «0.1.0 Page» ranked 2, level with «0.0.1 Page» (2026-11-16).

export interface Findable {
  /** The note's slug (contentIndex key): what the views know it by. */
  id: string;
  title: string;
  /** The page's text (contentIndex `content`), if the index has it. */
  text?: string;
}

export interface Found extends Findable {
  /** Why it was found: 0–5 as above. */
  rank: number;
  /** For a match in the text: a few words around the first mention. */
  snippet?: string;
}

/** Lower case, without accents or other marks, with spaces collapsed. */
export function fold(s: string): string {
  return s
    .normalize("NFD")
    .replace(/\p{M}+/gu, "")
    .toLocaleLowerCase("und")
    .replace(/\s+/g, " ")
    .trim();
}

/** The words of a folded text: runs of letters and digits. */
const words = (folded: string): string[] => folded.match(/[\p{L}\p{N}]+/gu) ?? [];

interface Prepared {
  note: Findable;
  title: string;
  titleWords: string[];
  pathWords: string[];
  text: string;
  textWords: string[];
}

// Folding and splitting the text of every page on each key was most of the cost:
// done once per list of notes.
const prepared = new WeakMap<readonly Findable[], Prepared[]>();
function prepare(notes: readonly Findable[]): Prepared[] {
  let p = prepared.get(notes);
  if (!p) {
    p = notes.map((note) => {
      const title = fold(note.title);
      const text = fold(note.text ?? "");
      return {
        note,
        title,
        titleWords: words(title),
        pathWords: words(fold(note.id.replace(/[/_-]+/g, " "))),
        text,
        textWords: words(text),
      };
    });
    prepared.set(notes, p);
  }
  return p;
}

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const startsAWord = (ws: string[], q: string) => ws.some((w) => w.startsWith(q));
/** Folded words as a phrase: in order, anything but letters and digits between. */
const phraseOf = (ws: string[]) => ws.map(escape).join("[^\\p{L}\\p{N}]+");
/** Where `phrase` starts at a word boundary of `s` (folded), or -1. */
function phraseAt(s: string, phrase: string): number {
  const m = new RegExp(`(^|[^\\p{L}\\p{N}])${phrase}`, "u").exec(s);
  return m ? m.index + m[1]!.length : -1;
}

/** A few words of the text around position `at` of its folded form. */
function snippetAt(original: string, at: number): string {
  // Without its marks, the text has one character for each of the folded one.
  const plain = original
    .normalize("NFD")
    .replace(/\p{M}+/gu, "")
    .replace(/\s+/g, " ")
    .trim();
  const from = at <= 40 ? 0 : plain.lastIndexOf(" ", at - 40) + 1;
  const end = plain.indexOf(" ", Math.min(plain.length, at + 70));
  const to = end < 0 ? plain.length : end;
  return `${from > 0 ? "… " : ""}${plain.slice(from, to).trim()}${to < plain.length ? " …" : ""}`;
}

/** At most `limit` notes that match `query`, best first. Nothing for an empty query. */
export function findNotes(notes: readonly Findable[], query: string, limit = 8): Found[] {
  const qs = words(fold(query));
  if (qs.length === 0 || limit <= 0) return [];
  const phrase = phraseOf(qs);
  const startsWithIt = new RegExp(`^${phrase}`, "u");
  const found: { f: Found; mentions: number; title: string }[] = [];
  for (const p of prepare(notes)) {
    let rank: number | null = null;
    let mentions = 0;
    let at = -1;
    if (qs.every((w) => startsAWord(p.titleWords, w))) {
      rank = startsWithIt.test(p.title) ? 0 : phraseAt(p.title, phrase) >= 0 ? 1 : 2;
    } else if (qs.every((w) => startsAWord(p.textWords, w))) {
      at = phraseAt(p.text, phrase);
      rank = at >= 0 ? 3 : 4;
      if (at < 0) at = phraseAt(p.text, phraseOf(qs.slice(0, 1)));
      for (const w of p.textWords) if (qs.some((x) => w.startsWith(x))) mentions++;
    } else if (qs.every((w) => startsAWord(p.pathWords, w) || startsAWord(p.titleWords, w))) {
      rank = 5;
    }
    if (rank === null) continue;
    const f: Found = { ...p.note, rank };
    if (at >= 0) f.snippet = snippetAt(p.note.text ?? "", at);
    found.push({ f, mentions, title: p.title });
  }
  found.sort(
    (a, b) =>
      a.f.rank - b.f.rank ||
      b.mentions - a.mentions ||
      a.title.length - b.title.length ||
      a.title.localeCompare(b.title) ||
      a.f.id.localeCompare(b.f.id),
  );
  return found.slice(0, limit).map((x) => x.f);
}
