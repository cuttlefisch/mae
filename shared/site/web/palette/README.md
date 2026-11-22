# mae-site-palette

The colour palettes of MAE's site engine (ADR-119 D17): one file of values per
palette, each with a light (`light`) and a dark (`dark`) value for every named
role, checked for contrast when derived, and written out as CSS variables
(`dist/palette.css`) and resolved JSON tokens (`dist/tokens.json`).

- One file per palette in `palettes/`: `neutral` (the default), `gruvbox`,
  `monokai` and the palettes generated from Doom Emacs themes.
- The tests require WCAG AA of the default palette in both modes, and of every
  palette what makes the site work (links, legible code, distinguishable map
  zones, the graph's bands).
- `dist/` is generated (`npm run build`) and committed: MAE's daemon embeds it, so
  a Rust build needs no node (owner, 2026-11-22). The build is byte-deterministic,
  and CI rebuilds it and fails on any difference. Edit `palettes/`, never `dist/`.

The data vocabulary (`light`/`dark`, the group and role names, the `data-theme`
and `data-emphasis` attributes, swatch display names) is the package's data format
and is kept as written. ADR-119 D17 owns the full token set; the `ui` group here is
a deliberately minimal one.

## Roles

| Group | Roles | Notes |
|---|---|---|
| `ui` | `background`, `panel`, `foreground`, `muted`, `link`, `accent`, `mark` | The page. Text roles reach 4.5:1 on `background` and `panel`; `foreground` reaches 4.5:1 on `mark` (a translucent veil); `background` reaches 4.5:1 on `link` (text on accent fills). |
| `gallery` | `title` | A title on a fixed dark scrim (`#000000bf`): the same value in both modes. |
| `diagram` | `ink`, `background`, `border`, nodes, lines, `highlight`, `blocked`, seven zones | Graphviz diagrams and maps: ink 4.5:1 and border 3:1 on the background; map roles pairwise distinguishable. |
| `graph` | `ink`, `accent`, `neighbour`, `visited`, `edge` | The chord graph: 3:1 on the page, inside the graph's chroma/lightness band, ΔE ≥ 15 apart. Written as `--chord-<role>-light/-dark`. |
| `code` | `text`, `background` and nine token classes | Code blocks: each role 4.5:1 (default) or 3:1 (others) on the block background. |

A value is a lowercase `"#rrggbb"` or `"#rrggbbaa"` in quotes, or a reference:
`"{group.role}"` (same mode) or `"{light.group.role}"` / `"{dark.group.role}"`.
So each colour is written once. A reference to no role, a cycle, a missing or
extra role stops the build with the value's path (`neutral.dark.ui.muted: …`).

## Changing a colour, adding a palette

1. Change the value in `palettes/<palette>.yaml` (or add a new file with every
   role of both modes; only one palette has `isDefault: true`).
2. `npm test` rebuilds `dist/` and checks types and contrasts. A pair below its
   minimum is named with its ratio (`muted on panel: 4.21 < 4.5`).

The Doom Emacs palettes are not written by hand: `npm run doom` generates them
from `data/doom.json` (each theme's colours, extracted by
`scripts/extract-doom.mjs`; provenance and licences in `data/PROVENANCE`) with
`scripts/doom.mjs`, which says which Doom variable goes to each role. Doom ships
each theme in one mode; where a theme has a sibling (doom-one and doom-one-light)
each mode comes from its own, otherwise the missing mode is composed from the same
hues. The program adjusts zones, nodes and the graph until they meet the
functional checks, and fails if a theme cannot. `test/doom.test.ts` requires
`palettes/` to be the program's output.

Four near-duplicate palettes were removed (owner, 2026-11-16): `palenight`
(→ `material`), `opera` (→ `nord`), `snazzy` (→ `dracula`), `moonlight`
(→ `tokyo-night`). `city-lights` was not imported into MAE: its upstream licence
is CC-BY-NC-ND-4.0.

## Emphasis

Each Doom palette, and gruvbox and monokai, offers emphasis swatches (owner,
2026-11-16): named colours of its Doom theme that a reader may choose as accent
(`data-emphasis="<id>"` on `<html>`). Each swatch has three tones per mode:

| Tone | Requirement | Role it carries |
|---|---|---|
| `text` | 4.5:1 on `background` and `panel` | `ui.accent` (`--ui-accent`) |
| `graphic` | 3:1 on both | `diagram.highlight` |
| `graph` | the graphic tone inside the graph's band; 3:1 on both | `graph.accent` |

Each tone is the Doom colour moved only in OKLCH lightness, as little as possible.
A swatch is offered only if it passes in both modes and stays ΔE00 ≥ 12 from the
danger red (`diagram.blocked`) and the links, ≥ 16 from the graph's ink and
neighbour, and ≥ 12 from every other swatch offered (`SEP_SWATCHES` in
`scripts/doom.mjs` explains why 12). The default ("signature") is the most
chromatic swatch that is not like the links; gruvbox's and monokai's default is
their own hand-written accent. Nothing else follows the emphasis.

In `palette.css`, each non-default swatch has a `[data-theme][data-emphasis]` block
and its `[data-mode="dark"]` twin; they win by specificity. `tokens.json` lists
the swatches per palette and mode.

## Using it

- `dist/palette.css`: the default palette on `:root`, every palette under
  `:root[data-theme="<name>"]`; dark mode is `data-mode="dark"` on `<html>`.
- `dist/tokens.json`: every role's variable and resolved value, per palette and
  mode, for consumers that read neither CSS nor YAML.

## Development

```sh
npm ci          # from shared/site/web
npm test        # build dist/, then vitest
npm run typecheck
npm run lint
npm run doom    # regenerate the Doom palettes
```

`mutants.json` lists this package's mutants (file, exact text, replacement, the
test that must fail). MAE's `tools/mutate` does not yet run Vitest targets, so CI
does not execute them yet (follow-up).
