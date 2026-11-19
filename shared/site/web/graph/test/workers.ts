// SPDX-License-Identifier: GPL-3.0-or-later
import { availableParallelism } from "node:os";

/**
 * How many workers vitest uses: the job's cores. SLURM_CPUS_PER_TASK if it runs in the
 * queue; otherwise the ones the process sees (availableParallelism, which follows affinity). vitest, on
 * its own, uses one fewer: in a 2-core slot, 1, and the other sits idle (measured
 * 2026-11-17: the unit suite, 77 s with cpu_mean 1.01 of 2). The minimum is 1, so that a lower
 * --maxWorkers on the command line does not clash with it (vitest fails if the minimum exceeds
 * the maximum).
 */
export function workers(
  env: NodeJS.ProcessEnv = process.env,
  seen: number = availableParallelism(),
): { maxWorkers: number; minWorkers: number } {
  const slurm = Number(env.SLURM_CPUS_PER_TASK);
  return { maxWorkers: Number.isInteger(slurm) && slurm > 0 ? slurm : seen, minWorkers: 1 };
}
