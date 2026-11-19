// SPDX-License-Identifier: GPL-3.0-or-later
// npm run doom: writes palettes/<name>.yaml for each palette in data/doom.json
// (scripts/doom.mjs says how) and removes those it generated before that are no longer
// there; and, at the end of the YAML of each hand-written palette in «emphasisOnly», its
// block of emphasis swatches (the hand-written part is not touched).
import { readFileSync, readdirSync, unlinkSync, writeFileSync } from "node:fs"
import path from "node:path"
import { resolveText } from "../src/tokens.ts"
import { MARK, ROOT, generate, generateByHand, readData } from "./doom.mjs"

const dir = path.join(ROOT, "palettes")
const files = generate(readData())
// Those this program generated that are no longer in the data: out.
for (const f of readdirSync(dir))
  if (readFileSync(path.join(dir, f), "utf8").includes(MARK) && !files.has(f.replace(/\.yaml$/, "")))
    unlinkSync(path.join(dir, f))
for (const [n, text] of files) writeFileSync(path.join(dir, `${n}.yaml`), text)
console.log(`${files.size} Doom palettes in palettes/`)
const byHand = generateByHand(readData(), (n) => readFileSync(path.join(dir, `${n}.yaml`), "utf8"), resolveText)
for (const [n, text] of byHand) writeFileSync(path.join(dir, `${n}.yaml`), text)
console.log(`${byHand.size} hand-written palettes with Doom swatches: ${[...byHand.keys()].join(", ")}`)
