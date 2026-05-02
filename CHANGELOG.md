# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Bug Fixes

- *(tests)* Harden flaky timing-dependent tests ([84a696d](https://github.com/cuttlefisch/mae/commit/84a696da9862a368aa60d6a40ecf198a62dbe2b1))
- *(v0.6.0)* Org keymap fallback, insert undo groups, change markers, link rendering, bold baseline ([ea11b33](https://github.com/cuttlefisch/mae/commit/ea11b336f01c827dc64661d4871dc327e3cd1be2))
- *(v0.6.0)* Parse_key_seq bracket fix, GUI perf (font cache, scroll, span search) ([be50f9e](https://github.com/cuttlefisch/mae/commit/be50f9eaed585915c0da04322b1440818423f1a6))
- *(v0.6.0)* Cursor alignment on scaled headings + bottom row overlap ([37c2e8d](https://github.com/cuttlefisch/mae/commit/37c2e8dc219de8d7567bda645a3b6b8027ac660c))
- *(gui)* Fold-aware relative line numbers + cursor X alignment ([3e5c386](https://github.com/cuttlefisch/mae/commit/3e5c3864d2afb364693ae762c72fd2a37e784390))
- *(gui)* Pixel-precise cursor X, fold-aware scroll, viewport overflow guard ([88081e8](https://github.com/cuttlefisch/mae/commit/88081e8fb5934f1901049d411b1e6c7c14873a01))
- *(gui)* Use actual font glyph advance for cursor + text positioning ([1445706](https://github.com/cuttlefisch/mae/commit/144570633f47397a822a4fac8b860558fa2cfead))
- *(gui)* Pixel-precise multi-run text rendering for scaled headings ([9b61731](https://github.com/cuttlefisch/mae/commit/9b617311c20d8341e23b24b738a7634757b7a730))
- *(gui)* Scrollbar redesign + horizontal mouse scroll + toggle ([df284f4](https://github.com/cuttlefisch/mae/commit/df284f4db0c1c66239dfe5af96f94cc7624aad9a))
- *(gui)* Scrollbar thumb visibility + horizontal scroll clamping ([a2e2444](https://github.com/cuttlefisch/mae/commit/a2e2444cea407a66e80b2c1a040b4f51ffca41b1))
- *(core)* Wrap-aware scroll-up cursor clamping (C-y + mouse) ([c410639](https://github.com/cuttlefisch/mae/commit/c410639ff102e328a650abe7fa4f1e33e12eab84))
- *(core)* Fold-aware scroll — skip invisible lines in C-y/C-e/mouse scroll ([75f0ddd](https://github.com/cuttlefisch/mae/commit/75f0ddde5b425197cd90530bcf8274d8dca8b137))
- *(gui)* Heading-scale-aware viewport guard prevents cursor below viewport ([71a1bfb](https://github.com/cuttlefisch/mae/commit/71a1bfb472ff94d07c92f8eb4df8775c5883abfa))
- *(core)* Unified line_visual_rows eliminates scroll-up/guard desync ([e72cdf8](https://github.com/cuttlefisch/mae/commit/e72cdf83c246a0cf4f2271b5134d99d68eef0e0d))
- *(gui)* Place *ai* scrollbar inside window border, not overlapping it ([d05dbb6](https://github.com/cuttlefisch/mae/commit/d05dbb6c6873b75d066799cf83edce6c4fc74e7a))
- *(gui)* Render cursor on trailing empty line after newline insertion ([6a66e6b](https://github.com/cuttlefisch/mae/commit/6a66e6b05efbc4159c8baca6735f52f39b9ec47a))
- *(tui)* ConversationInput cursor follows typed text ([6a852f5](https://github.com/cuttlefisch/mae/commit/6a852f5d6b91b815eeb01094ed276523350c260c))
- *(conv)* Unified cursor/viewport via FrameLayout + command registry gap ([f8448ee](https://github.com/cuttlefisch/mae/commit/f8448ee5848906e3d2c24bda9664fbea81749a77))
- *(gui)* Conversation tilde bug, line count audit, text-width unification, horizontal scroll ([c504b04](https://github.com/cuttlefisch/mae/commit/c504b044dd763aa89b095307ce68949bec145889))
- *(self-test+scrollbar)* Max_rounds override, benchmark threshold, scrollbar clamping ([c95c327](https://github.com/cuttlefisch/mae/commit/c95c3273bbca32c5a8287719dfd76a4e4500b0d4))
- *(lsp)* Popup theming on light themes, scroll indicator artifact, KB docs + e2e tests (2,171 tests) ([0968937](https://github.com/cuttlefisch/mae/commit/096893732b3a5c1fe26b41f9a633283c0b3e2f15))
- *(gui)* Ghost line at bottom of viewport from FP drift + phantom line ([a1cfc75](https://github.com/cuttlefisch/mae/commit/a1cfc75d5c47162c301f198da864e8ef299c7558))
- *(gui)* Hover popup dismiss on click, popup gap, font reset default, status bar clipping (2,195 tests) ([9cd3d72](https://github.com/cuttlefisch/mae/commit/9cd3d72df429ddffc6c86ace96948796678db31b))

### Documentation

- *(v0.6.0)* KB nodes for org/markdown structural editing, ROADMAP update ([bcdcdc3](https://github.com/cuttlefisch/mae/commit/bcdcdc398438dd7231130e85c43e397e220a6512))
- Update ROADMAP + KB nodes for v0.6.0 Round 3 features ([b165f2c](https://github.com/cuttlefisch/mae/commit/b165f2c757a961e993facfd5a1ce0fba1438148b))
- Update README badges and feature list for v0.6.0 ([55a9077](https://github.com/cuttlefisch/mae/commit/55a9077eb71fe7a914798f7e5ece10f5f24e01aa))

### Features

- *(v0.6.0)* Chained ex commands, org structural editing, autosave, change markers, links ([4bd81d2](https://github.com/cuttlefisch/mae/commit/4bd81d2fdd3be40fd0c6021eadf4f4bede8c16e1))
- *(v0.6.0)* Help buffer heading scaling ([1f30f38](https://github.com/cuttlefisch/mae/commit/1f30f386c7c5cf9ccd22bc9fbe2b84e54ee8f63b))
- *(v0.6.0)* SPC r register access leader keys ([de12c78](https://github.com/cuttlefisch/mae/commit/de12c78c0d3603ad60e327e64be142c8acfaac89))
- *(v0.6.0)* Window resize, move, balance, maximize (SPC w +/-/=/m/H/J/K/L) ([c5b4fa5](https://github.com/cuttlefisch/mae/commit/c5b4fa5d7cf9563e1daf15689b6c78f4ecd57601))
- *(v0.6.0)* Per-message token display in conversation ([f7a28fd](https://github.com/cuttlefisch/mae/commit/f7a28fd72e4a8df2df6e38698d4c38c6fcdf3c32))
- *(v0.6.0)* Code folding (za/zM/zR) with tree-sitter fold ranges ([67a5cf6](https://github.com/cuttlefisch/mae/commit/67a5cf68fc948f0acf634470a499bdbd34dcb529))
- *(v0.6.0)* Unified diff display for AI propose_changes ([ff8a1f0](https://github.com/cuttlefisch/mae/commit/ff8a1f0c05fcdd80713ceecb2dec75293996873c))
- *(v0.6.0)* Three-state org heading cycle + fold-aware move subtree ([891c66d](https://github.com/cuttlefisch/mae/commit/891c66df73d7419d365bcc0f36ce4f5022b37c3c))
- *(v0.6.0)* Markdown structural editing parity + zM/zR for headings ([7819c0f](https://github.com/cuttlefisch/mae/commit/7819c0f6f4133ebbbf2edc06a0b2f230f827105d))
- *(v0.6.0)* Heading_scale option (toggle heading font scaling) ([1ed876a](https://github.com/cuttlefisch/mae/commit/1ed876adcdde6a95de3c08e45361898f4c762888))
- *(v0.6.0)* Narrow/widen for org and markdown (SPC m s n / SPC m s w) ([6eefac3](https://github.com/cuttlefisch/mae/commit/6eefac309690c1743b65037d16055e75346d83f3))
- *(core,gui)* V0.6.0 round 3 — framework solutions + GUI features ([61e1797](https://github.com/cuttlefisch/mae/commit/61e17976379d331837a1bfbc323bf300fd1147e6))
- *(conv)* Split-pair conversation input + clamp_cursor fix + TUI cursor shape ([4250f14](https://github.com/cuttlefisch/mae/commit/4250f14068e5fa7eb2abc6edfe0c8c8a60354eae))
- *(keys)* Doom which-key parity + file tree enhancements (1,915 tests) ([0f9dd71](https://github.com/cuttlefisch/mae/commit/0f9dd71c6998f27ea528a54d1ee8151f634aaf48))
- *(git+kb)* File tree git markers, gutter diff indicators, enriched help (1,995 tests) ([f8e18fa](https://github.com/cuttlefisch/mae/commit/f8e18fa3fa76b6e978405584898caf7439672c9d))
- *(render+tree)* Unified markup rendering, help span fix, file tree keymap (2,007 tests) ([a962ebb](https://github.com/cuttlefisch/mae/commit/a962ebb5455ac1e4a67c21179d53514395954b96))
- *(options)* Per-buffer word-wrap via BufferLocalOptions + :setlocal + CI clippy fix (2,011 tests) ([8e99718](https://github.com/cuttlefisch/mae/commit/8e9971892036b2d77abef845be181077fa9ecc4b))
- *(links+options+selftest)* Clickable links (gx), expanded buffer-local options, atomic self-tests (2,018 tests) ([22a7b02](https://github.com/cuttlefisch/mae/commit/22a7b02932ca1b61f19f0920aab28e5e5387fe46))
- *(display)* Display overlays for link concealment in text buffers (2,081 tests) ([6cea050](https://github.com/cuttlefisch/mae/commit/6cea0508f127372afd13bbc9ca04d5ff2547feaf))
- *(display)* Cursor sensors + org-appear reveal + Tab link navigation (2,091 tests) ([988ab1f](https://github.com/cuttlefisch/mae/commit/988ab1f43ef663d3317690a0275dcf47d24332d8))
- *(core)* BufferView enum + BufferMode trait + enhanced git status (2,091 tests) ([cd478c2](https://github.com/cuttlefisch/mae/commit/cd478c21e29d8b02ab6976897bcd7c78dfc27fd1))
- *(ux)* Keymap overlay architecture + Magit parity + which-key discoverability (2,039 tests) ([9f87e6b](https://github.com/cuttlefisch/mae/commit/9f87e6b5dddee27240a21765bcbb1627931c93da))
- *(core)* Swap files + rendering dedup + code map (2,066 tests) ([2c4030c](https://github.com/cuttlefisch/mae/commit/2c4030c6be10e56ea69b334075bd12d5751b94a2))
- *(core)* File-type hooks + display optimization + variable-height polish + code block fix (2,103 tests) ([4e3ada3](https://github.com/cuttlefisch/mae/commit/4e3ada3a8f748e9d8923652c39e3c4636e5343f3))
- *(lsp)* Hover popup, inline diagnostics, code actions, config discovery (2,165 tests) ([51b84d9](https://github.com/cuttlefisch/mae/commit/51b84d9f408f87c0a65f35f0c8fab168dc70683c))
- *(lsp)* Popup hints, loading feedback, enriched status indicator (2,175 tests) ([67162ab](https://github.com/cuttlefisch/mae/commit/67162ab996fa2ce86e33cf26f65cfa431f4a2042))
- Full audit — LSP fix, 12 configurable options, Scheme API, package system, KB docs (2,252 tests) ([0e8f88f](https://github.com/cuttlefisch/mae/commit/0e8f88f7733c2e805a583f2fe3e5ef8f788e9777))

### Performance

- *(v0.6.0)* Eliminate per-frame syntax span cloning via Arc ([20e2dca](https://github.com/cuttlefisch/mae/commit/20e2dcab322075acb962f350cf18a125f94d3086))
- *(v0.6.0)* Font pre-scaling cache in SkiaCanvas ([19f56a6](https://github.com/cuttlefisch/mae/commit/19f56a672878ded17072156ca9c22d56452a74e8))
- *(v0.6.0)* Incremental syntax reparse via Tree::edit() ([ac1c099](https://github.com/cuttlefisch/mae/commit/ac1c0995f78ce77430b9d77446de27d7fcb66f3c))

### Refactor

- *(v0.6.0)* Extract render_common shared module, deduplicate GUI/TUI rendering ([144e80b](https://github.com/cuttlefisch/mae/commit/144e80b5b87b725a914bb96ecc32415de35e02cd))
- *(v0.6.0)* Extract debug render_common, optimize Theme::style() hot path ([6961773](https://github.com/cuttlefisch/mae/commit/696177352fea58f17bf7afa156593f1eeb432cc7))
- *(v0.6.0)* Deduplicate Buffer and Editor constructors ([1c4e019](https://github.com/cuttlefisch/mae/commit/1c4e01957944ce5cd1084e13c90e7a3fedb25ba6))
- *(v0.6.0)* Extract shared color utilities (parse_hex, luminance) ([f343d08](https://github.com/cuttlefisch/mae/commit/f343d08a3fbb1e9aec4348389413bec030494e58))
- *(v0.6.0)* Extract dispatch.rs helper methods, remove copy-paste ([66167aa](https://github.com/cuttlefisch/mae/commit/66167aa31e59550fa7a8423dfc00101f9c3baeb5))
- *(v0.6.0)* Modularize dispatch.rs into 10 submodules ([379957b](https://github.com/cuttlefisch/mae/commit/379957bbfd1c2ffb0ed91770ea2ce4db203eff02))
- *(gui)* Extract FrameLayout as single source of truth for text positioning ([3102a01](https://github.com/cuttlefisch/mae/commit/3102a01a5519258acce1ed898c5466b235a8a8e9))
- *(gui)* Remove heading_extra_rows() — popup uses FrameLayout directly ([4b0fae6](https://github.com/cuttlefisch/mae/commit/4b0fae6952f5656ba04da7961f6d6426a1396f9d))
- *(core)* Audit cleanup — remove dead fields, dedup git ops, add mode_theme_key() (2,033 tests) ([9cd71fd](https://github.com/cuttlefisch/mae/commit/9cd71fd74d924989f159b62926a2a7dbff603e1e))
- *(core)* Structural fixes — dedup, type safety, file-type hooks, span sharing (2,047 tests) ([f94b8b2](https://github.com/cuttlefisch/mae/commit/f94b8b25c4f6f2d9f52d55f3e717bd1569ca4221))
- Split syntax.rs into 7 submodules, add BufferKind::Diff, split executor into submodules (2,243 tests) ([e1012ce](https://github.com/cuttlefisch/mae/commit/e1012ce35d0a497828b8030d074cb1bb9c574980))

### Testing

- *(v0.6.0)* AI guidance self-test category (keybindings, windows, themes) ([3e22ad9](https://github.com/cuttlefisch/mae/commit/3e22ad948d875b164d32e549adc0a0ab90fb8d43))
- *(v0.6.0)* Regression tests for keymap fallback, change markers, link rendering ([5048a0a](https://github.com/cuttlefisch/mae/commit/5048a0a5abfcdcea0c941a31e88c029b18da8e41))

## [0.5.1] - 2026-04-28

### Bug Fixes

- *(ci)* Version-bump workflow — target root Cargo.toml, handle tag conflicts ([f612752](https://github.com/cuttlefisch/mae/commit/f6127520bedf1fd33f107ed7850721bdeba83cf3))
- *(v0.5.1)* Hardening, config error surfacing, docs update (1,673 tests) ([a94893d](https://github.com/cuttlefisch/mae/commit/a94893da69c1a7d6119882de56136422139812a7))
- *(v0.5.1)* Block visual I, undo grouping, search perf, range :s, :set completion ([71dcee3](https://github.com/cuttlefisch/mae/commit/71dcee331c851a1309721a1533865d50f64b6c13))
- *(v0.5.1)* Substitute undo grouping, search highlight drift, command cursor ([a358fd4](https://github.com/cuttlefisch/mae/commit/a358fd4157e70b4d68d99a005bb8b9e0ce1eb0a7))
- *(v0.5.1)* Debounced syntax reparse, HighlightConfig cache, deduplicated render path ([7c7a68e](https://github.com/cuttlefisch/mae/commit/7c7a68e5f72f1847e1620495ee523f18cde95012))
- *(v0.5.1)* Cached lazy theme resolution, scaled heading overflow, roadmap updates ([cda8475](https://github.com/cuttlefisch/mae/commit/cda847541f674264b97c423014804f1914600ffe))
- *(ci)* Use RELEASE_PAT for version bump workflow ([5835548](https://github.com/cuttlefisch/mae/commit/5835548e66693a91c2d2b940dc1705e32d5c76d2))

### Features

- *(v0.5.1)* Ghost cursor fix, status bar overhaul, vim parity, AI help ([479e5fd](https://github.com/cuttlefisch/mae/commit/479e5fd62e5c814f59e6e3a25dd86a17444ee5bd))
- *(gui)* Org heading tiered scaling, cursor/cursorline fixes, roadmap additions ([7a6807e](https://github.com/cuttlefisch/mae/commit/7a6807e1c59c73d933af617bcc1754fe8a373df4))
- *(gui)* Pixel-based variable-height line rendering ([69801c3](https://github.com/cuttlefisch/mae/commit/69801c3221f005f64068971623d648fa6e55b69c))

### Miscellaneous

- Bump version to 0.5.1 ([585a7a0](https://github.com/cuttlefisch/mae/commit/585a7a0f1f0a60377d7a0d715ec7df0660286bea))

## [0.5.0] - 2026-04-26

### Bug Fixes

- *(ai)* Fix infinite loop via context protection and double-esc state cleanup; add regression tests ([4c4d36e](https://github.com/cuttlefisch/mae/commit/4c4d36ec4edd14d0c595ff9c4b86b4424b98bdd3))
- *(ai)* Update AiEvent::Error signature and fix tests ([a7ae7ba](https://github.com/cuttlefisch/mae/commit/a7ae7bafb87c1f7d3fcd363a54f90d8c45cd4cb0))
- *(ai)* Enforce max_rounds, fix oscillation detection, soften pruning ([a82fc1e](https://github.com/cuttlefisch/mae/commit/a82fc1ec8a6597d4819c6dfb0a2ba4382959f089))
- *(ai)* Fix mode system — default, keybinding, status bar, enforcement ([965bb32](https://github.com/cuttlefisch/mae/commit/965bb32242d1c41d89713d0cdd0cbf0cde4ed12b))
- *(ai)* Clean up executor tool names, centralize AI_PROFILES constant ([a9adfc5](https://github.com/cuttlefisch/mae/commit/a9adfc54ca5a2f5968ef11712169208fc8779634))
- *(ai)* Redesign prompts for model-agnostic, weak-model-friendly use ([ebe94a3](https://github.com/cuttlefisch/mae/commit/ebe94a340cef78595832de5540aafac57257313d))
- *(ai)* Lower max_rounds for weak models, fix trim_messages orphan stripping ([7e297bc](https://github.com/cuttlefisch/mae/commit/7e297bcde8df91ebed8c3b0cadf8054e32761e5b))
- *(ai)* Code smell audit — 12 fixes across providers, session, executor ([8594292](https://github.com/cuttlefisch/mae/commit/8594292c1a4530773c94f0aeb6655a2a27eab878))
- Gitignore recursive agent dirs, add GEMINI.md, XDG transcript path ([9c7a451](https://github.com/cuttlefisch/mae/commit/9c7a4515892afc2915899c60a98aab0993103450))
- Conversation G/gg scroll, yank-file-path clipboard, command_list bloat ([17fcdd6](https://github.com/cuttlefisch/mae/commit/17fcdd619eda16555d61653b9117d1ebf6f56e7c))
- *(self-test)* Prevent agent context bloat loop during self-test ([6eecebe](https://github.com/cuttlefisch/mae/commit/6eecebe272c915b6dc8a93acf9332aa8c28d58fc))
- *(conv)* 4 conversation buffer UX bugs — scroll, status bar, perf ([7d94fa9](https://github.com/cuttlefisch/mae/commit/7d94fa9f4cccc998374464a2e5a4cf15101d69ea))
- *(gui)* Prevent div-by-zero in screen_line_count with narrow windows ([ac03935](https://github.com/cuttlefisch/mae/commit/ac039350129fa9eb03fbebd5cf063e7cfc584c3f))
- *(ai)* Bump DeepSeek max_rounds from 25 to 50 ([f4430f1](https://github.com/cuttlefisch/mae/commit/f4430f17704f6ffeec0a200a1b84ec7c47214ea6))
- Adjust ai_target_buffer_idx when buffers are removed ([f1fe4c4](https://github.com/cuttlefisch/mae/commit/f1fe4c4854e036298a9fb1efa47d18a3f43c899e))
- *(ai)* Reload buffer from disk in create_file when file already open ([017a7f8](https://github.com/cuttlefisch/mae/commit/017a7f8383f3665c8ba40aa6a8d4fcb2ab4ce60a))
- *(ai)* Self-test plan v2 — setup/cleanup per category, anti-loop instructions ([e401d4d](https://github.com/cuttlefisch/mae/commit/e401d4de0c29c3928e367b570e395ff9685be842))
- *(gui)* Conversation scroll, idle CPU, regression tests ([138afba](https://github.com/cuttlefisch/mae/commit/138afbae2d7627141c542e86ab63064b4b93b699))
- *(ai)* Tool visibility — 27 tools were invisible to the agent ([b2a41e2](https://github.com/cuttlefisch/mae/commit/b2a41e2744cee020140804be1ced4b62587d2328))
- *(self-test)* Add test fixtures + rewrite LSP/DAP test plans for real coverage ([362753e](https://github.com/cuttlefisch/mae/commit/362753ed6a128822bf72e8a3ff0215e902b876ea))
- *(self-test)* Make test_fixtures a workspace member for full LSP indexing ([526284f](https://github.com/cuttlefisch/mae/commit/526284f82b1f5ea268688b813f95cfeaa0a3b4db))
- *(dap+messages)* Stop_on_entry for DAP self-test, Messages buffer audit ([19afe83](https://github.com/cuttlefisch/mae/commit/19afe835c1cf49704f841e3206e1439e8d3a4c08))
- *(messages+dap)* Messages scroll_offset semantics, DAP self-test reliability ([8ff749d](https://github.com/cuttlefisch/mae/commit/8ff749d0bf19763ab022e9ecde286a7f70e2b98b))
- *(dap+agent)* DAP audit — adapterID fix, stop_on_entry resolution, read_messages tool, word wrap scroll ([d1fded1](https://github.com/cuttlefisch/mae/commit/d1fded193a35385330386275a6f84f43c0d2daf6))
- *(dap)* Remove spurious "request" key from launch args, fix initialized event ordering ([db07004](https://github.com/cuttlefisch/mae/commit/db07004afd6fe4c0922f69a85a39a8cc31725718))
- *(dap)* Deferred launch response to prevent debugpy deadlock, log rotation ([b059f7a](https://github.com/cuttlefisch/mae/commit/b059f7adbb86ded5280772be81f6b0b93a74e4ec))
- *(dap)* Observability — enriched timeouts, protocol tracing, agent failure guidance ([d743eb7](https://github.com/cuttlefisch/mae/commit/d743eb74cf6595b4ecc11be72e2875fd3b4c33b1))
- *(gui)* C-o insert oneshot — add status indicator and tests ([533de70](https://github.com/cuttlefisch/mae/commit/533de70694d9a7f7fbd4f247993dd0c56bcb612e))

### Documentation

- Update ROADMAP, README, CLAUDE.md for v0.4.1 modularization ([6e815e1](https://github.com/cuttlefisch/mae/commit/6e815e17ccc900b3e162c3b5bfc1bebe2ec33d45))
- Update GEMINI.md with v0.5.0 test count and missing crates ([a704de4](https://github.com/cuttlefisch/mae/commit/a704de4ddbebf6c72dd44fe39232261a785dfafd))
- *(kb)* Fix DAP tool names, add tool architecture + missing self-test categories ([8a5d7df](https://github.com/cuttlefisch/mae/commit/8a5d7dfd6502f666f813a560afcf2e10fdb5d296))
- *(roadmap)* Update v0.5.0 items, mark completed v0.6.0, fix tool counts ([b8790f9](https://github.com/cuttlefisch/mae/commit/b8790f912ac65e936a57dc0fbf8e5079240d662e))
- Expand Getting Started with prerequisites + AI setup, add CONTRIBUTING.md ([3f0325e](https://github.com/cuttlefisch/mae/commit/3f0325e79972e52e9004e537d1209dc2605fe13c))
- LSP self-test retry guidance, dev dependencies in CLAUDE.md ([6f1b915](https://github.com/cuttlefisch/mae/commit/6f1b915a4f974d2840568d7bf2e8afeb843b75c0))
- Update test counts (1,641), LOC badge (~82k), v0.5.0 summary ([dd9db98](https://github.com/cuttlefisch/mae/commit/dd9db98bd2e12833ff9873b8640c2f98f3edf4fa))
- Mark C-o insert mode as complete in ROADMAP ([1945763](https://github.com/cuttlefisch/mae/commit/1945763facb481714e8e74fd18a1704f42e3b5ef))

### Features

- *(ai)* Advanced buffer UI, infinite tool loop, and KB exploration guardrails ([fc3e623](https://github.com/cuttlefisch/mae/commit/fc3e6234065349f8f76b0a062aee964bf6636763))
- *(ai)* Add SOPs and workflow hints for improved multi-tool reasoning ([1d65cc6](https://github.com/cuttlefisch/mae/commit/1d65cc69d2e0ef366450e64061addd8a47604aae))
- *(ai)* Gemini provider support, loop protection, and transcript logging ([2344ca6](https://github.com/cuttlefisch/mae/commit/2344ca637ecda37912a40c26e80c9e9614c5ae1e))
- *(ai)* Progress checkpoint system + watchdog recovery (v0.5.0) ([995a628](https://github.com/cuttlefisch/mae/commit/995a628afdc60911cb39b15c97947eab3b875913))
- *(ai)* Enable Claude prompt caching for system prompt + tools ([d1eebaf](https://github.com/cuttlefisch/mae/commit/d1eebaf6f61a66c729f06ea5efd386e8904ee112))
- *(ai)* Token dashboard, context compaction, graceful degradation, web_fetch (v0.5.0) ([75074a2](https://github.com/cuttlefisch/mae/commit/75074a2a9a22c03dc45f467d814a35b911e9d077))
- *(theme)* Add light-ansi and dark-ansi ANSI-only themes ([44087bc](https://github.com/cuttlefisch/mae/commit/44087bcdadf2d379d108cf3662333cc64ae840f5))
- C-e/C-y scroll, C-o insert oneshot, git stash/branch tools, ai-status metrics (v0.5.0) ([9caa50a](https://github.com/cuttlefisch/mae/commit/9caa50a339c29ef3a9f141f81ebc56d475078619))
- Perf + CJK rendering + self-test budget fixes ([42d5091](https://github.com/cuttlefisch/mae/commit/42d509193ab144df0f781fdeda66b314694df4dd))
- V0.5.0 — compaction redesign, regression fixes, 25 new CI tests ([a67666b](https://github.com/cuttlefisch/mae/commit/a67666bebad202f4ad08371e595a4c9df9acfc28))
- *(ai)* Workflow tracker — compaction-resilient progress for multi-step tasks ([a3a1fee](https://github.com/cuttlefisch/mae/commit/a3a1feee74d170528f864b55ae0b44bb4ebcc309))
- *(ai+gui)* Self-test reliability, tool display, AI buffer perf ([2638854](https://github.com/cuttlefisch/mae/commit/26388546f8c5476e6ab197e7eae58695a2acd1df))
- *(ai)* Editor_save_state / editor_restore_state tools ([be76722](https://github.com/cuttlefisch/mae/commit/be76722a27bf0d1cd7326e13f5a01743824f24dd))

### Miscellaneous

- Bump version to v0.4.1, add .mae to gitignore ([a4b7795](https://github.com/cuttlefisch/mae/commit/a4b7795e7881b867695cba72a4a3a417d87511fa))

### Performance

- *(conv)* Eliminate O(N) bottlenecks in conversation buffer rendering ([aecd6c7](https://github.com/cuttlefisch/mae/commit/aecd6c71e8c734ef39dbfd3f169af39f237b5977))
- *(gui)* Text run batching + C-e/C-y scroll fix ([d06655c](https://github.com/cuttlefisch/mae/commit/d06655c79ac80fae24028c51f35235fbf27edd15))
- *(gui)* Display optimization — input-pending, layout fix, CJK correctness ([dedbf20](https://github.com/cuttlefisch/mae/commit/dedbf205690dc1db8cc2fb134c65c9e1dd37bd56))

### Refactor

- *(core)* Split 4458-line tests.rs into 14 focused test modules ([ab46919](https://github.com/cuttlefisch/mae/commit/ab4691995664cb7b858507f8531bd8431ba8dc11))
- *(mae)* Split 2056-line key_handling.rs into 10 mode-specific modules ([fb01916](https://github.com/cuttlefisch/mae/commit/fb01916b8705465b34c5d0ceb675143a2f84ba9b))
- *(ai)* Split tools.rs and executor.rs into module directories ([5da52f0](https://github.com/cuttlefisch/mae/commit/5da52f0b18b1ff88bef1a6aa9a2e04a916346625))
- *(mae)* Extract terminal_loop, lsp_bridge, dap_bridge, shell_keys from main.rs ([475e55e](https://github.com/cuttlefisch/mae/commit/475e55e3db444dad0484bae84c0a313e7165d6ef))
- *(ai)* Split session.rs (2791 lines) into session/ directory ([67156fc](https://github.com/cuttlefisch/mae/commit/67156fc56e7dfe8c632cec50188868000f657e1f))

### Testing

- *(ai)* Add regression tests for mid-flight compaction, UI events, and log_activity ([029023e](https://github.com/cuttlefisch/mae/commit/029023e46574cbf59f2aa2a021917970bcad52b6))

### Build

- Add setup-dev script + make target for dev dependency installation ([9fe969b](https://github.com/cuttlefisch/mae/commit/9fe969b7283531ca711fe3c05659107d484f66b2))

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


