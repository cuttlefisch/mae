# `kb-export-subgraph-html` browser test suite

Layer 2 (real-browser, no mocks) behavioral tests for the chord-diagram
export's client-side `graph.js`/`graph.css` (`crates/export/assets/`).
`crates/export/src/html_graph.rs`'s own `mod tests` only assert on
generated HTML/JS *source text* — they can't catch a bug that only shows
up when the page is actually loaded and driven (a real regex literal
corrupted by the inline-script escaper shipped unnoticed this way, caught
only by manually running `node --check` before this suite existed — see
`html_graph.rs`'s own doc comment).

## Running

```sh
npm install   # once
npm test      # regenerates both fixtures via `cargo run --example
              # fixture_export`, then runs the suite against real
              # Chromium and Firefox (both required — see behavior.test.js's
              # header comment for why: two real bugs were each only
              # reproducible in one specific engine)
```

Requires `/usr/bin/chromium-browser` and `/usr/bin/firefox` (hardcoded in
`behavior.test.js` — adjust `executablePath` there if your system installs
elsewhere) and Node.js. Not wired into `make test`/CI yet — run manually
before merging any change to `crates/export/assets/graph.js`,
`graph.css`, or `crates/export/src/html_graph.rs`'s HTML assembly.

`fixture.html`/`fixture-custom-config.html` are generated, gitignored —
never hand-edit them, regenerate via `npm test`'s `pretest` step (or
`cargo run --example fixture_export -p mae-export -- <path> [hover-growth-factor]`
directly).
