// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { slugOfHref } from "../src/link";
import { normaliseSlug } from "../src/geometry";

const PAGE = "https://kb.example/part-0/chapter-0/page-01";

describe("slugOfHref: a link of the page to its note in the graph", () => {
  it("resolves a relative link from the page, as the browser does", () => {
    expect(slugOfHref("../chapter-1/page-02", PAGE)).toBe("part-0/chapter-1/page-02");
    expect(slugOfHref("./page-03", PAGE)).toBe("part-0/chapter-0/page-03");
    expect(slugOfHref("/part-2/x", PAGE)).toBe("part-2/x");
  });

  it("gives the slug the graph gives the note (normaliseSlug): no /index, no trailing slash", () => {
    expect(slugOfHref("/part-0/", PAGE)).toBe("part-0");
    expect(slugOfHref("/part-0/index", PAGE)).toBe("part-0");
    expect(slugOfHref("/", PAGE)).toBe("index");
    for (const raw of ["part-0/index", "a/b", "index"])
      expect(slugOfHref(`/${raw}`, PAGE)).toBe(normaliseSlug(raw));
  });

  it("ignores the fragment and the query", () => {
    expect(slugOfHref("../chapter-1/page-02#h--two", PAGE)).toBe("part-0/chapter-1/page-02");
    expect(slugOfHref("/a/b?x=1", PAGE)).toBe("a/b");
  });

  it("decodes the path: a note named with accents is found by its own slug", () => {
    expect(slugOfHref("/notas/a%C3%B1o", PAGE)).toBe("notas/año");
    // A stray % is kept as it is, not an error.
    expect(slugOfHref("/notas/100%", PAGE)).toBe("notas/100%");
  });

  it("drops a .html ending", () => {
    expect(slugOfHref("/a/b.html", PAGE)).toBe("a/b");
  });

  it("strips the site's base path, and a link outside it is no note", () => {
    const page = "https://kb.example/docs/part-0/page-01";
    expect(slugOfHref("../part-1/page-02", page, "/docs")).toBe("part-1/page-02");
    expect(slugOfHref("/docs/", page, "/docs/")).toBe("index");
    expect(slugOfHref("/docs", page, "/docs")).toBe("index");
    expect(slugOfHref("/docsx/a", page, "/docs")).toBeNull();
    expect(slugOfHref("/other/a", page, "/docs")).toBeNull();
  });

  it("a link to another origin, or no URL at all, is no note", () => {
    expect(slugOfHref("https://elsewhere.example/part-0/page-01", PAGE)).toBeNull();
    expect(slugOfHref("http://[", PAGE)).toBeNull();
  });
});
