# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Bug Fixes

- *(ai)* Fix infinite loop via context protection and double-esc state cleanup; add regression tests ([4c4d36e](https://github.com/cuttlefisch/mae/commit/4c4d36ec4edd14d0c595ff9c4b86b4424b98bdd3))
- *(ai)* Update AiEvent::Error signature and fix tests ([a7ae7ba](https://github.com/cuttlefisch/mae/commit/a7ae7bafb87c1f7d3fcd363a54f90d8c45cd4cb0))

### Features

- *(ai)* Advanced buffer UI, infinite tool loop, and KB exploration guardrails ([fc3e623](https://github.com/cuttlefisch/mae/commit/fc3e6234065349f8f76b0a062aee964bf6636763))
- *(ai)* Add SOPs and workflow hints for improved multi-tool reasoning ([1d65cc6](https://github.com/cuttlefisch/mae/commit/1d65cc69d2e0ef366450e64061addd8a47604aae))
- *(ai)* Gemini provider support, loop protection, and transcript logging ([2344ca6](https://github.com/cuttlefisch/mae/commit/2344ca637ecda37912a40c26e80c9e9614c5ae1e))

### Testing

- *(ai)* Add regression tests for mid-flight compaction, UI events, and log_activity ([029023e](https://github.com/cuttlefisch/mae/commit/029023e46574cbf59f2aa2a021917970bcad52b6))

## [0.4.0] - 2026-04-21

### Bug Fixes

- MCP shim, LSP init, AI context overflow, and session persistence ([e4623ff](https://github.com/cuttlefisch/mae/commit/e4623ff1b501599d920d41d56da18710b29ed7e1))
- QoL improvements — GUI word wrap, AI selection, and viewport height ([e2c9245](https://github.com/cuttlefisch/mae/commit/e2c9245827608c62824aeee77c58fe9fa1bcdc2d))
- *(gui)* Resolve unused imports and variables in lib.rs ([a32bfc4](https://github.com/cuttlefisch/mae/commit/a32bfc48025cc3f6e6af2addb7d9a3be8db64378))
- *(ai)* Resolve context overflow errors and clean up conversational leaks ([a9f8e97](https://github.com/cuttlefisch/mae/commit/a9f8e97bd3921f29e9fc8bc0ff401abc0fa3c956))
- *(ci)* Resolve clippy lints across the workspace ([0819dc6](https://github.com/cuttlefisch/mae/commit/0819dc6c601893ff279567af68c0743bc95b8e49))
- *(ai)* Infinite loop circuit breaker, GitHub tools, and cancellation fix; fix(gui): startup font size clobbering ([be4e519](https://github.com/cuttlefisch/mae/commit/be4e5194e7bd4d74e0c384825edf36066995fd8f))

### Documentation

- Credit Gemini and DeepSeek for their assistance in development ([51c849d](https://github.com/cuttlefisch/mae/commit/51c849dc9e70773da2a044c15310ab7524fd88df))
- Add alpha disclaimer and AI cost warning; ci: include clippy in pre-commit hook ([31c59f3](https://github.com/cuttlefisch/mae/commit/31c59f32461c86cc81b0b0805bdae7d7a02795b1))

### Features

- Gemini AI agent integration and gemini-cli agent support ([266bdd9](https://github.com/cuttlefisch/mae/commit/266bdd96b4c2a74eafeaf0507703032483ab5198))
- Sync PATH from shell on startup and added :debug-path ([4008dad](https://github.com/cuttlefisch/mae/commit/4008dada9eed1ec81a994c94e1ab6ad1e6ae6aca))
- TextWithToolCalls + AI buffer focus preservation + prompt wrap fix ([e85f468](https://github.com/cuttlefisch/mae/commit/e85f4689bef8930e5a3e94be2ef1b51743628be3))
- Transactional AI tool callstack and grounded limits ([d45d4e2](https://github.com/cuttlefisch/mae/commit/d45d4e2409e351a59034aaacc313282a61669465))
- Cross-session persistence, KB audit, and tool context fixes ([fe6d35b](https://github.com/cuttlefisch/mae/commit/fe6d35b366eab75ddd9284c21ed325b1d0c58e5e))
- AI dogfooding completeness — buffer focus, git tools, and introspection ([286fea9](https://github.com/cuttlefisch/mae/commit/286fea9e1957f9fe424d3d4121b51146988f1053))
- Comprehensive lifecycle hooks and debug preservation ([b08c21b](https://github.com/cuttlefisch/mae/commit/b08c21bdc14cfe544c1e8bb1730ff4f8125ef478))
- Magit-lite, Org-mode core, and GUI font fallback ([40c21b7](https://github.com/cuttlefisch/mae/commit/40c21b76a1ba4ae7b54b269a10fa5cac748a90da))
- Org tools, robust logging, and introspection parity ([8266168](https://github.com/cuttlefisch/mae/commit/8266168e715928cf9c24a5d7ffc2e88e315f5611))
- Visual Debugger Foundation & Org Rendering Polish ([93367e2](https://github.com/cuttlefisch/mae/commit/93367e2f593288b5d74910e67525c22050b83f89))
- *(ai)* Multi-agent infra, cache-aware pricing, and XML prompt library ([2ad7cf6](https://github.com/cuttlefisch/mae/commit/2ad7cf6a436aff27b04b5ecefdc7ff9808b18334))
- *(ai)* Mode/profile switching, command palette integration, and UX controls ([46ee0b1](https://github.com/cuttlefisch/mae/commit/46ee0b16467c0cc7a9a4bc8941028ec0d20cfda3))
- *(ai)* Multi-agent orchestration, delegation, and memory/planning tools ([802f849](https://github.com/cuttlefisch/mae/commit/802f849545018536b13b853b98b1ec5ae21c28ca))
- *(ai)* Interactive UX, multi-agent delegation, and memory/planning infra ([9d252f6](https://github.com/cuttlefisch/mae/commit/9d252f69f98816621cc93a2cbeedae15986710fc))
- *(ai)* Enhance agent prompt guardrails, UX mode cycling, and resilience; bump to v0.4.0 ([08474e0](https://github.com/cuttlefisch/mae/commit/08474e0316ac3f64af624a33ac4eb058429b2d83))

### Miscellaneous

- *(deps)* Bump the rust-dependencies group with 10 updates ([a7da3fd](https://github.com/cuttlefisch/mae/commit/a7da3fd11ea006a53dd2f1a3af082e23ec530cb0))

## [0.3.0] - 2026-04-20

### Bug Fixes

- Horizontal scroll in split windows and AI timeout for Ollama ([c43f113](https://github.com/cuttlefisch/mae/commit/c43f11355987bfef004dcbd4237759d05518d4bc))
- Resolve clippy warnings breaking CI on Rust 1.95 ([967d561](https://github.com/cuttlefisch/mae/commit/967d5613ab9d2a5c162836b224f1d1edc7c34abd))
- Resolve collapsible_match clippy warnings in key_handling ([a3f742a](https://github.com/cuttlefisch/mae/commit/a3f742ab169c0e90e0de9d984d0aebe6f87a912b))
- Cursor-aware help link navigation + config persistence ([3d4bf5d](https://github.com/cuttlefisch/mae/commit/3d4bf5d08eec1f5eb36d362dca4c78b63d51ac28))
- Resolve clippy warnings (collapsible_match, unneeded return) ([5028c6d](https://github.com/cuttlefisch/mae/commit/5028c6d5b4f90bcec010d2a97406e83af269a898))
- Operator-pending mode, linewise yank/paste, find-file creation ([a6f4439](https://github.com/cuttlefisch/mae/commit/a6f4439c5b1e728ea45b7fdb7f6deeb47804ed54))
- File picker fuzzy matching for path queries + root navigation ([10afae2](https://github.com/cuttlefisch/mae/commit/10afae2b0c2a920ff6f71de12cf3ab40ff8e45ec))
- D3k/d2j — extract digit count from operator split remainder ([c551ad0](https://github.com/cuttlefisch/mae/commit/c551ad0c75db9d3a3341dcae2939eb350d07b821))
- Line number toggles + relative numbers + word wrap in renderer ([4fd8e0b](https://github.com/cuttlefisch/mae/commit/4fd8e0b41e8e499619958473a1958d92bb0ad1fe))
- Cursor position with word wrap + hidden line numbers ([9d65d1d](https://github.com/cuttlefisch/mae/commit/9d65d1d987b5be69802579861f8ce1fbecaca93a))
- Add spacing after wrap indicator to separate from text ([46c3b66](https://github.com/cuttlefisch/mae/commit/46c3b6624ae5b289fc6e08d4c4190cc1ff825465))
- Hide phantom trailing-newline line from display ([f7e1016](https://github.com/cuttlefisch/mae/commit/f7e1016aa84a0612c6418a3275cf203d54a937ad))
- Atomic save, crash-safe deferred AI, clipboard feedback, search on switch ([3e07f9f](https://github.com/cuttlefisch/mae/commit/3e07f9f6bf603ee68fffc4a72d00d16faeda784b))
- Exclude mae-gui from CI workspace builds ([ec0cd0b](https://github.com/cuttlefisch/mae/commit/ec0cd0b086fca30d2f54d587986955d4250e9f1b))
- Parse_key_seq supports <Token> bracket syntax for define-key ([2e4cb99](https://github.com/cuttlefisch/mae/commit/2e4cb99d5bc735bc909ab2923e9f4cf776c42fa7))
- Warn on empty key sequence from define-key ([9dbc726](https://github.com/cuttlefisch/mae/commit/9dbc726edee74d3cf65ff29bd0db5ff5227854ff))
- Focus/mode sync, AI cursor visibility, MCP tool gaps, LSP symbol tools ([0ff520e](https://github.com/cuttlefisch/mae/commit/0ff520eeb770198f80c0a23b42075f7bcb5c9ed5))
- Input lock covers all modes, add input_lock tool for MCP agents ([7d03c5b](https://github.com/cuttlefisch/mae/commit/7d03c5b28fef35d014080119a72465bfb3ee7baf))
- Clamp all window cursors before render to prevent rope panic ([e9badb4](https://github.com/cuttlefisch/mae/commit/e9badb45c015be226c35fbb3a224d129a418235b))
- GUI build borrow conflict + ROADMAP milestone updates ([915fac0](https://github.com/cuttlefisch/mae/commit/915fac0d7354259eada30ba2d3783d2ea8956254))
- Collapsible_match clippy lint in debug panel ops ([d1266c0](https://github.com/cuttlefisch/mae/commit/d1266c0180e326ef47ec43fc9b3456e901ae4165))
- Project lifecycle, config wiring, CPU usage + AI tool gaps ([c7e27de](https://github.com/cuttlefisch/mae/commit/c7e27de595e2780cefed48a95027fa0b017d3a27))
- KB-linked tutor, shell auto-close, CPU idle, find-file project root ([6d964c1](https://github.com/cuttlefisch/mae/commit/6d964c1a5920ed4e5746bbfcd702f4aacba92361))
- Agent shell lifecycle tied to command, not parent shell ([63af155](https://github.com/cuttlefisch/mae/commit/63af15549ab6010f00193460d9834e4d87887a44))
- PATH inheritance, messages buffer, dashboard/scratch, CPU idle ([ecaa088](https://github.com/cuttlefisch/mae/commit/ecaa0888f564bad3f160e9edcbcea81a2f2f81ed))
- FairMutex deadlock, splash nav, shell dims, AI tools, theme colors ([083156e](https://github.com/cuttlefisch/mae/commit/083156ec8c66ffacd2532c333612e9afa5d6ff88))
- CI E2E build needs --workspace flag, document GUI color bug ([151cc45](https://github.com/cuttlefisch/mae/commit/151cc4517570a60ab4203e3cef93fb51a92f49d5))

### CI

- Add GitHub Actions CI, release workflow, README, and changelog config ([e8442bb](https://github.com/cuttlefisch/mae/commit/e8442bb70921be87c3e07e329bcfa79c22a7c70f))
- Add semantic version bumping on PR merge ([69cd7b8](https://github.com/cuttlefisch/mae/commit/69cd7b8d6996f250817bb2eae3803469cbaa5961))
- *(deps)* Bump the ci-dependencies group with 2 updates ([e907ab8](https://github.com/cuttlefisch/mae/commit/e907ab8e1341b318abff25f47167f19bbc6a9735))

### Documentation

- Update roadmap with Phase 3g hardening, editor history lessons, and revised priorities ([5003224](https://github.com/cuttlefisch/mae/commit/5003224a1b65c38eb57b7c14badcbef2fe15fb9e))
- Add repo badges and update test count to 1,303 ([325f733](https://github.com/cuttlefisch/mae/commit/325f7338214f206010b9346a9674b2f72d1f1b40))
- Update ROADMAP + CLAUDE.md — 1,509 tests, v0.3.0 status ([338072c](https://github.com/cuttlefisch/mae/commit/338072cc8f8871967c78026e8355aadc4fd58b35))
- Update ROADMAP — 5 GUI features were already implemented ([74c7da5](https://github.com/cuttlefisch/mae/commit/74c7da5fa248c61393b6e865078738ac88f8bd84))

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
- WIP foundation for help browser redesign + QoL bundle ([441e547](https://github.com/cuttlefisch/mae/commit/441e5478ecc9e16bab485ae06dabe2ce52e84e52))
- Help browser redesign, splash screen, QoL bundle ([c1f6f18](https://github.com/cuttlefisch/mae/commit/c1f6f1880e0bac1cea0144e6c452b3f68cdd6870))
- Command-line tab-completion + SPC : binding ([2bf2f65](https://github.com/cuttlefisch/mae/commit/2bf2f651e8ee86ea34bade1e23e110e8ec066c07))
- Visual bell (Emacs visible-bell equivalent) ([7b4976e](https://github.com/cuttlefisch/mae/commit/7b4976e65162d240898bc67a3e1191a4adfb7529))
- Operator-pending mode, 14 SPC groups, project infra, Doom parity ([dfc080e](https://github.com/cuttlefisch/mae/commit/dfc080ed877ded72be64f18d6cb2861d0735d1cc))
- Ys{motion}{char} surround + linewise j/k operators ([15225c7](https://github.com/cuttlefisch/mae/commit/15225c72544bcf01452bcadbe4325fd72c539103))
- Operator count parity, motion fixes, picker/browser QoL, project switch ([e44b9ac](https://github.com/cuttlefisch/mae/commit/e44b9ac69a543ed1f3fbde3da000ced0ace84123))
- Word wrap cursor fix, display-line motions (gj/gk/g0/g$), wrap indicator ([d07053f](https://github.com/cuttlefisch/mae/commit/d07053f32565ee0b95fd95208e03b632a7d8ad56))
- Word-boundary wrapping, breakindent, configurable showbreak ([d7b23f5](https://github.com/cuttlefisch/mae/commit/d7b23f59c7e43cbed2e88cc5a9543a78200222a6))
- Mae-shell crate — terminal emulator via alacritty_terminal ([c1bbfdd](https://github.com/cuttlefisch/mae/commit/c1bbfdda64f2d5422567563a04fab5c6b289fb9e))
- Shell integration, hooks, options, bug fixes, README overhaul ([bf71c73](https://github.com/cuttlefisch/mae/commit/bf71c73206eb52855f28beddc2112c1e0762c9ab))
- MCP bridge, agent bootstrap, file auto-reload, shell Scheme functions ([f776603](https://github.com/cuttlefisch/mae/commit/f77660383c10d904fe899f4705c2bc71576ca77b))
- Configurable shell keymap, AI permissions, GUI foundation (Phase 8 M1) ([8d175de](https://github.com/cuttlefisch/mae/commit/8d175de97a3cb1b99bf7c1606e6993eab626af34))
- Wire --gui flag with feature gate and help text ([16172ce](https://github.com/cuttlefisch/mae/commit/16172ce5585d81a95c19902409b55c77b02d4c4e))
- GUI event loop via winit pump_app_events (Phase 8 M2) ([af6e4b6](https://github.com/cuttlefisch/mae/commit/af6e4b6ff2b9fc1c22e1bd8db57aa3499a75f1dd))
- Softbuffer presentation + GUI feature checklist (Phase 8 M2) ([488f393](https://github.com/cuttlefisch/mae/commit/488f393672a151a6916d7c2a5ea0b333deedd5c7))
- Shell split dimensions, agent auto-approval, ai_permissions tool ([1c348f7](https://github.com/cuttlefisch/mae/commit/1c348f7a00f21c9e9e9c43a450dfa0d401b5cb6e))
- Session-scoped MCP input lock + deferred MCP tool support ([5d9be11](https://github.com/cuttlefisch/mae/commit/5d9be116c460f96054188043a9c14742166fcec2))
- DAP debug panel + 6 new AI debug tools + self-test dap category ([f91f9ba](https://github.com/cuttlefisch/mae/commit/f91f9ba11055cff3d604a3d8b81104ec1ce46354))
- GUI render module extraction — 10 modules, 65 tests (Phase 8 M3) ([b6a7f8e](https://github.com/cuttlefisch/mae/commit/b6a7f8e1035e555b5b9b447dbd777c6dac257e57))
- Observability + AI awareness — FPS overlay, tool timing, KB debugging node ([534ce0c](https://github.com/cuttlefisch/mae/commit/534ce0cdc994acac5ebca75c0d84d7a6509d360e))
- GUI visual polish, OptionRegistry, desktop launcher, docs overhaul (Phase 8 M3) ([4242ff7](https://github.com/cuttlefisch/mae/commit/4242ff773dedc2a8b14ba55ded37a1e46a299df1))
- Debug mode, perf tools, GUI polish — font config, theme-aware shell, syntax caching ([4329193](https://github.com/cuttlefisch/mae/commit/43291935bf74482e4f7a9a8a278b59356fa60358))
- Clipboard option, theme-aware splash, perf stats, GUI polish ([f3b836a](https://github.com/cuttlefisch/mae/commit/f3b836ac09aef500b2a4339cf82f9f858c0920f2))
- GUI event loop refactor — run_app + EventLoopProxy (Phase 8 M4) ([c9584fd](https://github.com/cuttlefisch/mae/commit/c9584fd0c88430f22da58f84abdddde517444b5b))
- Get_option AI tool, shell theme fix, set_option registry sync ([d6a8464](https://github.com/cuttlefisch/mae/commit/d6a8464a68efe22e512fc6b4d9a8cd5a2c07171c))
- Editor polish, v0.3.0 — 14 features, 1,508 tests ([048bf33](https://github.com/cuttlefisch/mae/commit/048bf33b5c7532a2e9e1c5e8d0eb8e4560df5d55))
- Debugging powerhouse — watchdog, introspect, event recording, DAP attach/evaluate ([42cd5bf](https://github.com/cuttlefisch/mae/commit/42cd5bffbc49870db8c50074118d53bd0277dd9d))
- Docs, Doom init.scm, tutor KB, CI E2E, clippy fix ([df7906e](https://github.com/cuttlefisch/mae/commit/df7906e3649d758c3803840a9c0690c39f26e80d))

### Miscellaneous

- Fix clippy warnings and apply cargo fmt ([a6db8ca](https://github.com/cuttlefisch/mae/commit/a6db8cadd9318c7d0c750b1d6790c19c81a2dbf3))
- Update ROADMAP — Phase 3f M3 complete (conversation persistence) ([df87f8f](https://github.com/cuttlefisch/mae/commit/df87f8f4b405033e6bbdb7063dba6570dca1340a))
- Update ROADMAP — macros done, all Tier 1 self-hosting blockers complete ([437ae19](https://github.com/cuttlefisch/mae/commit/437ae19917e49468399dd868fa8dcfe05124434c))
- Update CLAUDE.md — current phase status and next targets ([af75252](https://github.com/cuttlefisch/mae/commit/af7525288df33d22c8ec6c89aff63a848e85b193))
- *(deps)* Update toml requirement from 0.8 to 1.1 ([6270aac](https://github.com/cuttlefisch/mae/commit/6270aacfd76e4caaf5f0a71ff4bb2bd0a854199d))
- Group dependabot updates into batched PRs ([c1096c8](https://github.com/cuttlefisch/mae/commit/c1096c899f23166d6e7d9c378321ba58bdfb7b5e))

### Performance

- Cap TUI render rate at 60fps with deferred frame timer ([48d8b5c](https://github.com/cuttlefisch/mae/commit/48d8b5c212a57cdb86f5d82642210224cb034c4e))

### Refactor

- Simplify splash to bat-only, art infra for future PR ([9d1b514](https://github.com/cuttlefisch/mae/commit/9d1b5144bc5750dfb0d800b1ee27205f54651d6e))
- Extract shared event loop helpers, simplify review, roadmap update ([1a794ca](https://github.com/cuttlefisch/mae/commit/1a794cab6e57381b3dfdcb7b292d28925374199d))

### Infra

- Add pre-commit hook for cargo fmt check ([6384406](https://github.com/cuttlefisch/mae/commit/63844067ebcd2ffa8cd7769faadd1cdfc01152e8))

### Style

- Cargo fmt ([5c8f6fe](https://github.com/cuttlefisch/mae/commit/5c8f6fe98ca2db7c4d921ce65ce68aa94bb560fd))

### Tmp

- Add theme test file for diff testing ([8c603ae](https://github.com/cuttlefisch/mae/commit/8c603aeecb7e64d2f8bf6ec8a457f251d2730f72))


