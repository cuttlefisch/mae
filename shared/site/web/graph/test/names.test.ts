// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { chooseNames, NAMES_MODES } from "../src/browser/names";

describe("which pages the expanded graph names", () => {
  it("defaults to the current page only; a stored choice is kept only if it is one", () => {
    expect(NAMES_MODES).toEqual(["current", "all"]);
    expect(chooseNames(null)).toBe("current");
    expect(chooseNames("")).toBe("current");
    expect(chooseNames("everything")).toBe("current");
    // The old third option drew what «all» draws: kept as «all».
    expect(chooseNames("neighbours")).toBe("all");
    expect(chooseNames("all")).toBe("all");
  });
});
