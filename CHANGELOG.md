# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Bug Fixes

- Horizontal scroll in split windows and AI timeout for Ollama ([c43f113](https://github.com/cuttlefisch/mae/commit/c43f11355987bfef004dcbd4237759d05518d4bc))
- Resolve clippy warnings breaking CI on Rust 1.95 ([967d561](https://github.com/cuttlefisch/mae/commit/967d5613ab9d2a5c162836b224f1d1edc7c34abd))
- Resolve collapsible_match clippy warnings in key_handling ([a3f742a](https://github.com/cuttlefisch/mae/commit/a3f742ab169c0e90e0de9d984d0aebe6f87a912b))

### CI

- Add GitHub Actions CI, release workflow, README, and changelog config ([e8442bb](https://github.com/cuttlefisch/mae/commit/e8442bb70921be87c3e07e329bcfa79c22a7c70f))
- Add semantic version bumping on PR merge ([69cd7b8](https://github.com/cuttlefisch/mae/commit/69cd7b8d6996f250817bb2eae3803469cbaa5961))
- *(deps)* Bump the ci-dependencies group with 2 updates ([e907ab8](https://github.com/cuttlefisch/mae/commit/e907ab8e1341b318abff25f47167f19bbc6a9735))

### Documentation

- Update roadmap with Phase 3g hardening, editor history lessons, and revised priorities ([5003224](https://github.com/cuttlefisch/mae/commit/5003224a1b65c38eb57b7c14badcbef2fe15fb9e))

### Features

- Implement terminal editor with vi-like modal editing and AI integration ([bd36eec](https://github.com/cuttlefisch/mae/commit/bd36eec62ed399f57abb6c896c0ebd1e95e58795))
- Text objects, line join, indent, case change, alt-file, cmd history, shell escape ([5609863](https://github.com/cuttlefisch/mae/commit/56098631bb90c562ab7bb0113c0d8983a33ec485))
- AI multi-file tools — open, switch, close buffers, project search (Phase 3f M1/M2/M4) ([2384282](https://github.com/cuttlefisch/mae/commit/23842821ab582fc6b4eff1339062bdfb93e06402))
- *(lsp)* Phase 4a M1/M2 — LSP client + navigation, plus Phase 3g hardening ([a0e38fb](https://github.com/cuttlefisch/mae/commit/a0e38fb90bd66e08dfecf0492a2bb0b9f9664913))
- *(lsp)* Phase 4a M3 — diagnostics (publish, gutter, list, jump) ([9297b51](https://github.com/cuttlefisch/mae/commit/9297b5129eefacb01ead709e436b350ad843eb50))
- *(ai)* Lsp_diagnostics tool — structured diagnostics for the AI (Phase 4a M5 partial) ([0f64be2](https://github.com/cuttlefisch/mae/commit/0f64be2e4d1aa2aebb0bd788a7e40a99cee548af))
- *(syntax)* Phase 4b M1/M2 — tree-sitter parsing + highlighting ([3a69619](https://github.com/cuttlefisch/mae/commit/3a6961904e563bacbc190658d37dfdbb2169adcf))
- *(syntax)* Phase 4b M3 — structural selection + syntax_tree AI tool ([8e76fd8](https://github.com/cuttlefisch/mae/commit/8e76fd81f7cd3c915a50b754ca9f6d254c9ac241))
- *(dap)* Phase 4c M1 — DAP client (connection + lifecycle) ([a61f000](https://github.com/cuttlefisch/mae/commit/a61f000cc5bae4290552b616679411d1e5fdf6c8))
- *(dap)* Phase 4c M1 — DapManager event/command translator ([f925c5e](https://github.com/cuttlefisch/mae/commit/f925c5e7fd457be3aca83d038aaa70866c4b2ddb))
- *(dap)* Phase 4c M1.5 — editor integration for DAP sessions ([a250a7c](https://github.com/cuttlefisch/mae/commit/a250a7cff247695506975ef36db154de831df8dd))
- *(dap)* Phase 4c M4 — AI debug tools (start/break/continue/step/inspect) ([dad4f2e](https://github.com/cuttlefisch/mae/commit/dad4f2e5d8729241983d09fe9687f1f6f8d4be89))
- *(dap)* Phase 4c M2 — gutter breakpoint + execution-line rendering ([6740a40](https://github.com/cuttlefisch/mae/commit/6740a4009bd6a805726e130ab7e032e1d069c887))
- *(editor)* Phase 3e M6 marks — m<letter> sets, '<letter> jumps ([9f955c6](https://github.com/cuttlefisch/mae/commit/9f955c618fc2c2be9d8849a3ae09ffc6eaf86e4b))
- *(core)* Phase 3f M3 — conversation persistence (:ai-save / :ai-load) ([4e7ffec](https://github.com/cuttlefisch/mae/commit/4e7ffecdb72f2da00c88deac84e7ad6d3c0f3423))
- *(core)* Phase 3e M6 macros — q<letter> record, @<letter> replay ([f21b0a6](https://github.com/cuttlefisch/mae/commit/f21b0a6ba985ecf6be67d34c0ee35fe1d8558fef))
- Phases 3h M3-M8, 4a-4d, 5 — vim parity, LSP/DAP/syntax/KB, Scheme REPL ([406ca1d](https://github.com/cuttlefisch/mae/commit/406ca1d65002efcb61140a7e57e09950805d3848))
- Phase 4a M5 — async LSP AI tools (lsp_definition, lsp_references, lsp_hover) ([5619437](https://github.com/cuttlefisch/mae/commit/56194377041e4c0ea50f71863548892bf16f7fe1))

### Miscellaneous

- Fix clippy warnings and apply cargo fmt ([a6db8ca](https://github.com/cuttlefisch/mae/commit/a6db8cadd9318c7d0c750b1d6790c19c81a2dbf3))
- Update ROADMAP — Phase 3f M3 complete (conversation persistence) ([df87f8f](https://github.com/cuttlefisch/mae/commit/df87f8f4b405033e6bbdb7063dba6570dca1340a))
- Update ROADMAP — macros done, all Tier 1 self-hosting blockers complete ([437ae19](https://github.com/cuttlefisch/mae/commit/437ae19917e49468399dd868fa8dcfe05124434c))
- Update CLAUDE.md — current phase status and next targets ([af75252](https://github.com/cuttlefisch/mae/commit/af7525288df33d22c8ec6c89aff63a848e85b193))
- *(deps)* Update toml requirement from 0.8 to 1.1 ([6270aac](https://github.com/cuttlefisch/mae/commit/6270aacfd76e4caaf5f0a71ff4bb2bd0a854199d))
- Group dependabot updates into batched PRs ([c1096c8](https://github.com/cuttlefisch/mae/commit/c1096c899f23166d6e7d9c378321ba58bdfb7b5e))

### Style

- Cargo fmt ([5c8f6fe](https://github.com/cuttlefisch/mae/commit/5c8f6fe98ca2db7c4d921ce65ce68aa94bb560fd))


