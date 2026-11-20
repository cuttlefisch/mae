// SPDX-License-Identifier: GPL-3.0-or-later
// npm run build: reads palettes/, resolves and checks the references, and rewrites
// all of dist/ (whatever is no longer generated disappears).
import { mkdirSync, rmSync, writeFileSync } from "node:fs"
import path from "node:path"
import { generate } from "./generate.ts"
import { loadPalettes } from "./tokens.ts"

const root = path.resolve(import.meta.dirname, "..")
const dist = path.join(root, "dist")
const files = generate(loadPalettes(path.join(root, "palettes")))
rmSync(dist, { recursive: true, force: true })
for (const [ruta, contenido] of files) {
  const target = path.join(dist, ruta)
  mkdirSync(path.dirname(target), { recursive: true })
  writeFileSync(target, contenido)
}
console.log(`dist/: ${[...files.keys()].join(", ")}`)
