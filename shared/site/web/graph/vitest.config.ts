// SPDX-License-Identifier: GPL-3.0-or-later
import { defineConfig } from "vitest/config";
import { workers } from "./test/workers";

export default defineConfig({
  test: {
    environment: "node",
    include: ["test/**/*.test.ts"],
    reporters: ["default"],
    // As many workers as the job has cores, not one fewer (test/workers.ts).
    ...workers(),
    // Coverage of the code that runs in Node. The browser scripts (src/browser) and
    // the mount (src/mount.ts) run in a browser; test/mount.test.ts exercises them
    // under happy-dom, without counting them here.
    // Every pure module is in (2026-11-06: the hierarchical views' modules joined,
    // src/subgraph.ts left with the subgraphs). The thresholds are a floor set
    // from the measured baseline (2026-10-30): a change must not lower them.
    coverage: {
      provider: "v8",
      include: [
        "src/geometry.ts",
        "src/hierarchy.ts",
        "src/sunburst.ts",
        "src/constellation.ts",
        "src/scene.ts",
        "src/viewport.ts",
        "src/cull.ts",
        "src/labels.ts",
        "src/layouts.ts",
        "src/here.ts",
        "src/i18n/**",
      ],
      reporter: ["text", "cobertura"],
      thresholds: { statements: 100, branches: 95, functions: 100, lines: 100 },
    },
  },
});
