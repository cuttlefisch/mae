// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, it, expect } from "vitest";
import { i18n } from "../src/i18n";
import enUS from "../src/i18n/locales/en-US";
import esES from "../src/i18n/locales/es-ES";

// Every leaf path of a locale object, e.g. "components.chordGraph.title".
const leaves = (o: object, prefix = ""): [string, unknown][] =>
  Object.entries(o).flatMap(([k, v]) =>
    v && typeof v === "object"
      ? leaves(v as object, `${prefix}${k}.`)
      : [[`${prefix}${k}`, v] as [string, unknown]],
  );

describe("i18n", () => {
  it("returns the exact locale when it exists", () => {
    expect(i18n("es-ES")).toBe(esES);
    expect(i18n("en-US")).toBe(enUS);
  });

  it("falls back by language, then to English", () => {
    expect(i18n("es-MX")).toBe(esES);
    expect(i18n("es")).toBe(esES);
    expect(i18n("fr-FR")).toBe(enUS);
    expect(i18n("")).toBe(enUS);
  });

  it("has every English string in Spanish, non-empty (a missing one renders as undefined)", () => {
    const en = new Map(leaves(enUS));
    const es = new Map(leaves(esES));
    expect([...es.keys()].sort()).toEqual([...en.keys()].sort());
    for (const [k, v] of es) {
      expect(typeof v, k).toBe("string");
      expect((v as string).trim().length, k).toBeGreaterThan(0);
    }
  });
});
