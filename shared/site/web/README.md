# shared/site/web — the site engine's browser runtime

ADR-119 D2 puts the web core's browser-side code here: TypeScript, built in CI into versioned assets
that the daemon serves and the static export copies. It is one npm workspace with one lock.

| Package | What it is |
|---|---|
| `graph/` | The knowledge-base graph: chord, hierarchy, sunburst and constellation layouts, scenes, culling, search, the expanded view, and the themes. `init(container, data, options)` (`src/mount.ts`) builds its markup and wires it; the host supplies the data and the navigation. |
| `palette/` | Colour palettes as data (`palettes/*.yaml`, one per theme, light and dark), resolved and checked for contrast (WCAG AA) and separation, and generated into `palette.css`. `scripts/` imports palettes from Doom Emacs's themes. |

## Provenance and licence

Both packages were written by the project owner as standalone libraries and relicensed by the owner
GPL-3.0-or-later for MAE (2026-11-18). They come across as a snapshot, without their history, minus:

- everything derived from the third-party static-site generator they used to serve: the plugin
  wrapper, its build configuration, its default theme and page colours, and the outputs that existed
  only for it (its configuration and callout styles);
- their CI, publishing and package-registry files, and their browser-level tests, which need a built
  site and a headless browser (ADR-119's harness replaces them).

Third-party colour values are recorded with their upstream and licence in
`palette/data/PROVENANCE`. One palette was not imported because its upstream licence
(CC-BY-NC-ND-4.0) is not GPL-compatible.

## Working on it

```sh
npm ci
npm run typecheck && npm run lint && npm test
```

The CI job `site web runtime` runs the same three on any change under this directory.

Each package's `mutants.json` lists the mutants its tests must catch. MAE's `tools/mutate` does not run
Vitest targets yet (#956); until it does, those lists are carried, not run.
