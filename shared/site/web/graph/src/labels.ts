// SPDX-License-Identifier: GPL-3.0-or-later
// Label placement helpers. Pure: no DOM.

const TAU = Math.PI * 2;

/**
 * Spreads angles (radians) so that neighbours are at least `gap` apart,
 * keeping their circular order and moving them as little as possible: a
 * crowded group is pushed apart as a block centred where its members wanted to
 * be. The output is in the input's order. If they cannot fit in one turn at
 * that gap, they are spaced evenly.
 *
 * Method: cut the circle at the widest empty stretch between wanted angles,
 * unroll it there into a line, and solve the line exactly (pool adjacent
 * blocks while they overlap: each block sits at the mean of what its members
 * want, minus their offsets inside it, which minimises the squared movement).
 */
export function spreadAngles(angles: number[], gap: number): number[] {
  const n = angles.length;
  if (n < 2 || !(gap > 0)) return [...angles];
  const norm = (a: number) => ((a % TAU) + TAU) % TAU;
  const order = angles.map((a, i) => ({ a: norm(a), i })).sort((x, y) => x.a - y.a || x.i - y.i);
  if (gap * n > TAU) {
    const out = new Array<number>(n);
    order.forEach((o, k) => (out[o.i] = order[0]!.a + (k * TAU) / n));
    return out;
  }
  // Widest empty stretch: after element c.
  let c = n - 1;
  let widest = order[0]!.a + TAU - order[n - 1]!.a;
  for (let k = 0; k < n - 1; k++) {
    const g = order[k + 1]!.a - order[k]!.a;
    if (g > widest) {
      widest = g;
      c = k;
    }
  }
  const seq = [...order.slice(c + 1), ...order.slice(0, c + 1)];
  const want: number[] = [];
  for (const [k, o] of seq.entries()) {
    let a = o.a;
    if (k > 0) while (a < want[k - 1]!) a += TAU;
    want.push(a);
  }
  type Block = { start: number; count: number; sum: number };
  const at = (b: Block) => b.sum / b.count;
  const blocks: Block[] = [];
  for (let k = 0; k < n; k++) {
    blocks.push({ start: k, count: 1, sum: want[k]! });
    for (;;) {
      const b = blocks[blocks.length - 1]!;
      const a = blocks[blocks.length - 2];
      if (!a || at(a) + a.count * gap <= at(b) + 1e-12) break;
      blocks.splice(blocks.length - 2, 2, {
        start: a.start,
        count: a.count + b.count,
        // b's members sit a.count places further into the merged block.
        sum: a.sum + b.sum - a.count * b.count * gap,
      });
    }
  }
  const placed: number[] = new Array<number>(n);
  for (const b of blocks) for (let k = 0; k < b.count; k++) placed[b.start + k] = at(b) + k * gap;
  // Across the cut: the line's ends must still be a gap apart around the turn.
  if (placed[n - 1]! - placed[0]! > TAU - gap + 1e-12) {
    const out = new Array<number>(n);
    seq.forEach((o, k) => (out[o.i] = placed[0]! + (k * TAU) / n));
    return out;
  }
  // Each label moves by its displacement, from its own input value: one that
  // did not move comes back exactly as given (not shifted by a turn).
  const out = new Array<number>(n);
  seq.forEach((o, k) => (out[o.i] = angles[o.i]! + (placed[k]! - want[k]!)));
  return out;
}

/**
 * A folder's name with its size: its title and how many notes it holds. `many` holds
 * "{n}". One note is possible (a folder of one note and no folder page, on a
 * site that generates no folder pages).
 */
export function foldedName(title: string, size: number, many: string, one: string): string {
  return `${title} · ${size === 1 ? one : many.replace("{n}", String(size))}`;
}
