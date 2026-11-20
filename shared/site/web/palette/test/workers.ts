// SPDX-License-Identifier: GPL-3.0-or-later
import { availableParallelism } from "node:os"

/**
 * How many workers vitest uses: the job's cores. SLURM_CPUS_PER_TASK if it runs in the
 * queue; if not, those the process sees (availableParallelism, which follows the
 * affinity). vitest on its own uses one fewer: in a 2-core slot, 1, and the other sits
 * idle (measured 2026-11-17: cpu_media 0.8–1.0 of 2). The minimum is 1, so that a lower
 * --maxWorkers on the command line does not clash with it (vitest fails if the minimum
 * exceeds the maximum).
 */
export function workers(
  env: NodeJS.ProcessEnv = process.env,
  seen: number = availableParallelism(),
): { maxWorkers: number; minWorkers: number } {
  const slurm = Number(env.SLURM_CPUS_PER_TASK)
  return { maxWorkers: Number.isInteger(slurm) && slurm > 0 ? slurm : seen, minWorkers: 1 }
}
