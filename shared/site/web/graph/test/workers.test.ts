// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import config from "../vitest.config";
import { workers } from "./workers";

describe("vitest workers", () => {
  it("the queue's (SLURM_CPUS_PER_TASK) win over the ones seen", () => {
    expect(workers({ SLURM_CPUS_PER_TASK: "4" }, 16)).toEqual({
      maxWorkers: 4,
      minWorkers: 1,
    });
  });

  it("without the queue, or with a value that is not a positive integer, the ones seen", () => {
    expect(workers({}, 6).maxWorkers).toBe(6);
    expect(workers({ SLURM_CPUS_PER_TASK: "0" }, 6).maxWorkers).toBe(6);
    expect(workers({ SLURM_CPUS_PER_TASK: "two" }, 6).maxWorkers).toBe(6);
  });

  it("the minimum never exceeds the maximum, nor a --maxWorkers=1", () => {
    for (const n of [1, 2, 16]) {
      const t = workers({ SLURM_CPUS_PER_TASK: String(n) });
      expect(t.minWorkers).toBe(1);
      expect(t.minWorkers).toBeLessThanOrEqual(t.maxWorkers);
    }
  });

  it("vitest.config.ts uses them", () => {
    expect(config.test?.maxWorkers).toBe(workers().maxWorkers);
    expect(config.test?.minWorkers).toBe(1);
  });
});
