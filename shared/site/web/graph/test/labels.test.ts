// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import fc from "fast-check";
import { real } from "./arbitraries";
import { foldedName, spreadAngles } from "../src/labels";

const TAU = Math.PI * 2;
const norm = (a: number) => ((a % TAU) + TAU) % TAU;
/** Smallest circular distance between any two outputs. */
function minSep(xs: number[]): number {
  const s = xs.map(norm).sort((a, b) => a - b);
  let m = s[0]! + TAU - s[s.length - 1]!;
  for (let i = 1; i < s.length; i++) m = Math.min(m, s[i]! - s[i - 1]!);
  return m;
}

describe("spreadAngles", () => {
  it("leaves angles alone that are already far enough apart", () => {
    const xs = [0.1, 1, 2, 3, 5];
    expect(spreadAngles(xs, 0.2)).toEqual(xs);
  });

  it("pushes a crowded pair apart symmetrically", () => {
    const [a, b] = spreadAngles([1, 1.01], 0.1);
    expect(a).toBeCloseTo(0.955, 9);
    expect(b).toBeCloseTo(1.055, 9);
  });

  it("spreads three on one spot around it, keeping input order", () => {
    const out = spreadAngles([2, 2, 2], 0.1);
    expect(out[0]).toBeCloseTo(1.9, 9);
    expect(out[1]).toBeCloseTo(2, 9);
    expect(out[2]).toBeCloseTo(2.1, 9);
  });

  it("handles a cluster across the top (0 / 2π)", () => {
    const out = spreadAngles([TAU - 0.01, 0.01], 0.1);
    expect(minSep(out)).toBeGreaterThanOrEqual(0.1 - 1e-9);
    // Still centred on the top: one each side.
    expect(norm(out[0]!)).toBeCloseTo(TAU - 0.05, 9);
    expect(norm(out[1]!)).toBeCloseTo(0.05, 9);
  });

  it("spaces evenly when the spread clusters would wrap round into each other", () => {
    // n * gap fits in a turn, yet three crowded clusters, each spread as a
    // block, run past the cut into each other (found by a random search over
    // an instrumented copy: a single cluster cannot do it).
    const gap = 0.6909507575289654;
    const xs = [
      4.689702737934642,
      Math.PI,
      4.80875256744829,
      Math.PI,
      6.257034761113786,
      Math.PI / 2,
      Math.PI / 2,
      Math.PI,
      Math.PI / 2,
    ];
    const out = spreadAngles(xs, gap);
    expect(minSep(out)).toBeCloseTo((2 * Math.PI) / xs.length, 9);
    expect(minSep(out)).toBeGreaterThanOrEqual(gap);
  });

  it("spaces evenly when they cannot fit", () => {
    const out = spreadAngles([0, 0.1, 0.2, 0.3], 2);
    expect(minSep(out)).toBeCloseTo(TAU / 4, 9);
  });

  it("property: every pair at least `gap` apart, whenever n * gap fits in a turn", () => {
    fc.assert(
      fc.property(
        fc.array(real(0, TAU), { minLength: 2, maxLength: 40 }),
        real(0.001, 0.5),
        (xs, gap) => {
          fc.pre(xs.length * gap <= TAU);
          expect(minSep(spreadAngles(xs, gap))).toBeGreaterThanOrEqual(gap - 1e-9);
        },
      ),
    );
  });

  it("property: keeps the circular order of the labels", () => {
    fc.assert(
      fc.property(
        // Distinct by more than rounding: labels on the very same angle have
        // no circular order to keep (5e-324 vs 0 was the counterexample).
        fc
          .uniqueArray(fc.integer({ min: 0, max: 62830 }), { minLength: 3, maxLength: 30 })
          .map((xs) => xs.map((x) => x / 10000)),
        real(0.001, 0.2),
        (xs, gap) => {
          fc.pre(xs.length * gap <= TAU);
          const out = spreadAngles(xs, gap);
          const before = xs
            .map((a, i) => [a, i] as const)
            .sort((p, q) => p[0] - q[0])
            .map((p) => p[1]);
          const after = out
            .map((a, i) => [norm(a), i] as const)
            .sort((p, q) => p[0] - q[0])
            .map((p) => p[1]);
          // Equal up to rotation.
          const r = after.indexOf(before[0]!);
          expect([...after.slice(r), ...after.slice(0, r)]).toEqual(before);
        },
      ),
    );
  });

  it("property: moves nothing further than the crowding forces (at most n * gap)", () => {
    fc.assert(
      fc.property(
        fc.array(real(0, TAU), { minLength: 2, maxLength: 30 }),
        real(0.001, 0.2),
        (xs, gap) => {
          fc.pre(xs.length * gap <= TAU / 2);
          const out = spreadAngles(xs, gap);
          xs.forEach((a, i) => {
            const d = Math.abs(norm(out[i]! - a + Math.PI) - Math.PI);
            expect(d).toBeLessThanOrEqual(xs.length * gap + 1e-9);
          });
        },
      ),
    );
  });
});

describe("foldedName", () => {
  it("names a folder with its size, singular or plural", () => {
    expect(foldedName("STD", 42, "{n} notes", "1 note")).toBe("STD · 42 notes");
    expect(foldedName("annex", 1, "{n} notes", "1 note")).toBe("annex · 1 note");
  });
});
