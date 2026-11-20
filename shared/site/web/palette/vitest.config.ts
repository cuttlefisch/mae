// SPDX-License-Identifier: GPL-3.0-or-later
import { defineConfig } from "vitest/config"
import { workers } from "./test/workers.ts"
export default defineConfig({ test: { globals: true, include: ["test/**/*.test.ts"], ...workers() } })
