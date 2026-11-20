// SPDX-License-Identifier: GPL-3.0-or-later
// Generated content indexes for the hierarchical layouts' property tests. The
// same awkward alphabet the ring's old subgraph tests used: folders repeat, nest
// and collide with notes; "index" leaves make folder pages; titles collide.
import fc from "fast-check";
import type { IndexEntry } from "../src/geometry";

const seed = process.env.FC_SEED ? Number(process.env.FC_SEED) : undefined;
export const params = {
  numRuns: 150,
  ...(seed !== undefined ? { seed } : {}),
  ...(process.env.FC_NO_SHRINK ? { endOnFailure: true } : {}),
};

/**
 * A real number in [min, max], spread over the range. fast-check's own
 * fc.double draws evenly over floating-point representations, which crowd near
 * zero: measured, double(0, 1) gave a value under 0.001 91% of the time, and
 * double(-20, 20) an angle on the bottom half of a circle 0.3% of the time, so
 * properties written for the whole range tested almost none of it (a mutation
 * that turned every bottom-half label upside down survived). Uniform three
 * draws in four; fc.double's edge-heavy draw the fourth, for the edge cases it
 * is good at.
 */
export const real = (min: number, max: number) =>
  fc.oneof(
    {
      weight: 3,
      arbitrary: fc
        // noBias: fc.integer leans to its ends as well (review: 54% of draws
        // within 0.001 of an end of real(0, 1), 24% in its middle half).
        .integer({ min: 0, max: 0x7fffffff })
        .noBias()
        .map((i) => Math.min(max, min + ((max - min) * i) / 0x7fffffff)),
    },
    { weight: 1, arbitrary: fc.double({ min, max, noNaN: true, noDefaultInfinity: true }) },
  );

/**
 * A positive scale in [min, max], spread evenly over its orders of magnitude
 * (log-uniform): a zoom of 0.01 to 50 drawn uniformly is above 1 98% of the
 * time, and the cancellation CI once found in the smooth zoom (at small zooms)
 * went untested.
 */
export const logReal = (min: number, max: number) =>
  real(Math.log(min), Math.log(max)).map((l) => Math.min(max, Math.max(min, Math.exp(l))));

const seg = fc.constantFrom(
  "a",
  "b",
  "c",
  "lista",
  "x1",
  "index",
  "folder:a",
  "Foo",
  "foo",
  "n1",
  "n01",
);
const pathArb = fc.array(seg, { minLength: 1, maxLength: 6 }).map((s) => s.join("/"));
const flatArb = fc
  .tuple(fc.constantFrom("", "plano/", "a/b/", "a/b/c/d/"), fc.integer({ min: 0, max: 200 }))
  .map(([pre, n]) => `${pre}p${n}`);
const slugArb = fc.oneof({ weight: 3, arbitrary: pathArb }, { weight: 2, arbitrary: flatArb });

export const indexArb = fc
  .uniqueArray(slugArb, { minLength: 0, maxLength: 200, size: "max" })
  .chain((slugs) =>
    fc
      .array(fc.tuple(fc.nat(), fc.nat()), { maxLength: slugs.length * 2, size: "max" })
      .map((pairs) => {
        const idx: Record<string, IndexEntry> = {};
        for (const s of slugs) idx[s] = { title: ["Uno", "Dos", "Tres"][s.length % 3]!, links: [] };
        if (slugs.length > 0) {
          for (const [i, j] of pairs)
            idx[slugs[i % slugs.length]!]!.links!.push(slugs[j % slugs.length]!);
        }
        return idx;
      }),
  );

/** The same index with its entries in another order. */
export function shuffled(idx: Record<string, IndexEntry>, by: number): Record<string, IndexEntry> {
  const keys = Object.keys(idx);
  const out: Record<string, IndexEntry> = {};
  for (let i = 0; i < keys.length; i++) {
    const k = keys[(i * (by | 1) + by) % keys.length]!;
    if (!(k in out)) out[k] = idx[k]!;
  }
  for (const k of keys) if (!(k in out)) out[k] = idx[k]!;
  return out;
}
