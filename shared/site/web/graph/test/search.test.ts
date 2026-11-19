// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import fc from "fast-check";
import { findNotes, fold, type Findable } from "../src/search";
import { params } from "./arbitraries";

const notes: Findable[] = [
  { id: "diseno/ano-de-diseno", title: "Año de diseño" },
  { id: "diseno/index", title: "Diseño" },
  { id: "ops/copias", title: "Copias de seguridad", text: "Cada noche, una copia cifrada." },
  {
    id: "ops/plano",
    title: "Plano de control: quién despliega la plataforma",
    text: "El acceso al plano de control pasa por la VPN.",
  },
  {
    id: "seguridad/accesos",
    title: "Accesos y permisos",
    text: "El control de acceso se hace por grupos. Control de acceso por rol; control de acceso al registro.",
  },
  { id: "mapas/modelo", title: "Modelo de datos", text: "Un modelo de datos sin control." },
  { id: "notas/largo", title: "Una página larga" },
];
const ids = (q: string, limit?: number) => findNotes(notes, q, limit).map((n) => n.id);

describe("finding a note", () => {
  it("fold: no case, no accents, spaces collapsed", () => {
    expect(fold("  Año   DE  Diseño ")).toBe("ano de diseno");
    expect(fold("Quién")).toBe("quien");
    expect(fold("ÇÀ")).toBe("ca");
  });

  it("matches without accents or case, either way round", () => {
    expect(ids("ano")).toContain("diseno/ano-de-diseno");
    expect(ids("AÑO")).toContain("diseno/ano-de-diseno");
    expect(ids("quien")).toEqual(["ops/plano"]);
  });

  it("a word matches the start of a word, as the site's search bar: never its middle", () => {
    // "de" is inside "modelo" and "Accesos"; it starts "de" and "despliega".
    expect(findNotes([{ id: "x", title: "Modelo" }], "de")).toEqual([]);
    expect(ids("acc")).toEqual(expect.arrayContaining(["seguridad/accesos"]));
    expect(findNotes([{ id: "x", title: "Sinacceso" }], "acceso")).toEqual([]);
  });

  it("every word typed must match, in the title, the path or the text", () => {
    expect(ids("copias seguridad")).toEqual(["ops/copias"]);
    expect(ids("copias control")).toEqual([]);
    // "ops" is only in the path.
    expect(ids("ops plano")).toEqual(["ops/plano"]);
    // "cifrada" is only in the text.
    expect(ids("cifrada")).toEqual(["ops/copias"]);
  });

  it("«control de acceso»: the page that says it first, then those with all three words, title before text", () => {
    // Accesos: the phrase in its text (rank 3). Plano: every word in its text, not
    // as a phrase (rank 4). Modelo: «acceso» nowhere, so not at all.
    expect(ids("control de acceso")).toEqual(["seguridad/accesos", "ops/plano"]);
  });

  it("order: title starts with it, title holds the phrase, title has the words, text phrase, text words, path", () => {
    const n: Findable[] = [
      { id: "a", title: "Otra cosa", text: "aquí: control de acceso" },
      { id: "b", title: "El acceso de su control" },
      { id: "c", title: "Normas de control de acceso" },
      { id: "d", title: "Control de acceso" },
      { id: "e", title: "Otra", text: "acceso con control de" },
      { id: "control/de/acceso", title: "Sin nada" },
    ];
    expect(findNotes(n, "control de acceso").map((x) => [x.id, x.rank])).toEqual([
      ["d", 0],
      ["c", 1],
      ["b", 2],
      ["a", 3],
      ["e", 4],
      ["control/de/acceso", 5],
    ]);
  });

  it("a phrase is its words in order, whatever separates them in the title or text: «0.1.0 Page» is not «0.0.1 Page»", () => {
    // Typed and folded, «0.1.0 Page» is the words 0 1 0 page. The phrase was
    // those words joined by spaces, looked for in the title as written («0.1.0
    // page»): never there, so the title that IS what was typed ranked 2, level
    // with «0.0.1 Page» (same words, other order), and came second
    // (a browser test on the manual KB: finding a note lists it first).
    const versions: Findable[] = [
      { id: "v001", title: "0.0.1 Page" },
      { id: "v010", title: "0.1.0 Page" },
    ];
    expect(findNotes(versions, "0.1.0 Page").map((x) => [x.id, x.rank])).toEqual([
      ["v010", 0],
      ["v001", 2],
    ]);
    // In the middle of the title (rank 1), in the text (rank 3, with its snippet),
    // and typed with other separators than the title's.
    const n: Findable[] = [
      { id: "a", title: "Notas de la 0.1.0" },
      { id: "b", title: "Otra", text: "Publicada la entrega 0.1.0 el lunes." },
      { id: "c", title: "Otra más", text: "0 de 1, y 0 de nada" },
      { id: "d", title: "Copias: seguridad" },
    ];
    expect(findNotes(n, "0 1 0").map((x) => [x.id, x.rank])).toEqual([
      ["a", 1],
      ["b", 3],
      ["c", 4],
    ]);
    expect(findNotes(n, "0.1.0")[1]!.snippet).toContain("entrega 0.1.0 el lunes");
    expect(findNotes(n, "copias seguridad").map((x) => [x.id, x.rank])).toEqual([["d", 0]]);
    // Whole words between: «0.10» has the words (1 starts 10) but not the phrase 0 1 0.
    expect(findNotes([{ id: "x", title: "0.10" }], "0.1.0").map((x) => x.rank)).toEqual([2]);
  });

  it("in the text, the page that mentions it more comes first; within a rank, the shorter title", () => {
    const n: Findable[] = [
      { id: "una", title: "B", text: "cifrado" },
      { id: "tres", title: "A más largo", text: "cifrado, cifrado y cifrado" },
    ];
    expect(findNotes(n, "cifrado").map((x) => x.id)).toEqual(["tres", "una"]);
    const t = findNotes(
      [
        { id: "a", title: "Copias de seguridad de la base" },
        { id: "b", title: "Copias" },
      ],
      "cop",
    );
    expect(t.map((x) => x.id)).toEqual(["b", "a"]);
  });

  it("a match in the text comes with a few words around it, as written", () => {
    const [r] = findNotes(notes, "control de acceso");
    expect(r!.snippet).toContain("control de acceso se hace por grupos");
    const largo = "palabra ".repeat(30) + "El ACCESO al sistema" + " cola".repeat(30);
    const [s] = findNotes([{ id: "x", title: "X", text: largo }], "acceso");
    expect(s!.snippet).toMatch(/^… .*El ACCESO al sistema.* …$/);
    expect(s!.snippet!.length).toBeLessThan(140);
    // A title match needs no snippet.
    expect(findNotes(notes, "copias")[0]!.snippet).toBeUndefined();
  });

  it("nothing for an empty query; never more than the limit", () => {
    expect(ids("")).toEqual([]);
    expect(ids("   ")).toEqual([]);
    expect(ids("de", 2)).toHaveLength(2);
    expect(ids("de", 0)).toEqual([]);
  });

  it("property: every result has every word at the start of a word of its title, path or text; unique; at most the limit", () => {
    const noteArb = fc.record({
      id: fc.string({ minLength: 1 }),
      title: fc.string(),
      text: fc.option(fc.string(), { nil: undefined }),
    });
    const ws = (s: string) => fold(s).match(/[\p{L}\p{N}]+/gu) ?? [];
    fc.assert(
      fc.property(
        fc.uniqueArray(noteArb, { selector: (n) => n.id, maxLength: 30 }),
        fc.string(),
        fc.integer({ min: 0, max: 10 }),
        (ns, q, limit) => {
          const r = findNotes(ns, q, limit);
          const qs = ws(q);
          return (
            r.length <= limit &&
            new Set(r.map((n) => n.id)).size === r.length &&
            r.every((n) => {
              const all = [
                ...ws(n.title),
                ...ws(n.id.replace(/[/_-]+/g, " ")),
                ...ws(n.text ?? ""),
              ];
              return qs.every((w) => all.some((x) => x.startsWith(w)));
            })
          );
        },
      ),
      params,
    );
  });
});
