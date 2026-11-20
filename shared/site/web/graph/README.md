# mae-site-graph

The chord-diagram graph view of MAE's KB site: every note on a ring, every link
as a chord between two notes, with two hierarchical views (sunburst and balloon)
for moving around a large site. The layout code is pure TypeScript with no DOM
(`src/*.ts`); the browser scripts (`src/browser/`) turn it into SVG and wire up
the interaction.

## Interface

```ts
import { mountChordGraph } from "mae-site-graph";

const graph = mountChordGraph(container, data, {
  current: "notes/some-page", // the current page's slug
  locale: "en-US", // the labels' language ("en-US" or "es-ES")
  basePath: "", // the path the site is served under
  navigate: (url) => router.go(url), // optional: open a page in place
  bodyLinks: "article a.internal", // optional: body links that highlight their note
  layouts: ["chord", "sunburst", "balloon"], // any of the ChordGraphOptions
});

graph.update("notes/another-page"); // the page changed
graph.dispose(); // remove the graph and everything it added
```

- `data` is a `GraphData` (or a promise of one): the site's pages keyed by slug,
  each with an optional `title`, `links`, `tags` and `content` (searched in the
  expanded view).
- Without `navigate`, a click on a note is left to the browser.
- Without `bodyLinks`, no listener is installed on the document; with it, one pair
  per document, removed with the last mount that asked for it.
- `update` runs the pending cleanups, closes the expanded view, marks the page
  visited (unless `markVisited: false`) and draws the graph again.
- Mounting throws on a removed or unknown «You are here» setting
  (`hereMarker`, `hereToggle`).

## Testing

```sh
npm install
npm test          # vitest
npm run typecheck # tsc --noEmit
npm run lint      # eslint
npm run format    # prettier --check
```

`test/mount.test.ts` runs under happy-dom; the rest runs in Node.

`mutants.json` lists the mutants each test must kill. MAE's `tools/mutate` does
not run Vitest targets yet, so the list is not executed by CI; that is a
follow-up.

## Not yet built

`src/browser/chord.scss` is not compiled by anything yet: building the site's
assets is later work.
