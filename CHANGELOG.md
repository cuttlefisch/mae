# Changelog

All notable changes to this project will be documented in this file.

## [0.14.119] - 2026-08-28




### Features

- *(kb)* Give the cutover commands Scheme, keymap and help parity ([6092a3c](https://github.com/cuttlefisch/mae/commit/6092a3cbd9483a6e2a4e3868b31afa0c31c5b302))

### Bug Fixes

- *(daemon)* Expand ~ and $HOME in configured paths, and name one that cannot resolve ([8a338a8](https://github.com/cuttlefisch/mae/commit/8a338a8b6f209c739205200ab4acea7aa921f4b0))

### Documentation

- *(adr)* Record the KB cutover (ADR-110), and fix the checksum that could not attest it ([81cb5f1](https://github.com/cuttlefisch/mae/commit/81cb5f1f61b571211e3e6b8d060aa2089baa933b))
- *(manual)* The shipped corpus said the store was disposable — it is now the KB ([62bcdef](https://github.com/cuttlefisch/mae/commit/62bcdef0299d6d71f075a01397f388555b41d514))

### Miscellaneous

- Bump version to 0.14.119 ([e82fb87](https://github.com/cuttlefisch/mae/commit/e82fb87621101c75f643f9f7e9ec6f8dcd36d711))

## [0.14.118] - 2026-08-28




### Miscellaneous

- *(deps)* Bump the rust-dependencies group in /daemon with 3 updates ([6861f3e](https://github.com/cuttlefisch/mae/commit/6861f3e583fb6fd7406221b90b1a37aa7459fa9c))
- Bump version to 0.14.118 ([50f91ca](https://github.com/cuttlefisch/mae/commit/50f91ca8196c96aced45348326ee6c847d04cbc0))

## [0.14.117] - 2026-08-27




### Features

- *(kb)* `:kb-new` — create a KB that never had an org directory ([c906e27](https://github.com/cuttlefisch/mae/commit/c906e2786d13a1b307890962b4e18c03072282a8))
- *(kb)* Images resolve in a node buffer, and the audit commands stop lying ([a78341b](https://github.com/cuttlefisch/mae/commit/a78341bd01d5de6e5c74656f02573ad1008045ee))

### Miscellaneous

- Bump version to 0.14.117 ([8f05112](https://github.com/cuttlefisch/mae/commit/8f05112b9b0b81f3df73e92f5c3ecaff33f80249))

## [0.14.116] - 2026-08-27




### Features

- *(kb)* Guard the HUMAN paths into a detached KB's archive, from one place ([1755ae2](https://github.com/cuttlefisch/mae/commit/1755ae28a6e28b68f8f022dee0e94661597b53ce))
- *(kb)* `:kb-retire-archive` — the step that finishes a cutover ([6551457](https://github.com/cuttlefisch/mae/commit/6551457e5a80f86c57c916785b2f972dcfaf1a43))

### Bug Fixes

- *(kb)* Stop two write paths from silently writing into a detached KB's stale archive ([7bba114](https://github.com/cuttlefisch/mae/commit/7bba114c2537f5eb77c18e62bc9c599cfd4676a3))
- *(e2e)* Release the two-tenant editors on a condition, not a fixed sleep ([cf08915](https://github.com/cuttlefisch/mae/commit/cf08915f06c02b69064c8626a758a130d74e256a))
- *(daemon)* Route a KB's store by uuid, not by the `primary` flag ([26f283f](https://github.com/cuttlefisch/mae/commit/26f283fc718c8f283133d9c0caddcb3be10cf6bb))
- *(kb)* A KB's org dir is often a whole repo — claim only files it imported ([e6959ba](https://github.com/cuttlefisch/mae/commit/e6959ba608552c8e3a75c12445ebc917ce7688fd))
- *(test)* Build the retirement fixture's paths component-wise (Windows) ([817514e](https://github.com/cuttlefisch/mae/commit/817514e2de7a127ba2e8e6c24ba1ac0d8fe12003))
- *(kb)* Close three paths the first pass missed ([5dbd6e7](https://github.com/cuttlefisch/mae/commit/5dbd6e797556f32fc5c7402972761967ea1d2a98))

### Miscellaneous

- Bump version to 0.14.116 ([a68eb78](https://github.com/cuttlefisch/mae/commit/a68eb78190ad581ac9098965dee7fdbf68f75a0e))

## [0.14.115] - 2026-08-26




### Security

- *(security)* Close the yrs length-prefix allocation bomb on the sync/diff read path ([91419f8](https://github.com/cuttlefisch/mae/commit/91419f86b19a1001e8839b9adf9cfedc76105929))
- *(security)* Carry y-crdt PR #644 to close the decoder's UTF-8 UB, and scan the daemon workspace ([b300172](https://github.com/cuttlefisch/mae/commit/b300172650674d779518ab41aee31818bcf067d6))

### Features

- *(kb)* Give KB identity a real predicate, not a name comparison (ADR-105 D4) ([6335063](https://github.com/cuttlefisch/mae/commit/63350634f569d4f8f3400e77dfa15805a4bdd390))
- *(kb)* A KB syncs under its own minted id, not its display name (ADR-105 D4) ([d04cf8b](https://github.com/cuttlefisch/mae/commit/d04cf8b4cc316860cc7a374db79478a52f594bde))
- *(daemon)* Migrate legacy kb: node documents to per-KB addresses (ADR-105 Stage 4) ([09c782e](https://github.com/cuttlefisch/mae/commit/09c782e019ea614502e73e49423d20fd07ce5626))
- *(kb)* Give snapshot_version its first production caller, and settle node_versions residency (ADR-106) ([a7b8a1c](https://github.com/cuttlefisch/mae/commit/a7b8a1c8fa475aaebe1a09612d7f9f16fd442738))
- *(kb)* Per-instance ingest policy — a detached KB's store is the source of truth (Phase 1) ([06cad3d](https://github.com/cuttlefisch/mae/commit/06cad3d647475b019631baaf51689055763e7832))
- *(daemon)* Wire ADR-032 checkpoints to a real surface, and prove rollback (#632) ([28d6fa1](https://github.com/cuttlefisch/mae/commit/28d6fa1aaa8515436e3c9f399f360b1af0641bd4))
- *(kb)* Put sqlite stores into WAL mode, out of band ([096441c](https://github.com/cuttlefisch/mae/commit/096441ce8dac2129ef6ce142cf968b15d9917662))
- *(daemon)* Give the unsigned-content-op migration a lever and an exit criterion ([3f2847d](https://github.com/cuttlefisch/mae/commit/3f2847d3c18be64a9899878cd41be01d3b3eb382))
- *(kb)* Retire sled as a default — sqlite is the backend (ADR-108, C6) ([9deed9c](https://github.com/cuttlefisch/mae/commit/9deed9c2a01722ed5fd7941da795b42c50637e29))
- *(kb)* One capability model, and the conformance suite it makes writable (C1) ([6c51822](https://github.com/cuttlefisch/mae/commit/6c51822d70968b31e0bcb1c4952833a3cbbaa98f))
- *(daemon)* Close four network-surface gaps — links, neighborhood, titles (D1a) ([a03d618](https://github.com/cuttlefisch/mae/commit/a03d618fdb0e31e78b60b7f48cb31ee7a9b40a23))
- *(kb)* Dailies and capture work with no .org directory (Phase 3) ([16b9a6f](https://github.com/cuttlefisch/mae/commit/16b9a6f7227545c5dfe9ce591f676a16f352403e))
- *(sync)* The node-rebirth op — a signed, owner-only growth bound (ADR-107) ([01473a6](https://github.com/cuttlefisch/mae/commit/01473a6e96a84dbdfb3de2bc160352c17f3cda05))
- *(sync)* Rebirth a node, and MEASURE the growth bound it claims (ADR-107) ([4e5eb83](https://github.com/cuttlefisch/mae/commit/4e5eb83d5b5634a13f576a07d42c1105049c51ac))
- *(daemon)* The rebirth adoption path — replace, never merge (ADR-107) ([e3805d5](https://github.com/cuttlefisch/mae/commit/e3805d5678c052ed7cdaea8f7fb9f662b1fcabcd))
- *(kb)* Close three network gaps with no new endpoints, and name the two that never close (D1b) ([7ab04fd](https://github.com/cuttlefisch/mae/commit/7ab04fd109bd28cb394239fca69c82d0d358846d))
- *(ai)* File_read refuses a detached KB's stale archive (Story C) ([757342a](https://github.com/cuttlefisch/mae/commit/757342a914df4ba0ec397ddcbdc03f5a04514fbc))
- *(kb)* An independent import census, because counters demonstrably lie (Story D) ([449b540](https://github.com/cuttlefisch/mae/commit/449b540e43d1521d4da937e6bf6788f8e36ddd38))
- *(kb)* Stable project-KB identity — mint an opaque id, derive nothing (Story B) ([ad9f623](https://github.com/cuttlefisch/mae/commit/ad9f623ff2ce4ff68c12e58617c2419044248a05))
- *(kb)* The import audit — assess before, reconcile after, report into the destination (Story D / R8) ([8a12d9a](https://github.com/cuttlefisch/mae/commit/8a12d9aabe766f201c151b6856554a09f825b69d))
- *(kb)* A project KB is keyed on a durable identity, not a path (Story B / R11) ([182bdbc](https://github.com/cuttlefisch/mae/commit/182bdbccbf2a9b6fb303ea25a1efb438f5a86361))
- *(kb)* Close the last three closeable network-parity gaps (D1b) ([ccc2d46](https://github.com/cuttlefisch/mae/commit/ccc2d46e6ae5b406d5e6ade034cb62cd13d5198b))
- *(kb)* Edit a KB node as its org source text (ADR-092 D3/D5) ([7579daa](https://github.com/cuttlefisch/mae/commit/7579daa9b588ea578660c46e463b866c8811e010))
- *(kb)* Surface a member's replication policy, not just its risk signal (ADR-067) ([d904078](https://github.com/cuttlefisch/mae/commit/d904078990af342b46132913b0f61f3f279594d6))

### Bug Fixes

- *(daemon)* Kb/list must not enumerate KBs the caller cannot read (#653) ([650d086](https://github.com/cuttlefisch/mae/commit/650d086eb39716966dd7d87eec39c710d4292748))
- *(daemon)* Enforce per-tenant quotas on the collab and OAuth listeners (#456) ([7e3276d](https://github.com/cuttlefisch/mae/commit/7e3276db86f8bf6820e6fa99527382580c3a8589))
- *(kb)* Address KB node documents per-KB (ADR-105 Stage 2, closes #718) ([51a673d](https://github.com/cuttlefisch/mae/commit/51a673d04a0e50f8052ef61fb30dc20a2aa8dc96))
- *(test)* Repair two more assertions ADR-105 Stage 2 left stale ([b45d532](https://github.com/cuttlefisch/mae/commit/b45d5328f21de2d3b177b4dd913a79660ea0476c))
- *(kb)* Enforce that a KB id is addressable (ADR-105 D3) ([478fbba](https://github.com/cuttlefisch/mae/commit/478fbbacaf0127f362c183276333d0e814d60a6e))
- *(kb)* A KB id belongs to whoever shared it first (ADR-105 D5) ([7e7a81d](https://github.com/cuttlefisch/mae/commit/7e7a81d87f973dced54e7915b074c4ec1a580536))
- *(kb)* Make a colliding KB collab id recoverable, not permanent (ADR-105 D4/D5) ([ce3b30d](https://github.com/cuttlefisch/mae/commit/ce3b30df501094358d82e3ddff2220c54bd4754f))
- *(kb)* Resolve a typed KB name to its collab id, and stop reads creating KBs ([4d81f91](https://github.com/cuttlefisch/mae/commit/4d81f914a091ef4843f000447ae2e2f6acb7cfee))
- *(kb)* The must-exist guard must not mistake an evicted KB for a missing one ([22e418a](https://github.com/cuttlefisch/mae/commit/22e418a4b96a42ed1d30e528ae2ece6a81d60637))
- Arm the watchdog on the event loop, and stop reads creating durable docs ([759852e](https://github.com/cuttlefisch/mae/commit/759852e76a54af44079019f140174e4a2bbd0745))
- *(kb)* Reading a node no longer reverts it to its .org file (#729) ([f8ec1d0](https://github.com/cuttlefisch/mae/commit/f8ec1d0b242ea27a9ed1f0cfd09d2e788340b035))
- *(kb)* Make projection drift detectable and stop two silent KB-blanking paths ([0ee720f](https://github.com/cuttlefisch/mae/commit/0ee720f10c43aa13abd819da820225b6a8c0a74b))
- *(kb)* Do not snapshot when an ingest overwrites SEEDED content ([507c867](https://github.com/cuttlefisch/mae/commit/507c86705b96466f9366518d58981a12de90324b))
- *(test)* The headless socket wait had negative margin against its own operation ([09a7615](https://github.com/cuttlefisch/mae/commit/09a7615b7b4938639bf55b4b9fe34ca6579c0e16))
- *(daemon)* Doctor reported a hardcoded yrs version that had drifted to a lie ([bfa9ffc](https://github.com/cuttlefisch/mae/commit/bfa9ffc0a79a6325829d49da1d1bef3f19a61a04))
- *(sync)* Stop rewriting the constant schema_v key on every field change ([c1c54a7](https://github.com/cuttlefisch/mae/commit/c1c54a794ff932f18d6156b39334a3ce746cfe80))
- *(kb)* Carry NodeSource in the CRDT payload so provenance survives sync (#710) ([2501d4a](https://github.com/cuttlefisch/mae/commit/2501d4aacfffefb88e42f8de8d6707564b37ae3f))
- *(kb)* Stamp created_at once in the node document instead of resetting it on every write ([f6551ba](https://github.com/cuttlefisch/mae/commit/f6551bac33c0e2ffd6d51a876a50bf504cd0e73a))
- *(kb)* External URLs are links in the text, not edges in the graph ([9aae036](https://github.com/cuttlefisch/mae/commit/9aae036e1f5db34e17e4880d6a6506892045f7aa))
- *(e2e)* Stop three scripts aborting before the guard written for that case ([b2312c5](https://github.com/cuttlefisch/mae/commit/b2312c57b2c4f98c1cdc27c47567817e0fa38de8))
- *(daemon)* Route dispatch through a typed Method so authorization cannot be forgotten ([39a14f8](https://github.com/cuttlefisch/mae/commit/39a14f8ce8218e4bcc55403ef09564597ffd4165))
- *(kb)* Close the two Datalog injections reachable at ReadOnly tier ([7114df7](https://github.com/cuttlefisch/mae/commit/7114df7771dc1c521c33e684e1c0b7af8fe6ed39))
- *(kb)* Stop reporting "deleted" for a node this replica merely does not hold ([9bb120e](https://github.com/cuttlefisch/mae/commit/9bb120e36b6b54dc02e0f5ec83cd941aa237f83b))
- *(kb)* Bind agenda and prefix filter values instead of escaping them by hand ([9eef8ab](https://github.com/cuttlefisch/mae/commit/9eef8ab6ea59c26777f70225b60c10c5696d5e18))
- *(kb)* Clean up and refuse to promote WAL sidecars ([10b686e](https://github.com/cuttlefisch/mae/commit/10b686ef46440983318a9314f55a0162ba7a2ae1))
- *(kb)* Checkpoint a staged store's WAL instead of refusing to promote it ([e52e0f5](https://github.com/cuttlefisch/mae/commit/e52e0f5e08d878ba7ccc582ca9b6afbe08918201))
- *(kb)* Make the vector path work at the shipped model's dimension (D2) ([6becfc4](https://github.com/cuttlefisch/mae/commit/6becfc4545c69bc671590ea52bbd0fb0851145d6))
- *(ai)* A command mirror may never be a weaker route than its tool (C3) ([f9825c1](https://github.com/cuttlefisch/mae/commit/f9825c17414f3498d37e6db289d23b97516689e5))
- *(e2e)* Wait for the canary docs to arrive, and stop clamp from panicking ([8c86450](https://github.com/cuttlefisch/mae/commit/8c86450a41d12975893bec4b576c589fc80c5cb4))
- *(kb)* Properties are stored once — the drawer is a rendering (#655) ([84713da](https://github.com/cuttlefisch/mae/commit/84713dac37bb3b24151dfc4cc21ca88d26067a8e))
- *(kb)* The four dead columns become derived, and a fresh CRDT doc keeps its v2 fields (D3, #656) ([a0b9ca0](https://github.com/cuttlefisch/mae/commit/a0b9ca0b1c79015416804476e9c5cda6585a8bd5))
- *(daemon)* Heal must not delete what the CRDT never knew about (C6) ([e057654](https://github.com/cuttlefisch/mae/commit/e057654abbf8abd1cc51ef425d4e5a3fa1bdd856))
- *(test)* The Scheme harness can finally verify KB persistence (#781) ([1c90b92](https://github.com/cuttlefisch/mae/commit/1c90b92166fc976991e768485bea496ba8c3d86d))
- *(kb)* Route meta-widen through the sole node mutator, and guard the rule (ADR-092 D1) ([ccc8cc0](https://github.com/cuttlefisch/mae/commit/ccc8cc0a133382d058c73bca137a86abf3a40345))
- *(kb)* The org export destroyed every link it wrote (ADR-092 D3) ([9f38210](https://github.com/cuttlefisch/mae/commit/9f38210fd896acd8b5732ea18beab4daed53f2e0))
- *(test)* A scenario step whose command was never dispatched must not report ok (#787) ([caf321c](https://github.com/cuttlefisch/mae/commit/caf321cf396c7673d16f95252bcb33148956cda7))
- *(kb)* Stop following links into a detached KB's stale archive (Phase 4/5) ([591feb8](https://github.com/cuttlefisch/mae/commit/591feb8f526d842d77205045be2bedf0abc84ab1))
- *(kb)* Kb-edit-source is the second door into the stale-archive trap (Phase 5) ([f11a752](https://github.com/cuttlefisch/mae/commit/f11a7524dbefd7f1bbcbf6184a6ceb013afe7d20))
- *(test)* A command that RUNS and fails must not report ok either (#787, part 2) ([ea9123b](https://github.com/cuttlefisch/mae/commit/ea9123b0501c460f1ee1c0ea725b1abc980257ba))
- *(kb)* Adopt a legacy-path KB registry instead of loading empty (#798) ([2aece30](https://github.com/cuttlefisch/mae/commit/2aece30af8543ff01cb2da92b46d89f285c622a7))
- *(kb)* The move tests were measuring Linux, not identity (CI Windows leg) ([ad4268a](https://github.com/cuttlefisch/mae/commit/ad4268a7db91ba3bf97a27cc59244aadcd814693))
- *(kb)* The org round trip lost six things, and ingest was one of them (ADR-092 D3) ([3a37d0a](https://github.com/cuttlefisch/mae/commit/3a37d0a0b191a8bfb9e19eb5127f274e116bd5ea))
- *(kb)* The cutover's detach was half-wired, and cleanup destroyed the KB ([8bb47c7](https://github.com/cuttlefisch/mae/commit/8bb47c7ec7a258f4c42850eb102a7374e68b4139))
- *(kb)* Dailies navigation and capture had no store backing (KB cutover) ([eeae8ce](https://github.com/cuttlefisch/mae/commit/eeae8ce67380f027ec296559f73dcc374bd7c6d4))
- *(kb)* A failed store read must not read as "no results" (KB cutover) ([3da2161](https://github.com/cuttlefisch/mae/commit/3da2161274a5d7dc5ee8f829609b0a15fabdae7a))
- *(kb)* Honour :daily-goto-date's argument, and stop a leaked dialog poisoning later tests ([bb51d59](https://github.com/cuttlefisch/mae/commit/bb51d59c2057a9b00c39b71b0b6d0495d2059c22))
- *(ai)* Let a tool pass an argument to a command, and report what happened (#808) ([492a975](https://github.com/cuttlefisch/mae/commit/492a9758d44b00a44d94a129079c33717b117225))
- *(build)* An untracked file is not an uncommitted change ([f304fc1](https://github.com/cuttlefisch/mae/commit/f304fc17c3f7e3acf21edeafa40f4231285e3074))
- *(kb)* Correct chrono_now()'s calendar arithmetic, and make the shell-escape test portable ([914ee4b](https://github.com/cuttlefisch/mae/commit/914ee4be2c9aff5884e81a043153866050b7137a))

### Performance

- *(kb)* Stop compiling keyed lookups to full relation scans ([2bbc0b8](https://github.com/cuttlefisch/mae/commit/2bbc0b8850d50440db7791b6ce2f2aac8a4c2d4d))

### Refactor

- *(daemon)* Guard KB documents by address type, not string prefix (ADR-105 Stage 1) ([b43608a](https://github.com/cuttlefisch/mae/commit/b43608a455a4a98e2c684964e56f2ee02fd8b9b2))
- Complete the doc-address sweep — one place spells the name form ([3411755](https://github.com/cuttlefisch/mae/commit/3411755dd1f37afc4484eb694271f903ba215d54))
- *(daemon)* Split the authorization decision out of the dispatcher ([6b22e5c](https://github.com/cuttlefisch/mae/commit/6b22e5c72badb5c23b94b7e53b6e1afa10e31fb5))
- *(daemon)* Split OAuth config out of config.rs ([0e4f305](https://github.com/cuttlefisch/mae/commit/0e4f30596e881c642e7911b9dc99fe7cf70bf826))
- *(kb)* Classify transient store errors, and drop the db_path trait leak (C2) ([0f4cc33](https://github.com/cuttlefisch/mae/commit/0f4cc33a6dedad52c0e6ed8857549c024dc4cff9))
- *(cli)* Extract the test-mode store opener ([730d96b](https://github.com/cuttlefisch/mae/commit/730d96bc447d4e8b7d2e113efe54773516a01d76))

### Documentation

- *(adr)* ADR-107 — node rebirth bounds CRDT growth using the consensus primitive MAE already has ([fcc48a6](https://github.com/cuttlefisch/mae/commit/fcc48a631e6b56e668f7181a7bdcc989dda3dbe1))
- *(adr)* ADR-108 -- one storage backend, and correct ADR-102's false premise ([b82b451](https://github.com/cuttlefisch/mae/commit/b82b451da062e3cc85a914d41e6952f4c78e6800))
- *(adr)* Restore the ADR-102 correction after the back-merge reverted it ([1e96b53](https://github.com/cuttlefisch/mae/commit/1e96b5346e0fe3d2ac2988936e2c5c15abeceeb0))
- *(adr)* Record the post-fix benchmark — the scaling risk was ours, not Cozo's ([781d990](https://github.com/cuttlefisch/mae/commit/781d99012f06d979d499d7b4f7eb1ce59cde1989))
- *(adr)* Price the hosted deployment before launch, not after (ADR-109, D4) ([fa7c160](https://github.com/cuttlefisch/mae/commit/fa7c1606c450c76d62cca5803676877ad0758ff8))

### Testing

- *(daemon)* Scope the auth-mode e2e's kb/node_crdt calls to a KB (ADR-105 Stage 2) ([a3d780a](https://github.com/cuttlefisch/mae/commit/a3d780a034e2d3e2b167945ea997fb1a0c146afa))
- *(collab)* Two tenants must both be able to share their primary KB (ADR-105 F) ([8722dd9](https://github.com/cuttlefisch/mae/commit/8722dd9f8791b8db7407cf0910fa0aa50a5ba64c))
- *(sync)* Reproduce the yrs v1 decoder's UTF-8 undefined behaviour (y-crdt#415) ([0078847](https://github.com/cuttlefisch/mae/commit/0078847a4ade75f5e0e71035c663cd8266f014be))
- *(daemon)* Prove the docs/* and sync/update authorization holes (ADVERSARIAL) ([084146b](https://github.com/cuttlefisch/mae/commit/084146b34ece66e8abbea8a6bbdb22c99c2612dc))
- Bound e2e readiness waits by wall-clock, not iteration count (#769) ([a8d7a6c](https://github.com/cuttlefisch/mae/commit/a8d7a6c2b56f848d405af12bfc2d3bf2a6bcdf7a))
- *(sync)* ADR-107 gates 2 and 3 — convergence, and the replay attack (ADR-107) ([32cf3b6](https://github.com/cuttlefisch/mae/commit/32cf3b67fc88ae6cbcc13a30198ecfbd0d4474a0))
- *(kb)* Story A — the day-one experience, proven end to end (Phase 6) ([9f165cc](https://github.com/cuttlefisch/mae/commit/9f165cc8e2673dec362ef0395da450ad2119956d))
- *(ai)* Measure and ratchet the advertised tool surface (Story C / R10) ([d605ab9](https://github.com/cuttlefisch/mae/commit/d605ab9dd6da2ec8cf3133fa991ebc9fed2e136e))
- *(kb)* Drive a detached KB end to end, and widen the silent-status net ([1d9e42c](https://github.com/cuttlefisch/mae/commit/1d9e42ccae7ea1147e08cd8651473070959b1c3a))

### Build System

- *(audit)* Gate function length and nesting, stop gating file size ([b88c28f](https://github.com/cuttlefisch/mae/commit/b88c28ffe1738140133d0d9792ed6c7867c7eb13))

### CI

- Supersede a PR's own in-flight run when it is pushed again ([af72dfc](https://github.com/cuttlefisch/mae/commit/af72dfcbb75e99c63313af6590646ae557100028))

### Styling

- *(daemon)* Apply daemon-workspace rustfmt ([2832dc0](https://github.com/cuttlefisch/mae/commit/2832dc0a620f0e7e2ff628545c4fb812b805cfaa))
- *(core)* Reattach dispatch_kb's doc comment to dispatch_kb ([302fd98](https://github.com/cuttlefisch/mae/commit/302fd98f35445ba2ea3e8d4e9699831d0d83a4f2))

### Miscellaneous

- *(daemon)* Confirm the ADR-105 migration ran, at debug level ([15e4289](https://github.com/cuttlefisch/mae/commit/15e428931200404bb05c5e9b4eda3703329cdc05))
- Bump version to 0.14.115 ([432e68d](https://github.com/cuttlefisch/mae/commit/432e68d14b97098e7b3a4f56a6e3d88044c60358))
- Satisfy the structural-ceiling ratchet, and record why registry.rs was re-blessed ([8cdc46c](https://github.com/cuttlefisch/mae/commit/8cdc46cf3d34002d735216d300111503bdae16af))
- *(adr)* Regenerate ADR KB asset after back-merging main ([c70bea2](https://github.com/cuttlefisch/mae/commit/c70bea202feac7213a19a98ea3e6b1c9946fc59b))
- Keep the ADR-108 work out of the security PR ([57fa7be](https://github.com/cuttlefisch/mae/commit/57fa7be9ee1fa81a7eac7ea77d6bae296c129268))
- *(adr)* Regenerate the ADR KB asset after the back-merge ([59578e9](https://github.com/cuttlefisch/mae/commit/59578e9a9ab87677dce226bf0fd41919a20005a8))
- *(deps)* Bump the rust-dependencies group in /daemon with 19 updates ([16e7535](https://github.com/cuttlefisch/mae/commit/16e7535c454574279b80b8f09ad141c01ea65bc1))
- *(deps)* Bump the rust-dependencies group across 1 directory with 5 updates ([6810090](https://github.com/cuttlefisch/mae/commit/68100905fbf1f6b36c156880aa9869222670d583))
- *(deps)* Land both dependency bumps, with the rand 0.10 API break fixed ([431eb65](https://github.com/cuttlefisch/mae/commit/431eb650eb7900d128ca5ff4a2dee2ae7b89fbc4))
- Regenerate audit metrics for the consolidated branch ([7529c8b](https://github.com/cuttlefisch/mae/commit/7529c8b43e97d7118084193530af158bf19dd633))
- Regenerate audit metrics after rebase onto #776 ([1d601ed](https://github.com/cuttlefisch/mae/commit/1d601edb86340c7b898d6b780583d6413eb0c393))
- *(adr)* Regenerate the ADR KB checksum for ADR-109 ([ad3a377](https://github.com/cuttlefisch/mae/commit/ad3a3770c174b7179e104ea31d7644fe4cd14511))
- Regenerate audit metrics after rebase ([4768d3e](https://github.com/cuttlefisch/mae/commit/4768d3ec54499ebf3182a338498eec47b3fb2e05))
- Regenerate audit metrics after rebase ([092e05c](https://github.com/cuttlefisch/mae/commit/092e05cd5b350c857b4cd7605086e5224b2c9d9f))
- Regenerate audit metrics and code map for the consolidated branch ([56895c9](https://github.com/cuttlefisch/mae/commit/56895c9945ff89464acda5c3a93b6f673ab55387))
- *(deps)* Bump the rust-dependencies group with 3 updates ([8cbb955](https://github.com/cuttlefisch/mae/commit/8cbb955fe2982d83dbc9f71f26b8e354c8a7d85a))
- Bump version to 0.14.115 ([3eda486](https://github.com/cuttlefisch/mae/commit/3eda48669f669c313815cdb17964d92174e42eff))

## [0.14.114] - 2026-08-14




### Bug Fixes

- *(collab)* Bind the owner test inside the E2E anchor search (#573) ([6dcc339](https://github.com/cuttlefisch/mae/commit/6dcc3399bda1639b8f8a9f98483619796d9c2122))

### Miscellaneous

- Bump version to 0.14.114 ([9934c70](https://github.com/cuttlefisch/mae/commit/9934c702c17d754dafa067bbc1b9d3930a02dc55))

## [0.14.113] - 2026-08-13




### Bug Fixes

- *(help)* Announce a fuzzy help substitution, and refuse an ambiguous one ([e91a1f2](https://github.com/cuttlefisch/mae/commit/e91a1f23b374b33fd1b1800cbd8bfe12997e89f9))

### Miscellaneous

- Bump version to 0.14.113 ([a02084b](https://github.com/cuttlefisch/mae/commit/a02084bbf6e523fce8a953c81828d866bf2a9541))

## [0.14.112] - 2026-08-13




### Documentation

- *(kb)* Condense the snapshot guard's rationale to clear the file ceiling ([a2f66e6](https://github.com/cuttlefisch/mae/commit/a2f66e6371fc9eadabd0c99a2691b3acf48f266a))

### Testing

- *(kb)* Make the staging guarantee testable without a timing race ([78b653a](https://github.com/cuttlefisch/mae/commit/78b653a367b85f78f51812a5d77665d9428a7340))

### Miscellaneous

- Bump version to 0.14.112 ([f874027](https://github.com/cuttlefisch/mae/commit/f874027ee04b847097249103a9f883d3cea6c9bf))

## [0.14.111] - 2026-08-13




### Bug Fixes

- *(kb)* Build a KB store to a staging path and rename it into place ([5d1c0b3](https://github.com/cuttlefisch/mae/commit/5d1c0b3c1904726ac5f71ce2b1227a98c23778ad))
- *(kb)* Stop the shutdown snapshot copying MAE's manual into user storage ([94f9ad2](https://github.com/cuttlefisch/mae/commit/94f9ad2f67538460e974d7c3a89adf6e6c7e7b19))

### Refactor

- *(kb)* Split kb_build's tests out to stay under the file ceiling ([8502bab](https://github.com/cuttlefisch/mae/commit/8502bab2f8b689a7bb72fd0d2e8c2b7ab120a6b0))

### Miscellaneous

- Bump version to 0.14.110 ([0e4d1f6](https://github.com/cuttlefisch/mae/commit/0e4d1f6a8fe834d53e166f63b081efef3d29da91))
- Bump version to 0.14.111 ([acc053d](https://github.com/cuttlefisch/mae/commit/acc053ddc9061b9c20419e6f440358621dfd3f06))

## [0.14.109] - 2026-08-13




### Documentation

- *(adr)* ADR-104 — system KBs are a distinct class; supersedes ADR-076 ([34c449e](https://github.com/cuttlefisch/mae/commit/34c449eb073494a9bb09d26f8ed3892bca55b308))
- *(adr)* Record the ADR-058 Phase A drift that ADR-104 fixes ([c9905e1](https://github.com/cuttlefisch/mae/commit/c9905e19bc284271efc8d7a5e5234c6bc6d57841))

### Miscellaneous

- Bump version to 0.14.109 ([8294c26](https://github.com/cuttlefisch/mae/commit/8294c2653234efa86dbdbfd6f9309293e90a24e7))

## [0.14.108] - 2026-08-13




### Bug Fixes

- *(kb)* Resolve the guidance env lookup before spawning the thread ([5698731](https://github.com/cuttlefisch/mae/commit/5698731eaa56a2b18a68f7dec88c1f67ecce0339))

### Miscellaneous

- Bump version to 0.14.108 ([1296d8d](https://github.com/cuttlefisch/mae/commit/1296d8d18349df5275bf6d976be12300d7545245))

## [0.14.107] - 2026-08-13




### Bug Fixes

- *(kb)* Graph reads see the federated graph, and both link directions agree ([ce03424](https://github.com/cuttlefisch/mae/commit/ce034240681b48e86d0361ce54276b94fc727798))
- *(kb)* Build guidance corpora off the startup critical path (#713) ([cd50d14](https://github.com/cuttlefisch/mae/commit/cd50d142403ba43aee8262cc0f18cceba810f91e))
- *(kb)* Module hot-reload reaches the query layer ([3d64ffa](https://github.com/cuttlefisch/mae/commit/3d64ffacfe4ec87e71abd20c9b164f71e14a6b27))

### Refactor

- *(mae)* Split the eviction tests out of kb_federation ([c0de78d](https://github.com/cuttlefisch/mae/commit/c0de78d3e1914f50c6772afb69d7850432037974))

### Testing

- *(guidance)* Isolate the cache dir in the diagnosis tests ([4d07d85](https://github.com/cuttlefisch/mae/commit/4d07d8534f70c2f54c11e67550e5a75a74c8bd50))
- *(kb)* The provisioning harness now measures the real startup path ([32f81d5](https://github.com/cuttlefisch/mae/commit/32f81d51c677c73f294f926a2b923bc9a958eff8))

### Miscellaneous

- *(deps)* Bump the rust-dependencies group with 6 updates ([2be4cdc](https://github.com/cuttlefisch/mae/commit/2be4cdca165c3b9466360ba212438e75a1ac978a))
- Bump version to 0.14.107 ([480e7a6](https://github.com/cuttlefisch/mae/commit/480e7a663b9f5ad1f2d6a934f82102662a3ab6d3))

## [0.14.106] - 2026-08-13




### Bug Fixes

- *(guidance)* The built store must be where the reader looks ([358d0a3](https://github.com/cuttlefisch/mae/commit/358d0a38e70dab6a95e22cead1f488b07ff6dc54))
- *(install)* Uninstall removes only what install placed ([d8bb7fa](https://github.com/cuttlefisch/mae/commit/d8bb7fa74a98c02b087f70b1d7f255a4e1585c8a))
- *(kb)* Headless and self-test never drained the KB background work ([b889633](https://github.com/cuttlefisch/mae/commit/b88963343cd0e4e9afd21a22f74d0085d10f27cb))

### Documentation

- *(install)* Correct the --help data-dir line ([5425fac](https://github.com/cuttlefisch/mae/commit/5425fac0f7cc74803d6495499afd55b237d40975))

### Testing

- *(kb)* Prove a store-only node becomes searchable headless ([2135938](https://github.com/cuttlefisch/mae/commit/2135938b6f56746d091adc8d8ab20610b7a8c763))

### Miscellaneous

- Bump version to 0.14.106 ([06eefbb](https://github.com/cuttlefisch/mae/commit/06eefbb27052273947749855c81497f28e92ec26))

## [0.14.105] - 2026-08-12




### Features

- *(delivery)* Stop shipping pre-built KB stores ([c814a9d](https://github.com/cuttlefisch/mae/commit/c814a9d5a80e47d943d46a51192794931f91f3c4))

### Bug Fixes

- *(scripts)* Back up the user's KBs, not MAE's regenerable ones ([88006d5](https://github.com/cuttlefisch/mae/commit/88006d5f1f6eac7258909c9dbeadb27838e6cbe8))
- *(install)* Stop asserting stores that are no longer shipped ([75c0273](https://github.com/cuttlefisch/mae/commit/75c0273c92774874e87d61195e82336a599757f7))

### Miscellaneous

- Bump version to 0.14.105 ([1a13e0d](https://github.com/cuttlefisch/mae/commit/1a13e0d715ecc8506229f65456426043d5479434))

## [0.14.104] - 2026-08-12




### Refactor

- *(mae)* Split bootstrap's inline tests out by subject ([21c54a6](https://github.com/cuttlefisch/mae/commit/21c54a6752a8e1aae422b3a2b913aa08672a5eaa))

### Miscellaneous

- Bump version to 0.14.104 ([c4bd454](https://github.com/cuttlefisch/mae/commit/c4bd454d792016179caaebbef54afce4f279fa62))

## [0.14.103] - 2026-08-12




### Bug Fixes

- *(kb)* Source the manual from its corpus, keeping the always-present invariant ([85bc109](https://github.com/cuttlefisch/mae/commit/85bc1096aad410d6e16dc2034fcfe37de8f2d048))

### Miscellaneous

- Bump version to 0.14.103 ([a9f1949](https://github.com/cuttlefisch/mae/commit/a9f1949846dbf7e25fbaa70357b5bbc88ac8b6da))

## [0.14.102] - 2026-08-12




### Features

- *(kb)* Make a missing guidance KB diagnosable instead of silent ([8604fd3](https://github.com/cuttlefisch/mae/commit/8604fd38e743831a430ace6d10992b78b843c8d7))
- *(kb)* Embed the system-KB corpora, and build guidance when no store ships ([fd8bdea](https://github.com/cuttlefisch/mae/commit/fd8bdea42d9ca9d7694af70e04d948ad78d64332))

### Miscellaneous

- Bump version to 0.14.102 ([c425b0b](https://github.com/cuttlefisch/mae/commit/c425b0b16bd759646a6f22afb1dc7c9cb6162618))

## [0.14.101] - 2026-08-12




### Features

- *(kb)* Evict system KBs from the registry, and label their hits ([e840aa6](https://github.com/cuttlefisch/mae/commit/e840aa65d975464812b153152be3aedb8b7f99fa))

### Bug Fixes

- *(kb)* Open system stores via the engine-aware path, not raw sled ([ec3bdc0](https://github.com/cuttlefisch/mae/commit/ec3bdc043a22421a68bed68160c73e0d3317aacd))

### Testing

- *(kb)* Phase 0 evidence — what runtime KB provisioning would cost ([c892a15](https://github.com/cuttlefisch/mae/commit/c892a152122c61ba0b8050b97aad96978d9578c1))

### Miscellaneous

- Bump version to 0.14.101 ([b593273](https://github.com/cuttlefisch/mae/commit/b593273dd00f2d0888c88f1445662fa2bbd21287))

## [0.14.100] - 2026-08-12




### Features

- *(kb)* The agent no longer chooses its own standing instructions ([9b42abd](https://github.com/cuttlefisch/mae/commit/9b42abded5aaa647eb254e13a20493447c5cb89f))

### Testing

- *(scheme)* Make the debug-overhead ratio robust to scheduler noise ([7e6c54f](https://github.com/cuttlefisch/mae/commit/7e6c54ff46a11ab2096bda03808e5b9a3f482a8f))
- *(kb)* Move the system-KB guards into their own module ([5392e50](https://github.com/cuttlefisch/mae/commit/5392e50da6693354e206c4e0771b3395ee2372bc))

### Miscellaneous

- Bump version to 0.14.100 ([930c882](https://github.com/cuttlefisch/mae/commit/930c882ba0fd6ec82bd11bbdc5c0e95dc9b85e9e))

## [0.14.99] - 2026-08-12




### Features

- *(kb)* A compile-time system-KB catalog, and reserved names ([2d6dc94](https://github.com/cuttlefisch/mae/commit/2d6dc943d4a56ae461500a1fa5bdbefb689ecf76))
- *(kb)* Refuse lifecycle operations on a system KB ([44143b8](https://github.com/cuttlefisch/mae/commit/44143b819a398b25ef6530be6cb7524004941eb7))
- *(#640)* :set ai-tier takes effect live, without a relaunch ([5e7ee23](https://github.com/cuttlefisch/mae/commit/5e7ee232dce2a69fb8e9547bde12429ad56fd111))

### Bug Fixes

- *(#487)* Make the write-failure guard deterministic, and drop the retry mask ([be67d04](https://github.com/cuttlefisch/mae/commit/be67d0470a0086f6cc73010bdee9b19c62a149a4))
- *(#693)* Condition waits for collab TCP e2e, and the ambient dependency behind them ([d9cdd89](https://github.com/cuttlefisch/mae/commit/d9cdd898c49c04435229b44ef43df4c87c814c27))
- *(#640)* Ai_tier reaches the enforced policy, and one tier vocabulary ([720c4a7](https://github.com/cuttlefisch/mae/commit/720c4a7fbfba9482b7d86299258721978b964f4f))
- *(#640)* The spawned agent inherits the resolved tier ([eb57f76](https://github.com/cuttlefisch/mae/commit/eb57f76dfb2c7a3b004b3619f17043568bfc5efd))

### Refactor

- *(kb)* One build function for org KBs, and one node-write boundary ([b886335](https://github.com/cuttlefisch/mae/commit/b8863358dd6cae2febbb268e71a935d3749002f4))
- *(kb)* Split migrate.rs tests into a child module ([95c24ea](https://github.com/cuttlefisch/mae/commit/95c24ea2ab291fdaf9656f3fce54fba5f647a439))

### Documentation

- The tier's help said it did nothing, and named only the legacy surface ([40c58b1](https://github.com/cuttlefisch/mae/commit/40c58b1efe7f75974a8922966e49f3d01c269eea))

### Build System

- *(deny)* Accept RUSTSEC-2026-0249 (smartstring unmaintained via cozo) ([c86a4b8](https://github.com/cuttlefisch/mae/commit/c86a4b88135fa32b1d5ec391d9a5d364fd295019))
- *(deps)* Bump lru to 0.18.2 (RUSTSEC-2026-0253) ([b6e092c](https://github.com/cuttlefisch/mae/commit/b6e092cbba7e17d3da1caddd0181f35390b8e41d))
- *(deny)* Accept RUSTSEC-2026-0249 (smartstring unmaintained via cozo) ([a910714](https://github.com/cuttlefisch/mae/commit/a910714afe83a1f81057fdeec46c72277b50555d))
- *(deps)* Bump lru to 0.18.2 (RUSTSEC-2026-0253) ([3e55b1a](https://github.com/cuttlefisch/mae/commit/3e55b1a3088364a99c43f9ef186e6c8bab1fbe6f))

### Miscellaneous

- Bump version to 0.14.98 ([d863073](https://github.com/cuttlefisch/mae/commit/d8630731eb29b4326c634f3b8b2f5d9761600025))
- Untrack the remaining ~58MB of rebuilt KB blobs ([bf9b9c9](https://github.com/cuttlefisch/mae/commit/bf9b9c9c42332c8e5bc9c9d544b09860608c5ef3))
- *(#640)* Move test modules out of two files at the size ceiling ([dbd1af9](https://github.com/cuttlefisch/mae/commit/dbd1af916092949d68a75b1cb095fe08291a5eb0))
- Bump version to 0.14.99 ([48b6d74](https://github.com/cuttlefisch/mae/commit/48b6d74cb9adb5a421c7ea06f423163f92394959))

## [0.14.97] - 2026-08-10




### Security

- *(security)* Two principle-#16 bypasses — babel trust-prefix escape and write-tier auto-accept ([e705619](https://github.com/cuttlefisch/mae/commit/e70561908255154096545dff2851ff3d06f56509))

### Bug Fixes

- *(test-safety)* The test suite was mutating the contributor's own repository ([4aa6859](https://github.com/cuttlefisch/mae/commit/4aa68594e4699f08f1a5f8fa1cfa07708651d8c4))
- *(test-safety)* Stop the suite writing the contributor's real data directory ([bbf15f1](https://github.com/cuttlefisch/mae/commit/bbf15f195c7562563fa41f1a73a4aa12518e5092))
- *(test-safety)* Extend the effect sandbox to the daemon workspace ([bd27731](https://github.com/cuttlefisch/mae/commit/bd27731ef473428438bb6a07d693dfac1042a6ae))
- *(test-safety)* One env lock for the whole process, and drop two more ambient dependencies ([c29424d](https://github.com/cuttlefisch/mae/commit/c29424dafa865a59c91b99867df75906ef055484))
- *(babel)* The trust-prefix fix was POSIX-only and broke matching on Windows ([5d76cd4](https://github.com/cuttlefisch/mae/commit/5d76cd4858bdb1dc587b74250083af702241c35a))
- *(test-safety)* Close the sandbox's cwd gap, and start actually looking ([d0311b7](https://github.com/cuttlefisch/mae/commit/d0311b7cf03982cf5179361855419bed10a384a9))
- *(kb)* Sled lock contention was classified permanent while sqlite got 45s ([5c161a6](https://github.com/cuttlefisch/mae/commit/5c161a642a56403fe5b4f14768d4ce60c21bfe2c))

### Refactor

- *(tests)* Split set_save_tests out of option_tests (ceiling ratchet) ([0ca13c1](https://github.com/cuttlefisch/mae/commit/0ca13c155d1d204369edbe41a9714ecb7629ecc9))

### Documentation

- Three claims that told readers work was done when it was not ([98469b3](https://github.com/cuttlefisch/mae/commit/98469b3aad49c6383e630ec72d29a44bbaf7c096))

### Build System

- *(make)* `make check` was type-checking half the tree ([7868523](https://github.com/cuttlefisch/mae/commit/7868523e799a0085208d2ed2dc9daf785e41bf8a))
- *(make)* A broken disk-reclaim target, an unreadable help, and a drifted .PHONY ([72d100b](https://github.com/cuttlefisch/mae/commit/72d100b169f0cfabde4ef9fc1ee9183cf2b8cce3))
- GUI is the default everywhere, TUI gets its own binary, timeout is portable ([1d62297](https://github.com/cuttlefisch/mae/commit/1d62297c9e95a4c3578d224084cb56ea38c66cf8))

### Miscellaneous

- Sync both Cargo.locks after the rebase onto 0.14.96 ([2225aa4](https://github.com/cuttlefisch/mae/commit/2225aa4aa72d36bf18a841b70cd1bb01a677eae1))
- Bump version to 0.14.97 ([276c78e](https://github.com/cuttlefisch/mae/commit/276c78e67c2870759ef683eae7b9ad58aebabe45))

## [0.14.96] - 2026-08-07




### Bug Fixes

- *(daemon)* The hub's raw sync surface was ungated, and an off-host bind was unauthenticated ([8518749](https://github.com/cuttlefisch/mae/commit/85187499a9e6e8833bc6752313a40128874abfb9))

### Refactor

- *(daemon)* Split the bind guard and its tests out of config.rs ([4ec291f](https://github.com/cuttlefisch/mae/commit/4ec291f111cafc61c9ad4d7e7f2ec69a7b24220b))

### Documentation

- *(adr)* ADR-101/102/103 — autonomous KB link enrichment design + prerequisites ([76f4c36](https://github.com/cuttlefisch/mae/commit/76f4c36905eb0ad0578d34644058bbf17accfc47))
- *(adr)* ADR-097 — Browser MAE is a KB surface, not a browser editor ([3d52920](https://github.com/cuttlefisch/mae/commit/3d52920a0dcb587eae2426b43749df286400b1a2))
- *(adr)* ADR-098 — durable identity for network clients ([eccddad](https://github.com/cuttlefisch/mae/commit/eccddad59ab7397a6230517c1f2dbbad36b0d85d))
- *(adr)* ADR-099 (browser sync transport) + ADR-100 (browser edit surface) ([ddfbfa2](https://github.com/cuttlefisch/mae/commit/ddfbfa2cb8f99a9609defcd657fcdf54d04441a7))

### Testing

- *(sync)* Phase 0 spike — browser Yjs converges with a real KbNodeDoc ([747a6c7](https://github.com/cuttlefisch/mae/commit/747a6c7e45f5dad09bbaca880c0992dc09459f5d))
- *(sync)* Multi-device identity spike — Rebind cannot express multi-device ([30516cf](https://github.com/cuttlefisch/mae/commit/30516cf83bfa391941f64269265eb97819b0d20e))
- *(sync)* Device secret-storage + revocation spike — devices are not members ([18ddc88](https://github.com/cuttlefisch/mae/commit/18ddc883e0363fc188f5ec877bc6429a40b7f839))
- *(sync)* Forward-secrecy spike — rotate-per-revocation is blocked on #176 ([03799bb](https://github.com/cuttlefisch/mae/commit/03799bba0698b2e2940e6cba559ab6450bd6d1ce))
- *(daemon)* Write-fencing spike — revocation is rotation-free but owner-bound ([0e037b1](https://github.com/cuttlefisch/mae/commit/0e037b172a190b6cf0648fcccc902f35057304b9))

### Build System

- Untrack the ADR KB blob, ship it as its own release asset ([a8c2e87](https://github.com/cuttlefisch/mae/commit/a8c2e878c8cfba7f362518f878a69f4d2b7cb71e))

### Miscellaneous

- *(adr-kb)* Regenerate for ADRs 097-100 ([edbab2d](https://github.com/cuttlefisch/mae/commit/edbab2da3bec0a848b2277c8695ba7414f6ae7fd))
- Bump version to 0.14.96 ([4023f3e](https://github.com/cuttlefisch/mae/commit/4023f3ec10d5257329dd524082559ba1898eec7a))

### Spike

- *(kb)* ADR-100 D4 — measure the org parser, and correct the question ([1f4d3c4](https://github.com/cuttlefisch/mae/commit/1f4d3c47b86d518b834981bcb41d8246b335602c))
- *(kb)* Measure the wasm bundle size — D4's last open condition closed ([6935546](https://github.com/cuttlefisch/mae/commit/693554685822a2033c823f549792461bf854e17a))

## [0.14.95] - 2026-08-07




### Security

- *(security)* Command mirrors laundered Shell-tier effects down to Write ([91f3012](https://github.com/cuttlefisch/mae/commit/91f3012eaf0fb1dc90f5ccdd458274dae27a817c))
- *(security)* Two permission bypasses reachable from the default tier ([d85dbc2](https://github.com/cuttlefisch/mae/commit/d85dbc2e6b8799da1fcb79912376dbba27ab0375))
- *(security)* Macro bodies escalated to Privileged at compile time ([be443d0](https://github.com/cuttlefisch/mae/commit/be443d0f7a22c5ff423be73dd31a86396b8dbd15))

### Features

- *(deploy)* Ansible role for mae-daemon, with enforced lint + safety-check tests ([9650bee](https://github.com/cuttlefisch/mae/commit/9650bee8a8c2280d3529bd05172be9424dd31bee))

### Bug Fixes

- *(release)* Publish checksums (#648) and sync lockfiles on version bump (#61) ([7c2cadb](https://github.com/cuttlefisch/mae/commit/7c2cadb402ff59175e9fedf64712cde0e537d95f))
- *(kb)* Deleting any node made the whole KB read as empty ([5946fce](https://github.com/cuttlefisch/mae/commit/5946fce5cf5bad00b044f349108441347869d2a6))
- *(ci)* Version-bump's `cargo update --offline` fails on a fresh runner ([10e35c0](https://github.com/cuttlefisch/mae/commit/10e35c05c3b9272f98e71d015864d1b6de19e5f3))
- *(daemon)* Collab handles were installed only under key-mode auth (#647) ([e9c288c](https://github.com/cuttlefisch/mae/commit/e9c288c6b6f98faf0d201b023bfc829c37a2e32b))
- *(kb)* Term:tier told every AI session a typo grants shell ([5dab30e](https://github.com/cuttlefisch/mae/commit/5dab30e2d5bd09812e27790c673095689ed7d76f))

### Refactor

- *(ai)* Split the mirror-tier parity suite out of categories.rs ([e6b5b6a](https://github.com/cuttlefisch/mae/commit/e6b5b6a5465187407aa72ab9c7af22fd5e7b24bc))

### Documentation

- MAE has no garbage collector — stop claiming one in three places ([ad49488](https://github.com/cuttlefisch/mae/commit/ad49488be27bd2bb1ad4c8fe30a0f50e83cab5d1))
- CLAUDE.md contradicted itself on tool count, and listed 11 of 20 self-test categories ([f945ded](https://github.com/cuttlefisch/mae/commit/f945dedd798a3ce572b4f3e7acfe4541d222b629))

### CI

- Keep open PR branches up to date with main automatically ([4764c71](https://github.com/cuttlefisch/mae/commit/4764c714abad0c2367cb9856809ddb611c00a678))
- Auto-update-prs must use RELEASE_PAT, not GITHUB_TOKEN ([72e2f8a](https://github.com/cuttlefisch/mae/commit/72e2f8a5c1683e1751e4322d0c0cc912cf737f45))
- *(auto-update-prs)* A denied token was reported as six merge conflicts ([cf8744c](https://github.com/cuttlefisch/mae/commit/cf8744c55fd7354055bfac6d62435b9955947e5e))

### Miscellaneous

- Bump version to 0.14.95 ([c21d9df](https://github.com/cuttlefisch/mae/commit/c21d9df87ebd76d920c56b2943ee9990df4b1fa1))

## [0.14.94] - 2026-08-05




### Features

- *(daemon)* Report connected clients on daemon/status ([c8fd08e](https://github.com/cuttlefisch/mae/commit/c8fd08e9bbe50acc5caab301b4a72285473d5323))
- *(deploy)* Ship the multi-instance template unit + a per-instance config ([dab65d2](https://github.com/cuttlefisch/mae/commit/dab65d2afbffa4b75ffe0d4f82249f48b53dd26a))

### Bug Fixes

- *(daemon)* Subcommands ignored --config and operated on the default instance (#645) ([79cb332](https://github.com/cuttlefisch/mae/commit/79cb332c7a556efa1587f894dd5e523955e0c715))
- *(config)* The generated template contradicted the shipped default (#639) ([a8cd97a](https://github.com/cuttlefisch/mae/commit/a8cd97ad3ad6293e3157c06b135f8cd8f37ddcce))

### Refactor

- *(config)* Split the #639 template gates out (ratchet) ([5a6ce80](https://github.com/cuttlefisch/mae/commit/5a6ce80cd02ead22a13c4f5e6c0ac740df248aa0))

### Build System

- Prune stale cargo test binaries (reclaimed 52 GB) ([8954c35](https://github.com/cuttlefisch/mae/commit/8954c3559f72d02296ae71b5f77c949bec78efe3))

### Miscellaneous

- Sync Cargo.lock versions to 0.14.93 (#61) ([969ebd9](https://github.com/cuttlefisch/mae/commit/969ebd90a52302623bbdb771775b287f80c42a92))
- Remove a real KB name from a test comment ([7bb55c7](https://github.com/cuttlefisch/mae/commit/7bb55c703df074c8104bbf148dba1ff375e34c5b))
- Bump version to 0.14.94 ([aa7206f](https://github.com/cuttlefisch/mae/commit/aa7206fbc836dba1744c09ee6889b7d7b92ffdd3))

## [0.14.93] - 2026-08-05




### Security

- *(security)* Route :w through the AI config-save guard ([58ba7ef](https://github.com/cuttlefisch/mae/commit/58ba7efb33722b00b4b00d80c776808d3656d89e))

### Features

- *(kb)* ADR-093 node CRDT schema v2 — carry every Node field ([33763f1](https://github.com/cuttlefisch/mae/commit/33763f1b05b1e196102d4f2957f96f089f6f56a1))
- *(daemon)* Wire the CRDT->Cozo projector (ADR-029 B2/B3) ([2c419e4](https://github.com/cuttlefisch/mae/commit/2c419e4883d75671e4466666753cf2f539602526))
- *(daemon)* Reconciliation + Gate C adversarial tests ([4df9024](https://github.com/cuttlefisch/mae/commit/4df902427e03c7bcedbd4a0c9d61c9d1742e10de))

### Bug Fixes

- *(guidance)* The guidance KB delivered nothing to anyone ([2950f9c](https://github.com/cuttlefisch/mae/commit/2950f9c298eb2916eb8d4f0660037158bf3593c8))
- *(guidance)* The guidance KB delivered nothing to anyone ([e835b8e](https://github.com/cuttlefisch/mae/commit/e835b8e55f6382d11a947e0c78b430b30aac816e))
- *(test)* Make the authorized_keys negative control deterministic ([c4aa82a](https://github.com/cuttlefisch/mae/commit/c4aa82a2e8f58ec1551ca074c7a9efa54401da34))
- *(cli)* --ensure-guidance-config lost its "already chosen" signal ([6c7ace0](https://github.com/cuttlefisch/mae/commit/6c7ace0fffd01ab518be81c6f909b42a15481fb9))
- *(daemon)* Scope kb/query.get + kb/node_fetch to the KB's own nodes (#571) ([091a65d](https://github.com/cuttlefisch/mae/commit/091a65d09aa9cb681486edbc2ce60182783e6bf1))
- *(dev)* The local quality gate never covered the daemon workspace ([ac377a0](https://github.com/cuttlefisch/mae/commit/ac377a02bfef361c22d7f3f8b285b0047a736a96))
- *(kb)* An empty org_dir instance claimed every path on the filesystem ([128e9a2](https://github.com/cuttlefisch/mae/commit/128e9a2355fc51b863dc25c14ed07b8f46d46749))
- *(kb)* Character-level CRDT updates for node title/body/tags ([5dea7e2](https://github.com/cuttlefisch/mae/commit/5dea7e202b8aa9337740af2e02fcbd85a6a9fa62))
- *(sync)* Materialize an op-set from the op SET, not from op order ([82f4c37](https://github.com/cuttlefisch/mae/commit/82f4c37d96068b2c4a812c747e1493eb69c4d9a3))
- *(sync)* Diff by word, and emit contiguous runs ([418b455](https://github.com/cuttlefisch/mae/commit/418b4557d06089e3e28e0abc85e2199c60eb1a0c))
- *(kb)* User: and project: links were destroyed at ingest (#627) ([ddf02ef](https://github.com/cuttlefisch/mae/commit/ddf02efa3b1af2b58ae4a1c0f9d9814453c5ddd7))

### Refactor

- *(daemon)* Split Gate C tests out of projector.rs (ratchet) ([1297b20](https://github.com/cuttlefisch/mae/commit/1297b2039596e2e8ccda2f0c5361fa3f28f4b117))

### Documentation

- *(adr)* ADR-092 — one write path for a KB node ([22cfe48](https://github.com/cuttlefisch/mae/commit/22cfe48bfef5e62be867c1c9f0a52ee252075679))
- *(adr)* ADR-093 — the node CRDT carries the whole node ([a46ecee](https://github.com/cuttlefisch/mae/commit/a46ecee6ae23ddfb6bcf67edc6eac7527e39f980))
- Audit AI-agent friction in config surfaces and the Ask state ([ff28916](https://github.com/cuttlefisch/mae/commit/ff28916245ebe0799d877c195aa7c8bc5e7e733f))
- *(adr)* ADR-095 — MCP elicitation carries the Ask state ([13c93c0](https://github.com/cuttlefisch/mae/commit/13c93c0d5f40b2bc4a76224e8353114c2a164565))
- *(adr)* ADR-096 — Scheme is the only editor config surface ([b6c29ee](https://github.com/cuttlefisch/mae/commit/b6c29eeea7298fec85188b40f2dd31cacecc9457))

### Testing

- *(kb)* Split instance path-matching into its own module ([a5e2957](https://github.com/cuttlefisch/mae/commit/a5e29572be584e09ef83ed22390c3a506d5eee85))
- *(kb)* Correct the n-peer oracle for character-level merge; split tests ([7514cfe](https://github.com/cuttlefisch/mae/commit/7514cfe8876c290426488c8e291e50fc48b2030d))
- *(kb)* ADR-093 Gate A.1 baseline — full-field CRDT round-trip (RED) ([c95bfe4](https://github.com/cuttlefisch/mae/commit/c95bfe470debaed2e37b683fd3356dfb1e7c3fe9))
- *(kb)* ADR-093 Gate A — schema v2 adversarial suite ([c03bc17](https://github.com/cuttlefisch/mae/commit/c03bc171ae7471fd3f8d62dc85c7afa73da88ecb))

### Styling

- *(daemon)* Rustfmt the new cross-KB isolation test ([dfbcdd4](https://github.com/cuttlefisch/mae/commit/dfbcdd431ebde69006e7582b0c1a410211538fc2))

### Miscellaneous

- *(deps)* Bump clap 4.6.4 -> 4.6.5 (supersedes #626) ([f0b35ed](https://github.com/cuttlefisch/mae/commit/f0b35ed37db4ea55ca29155daa4b8b39176d54ab))
- *(adr)* Regenerate the ADR KB for ADR-095/096 ([edd667f](https://github.com/cuttlefisch/mae/commit/edd667fe306a1f808041128f7e76324d0c1c9729))
- Bump version to 0.14.93 ([8146b87](https://github.com/cuttlefisch/mae/commit/8146b8728a045107c8c4fdfae7f5eb4224e9f360))

## [0.14.92] - 2026-08-04




### Documentation

- Correct security claims and ADR statuses that contradicted reality ([6768645](https://github.com/cuttlefisch/mae/commit/676864599d43fc2890463346758dda38efc59373))

### Miscellaneous

- *(dev)* Add scripts/new-workspace.sh — isolated clones for parallel work ([b996dca](https://github.com/cuttlefisch/mae/commit/b996dca3bc41253ee6797b089806cfeb1994cf89))
- Bump version to 0.14.92 ([0e06f4c](https://github.com/cuttlefisch/mae/commit/0e06f4cb3ed55d45f17ad4f0a73ba44c86f4f010))

## [0.14.91] - 2026-08-04




### Bug Fixes

- *(ci)* Badges ran the suite in debug, where headless startup nearly times out ([93634af](https://github.com/cuttlefisch/mae/commit/93634af741ecb86e55c2189ba60b34391c4f054f))

### Performance

- *(ci)* Cut the version-bump CI wait — it was 88% of a full test job ([446e464](https://github.com/cuttlefisch/mae/commit/446e4641195e6aa398d6da83b816034f27b379be))

### Documentation

- Principles 16 and 17 — the agent-bounding carve-out, and amendability ([b2bfb21](https://github.com/cuttlefisch/mae/commit/b2bfb2154c04ff0ab0353c57c42ed73e7202835d))
- Correct decision 12 — the sandbox guard is exam-only, not a security control ([b360d86](https://github.com/cuttlefisch/mae/commit/b360d86d8fbb4613cc689caa2dc22b516eaf17c7))

### Miscellaneous

- Bump version to 0.14.91 ([f9b9b53](https://github.com/cuttlefisch/mae/commit/f9b9b53e3c76007d89fc5f3d6c8002832d306fcc))

## [0.14.90] - 2026-08-04


### ⚠ Breaking Changes
- Unconfigured MAE now auto-approves reads and *asks* for writes and shell, instead of auto-approving up through shell. Nothing is silently denied on an interactive surface, so run_build/run_test still work via a prompt. Non-interactive surfaces (external MCP, --prompt, --self-test) deny and say so; those deployments need an explicit auto_approve_tier. ([ed5c9ab](https://github.com/cuttlefisch/mae/commit/ed5c9ab3ce44b93458394d864eb511522a566fa1))



### Features

- *(ai)* Permission decisions are three-state (ADR-090 D1/D2) ([0204b8f](https://github.com/cuttlefisch/mae/commit/0204b8f34cfd650680ce7e50da3917c2d01a7ec6))
- *(mae)* Wire ADR-090 Ask through every editor surface + ADR-084 D2/D7 ambient tier ([5b6f59c](https://github.com/cuttlefisch/mae/commit/5b6f59c80426e74eebc304e4212869d9ebbada45))
- Lower the default permission tier to readonly (ADR-090 D5) ([ed5c9ab](https://github.com/cuttlefisch/mae/commit/ed5c9ab3ce44b93458394d864eb511522a566fa1))

### Bug Fixes

- *(tests)* Close e2e headless/daemon process-leak paths ([676b0ff](https://github.com/cuttlefisch/mae/commit/676b0ff801244587e1eb12c248d5c4bc00a69143))
- *(mae)* Request_tools was dispatchable but absent from the dispatch tool list ([6bb3e83](https://github.com/cuttlefisch/mae/commit/6bb3e8388441bbdaf64bfaeb88705587cf690d6e))
- *(mcp)* Remove hand-rolled hex decoder — pre-auth remote DoS (#608.1, #608.2) ([4141a76](https://github.com/cuttlefisch/mae/commit/4141a76e9d61c37b5e053279438808b904fd0adc))
- *(collab)* Short_fingerprint panicked on a hostile peer's fingerprint (#589.1) ([df471d7](https://github.com/cuttlefisch/mae/commit/df471d75b0a582ead2c562aa2eddbc72feb4acb1))
- *(babel,pkg)* Close three silent-failure gaps (#596.1, #596.5, #607.2, #607.7) ([1c92864](https://github.com/cuttlefisch/mae/commit/1c92864eb08fa6814d56c29f85ec6314670f209c))
- *(daemon)* Signed membership op-log failures were reported as success (#589.4) ([bf6a12c](https://github.com/cuttlefisch/mae/commit/bf6a12ca67033898e549c35ce6b11113a0ad32b8))
- *(kb,export,org)* Four real defects — data loss, HTML injection, wrong subtree ([4f7855c](https://github.com/cuttlefisch/mae/commit/4f7855cdb8f9959fb264d4f189d6f186c4e48e61))
- *(babel,keystore,oauth)* Four security/correctness gaps (#596.4/.8, #608.4, #588.3) ([8a28eb5](https://github.com/cuttlefisch/mae/commit/8a28eb5022ce2d02a8dad030d578ed98ad63c880))
- *(options)* :set-save silently no-opped, and two writers of init.scm disagreed ([fd7a965](https://github.com/cuttlefisch/mae/commit/fd7a965e92e8b99188d283931a13afbbb44edc86))
- *(ai)* Shell_exec ignored its own advertised timeout argument (#590.3) ([2d555d1](https://github.com/cuttlefisch/mae/commit/2d555d1c8fd058e4073af7df8ecea48f1312f9a6))
- *(graph)* Closing a graph view returned to the wrong buffer (#591.1) ([e10e220](https://github.com/cuttlefisch/mae/commit/e10e2207a43fba28e144b614e974a496299aea14))
- *(spell,project)* A bound key that did nothing, and an AI/human path that diverged ([9459dd6](https://github.com/cuttlefisch/mae/commit/9459dd61eaab584aaf61f5af8532bf334043cfd3))
- *(kb)* One inotify instance per process, not one per KB ([26e874a](https://github.com/cuttlefisch/mae/commit/26e874a71fa5a26aeefff33c38f2eb5915190654))
- *(kb)* Release the shared watcher when nothing is watched; honest limit advice ([2968210](https://github.com/cuttlefisch/mae/commit/2968210a835d121cea35240544ee330c1fdb813d))
- *(build)* Use skia's own sync-deps opt-out; my ceiling fix did not work ([eac3418](https://github.com/cuttlefisch/mae/commit/eac34182f6992a8a66adcf16c98c39e11b23b618))
- *(tests)* Grant the KB-convergence e2e the tier ADR-090 D5 stopped giving it ([c57eb8d](https://github.com/cuttlefisch/mae/commit/c57eb8dd0083db55cedc1e0ea43bc92d4391c441))
- *(babel)* Resolve a real POSIX shell on Windows, normalize captured output ([bbe7eac](https://github.com/cuttlefisch/mae/commit/bbe7eacffa89008f6171ae79246efc2e876e1da8))
- *(babel)* Express the Windows shell rules over &str so they test off Windows ([b044170](https://github.com/cuttlefisch/mae/commit/b044170f38587644043c68d553b8ec866a8e29fd))
- *(build)* Stop worktree GIT_DIR leaking into builds and clobbering origin ([488e9ec](https://github.com/cuttlefisch/mae/commit/488e9ec3b42c2e3433be0d471a21b84424a6ebce))

### Refactor

- *(agent-cli)* Collapse PermissionMode into the shared PDP (ADR-090 D4) ([fb7873a](https://github.com/cuttlefisch/mae/commit/fb7873a9c9b49c8df8fa5e0a5bd2e512a32d63b4))
- *(ai)* Ai_permissions reports the three states in the one tier vocabulary ([11dad07](https://github.com/cuttlefisch/mae/commit/11dad07d7b86ed97515af51b3ddf51290458705e))
- *(mae)* Split ai_event_handler's tests to clear the ceiling ratchet ([3b3fb52](https://github.com/cuttlefisch/mae/commit/3b3fb52f072b3acc486921938baebce7c1aa9929))
- Extract inline tests from babel_ops and watch to clear the ratchet ([212a66e](https://github.com/cuttlefisch/mae/commit/212a66edb507156fe64d9bf31e434384ca962a12))
- Extract babel_ops/watch inline tests (recovered from auto-stash) ([c72ffc2](https://github.com/cuttlefisch/mae/commit/c72ffc2d39e69c8ffa6532764cd46eb48a9ab9e7))
- *(babel)* Extract Windows POSIX-shell resolution to its own module ([8c58165](https://github.com/cuttlefisch/mae/commit/8c581651d82d998cdcbd45b244f39686c6c48817))

### Documentation

- Queue the KB namespaced-ID search failure for review (decision 11) ([52f5683](https://github.com/cuttlefisch/mae/commit/52f5683cc1b8ef4cf9712481c5ebcf677e911b52))
- Diagnose MAE's inotify instance exhaustion ([7876ec0](https://github.com/cuttlefisch/mae/commit/7876ec0f6baf709af7ec3b53fda29051ebe23740))
- The tier is an auto-approval ceiling, and the default is readonly (ADR-090) ([1d5a315](https://github.com/cuttlefisch/mae/commit/1d5a3156c80b775c876d1c031a32d7e7193cc1ea))
- *(adr)* ADR-090 accepted + implementation notes; resolve decisions-for-review entry 1 ([c8ac18e](https://github.com/cuttlefisch/mae/commit/c8ac18ef093c76bb62b8cf3252dd52c79f0b3c41))
- *(decisions)* Three calls surfaced by the audit-tail fixes ([6adb612](https://github.com/cuttlefisch/mae/commit/6adb6125556c7c927f61f25716175037b91fe78f))
- *(decisions)* Renumber to 12-14, avoiding a collision with the base branch ([ea616c3](https://github.com/cuttlefisch/mae/commit/ea616c35f495d6114a2ad11f34d8b6cf35616269))
- Queue the external-MCP denial ergonomics call (decision 15) ([213444e](https://github.com/cuttlefisch/mae/commit/213444ea86aadd37fafee2481bb92ddd0878e0ec))
- *(adr)* Drop the closed advisory reference from the tracked docs ([63a0dd4](https://github.com/cuttlefisch/mae/commit/63a0dd4165651b090f957c6462bc48ed2907b7fb))

### Testing

- *(mae)* Split the nested-schema e2e now that propose_changes is embedded-only ([34aff60](https://github.com/cuttlefisch/mae/commit/34aff60165bf66e2bad432623ae821925c30d7f3))
- *(ai)* ADR-090 adversarial suite across the PDP + every surface ([0ad1dc2](https://github.com/cuttlefisch/mae/commit/0ad1dc253f4c9357d0d3480a197a28f352211a95))
- *(ai)* State the ceiling explicitly in pre-ADR-090 tool-behaviour tests ([52c197c](https://github.com/cuttlefisch/mae/commit/52c197c556081b3604ceb8328574e36bac530917))
- *(kb)* Let the instance-count assertions survive a parallel test binary ([f10eb0d](https://github.com/cuttlefisch/mae/commit/f10eb0d3f7f6286006fd24097cd057a786ec708c))

### Miscellaneous

- Sync Cargo.lock to the 0.14.89 version bump ([db62b00](https://github.com/cuttlefisch/mae/commit/db62b00e4f0f2e3a2ec8b3cd8c4e93f4b34640cf))
- Fmt + drop a now-unused import ([05e5b57](https://github.com/cuttlefisch/mae/commit/05e5b578bdf41f9b4fcc024cc0dca99beac10b3f))
- *(kb)* Drop the temporary inotify measurement probe ([e6982ed](https://github.com/cuttlefisch/mae/commit/e6982ed0c9b72157d0afd9f96f4e7bd7bf6241a3))
- *(changelog)* Surface breaking changes and security fixes in release notes ([7624591](https://github.com/cuttlefisch/mae/commit/76245919bad81fd6cb553db7afcfb7d7fab52365))
- Bump version to 0.14.90 ([5eadf54](https://github.com/cuttlefisch/mae/commit/5eadf5422bdf3b1acd859a16bb3d62d1f22bb35f))

## [0.14.89] - 2026-08-03




### Security

- *(security)* Require workspace trust for project-local init (ADR-089) ([52a99e9](https://github.com/cuttlefisch/mae/commit/52a99e946c0b757b180f52e8fa4744e7021fc259))
- *(security)* Permission tiers fail closed on unrecognised values (ADR-084 D4) ([d7c0027](https://github.com/cuttlefisch/mae/commit/d7c0027c25477750032335085ad2ab8227c260a5))
- *(security)* Split execution tools out of the Knowledge category (ADR-085) ([d613911](https://github.com/cuttlefisch/mae/commit/d61391153628916901429b7a9f0f298b4d9c6b06))
- *(security)* Correct the permission-tier claims SECURITY.md could not support ([d3e47d6](https://github.com/cuttlefisch/mae/commit/d3e47d61b57b86b3c201d64a6290fa31a4b6065e))

### Features

- *(scheme)* Required per-primitive tier classification (ADR-084 D3) ([d000736](https://github.com/cuttlefisch/mae/commit/d0007368a21bad9082a8599f67756ac491cda113))
- *(scheme)* Enforce the declared tier at the invocation chokepoint (ADR-084 D3/D5) ([30a2d63](https://github.com/cuttlefisch/mae/commit/30a2d63760a8a953d88fc9519c2c57136284dfa3))
- *(core)* Registration-parity guard for scheme:* KB docs (WS6) ([638bca2](https://github.com/cuttlefisch/mae/commit/638bca2eec62cb331adde383bf1b57514f479a82))
- *(ai)* Set_option persist param — MCP parity with :set-save (WS6) ([06a7e3f](https://github.com/cuttlefisch/mae/commit/06a7e3f327d2fc2e1ad8a377ae3aeefd5036db22))
- *(scheme)* KB CRUD + set-option-save! primitives (principle #3 parity) ([d3d2689](https://github.com/cuttlefisch/mae/commit/d3d2689fd69b651ed95d8114d7e57bec273600eb))
- *(scheme)* LSP + DAP primitives with an honest async boundary (principle #3) ([ab74f12](https://github.com/cuttlefisch/mae/commit/ab74f1250add4e778d42d91ec9c6dd874179b005))
- *(ai)* KB authorization tools are Privileged on every surface (decision #6) ([264d90d](https://github.com/cuttlefisch/mae/commit/264d90d2eee5824c46a4c95d0717f153e3481069))
- *(ai)* A session handle for MCP tool dispatch (ADR-091, decision #9) ([ab36ad4](https://github.com/cuttlefisch/mae/commit/ab36ad407eab7249d973abdfeaf74445a4760063))
- *(core)* ADR-087 Rule 4 scaffolding — byte-domain column conversions ([a5e5fd9](https://github.com/cuttlefisch/mae/commit/a5e5fd9cba0568bbd2f78aad48513f44cca12e19))
- *(core)* Window::cursor_col becomes a byte offset (ADR-087 Rule 4) ([93b43c3](https://github.com/cuttlefisch/mae/commit/93b43c3300b7acc50d609f15e67ee1240af20398))
- *(core)* Session-file column-domain migration (ADR-087 Rule 4) ([f241cc4](https://github.com/cuttlefisch/mae/commit/f241cc49be8f1d9624ab5ec7df5ba33c9b75894e))
- *(core)* Wrap.rs honours WidthPolicy; wrap columns become byte columns ([194c788](https://github.com/cuttlefisch/mae/commit/194c788ddebb61c17fc431cf3b29c28b38cc422e))
- *(render)* Thread WidthPolicy past the status bar (ADR-087 follow-up b) ([82c1488](https://github.com/cuttlefisch/mae/commit/82c1488c2178d0a7d5b61d45c3ee3a2915b3462b))
- *(render)* Thread WidthPolicy into splash + which-key separator width ([79d678a](https://github.com/cuttlefisch/mae/commit/79d678aae9e53656235fb8b9e9bfe664c684bae8))

### Bug Fixes

- *(ai,core)* Make registered-but-unreachable a build failure (WS1) ([d3be4bc](https://github.com/cuttlefisch/mae/commit/d3be4bc1c80501ffca55300271ac4723f236a2f3))
- *(ai,core,daemon)* Report refusals as errors, not success (ADR-086) ([9007bf2](https://github.com/cuttlefisch/mae/commit/9007bf28b19b4b180088096f0229d097094946be))
- *(kb)* A failing store is an error, not an empty result (WS3) ([1f43a78](https://github.com/cuttlefisch/mae/commit/1f43a788b18de55862f82666d6652b3ce339bddd))
- *(core,daemon)* Adapt remaining KB query-layer callers to Result (WS3 cont.) ([da3314e](https://github.com/cuttlefisch/mae/commit/da3314e1e544f1c3817e804bba7e03ff0e4b8823))
- *(scheme-extra)* Scope the tier import to the test module ([0f37d7d](https://github.com/cuttlefisch/mae/commit/0f37d7dbc67005201d343e2abfe6d15f4c6621c8))
- *(core)* Display_width_with must not delegate to whole-string width() ([241c3f9](https://github.com/cuttlefisch/mae/commit/241c3f941d1a170af71465875b92e2d2f46be56c))
- *(core)* Clippy fixes (derivable Default, bool_assert_comparison, pre-existing unused import) ([92b5dc0](https://github.com/cuttlefisch/mae/commit/92b5dc0cd6178137f4b8f847128215a28fcba48d))
- *(ai)* AI tool calls no longer report success on refusal/error (ADR-086, #590.2) ([873d531](https://github.com/cuttlefisch/mae/commit/873d5319ac12b9bd64417d6649c97d43247a9570))
- *(render)* TUI splash text centering used byte length, not display width ([9567f82](https://github.com/cuttlefisch/mae/commit/9567f8283f03c02c66f2df0a8623c34acb5982c5))
- *(core)* Separate flooring a byte budget from validating an offset ([5115857](https://github.com/cuttlefisch/mae/commit/5115857871a2859980c2729df022ff2c3586f691))
- *(scheme)* Match lsp-diagnostics buffer scope on the URI-derived path + cover the filter ([95aa73e](https://github.com/cuttlefisch/mae/commit/95aa73e99687d60368d553bea0fd660086e82af8))
- *(scheme)* Kb-search ranks with KnowledgeBase::search_ranked, not the lossy FTS index ([93b413c](https://github.com/cuttlefisch/mae/commit/93b413c4d73e0cff3008f4e8005a6e77dea6d08e))
- *(code-map)* Scheme primitive extractor had reported zero since the split ([e1a21bb](https://github.com/cuttlefisch/mae/commit/e1a21bb5a1b5c947a32ad83904b82845894b2e93))
- *(audit-metrics)* Exclude .claude worktrees; bless this branch's ceiling growth ([84e7574](https://github.com/cuttlefisch/mae/commit/84e7574bb51373711430b59bfa0a64684e73127c))
- *(kb)* Stop fts index welding title's last token to body's first ([b76fc47](https://github.com/cuttlefisch/mae/commit/b76fc4712f4acd03f7f461f53aaed6a7c3a74897))
- *(kb)* Stop fts post-filter discarding valid prefix-query hits; property test ([45ed74a](https://github.com/cuttlefisch/mae/commit/45ed74ab951d4a9feedd0e3adaf6fde503cca9c4))
- *(kb)* A failed fts rebuild must not stop the store opening ([48e3eef](https://github.com/cuttlefisch/mae/commit/48e3eef33da477d8440c6a239885832288e15db6))
- *(core)* Convert editor/ char-offset cursor columns to byte columns ([9ab5e83](https://github.com/cuttlefisch/mae/commit/9ab5e835ad8e484083284843e1fff272ad2861f9))
- *(core)* Explicit byte-col/LSP character conversions (ADR-087 Rule 1) ([5ebdb6a](https://github.com/cuttlefisch/mae/commit/5ebdb6ae36a3a03fddff716fc2a2baa64e97bf96))
- *(renderer)* Byte-col to screen-col via display_width_of_prefix_with; thread WidthPolicy ([5c25b9f](https://github.com/cuttlefisch/mae/commit/5c25b9fd0ff9b077b44a47f5662ed5ab9402bdbb))
- *(gui)* Byte-column cursor conversions + WidthPolicy threaded through the GUI renderer ([565f026](https://github.com/cuttlefisch/mae/commit/565f02668bd54f53adf650e3073d8082f07707b2))
- *(scheme,mae)* Byte-column conversions at the Scheme, key-handling and LSP-bridge boundaries ([fd9ce60](https://github.com/cuttlefisch/mae/commit/fd9ce604a7837790c0361e8fe5d0e7727f89b0e6))

### Refactor

- *(render)* Collapse hover/KB-preview popup + which-key duplication (WS5 findings 1, 3) ([21f0a63](https://github.com/cuttlefisch/mae/commit/21f0a635bf4e903b2871ffae5b82c574d2f66cd6))
- *(render)* Merge byte-identical status-buffer span computations (WS5 finding 4) ([4e9bacf](https://github.com/cuttlefisch/mae/commit/4e9bacf1d1dcff87ecac8b2c6468354b9457fa69))
- *(render)* Shell attribute/theme parity + splash dead-code cleanup (WS5 findings 2, 5) ([ebf0c64](https://github.com/cuttlefisch/mae/commit/ebf0c64e208e7ee48810b4a87c65d27e4f621d48))
- *(core)* Name the scheme-async snapshot slot type (clippy type_complexity) ([5657496](https://github.com/cuttlefisch/mae/commit/5657496156fc6f954330d141cc89022bb9bf7188))
- *(core)* Move the ADR-091 session accessors out of window_ops ([847f0b4](https://github.com/cuttlefisch/mae/commit/847f0b4069e95655e3619b110c1d080fe21e9ffe))
- *(kb)* Move SQLite busy-retry helper from schema.rs to db.rs ([6567669](https://github.com/cuttlefisch/mae/commit/6567669d36aaf6ef7cde74e051912fa6b0f9eb3a))

### Documentation

- *(adr)* ADR-084/085 — permission enforcement placement and the category/tier split ([ad93568](https://github.com/cuttlefisch/mae/commit/ad935687962ee970529c05dacdf04fe8a0c7c39c))
- *(adr)* Ground the permission model in prior art; reverse ADR-084 D2 ([a1050bf](https://github.com/cuttlefisch/mae/commit/a1050bf7f3d0b69ddfc884acd801120f0f401306))
- *(adr)* ADR-090 — permission decisions are three-state ([475f380](https://github.com/cuttlefisch/mae/commit/475f3806d5edbb9dddaae40254159c5dd0069612))
- *(devpractices)* Add prior-art review as a standing pre-decision practice ([d66e796](https://github.com/cuttlefisch/mae/commit/d66e7969566686bffa598e14b9d3a5b209489a6c))
- *(adr)* ADR-086/087/090, prior-art corrections, and parked decisions ([3963622](https://github.com/cuttlefisch/mae/commit/39636227e561133fe6f46c3f145bea3aa0d3ebed))
- Record the push decision on the disclosure question ([258eaf7](https://github.com/cuttlefisch/mae/commit/258eaf7323ff68185d0b47795943489bb9ae8c95))
- Cross-surface parity table + proposed Scheme primitives (WS6) ([5c00ae5](https://github.com/cuttlefisch/mae/commit/5c00ae5aba595926b3b72015061a3c471a236e11))
- Record the two ADR-087 follow-ups ([8298ba6](https://github.com/cuttlefisch/mae/commit/8298ba65f5e60f606ef7ec0d634a5c8d85fbf1c4))
- *(parity)* Record the closed gaps and how the LSP/DAP async boundary was resolved ([c94af1a](https://github.com/cuttlefisch/mae/commit/c94af1a63e6c9a75e693019879b40c725a664192))
- Flag set-option-save! as a persistence primitive under ADR-084 D7 ([7d09bc4](https://github.com/cuttlefisch/mae/commit/7d09bc48cfc105e74000031cba19edcf3dd3a37e))
- Reproduce the fts_search miss independently ([c3a6aaa](https://github.com/cuttlefisch/mae/commit/c3a6aaa3d732ce987a5078d458f54ca59c73bb54))
- Correct tracking docs from measured data, retire the hand-maintained list ([cc4ffd4](https://github.com/cuttlefisch/mae/commit/cc4ffd44f98bc85fce86430fd634853fc6a2f100))
- *(roadmap)* Record the pre-v0.15 audit findings and its coverage gaps ([8a8e8f7](https://github.com/cuttlefisch/mae/commit/8a8e8f70cba1cb973dd5b7c7318cd08b95c8f1a3))
- *(roadmap)* Audit complete — 211 confirmed findings across both axes ([cc82e34](https://github.com/cuttlefisch/mae/commit/cc82e34807cca3d34e92b65e3f6ffb09008a199c))
- Record the ten decisions ([6a3dd22](https://github.com/cuttlefisch/mae/commit/6a3dd22a90345c79cedd058e5a218e6fba2c8132))
- *(kb)* Record the fts root cause; correct the now-stale lossy-index claims ([2850d96](https://github.com/cuttlefisch/mae/commit/2850d9601352528551de2e3b92771f91dff014c0))

### Testing

- *(scheme)* Sweep on one VM, and record the shared-image debt at the VM ([9613273](https://github.com/cuttlefisch/mae/commit/961327315e5ebc208a11415b217c102dd1d54fd2))
- *(ai)* Require every registered tool to declare a permission tier ([3200cac](https://github.com/cuttlefisch/mae/commit/3200cacd55a9fdc276a63bda3767f9827b50a786))
- *(core)* ADR-087 nasty-string corpus (15 cases) + proptest invariants ([4b0bb9c](https://github.com/cuttlefisch/mae/commit/4b0bb9cfa58b0b31fe326b8c0b6b6b327a618bdb))
- *(ai)* Fix two adversarial tests intercepted by schema validation, not dispatch ([d779a34](https://github.com/cuttlefisch/mae/commit/d779a34a789d849689551fa58c9ba06e6a0eab14))
- *(scheme)* Adversarial + tier tests for the parity primitives; FTS query sanitization ([fbcaea9](https://github.com/cuttlefisch/mae/commit/fbcaea9bb3952f50831a922dbedf0908b77dfb8d))
- *(core)* Make the chokepoint tests profile-correct ([5b41685](https://github.com/cuttlefisch/mae/commit/5b41685b8bda1e5326b67412f3486179fd2fb9f3))
- *(kb)* Pin the fts index migration and the two search paths' agreement ([abd2fdc](https://github.com/cuttlefisch/mae/commit/abd2fdcff407e7e19e0afa6756e9fdac4d66080d))
- *(core)* Wrap byte-column + WidthPolicy coverage; thread policy into core callers ([e2eb277](https://github.com/cuttlefisch/mae/commit/e2eb277b8b49d8b0972b5072d2d276eb13c84aab))
- *(core)* Adversarial non-ASCII cursor/selection/edit suite for ADR-087 Rule 4 ([18efce9](https://github.com/cuttlefisch/mae/commit/18efce9d2f8368ae82d4ecc0bc6f2b45b51e018b))
- *(kb)* Split kb_store_impl_tests.rs to satisfy the 500-line test ceiling ([b14df38](https://github.com/cuttlefisch/mae/commit/b14df380e005edd27200db786fd0238d815e94d4))

### Build System

- *(core)* Add proptest dev-dependency for ADR-087 width invariants ([09c1d21](https://github.com/cuttlefisch/mae/commit/09c1d214d038280bc2b3955c7c2ae88e6e91b3dd))
- *(audit)* Mechanical structural-ceiling metrics + CI ratchet ([1bda211](https://github.com/cuttlefisch/mae/commit/1bda211a967bb2eeac25045eb44780f72f27578f))
- Stop skia's git-sync-deps from rewriting MAE's git remote ([995faf9](https://github.com/cuttlefisch/mae/commit/995faf97c6efa18ebabdb7f79920762c4de413d7))

### Styling

- Cargo fmt ([c810c6a](https://github.com/cuttlefisch/mae/commit/c810c6a835fbc3ae22935df44fae340fd348df4b))
- Cargo fmt --all ([9a87bdc](https://github.com/cuttlefisch/mae/commit/9a87bdc3679c65ca433c030cfbe775f6ba216750))
- Cargo fmt --all ([9f9fcf5](https://github.com/cuttlefisch/mae/commit/9f9fcf52ee9c3bb380d26ce4595d3e9f99c53864))
- *(core)* Rfind instead of filter().next_back() (clippy::filter_next) ([c9cea20](https://github.com/cuttlefisch/mae/commit/c9cea20baef1c1f0c1308a6f78f2e93bd8539843))

### Miscellaneous

- Sync Cargo.lock files to 0.14.88 ([550932e](https://github.com/cuttlefisch/mae/commit/550932e88efdd20468afe5087e146402029f3c6a))
- Bump version to 0.14.89 ([1084f97](https://github.com/cuttlefisch/mae/commit/1084f97754c9db12ff95eed34f145868a566e43a))

## [0.14.88] - 2026-08-01




### Features

- *(export)* Add kb_export_subgraph_html standalone graph export ([c208aa8](https://github.com/cuttlefisch/mae/commit/c208aa8bd1175b08d58f85977e626bf8bcebd2dd))
- *(export)* Chord nav widget, home/outline controls, muted+accent theming ([40654ce](https://github.com/cuttlefisch/mae/commit/40654ce27c09cd6c8db13cd1ea1583b81c1fef87))
- *(export)* Invert the chord widget's theme relative to the page ([c0c64cb](https://github.com/cuttlefisch/mae/commit/c0c64cbfffcf9564dd440c596a4dc9ea910745ac))
- *(export)* Sidebar restructure, link theming, prev/next, hover-preview ([69c46ef](https://github.com/cuttlefisch/mae/commit/69c46ef455aff858cb294651f3ca8e6f92572948))
- *(export)* Bold outline heading, persist theme choice via localStorage ([1ba4c50](https://github.com/cuttlefisch/mae/commit/1ba4c50fc87a5980026f804a3f8be394194ec805))
- *(scheme)* Add kb-export-subgraph-html primitive + kb-subgraph-export module ([6015002](https://github.com/cuttlefisch/mae/commit/60150026928b6e395f7cafc76893e4d82dba3d96))
- *(export)* Thread mae_kb::Node.tags through to the exported page's tag filter ([1c75dfb](https://github.com/cuttlefisch/mae/commit/1c75dfbad6de0cdf47e6534ee547347bc9ab39d1))
- *(export)* Support guidance_ids in kb_export_subgraph_html ([8dc4b26](https://github.com/cuttlefisch/mae/commit/8dc4b26b0bb6452f7054a6820c162dda7ff097a6))
- *(kb-export)* Surface kb-export-subgraph-html's constants as real kb-export-* options ([5e9fb29](https://github.com/cuttlefisch/mae/commit/5e9fb29cd362694de4f1dfb2f61330313d0e8e70))
- *(export)* Implement nested org sub-list parsing and rendering ([08256f7](https://github.com/cuttlefisch/mae/commit/08256f772d81797726c393f30a979034b3dd6390))
- *(kb)* Required_tag hard filter for export + fix kb_agenda federation gap ([6b815bb](https://github.com/cuttlefisch/mae/commit/6b815bbf44480864a734f2a6b1fe55894b194514))

### Bug Fixes

- *(export)* Render mae_kb's internal pipe-style links, strip PROPERTIES drawers ([c350fa6](https://github.com/cuttlefisch/mae/commit/c350fa69826b5f85241e494e14fc3c3c1da35581))
- *(export)* Decode Unicode scalars, not raw bytes, in inline markup conversion ([6445ede](https://github.com/cuttlefisch/mae/commit/6445ede3bcab5efd7393578bc2a35c576e41459c))
- *(export)* Selective chord-diagram labels, truncated, viewBox room for them ([0d34bf2](https://github.com/cuttlefisch/mae/commit/0d34bf278bdea47807ba561c7c57f3e59dd0641a))
- *(export)* Reusability review findings — real hit-target guarantee, no silent truncation ([8120d03](https://github.com/cuttlefisch/mae/commit/8120d0362bba0fc1663c1a0ce729d71d247f745a))
- *(export)* Graph-pane actually paints with its inverted background ([0278e81](https://github.com/cuttlefisch/mae/commit/0278e81c0ac3782fefe3e6d8bf4e9db5d1e99680))
- *(export)* Next no longer re-selects the visible anchor, body links navigate, inline code visible ([22f8630](https://github.com/cuttlefisch/mae/commit/22f863053f58f4d365676144bb82e755117e255b))
- *(export)* Flatten chord widget chrome, center the ring, fix label bias ([e28dfe4](https://github.com/cuttlefisch/mae/commit/e28dfe4d47396587211827f4dac92408327360ed))
- *(export)* Table markup, real UX for nav/popover, legible chord labels ([2f86552](https://github.com/cuttlefisch/mae/commit/2f86552085555c4777df6e44993f077561a8beeb))
- *(export)* Re-point kb_export_subgraph_html at bilingual-kb-export ([de14625](https://github.com/cuttlefisch/mae/commit/de14625c39c526de360e0d5eb6ed40c820816933))
- *(manual)* Convert markdown-flavored syntax to real org syntax ([40ef1ac](https://github.com/cuttlefisch/mae/commit/40ef1ac8c7f653d6303e5c0347d66c7bb9b704e0))
- *(ai-residency)* Restore kb_export_subgraph_html adversarial test coverage ([918c521](https://github.com/cuttlefisch/mae/commit/918c521e5e2cc11f4bd5670182de0862df5e581a))
- *(export)* Strip PROPERTIES drawers even when real prose precedes them ([7e0e1dd](https://github.com/cuttlefisch/mae/commit/7e0e1dd94ef64561acfb253aabbbc556294fba3a))
- *(export)* Don't leak a list item's own PROPERTIES drawer into its content ([f848b3a](https://github.com/cuttlefisch/mae/commit/f848b3ad9a603ca90e12cff2ac8984621903e61f))
- *(export)* Recognize multi-digit ordered-list markers, skip drawers in paragraphs too ([92f4ac0](https://github.com/cuttlefisch/mae/commit/92f4ac0319ca1a9e38613c78478fff7773c2425b))
- *(export)* HTML-escape inline code/strikethrough span content (XSS) ([aba69cc](https://github.com/cuttlefisch/mae/commit/aba69cc482655a2fc7f5af37d4b5d1819bd7e4f4))
- *(export)* HTML-escape #+begin_export html by default (XSS) ([4d7681a](https://github.com/cuttlefisch/mae/commit/4d7681a88217466146b47fd2b26cac7e3c04cb75))
- *(export)* Recognize any org drawer, not just :PROPERTIES: (content loss) ([4b388da](https://github.com/cuttlefisch/mae/commit/4b388daa2be3b0477ce4098929e9640a00b4ab94))
- *(ai)* Require Shell tier for kb_export_subgraph_html (mermaid subprocess) ([47e7a75](https://github.com/cuttlefisch/mae/commit/47e7a75d2e56b5064de5c455d5b5fa10daf9fe53))
- *(ai)* Residency-filter kb_export_subgraph_html's guidance_ids ([ec283b1](https://github.com/cuttlefisch/mae/commit/ec283b1a203d097c8c0e7956f3050c99db575066))
- *(ai)* Clamp chord_config numeric overrides to sane ranges ([8cff2f7](https://github.com/cuttlefisch/mae/commit/8cff2f706ba6e4c11928c1cd3eda211e32b5d9c1))
- *(ai)* Write the exported HTML file atomically ([6b4286d](https://github.com/cuttlefisch/mae/commit/6b4286dc5b6ba35de71e9e1e09ffc9dca3d93336))

### Refactor

- *(export)* Extract GRAPH_JS/STATIC_CSS to real asset files ([215d3ba](https://github.com/cuttlefisch/mae/commit/215d3baaec4a9ff5bae23275b30d7d2b710d9434))
- *(export)* Replace exact-substring config patching with real JSON injection ([d280192](https://github.com/cuttlefisch/mae/commit/d280192c11df0dc37b219431b754796fec53b105))

### Documentation

- Tag html_graph.rs as tracked architecture-debt per CLAUDE.md convention ([8e0b158](https://github.com/cuttlefisch/mae/commit/8e0b15824817bcfa750882a23cb9592678ae1606))
- *(kb-export)* Remove real RoamNotes UUID/paths from the invocation recipes ([fc1c8b3](https://github.com/cuttlefisch/mae/commit/fc1c8b3a29f4b722411bba81842ae11e63cbb48e))
- *(adr)* Port kb-export-subgraph-html ADRs, resolve dangling citations ([bc16964](https://github.com/cuttlefisch/mae/commit/bc16964cc853eb178990232b254fca88bd5c290b))
- *(architecture-debt)* Track GruvboxPalette theme-snapshot drift risk ([5edd2c9](https://github.com/cuttlefisch/mae/commit/5edd2c90059726554991467f25b987e36813b98a))

### Testing

- *(export)* Port the real-browser (Layer 2) behavioral suite for kb-export-subgraph-html ([df32bdf](https://github.com/cuttlefisch/mae/commit/df32bdfb8c7551168b03321fc4fc2a6a35ef6e3c))
- *(export)* Diversify unicorn-value fixtures in XSS/drawer tests ([76925e6](https://github.com/cuttlefisch/mae/commit/76925e64f6fbdae3352cd1439fc8fd0d7e7f9aed))
- *(ai)* Cover non-numeric/negative depth and node_cap fallback ([0df0f9f](https://github.com/cuttlefisch/mae/commit/0df0f9f3bfd4a66f10e4be0c03e1021e190ec3df))

### CI

- Add node --check gate on graph.js, update architecture-debt tracking ([8faf87f](https://github.com/cuttlefisch/mae/commit/8faf87fc4cf93a5b8ade31880f135da6a9e847e6))

### Miscellaneous

- *(daemon)* Sync Cargo.lock to workspace version 0.14.87 ([940ab25](https://github.com/cuttlefisch/mae/commit/940ab25ce3adf3a3dd931ca79d29994ef016088b))
- Fix CI — cargo fmt + regenerate stale code map ([a5a7146](https://github.com/cuttlefisch/mae/commit/a5a7146a557afafefe1d4051f6566ab6eaa0fdea))
- Bump version to 0.14.88 ([119c227](https://github.com/cuttlefisch/mae/commit/119c227141888b0dc02b3ee04221c39dff297a28))

## [0.14.87] - 2026-07-31




### CI

- Skip the slow job matrix on version-bump's own automated commit ([bf35a47](https://github.com/cuttlefisch/mae/commit/bf35a47c5cacca82e084dcd40b2f9a03ac08a6f0))

### Miscellaneous

- Bump version to 0.14.87 ([598c91c](https://github.com/cuttlefisch/mae/commit/598c91c727943cbe8702c82502ef5451c4cadd9d))

## [0.14.86] - 2026-07-31




### Bug Fixes

- *(options)* Set_option's dynamic-option fallback must not mask a genuinely unwired hardcoded option ([6a3140c](https://github.com/cuttlefisch/mae/commit/6a3140c415ce7c74f4536b7bb4f9ed648bdfaaca))

### Miscellaneous

- Sync Cargo.lock package versions to 0.14.85 ([3a05ded](https://github.com/cuttlefisch/mae/commit/3a05ded377cdf0a412905149aa4f88082de5715c))
- Bump version to 0.14.86 ([98c301b](https://github.com/cuttlefisch/mae/commit/98c301b9a581638ca6b14f7568a5835f53b58b3c))

## [0.14.85] - 2026-07-31




### Features

- *(scheme)* Three narrow SharedState accessors for #521 out-of-tree primitives ([a7e831a](https://github.com/cuttlefisch/mae/commit/a7e831a1f729c79ab880f2a74aa8c058d96d07fa))
- *(scheme-extra)* Wire in bilingual-kb-export-mae-bridge (#521) ([dd0f9c4](https://github.com/cuttlefisch/mae/commit/dd0f9c471354fa1012bcb4601266995e879125a2))
- *(kb)* Bundle a generic DevPractices KB, fix release/install gaps (#514, ADR-076) (#564) ([631a702](https://github.com/cuttlefisch/mae/commit/631a702e2ec84f4432778cfdf2f4d88fd9291022))

### Bug Fixes

- *(options)* Dynamic (define-option!) options were listed but unreadable ([04969b6](https://github.com/cuttlefisch/mae/commit/04969b663dc372b37b3dfa1652587ca89e495c79))
- *(ai-residency)* Classify kb_export_subgraph_html, a real gap this gate self-diagnoses ([5b51974](https://github.com/cuttlefisch/mae/commit/5b51974edfb8d156d60b031714198466d49cd085))

### Miscellaneous

- Update Cargo.lock for bilingual-kb-export-mae-bridge + pending 0.14.84 bump ([933714f](https://github.com/cuttlefisch/mae/commit/933714f6ce658e8555752cb093d20aad6107ef6a))
- Bump version to 0.14.85 ([e46d77e](https://github.com/cuttlefisch/mae/commit/e46d77e183acdb3b14ed0cb9b4e3b6918c5ee876))

## [0.14.84] - 2026-07-31




### CI

- *(windows)* Real window launch + SendInput click spike (ADR-066 Phase D, spike 2/2) (#563) ([62f89ce](https://github.com/cuttlefisch/mae/commit/62f89ce78a4a841bd7298a941a52504b58175559))

### Miscellaneous

- Bump version to 0.14.84 ([8e71f85](https://github.com/cuttlefisch/mae/commit/8e71f85c30f2caf18a3f1f1489512f75380e926b))

## [0.14.83] - 2026-07-31




### CI

- *(windows)* Add GUI build spike job (ADR-066 Phase D, spike 1/2) (#562) ([a420aa2](https://github.com/cuttlefisch/mae/commit/a420aa25b4e117b3b55866f6bf887df63f6bab0a))

### Miscellaneous

- Bump version to 0.14.83 ([2d97d3b](https://github.com/cuttlefisch/mae/commit/2d97d3bcd95a2166e4fd0a2e5d147bdb0b149c60))

## [0.14.82] - 2026-07-30




### Features

- *(daemon)* OAuth self-issued tokens bound to this daemon's identity (ADR-067 Phase D3a) (#561) ([a507a04](https://github.com/cuttlefisch/mae/commit/a507a044607fa255978839ab2cd46232090aff08))

### Miscellaneous

- Bump version to 0.14.82 ([d5d8c9c](https://github.com/cuttlefisch/mae/commit/d5d8c9cb2e88af92f85fec19db3be959667db91a))

## [0.14.81] - 2026-07-30




### Features

- *(daemon)* Wire kb/query.* into the mTLS collab handler (ADR-067 Phase D2) (#560) ([0fb8156](https://github.com/cuttlefisch/mae/commit/0fb81565eb45bdd5b6cc20c9cf6fa0b8653a64bc))

### Miscellaneous

- Bump version to 0.14.81 ([e219138](https://github.com/cuttlefisch/mae/commit/e219138d5ae3c37bb49bffb98499d2be6adc2429))

## [0.14.80] - 2026-07-30




### Refactor

- *(daemon)* Move kb_query into lib crate, fix ADR-067 doc drift (Phase D1) (#558) ([a97e2ba](https://github.com/cuttlefisch/mae/commit/a97e2ba37a1aebb51729869f237b934d209825c3))

### Miscellaneous

- Bump version to 0.14.80 ([475b35d](https://github.com/cuttlefisch/mae/commit/475b35de53849132fee5caa6f62c165e6fd03b91))

## [0.14.79] - 2026-07-30




### Features

- *(syntax,lsp)* IaC/DevOps language support — Terraform, Dockerfile, Ansible, Kubernetes, Helm (ADR-075) (#557) ([8ea8d1d](https://github.com/cuttlefisch/mae/commit/8ea8d1d93068cdfe405887d28f341728d5e4fe32))

### Miscellaneous

- Bump version to 0.14.79 ([9c3a3b8](https://github.com/cuttlefisch/mae/commit/9c3a3b8346095285e9954c7545db82098150610e))

## [0.14.78] - 2026-07-30




### Features

- *(daemon)* Live HTML KB view over OAuth listener, poll-based v1 (ADR-073, Phase E, #547) (#556) ([7b9ce30](https://github.com/cuttlefisch/mae/commit/7b9ce30e954623ba0b387c8e5e461adaf6a3b737))

### Miscellaneous

- Bump version to 0.14.78 ([c71bb91](https://github.com/cuttlefisch/mae/commit/c71bb913901a33ab43323ef78edb536dde9baf26))

## [0.14.77] - 2026-07-30




### Features

- *(canvas)* Mae-canvas substrate hardening — Wedge, ValueTween<T>, angular hit-testing, wedge options, fixed-pixel split primitive (ADR-070/072) (#555) ([b5769fa](https://github.com/cuttlefisch/mae/commit/b5769fa1a1d46b1368e61dfa6d11d74b1a7044d7))

### Miscellaneous

- Bump version to 0.14.77 ([75f64e6](https://github.com/cuttlefisch/mae/commit/75f64e60ee5b384f3a6ccfe127c2fa644c096be3))

## [0.14.76] - 2026-07-30




### Features

- *(module-system)* Init.scm KB graph-view guidance, pending_* docs, kb-register primitive (#539) ([d928f00](https://github.com/cuttlefisch/mae/commit/d928f006bf0b4c044e39f5411a72850edcaeb3bc))
- *(scheme)* Extra-kernel-crates extension point + JSON primitives (#540) ([128f0f5](https://github.com/cuttlefisch/mae/commit/128f0f5028c02b921f0af9434084ce282c24213b))

### Bug Fixes

- *(kb)* Backslash path-separator scoring bug (#534) + retry-deadline alignment (#518) (#537) ([8820f02](https://github.com/cuttlefisch/mae/commit/8820f028df14599b0a45b1f24268a1e10481a984))
- *(export)* PROPERTIES drawer leak (#528) + bare URLs, example blocks, checkboxes (#523) (#538) ([312baca](https://github.com/cuttlefisch/mae/commit/312baca3eaa03e66be9cc5a784f963935b3c3d59))

### Documentation

- *(adr)* ADR-070..074 — KB graph-view UX overhaul (canvas substrate, chord redesign, live daemon-hosted HTML view) (#541) ([bfc9c00](https://github.com/cuttlefisch/mae/commit/bfc9c00ef41f79cf3bb4cb7cc25392aa175983b9))

### Miscellaneous

- Bump version to 0.14.75 ([42dbf6d](https://github.com/cuttlefisch/mae/commit/42dbf6d7bd264b7149fad68aa59c3f6ee70e9345))
- Bump version to 0.14.76 ([f8587dd](https://github.com/cuttlefisch/mae/commit/f8587dd76f9066bd923a225ce611099e3c421a62))

## [0.14.74] - 2026-07-30




### Bug Fixes

- *(manual)* Convert markdown-flavored syntax to real org syntax (#535) ([ab170d6](https://github.com/cuttlefisch/mae/commit/ab170d66e870b2538bb4fc294967c5d03ba47f61))

### Miscellaneous

- Bump version to 0.14.74 ([895ea3f](https://github.com/cuttlefisch/mae/commit/895ea3f9337580b900ad9da2bdf7c2aa852338a8))

## [0.14.73] - 2026-07-30




### Bug Fixes

- *(export)* Heading misparse + UTF-8 byte-cast mangling (#536) ([90b0cd5](https://github.com/cuttlefisch/mae/commit/90b0cd514cefec8873852464d2011f72ded46e61))

### Miscellaneous

- Bump version to 0.14.73 ([7176010](https://github.com/cuttlefisch/mae/commit/717601002335f9d3c6499924c8c35f9ce11a4b43))

## [0.14.72] - 2026-07-29




### Features

- *(pickers)* #531 - boost recently-used items in typed-query ranking (#532) ([687955f](https://github.com/cuttlefisch/mae/commit/687955f9694836d9a4cd8f3fabe9ab2b752db444))

### Miscellaneous

- Bump version to 0.14.72 ([e3f4017](https://github.com/cuttlefisch/mae/commit/e3f4017752d8238e4fdc14b58a5ed87184f7bf08))

## [0.14.71] - 2026-07-29




### Miscellaneous

- *(deps)* Bump the rust-dependencies group across 1 directory with 13 updates (#529) ([6e409ff](https://github.com/cuttlefisch/mae/commit/6e409ffde4c2e8f8ef8ca7a88b9e18659254d9d1))
- Bump version to 0.14.71 ([23d9bac](https://github.com/cuttlefisch/mae/commit/23d9bacd6c849c09c60cdcee3f541333c7812219))

## [0.14.70] - 2026-07-29




### Bug Fixes

- *(rendering)* #353 - expand literal tab characters to their tab-stop width (#527) ([d2767db](https://github.com/cuttlefisch/mae/commit/d2767dba187d80e8d192bd70a5757072bb9ff961))

### Miscellaneous

- Bump version to 0.14.70 ([7602d10](https://github.com/cuttlefisch/mae/commit/7602d10ba0f6dbfa7b6e4ce2d11662e3d913564d))

## [0.14.69] - 2026-07-29




### Bug Fixes

- *(core)* #368 - extend multi-cursor visual-mode to indent/dedent/join/paste (#526) ([1fbd46d](https://github.com/cuttlefisch/mae/commit/1fbd46d3607657ce55672c4737ce7690b5a2a117))

### Miscellaneous

- Bump version to 0.14.69 ([0ec84ab](https://github.com/cuttlefisch/mae/commit/0ec84ab961df8be9bb17439453d1c18c5403aa85))

## [0.14.68] - 2026-07-29




### Bug Fixes

- *(kb)* #357/#366 search ranking + AI-residency Bucket B, #461 daemon --config (#525) ([f5750f9](https://github.com/cuttlefisch/mae/commit/f5750f963ef331d2e850a3c52f06546224046cad))

### Miscellaneous

- Bump version to 0.14.68 ([bbe8f49](https://github.com/cuttlefisch/mae/commit/bbe8f499e362fe9a8ee540edbd9906d5ef5a2ccf))

## [0.14.67] - 2026-07-29




### CI

- Dispatch mae-release event to mae-vscode on every release (#520) ([d65f63c](https://github.com/cuttlefisch/mae/commit/d65f63c527179d7f3d82061f541fba4231d479ad))

### Miscellaneous

- Bump version to 0.14.67 ([3f447b9](https://github.com/cuttlefisch/mae/commit/3f447b99e81b3f4562b6e1c902cbe3518bc1a5fe))

## [0.14.66] - 2026-07-29




### Security

- *(security)* Close eval_scheme AI-residency bypass (#478) + doc hygiene pass (#519) ([8681120](https://github.com/cuttlefisch/mae/commit/868112056071c41f07bc9fcbd3901ce1029c258e))

### Miscellaneous

- Bump version to 0.14.66 ([3094155](https://github.com/cuttlefisch/mae/commit/30941558dc9bc2693016b94db35f275cca337b82))

## [0.14.65] - 2026-07-29




### CI

- Fix ANSI-color parse bug in test-count grep, closes CI reliability sweep (#510) (#517) ([c65fabf](https://github.com/cuttlefisch/mae/commit/c65fabf81f5a64628833e8cf6f8699e1bdf87aa6))

### Miscellaneous

- Bump version to 0.14.65 ([00f3cca](https://github.com/cuttlefisch/mae/commit/00f3cca4e796268eea7f789a8c72d03090be7b1a))

## [0.14.64] - 2026-07-28




### CI

- Migrate Windows leg to nextest, closing #484 for real (#510 CI reliability sweep) (#516) ([6520c87](https://github.com/cuttlefisch/mae/commit/6520c872eac6d58acdf7c5f2151807025be0b3d0))

### Miscellaneous

- Bump version to 0.14.64 ([3e5cb13](https://github.com/cuttlefisch/mae/commit/3e5cb131b921b945255bc2f78cdf1de63f7294ef))

## [0.14.63] - 2026-07-28




### CI

- Add tool_schema_nested_e2e to heavy group via exhaustive sweep (#510) (#515) ([a4bda24](https://github.com/cuttlefisch/mae/commit/a4bda24d508d790495cfa794e71ba283f003ad2b))

### Miscellaneous

- Bump version to 0.14.63 ([a1125ef](https://github.com/cuttlefisch/mae/commit/a1125ef0370edd983cf0daea171fee5a6f997642))

## [0.14.62] - 2026-07-28




### CI

- Fix 3 latent bugs surfaced by #511's own verification run (#513) ([00766e4](https://github.com/cuttlefisch/mae/commit/00766e4e34435bce17c3388b8a973f6e5d2a20c1))

### Miscellaneous

- Bump version to 0.14.62 ([3992423](https://github.com/cuttlefisch/mae/commit/399242398a8b1b13babc2069a5656acbb6a429b3))

## [0.14.61] - 2026-07-28




### CI

- Structurally isolate heavy-subprocess-e2e from the full-suite startup burst (#510) (#511) ([0862b82](https://github.com/cuttlefisch/mae/commit/0862b82e99c65c8d27b6725dccc8763a10e5751b))

### Miscellaneous

- Bump version to 0.14.61 ([80eed08](https://github.com/cuttlefisch/mae/commit/80eed08eb5a4da9a3ce12d36dcfc2c208ec548fc))

## [0.14.60] - 2026-07-28




### Features

- *(daemon)* ADR-060 Phase E — mae-daemon@.service systemd template (#413) ([402b6df](https://github.com/cuttlefisch/mae/commit/402b6df7a6c5feadc6fc09b1084b5609c3de8cdc))
- *(daemon)* ADR-060 Phases F+G — N-tenant benchmark + config-change contract (#414, #415) ([afc30fb](https://github.com/cuttlefisch/mae/commit/afc30fb665bf59a8807d8a5d65e7ff0ab3851cb6))
- *(daemon)* ADR-066 Phase E — Windows remote-daemon verification (#375) ([0034ec2](https://github.com/cuttlefisch/mae/commit/0034ec252cb79da15c9eba9ec1f8291c306a6066))
- *(graph-view)* Default center to active project's KB instance (#462 Part 1 / PR2) ([afc7c3c](https://github.com/cuttlefisch/mae/commit/afc7c3cfbc2e3fa008a4873ea72146c4494e1bab))
- *(kb)* Cross-instance link detection for multi-KB graph view (#462 PR4 Part 1) ([f4ec9a3](https://github.com/cuttlefisch/mae/commit/f4ec9a3ee60a25877286e119e023b4d7ce1d706f))
- *(kb)* Multi-KB chord graph view composition (#462 PR4 Part 2/3) ([783c4e5](https://github.com/cuttlefisch/mae/commit/783c4e5be30f3566151c7690542831618bdcac63))
- *(kb)* TUI parity for multi-KB graph view (#462 PR4 Part 4) ([4e31524](https://github.com/cuttlefisch/mae/commit/4e3152410ec01c72a5da2d6e35ca3713247d5bef))
- *(ai)* ADR-061 Phase A -- pluggable embedding provider (Ollama-first) ([cb87e8b](https://github.com/cuttlefisch/mae/commit/cb87e8bebbb8c86fd8e00a8e7eab62cb9d269a83))
- *(sync)* ADR-067 Phase A -- adversarial tests + implementation note ([c95a1d0](https://github.com/cuttlefisch/mae/commit/c95a1d0f778982931f7440af9102edc7744b181a))
- *(kb-graph)* Phase A4 -- per-diagram health/loaded indicator (#479) ([e63699a](https://github.com/cuttlefisch/mae/commit/e63699a11b7dd409359eab519895b870b757e49d))
- *(kb-graph)* Phase B1 -- full-corpus extraction for multi-KB chord view ([1ef6a46](https://github.com/cuttlefisch/mae/commit/1ef6a46cbf018f191115f787d9f95820acbc8ef7))
- *(kb)* ADR-068 Phase B3 -- KnowledgeBase::hop_distances_from ([883f217](https://github.com/cuttlefisch/mae/commit/883f217085b60dc29b6c3a0d932b3625ae2e9857))
- *(kb-graph)* ADR-068 Phase B2-B5 -- DOI/LOD render-tier core ([4567573](https://github.com/cuttlefisch/mae/commit/45675736f1b6170499eed0139d4c19845eace6f9))
- *(kb-graph)* ADR-068 Phase B6 -- 4 remaining DOI/LOD tiering options ([6d95539](https://github.com/cuttlefisch/mae/commit/6d955398ccfc47cefeceb4a8987b80e0b7f0b9bd))
- *(kb-graph)* ADR-068 Phase B8 -- TUI decision for full-corpus text listing ([2ca20ca](https://github.com/cuttlefisch/mae/commit/2ca20cac8d573e84fc46607fd2743b5f0f5e33df))
- *(scheme)* ADR-068 -- kb-graph-view-state render-tier AI-peer parity ([62e750c](https://github.com/cuttlefisch/mae/commit/62e750c079ca47912f1b1216b88ea990730c2b0a))
- *(kb)* ADR-061 Phase B -- content-addressed embedding cache ([c1987b3](https://github.com/cuttlefisch/mae/commit/c1987b309e538bd0b95a594c52e5f69e647e7faf))
- *(daemon)* ADR-067 Phase B — kb_access Join/Read split + kb_join enforcement ([293660b](https://github.com/cuttlefisch/mae/commit/293660b75db6e9fdcd5f8abc727a3b989a76e198))
- *(kb)* ADR-067 Phase C + Phase E — QueryOnly key delivery + residual-risk visibility ([e34a9b4](https://github.com/cuttlefisch/mae/commit/e34a9b4908a18dbc7096a4553fae9718c9a552e5))
- *(kb)* ADR-061 Phase C + Phase E — enrichment scheduler wiring + enrich-now path ([3b5d29b](https://github.com/cuttlefisch/mae/commit/3b5d29b75cfab0b1bc744a0b0af27bc15ba14742))
- *(kb)* ADR-061 Phase D1 — real ADR-033 advisory lease primitive (#420) ([7ed875a](https://github.com/cuttlefisch/mae/commit/7ed875a3ae49343898cb7ce05852ba8d7df98a59))
- *(kb)* ADR-061 Phase D2 — enrichment becomes the lease's first caller (#420) ([37ce9c1](https://github.com/cuttlefisch/mae/commit/37ce9c13c969c12e403f18a78318022767eb0bd8))
- *(kb)* ADR-061 Phase D3 — ADR-034 cross-peer artifact sharing (#420) ([d5b967a](https://github.com/cuttlefisch/mae/commit/d5b967a1e1c852bb310c34bf299898ea25e725aa))
- *(kb)* ADR-061 Phase F -- kb_vector_search wiring + RRF blend, fix real macOS/Linux KB-watcher CI flakes (#498/#502) ([f6246e1](https://github.com/cuttlefisch/mae/commit/f6246e168e941ee8afca20bcb982cb589654256a))
- *(kb-graph)* Zoom-legibility floor lets DOI-elided nodes populate back in ([4d92973](https://github.com/cuttlefisch/mae/commit/4d9297315117bd875635efe6cf7846ab8e0c890d))
- *(kb-graph)* Weight-driven edge opacity + configurable edge width ([272377c](https://github.com/cuttlefisch/mae/commit/272377c1fa3755ae680e98e03eea85d62b3fe7b2))
- *(kb-graph)* Opt-in opacity-taper toward a curved edge's midpoint ([bc75011](https://github.com/cuttlefisch/mae/commit/bc750115e9278b680437cf30289f78bccc754142))

### Bug Fixes

- *(graph-view)* Audit fixes for stale layout race, label-cache, dedup, dead code, self-links (#462 PR1) ([b12f779](https://github.com/cuttlefisch/mae/commit/b12f779b9cf7dcbf2b6af20318e41c68c6961ab8))
- *(daemon)* Load federated KB instances at startup (#460) ([29436e4](https://github.com/cuttlefisch/mae/commit/29436e430b6fd0fbe9d6fc427ca6ddac5693443f))
- *(ci)* Daemon workspace fmt + real WSL2 distro for Phase E remote verify ([ed67fac](https://github.com/cuttlefisch/mae/commit/ed67fac8e55b2a474dcc02eff890ff37a144c850))
- *(kb)* Opt-in lightweight subgraph extraction (#462 PR3) ([ae936a6](https://github.com/cuttlefisch/mae/commit/ae936a6afbd6f0cd440ce886404562ca1d858966))
- *(kb)* Recompute per-diagram node_count after residency filter (#462 access-model review) ([aab54dd](https://github.com/cuttlefisch/mae/commit/aab54dd6e168ddadaf62370aad3bb46741c26432))
- *(ci)* Clippy 1.97 lints + stale code map on PR #480 ([b5b56f3](https://github.com/cuttlefisch/mae/commit/b5b56f36d0baa3f5711688405e267c961a8a277b))
- *(ci)* Clippy 1.97 cloned_ref_to_slice_refs lint in multi-KB test ([3e3e0e5](https://github.com/cuttlefisch/mae/commit/3e3e0e5a067e641f4857425ef30404d05d4a0b94))
- *(kb)* Reconcile FederatedQuery::health_report across federated instances (#474) ([d2f6b6c](https://github.com/cuttlefisch/mae/commit/d2f6b6cc5def0204d8c0082043e31da3d623493e))
- *(kb-graph)* Phase A6 configuration gaps in multi-KB chord view ([0815ef4](https://github.com/cuttlefisch/mae/commit/0815ef47a648bb7447207016fc0d6b6ac643bd37))
- *(kb-graph)* Phase A2 -- detect cross-instance links between two non-seed diagrams ([02b251c](https://github.com/cuttlefisch/mae/commit/02b251cbf9f240ff09a07cb9989b36120b40dbc7))
- *(kb-graph)* Phase A3 -- chord curvature bows toward each diagram's own center ([ecebd8e](https://github.com/cuttlefisch/mae/commit/ecebd8e7f685820671eeadd6a99af92699186c11))
- *(kb-graph)* Phase A1 -- overlay input-focus trap in focus_window_at ([bd2ac4d](https://github.com/cuttlefisch/mae/commit/bd2ac4dfdd006e8b0f3a5eae99800d5ac8d05e95))
- *(kb)* Phase A5 -- kb_cleanup_orphans deletes only from primary (#485) ([7dc452e](https://github.com/cuttlefisch/mae/commit/7dc452e453bb99c5333e67cf195854cf34fad9ab))
- *(babel)* Retry compiled-binary spawn on ExecutableFileBusy (issue #482) ([66199a1](https://github.com/cuttlefisch/mae/commit/66199a1d0680de4d4ea66bdb096382ba4b474a5f))
- *(kb)* Extract_subgraph no longer phantom-includes unresolvable BFS targets (#493) ([cb8ccce](https://github.com/cuttlefisch/mae/commit/cb8ccce7dc423db15d832ead4ca986a5c9a503c7))
- *(kb-graph)* Widen multi-KB grid columns for long diagram names ([e6739e5](https://github.com/cuttlefisch/mae/commit/e6739e51ad5f192c904abfc164f23dae0f9715b5))
- *(kb-graph)* Cluster-stub visual — spatial-aware bucketing + clearer copy ([bce06ad](https://github.com/cuttlefisch/mae/commit/bce06ada5b0230834167f5c378055d0930863d65))
- *(kb)* Widen external_store_change_arms_a_background_reload's deadline (#494) ([5847e98](https://github.com/cuttlefisch/mae/commit/5847e983fb1636d308cbd03aea4bbccd339d724b))
- *(kb)* The real fix for #494 -- simulate a genuinely separate writer ([cd25c41](https://github.com/cuttlefisch/mae/commit/cd25c4193760eca2cb13d3bac8a56b573cbcf2b0))
- *(kb)* Root-cause and fix #455/#498 -- kb_reimport_file path canonicalization mismatch ([d0c9e76](https://github.com/cuttlefisch/mae/commit/d0c9e76945df20c315886d8df08473a1a83f9e14))
- *(kb)* Bound SQLite busy-retry by wall-clock deadline, not attempt count (#484) ([932593b](https://github.com/cuttlefisch/mae/commit/932593bb857a56daba10fa975afb861a59d15b41))
- *(babel,collab)* Root-cause + fix remaining known macOS CI flakes (#500, #506) ([8a5cbb5](https://github.com/cuttlefisch/mae/commit/8a5cbb538ea6e48e0cde1c187103f99c9e374d90))
- *(kb)* Raise busy-retry deadline 20s -> 45s for Windows' in-process cargo-test contention ([7e0fafc](https://github.com/cuttlefisch/mae/commit/7e0fafc307ba3118462fc165f63a44f4060f509c))
- *(kb)* Canonicalize paths before instance-membership comparison (#496) ([d60b72c](https://github.com/cuttlefisch/mae/commit/d60b72c933eccf613e56e5398f1d0dc162c42359))
- *(kb-graph)* Stop drawing edges to Hidden/Clustered-tier nodes ([9867290](https://github.com/cuttlefisch/mae/commit/986729025843e60e4871103abee427839254b6b4))
- *(kb-graph)* Theme change now repaints the graph view without a click ([19de5da](https://github.com/cuttlefisch/mae/commit/19de5da8befa0df4687205029dc60ce4bffcbefb))
- *(kb-graph)* Zoom-out floor is now dynamic, not a flat 0.1 constant ([7fe2096](https://github.com/cuttlefisch/mae/commit/7fe209692b20048714ccb4abb48459a422e61a5d))
- *(kb-graph)* Revert edge-tier-skip -- edges must always render ([2c8d240](https://github.com/cuttlefisch/mae/commit/2c8d2407a8b568a7ae3c4bcbd3968fefdd99e8ba))
- *(kb-graph)* Cluster stub sits on the ring, not pulled inside it ([505a358](https://github.com/cuttlefisch/mae/commit/505a358acabeecf86a451a4167cf3bcba633d45b))
- *(kb)* Nested list-item :ID: drawers lose their opening lines on wrap ([fb5e4db](https://github.com/cuttlefisch/mae/commit/fb5e4db97dd15e4cb7f8bfdc48dead2508f696f1))
- *(kb-graph)* Guarantee an isolated node's label survives declutter ([00d0fb4](https://github.com/cuttlefisch/mae/commit/00d0fb4331f9038b6387b78842ca5449c9b79c86))
- *(mcp)* Declare tools/list_changed and actually send it ([a0808f8](https://github.com/cuttlefisch/mae/commit/a0808f8979a36c41219e251616bcaffb54c6583f))
- *(ci)* Portable harness_spawn fallback when setsid is unavailable ([c18ec13](https://github.com/cuttlefisch/mae/commit/c18ec133a4091ebf38f2b91eb1317b780b1d087c))

### Performance

- *(kb-graph)* Decouple expensive DOI candidate BFS from continuous zoom ([2a95b3d](https://github.com/cuttlefisch/mae/commit/2a95b3de917d6975173a85fb31059a3f90806f31))

### Refactor

- *(kb)* Extract compute_in_degree_map + KnowledgeBase::linked_in_degree (#474) ([3dbecb3](https://github.com/cuttlefisch/mae/commit/3dbecb3b4cf806329dfa3ec79bd2dbbbcfcfdfbc))

### Documentation

- *(adr)* ADR-066 Phase E round-1 implementation note; regen ADR KB ([4999322](https://github.com/cuttlefisch/mae/commit/49993221ee053618416dd8b0d7c26d2417f3183f))
- *(adr)* Fix stale ADR-057/066 status text; reconcile 6 tracking issues ([53f997e](https://github.com/cuttlefisch/mae/commit/53f997e0500cac85d326453a31c119d43a2b1b68))
- *(kb)* #462 PR4b — docs/API-parity pass for multi-KB chord graph view ([382a602](https://github.com/cuttlefisch/mae/commit/382a60297a47038a5e4aaed3e7d66d9a8c6cdfd7))
- *(adr)* Cross-link issue #474 as a second health_report drift correction ([63fa9bc](https://github.com/cuttlefisch/mae/commit/63fa9bc37a1898db80c32931bd1fea93b3339a45))
- *(adr)* ADR-068 -- full-corpus multi-KB retrieval + DOI-based LOD rendering ([6547bc0](https://github.com/cuttlefisch/mae/commit/6547bc0d7f50841284920586e8114926915104e1))
- *(adr)* ADR-069 -- scope force-directed edge bundling (not implemented) ([d306083](https://github.com/cuttlefisch/mae/commit/d30608394ef9e62887fc95e39d88064fa55ce01c))

### Testing

- *(core)* Kb_cleanup_orphans federation orphan-safety regression tests (#474) ([a265dd4](https://github.com/cuttlefisch/mae/commit/a265dd4d55833d27ed9aa75719d0971f61ab4d93))
- *(kb)* Consolidate watcher poll-loop + widen deadline for macOS CI (#494) ([b1ab9ee](https://github.com/cuttlefisch/mae/commit/b1ab9eedc8ced16f3567484eca4a4255927addc6))

### CI

- Raise version-bump's CI-wait poll budget from 35 to 60 minutes ([20a3e77](https://github.com/cuttlefisch/mae/commit/20a3e7747e6c5ef20698b349fdb73bda4109d479))
- Adopt cargo-nextest for the biggest test legs + add a macOS CI job ([33d875a](https://github.com/cuttlefisch/mae/commit/33d875ae9e3dacd5f4df192f45378a3e8e355914))
- Add sccache + mold to setup-rust for cross-job compile caching (#491) ([b525a72](https://github.com/cuttlefisch/mae/commit/b525a72e76358e3a814895236addd7ac036c6284))
- *(badges)* Fix test-count badge silently undercounting (452 vs ~7,223+) ([2bff279](https://github.com/cuttlefisch/mae/commit/2bff279c8a0a8aab2a7bbcf2e5f7696b7d19bc92))
- *(windows)* Skip new kb_reimport_file test for the same pre-existing #455 gap as its sibling ([ffbf7ea](https://github.com/cuttlefisch/mae/commit/ffbf7ea6df1583182bf2fc9219835bbf082989bb))
- Fix Version Bump gating on unrelated Badges failures + raise heavy-subprocess-e2e retries (#510) ([b466db8](https://github.com/cuttlefisch/mae/commit/b466db855173330cb390f37f942b9dc41f9f6742))

### Miscellaneous

- Bump version to 0.14.56 ([a5988d5](https://github.com/cuttlefisch/mae/commit/a5988d52b124841a00bed8d55218f48521b85585))
- Sync Cargo.lock with 0.14.56 version bump ([a67263e](https://github.com/cuttlefisch/mae/commit/a67263eeca69590f28b838679731b804fefb63d9))
- *(ci)* Efficiency pass -- composite actions, cache scoping, MSRV leg, path filters ([c420331](https://github.com/cuttlefisch/mae/commit/c42033181bd3e6fdf2d2f3fb4eb9a8f813738bb8))
- *(adr-kb)* Regenerate assets/mae-adr.cozo for ADR-034's status correction ([909622b](https://github.com/cuttlefisch/mae/commit/909622bbc8cc55d5bd272570165c1704026a2b84))
- Bump version to 0.14.57 ([15ec5de](https://github.com/cuttlefisch/mae/commit/15ec5de8879c4bf5345c49a4f32797d538a716cd))
- Bump version to 0.14.58 ([2d5855e](https://github.com/cuttlefisch/mae/commit/2d5855ebcd6c63ac8a0b33eeaf7cb4407555ae2e))
- *(adr-kb)* Regenerate assets/mae-adr.cozo for ADR-069 ([9facbe7](https://github.com/cuttlefisch/mae/commit/9facbe71646ef5a9d5dd6b84f84b74affae06446))
- Bump version to 0.14.59 ([ba9a5d5](https://github.com/cuttlefisch/mae/commit/ba9a5d5089384afb34fbee4c05e2d814f4efcf2d))
- Bump version to 0.14.60 ([18b5c77](https://github.com/cuttlefisch/mae/commit/18b5c7780bc9354af37624ceebc00da5ac1beb41))

## [0.14.55] - 2026-07-26




### Security

- *(security)* Close 3 real gaps found in a full-branch adversarial review ([e17768a](https://github.com/cuttlefisch/mae/commit/e17768a8deeb695dbb95dde4aea9331da841a6eb))

### Features

- *(mcp)* ADR-056 — session-scoped tool-category dispatch enforcement ([edefb8d](https://github.com/cuttlefisch/mae/commit/edefb8d39271d6595ad282f8af4973c00de6bf82))
- *(kb)* ADR-058 — per-project KB provisioning & KbScope::Project ([f6b7484](https://github.com/cuttlefisch/mae/commit/f6b7484a9821009b30f30798d577a365ce0bb3c4))
- *(kb)* ADR-059 — ADR-as-KB-node generalization (molecular decision records) ([ab5b384](https://github.com/cuttlefisch/mae/commit/ab5b384527a9ed20a1375f95ce7796179785230e))
- *(kb)* ADR-062 — federation registry scaling & unified local/remote-hub search ([230f400](https://github.com/cuttlefisch/mae/commit/230f400d175a7cda581733606d2b9cce22808714))
- *(ai)* ADR-063 — guidance-delivery uniformity across MCP clients ([bcd95ec](https://github.com/cuttlefisch/mae/commit/bcd95ec880ae9a1e3d18c4036a15e0a85fd701fa))
- *(mcp)* ADR-066 Phases A/B/C — Windows client support (local IPC + CI) ([8da1ebc](https://github.com/cuttlefisch/mae/commit/8da1ebc6e20b3ae65049855a491ff5be722fd081))
- *(daemon)* ADR-060 Phase A — per-tenant RPC addressing ([ede6b33](https://github.com/cuttlefisch/mae/commit/ede6b33a45b85258da309d30ad7b2ab30571f93a))
- *(daemon)* ADR-060 Phase B — verify concurrency isolation, no rewrite needed ([aae88b4](https://github.com/cuttlefisch/mae/commit/aae88b44419783ba9f8f4749cc6bebbbbc0d0ec0))
- *(daemon)* ADR-060 Phase D — IDOR + cross-KB role isolation, verified not rewritten ([126e201](https://github.com/cuttlefisch/mae/commit/126e201e0e7bcce6cd9b0c72c21fc10d8024a60c))
- *(daemon)* ADR-060 Phase C — per-tenant quotas + independent eviction (#411) ([7d9419a](https://github.com/cuttlefisch/mae/commit/7d9419aa4626eef98f860b7be67eccd244663f19))
- *(mae)* ADR-050 Phase H — :kb-export-guidance colon command (#383) ([c7794e3](https://github.com/cuttlefisch/mae/commit/c7794e3517203b30e5616928c1cd1266a6e6c722))

### Bug Fixes

- *(daemon,vscode)* 5 bugs from a QA/adversarial-review pass on Phases F/G/I ([d415e84](https://github.com/cuttlefisch/mae/commit/d415e84999ef8a145b28163b6d1708b6f0da9442))
- *(vscode)* Headless startup timeout too tight, make it configurable ([c2edc5f](https://github.com/cuttlefisch/mae/commit/c2edc5f04782b7ee61a84ccd067b756d689b1a8e))
- Post-ship quality pass — seed provenance, MCP tool tiering, guidance auto-config (K1-K5) ([a8ef364](https://github.com/cuttlefisch/mae/commit/a8ef36485d42443a5097a3397e144731745bfebf))
- Close out epic issues #376-385 for real — DoD verification pass (L1-L7) ([220169e](https://github.com/cuttlefisch/mae/commit/220169e9d0619f4ee2f583cb813ab00b36428d6e))
- 2 real CI failures — mae-daemon not built for new e2e test, flaky babel test collision ([75f9703](https://github.com/cuttlefisch/mae/commit/75f9703d7775a3d1cdd15e09e3b7a21dc9dc0c3a))
- *(daemon)* Eliminate a real CI-observed race in the OAuth cap test ([394254b](https://github.com/cuttlefisch/mae/commit/394254b6ac18c4ae0726863bde867728ce0d0349))
- *(vscode,ai)* Pre-extraction hardening from a VS Code extension best-practices review ([af8fe7e](https://github.com/cuttlefisch/mae/commit/af8fe7e0e16d040de8ff70fa85bc4221d8cc498d))
- *(kb,daemon)* ADR-065 — 4 drift corrections closing gaps behind sibling-proven patterns ([940864b](https://github.com/cuttlefisch/mae/commit/940864b6e6268fb9ae35af6927cc989bff39b1aa))
- *(ci)* ADR KB staleness gate referenced a script that was never created ([bdba243](https://github.com/cuttlefisch/mae/commit/bdba2433627ad924af0f8de712594980719c7669))
- *(mcp)* Real Windows CI failure — daemon_client.rs BufRead/Ordering imports ([7e2fd02](https://github.com/cuttlefisch/mae/commit/7e2fd02e78dec6e4477314bba142e341181a6435))
- *(shell)* Real Windows CI failure — alacritty_terminal's Pty has no .child() there ([b6fcfce](https://github.com/cuttlefisch/mae/commit/b6fcfce3d05708523ffee327525e847007639745))
- *(windows-ci)* Gate remaining Unix-only libc calls behind cfg(unix) (ADR-066) ([5b17149](https://github.com/cuttlefisch/mae/commit/5b17149424fee12a318ab568d7e62b8bdae8a09d))
- *(windows-ci)* Gate mae-mcp socket tests + skip pre-existing mae-core Windows failures ([b9801a4](https://github.com/cuttlefisch/mae/commit/b9801a4cdaeb27f9f90a4638c40d27c59e569f66))
- *(windows-ci)* Force bash for the multi-line --skip test command ([0aac8e6](https://github.com/cuttlefisch/mae/commit/0aac8e60d8ddd423ecc35f2b9b7681c88c05c4eb))
- *(windows-ci)* Gate remaining raw Unix-socket test code across 4 files (ADR-066) ([94e1bfd](https://github.com/cuttlefisch/mae/commit/94e1bfdb873243c8d8dd6f8d0f52d7e3ee0e8fe1))
- *(windows-ci)* Real process-liveness check for stale-lock cleanup on Windows ([eab9167](https://github.com/cuttlefisch/mae/commit/eab9167faa90ff24405bde9b6a60078ec3714f35))
- *(windows-ci)* Consolidate PID-liveness checks, fix pid_t overflow + EPERM handling ([696e01e](https://github.com/cuttlefisch/mae/commit/696e01e15b42462fabd3e311ab9f4d67203adf15))
- *(windows-ci)* Remove hardcoded PID-1 assumption from a portability test ([117b64e](https://github.com/cuttlefisch/mae/commit/117b64e0e0c3e70b9db011ab090b68862b50942a))
- *(windows-ci)* Gate daemon_client's next_id field + read_cl_message as dead on Windows ([9edbe65](https://github.com/cuttlefisch/mae/commit/9edbe65b85e23094dda4e17398e9ea0c2d68f81c))
- *(windows-ci)* Gate 3 more dead-Windows items + suppress LispError's borderline size lint ([b7e4b48](https://github.com/cuttlefisch/mae/commit/b7e4b484f009e559f13add8222583835e56e1ba6))
- *(windows-ci)* Gate a whole PTY integration test file + 2 self-inflicted lints ([0b62543](https://github.com/cuttlefisch/mae/commit/0b62543876a8a8540aafa43edcbb2a9cbfa81be9))
- *(kb)* Serialize concurrent first-time CozoDB store creation (#447) ([8f9cbb3](https://github.com/cuttlefisch/mae/commit/8f9cbb39396484b193dbc038dfd632dd6d29d957))
- *(kb)* Retry transient SQLITE_BUSY panic during store open (nightly CI) ([693214e](https://github.com/cuttlefisch/mae/commit/693214e5e3d81210b966f3b19127050f263d896c))
- *(ai)* Contain a panicking tool implementation instead of crashing the editor ([da26526](https://github.com/cuttlefisch/mae/commit/da265261f0b8b98c3625821362186213e4eae918))
- *(mcp)* Close a TOCTOU race in authorized_keys authorize/revoke ([a5c1c49](https://github.com/cuttlefisch/mae/commit/a5c1c4905df91dd75d75803e69a31e044af71ab0))
- *(kb)* Close two more gaps in the SQLite busy-retry coverage ([29b9b43](https://github.com/cuttlefisch/mae/commit/29b9b43e36d2a9c277d27bf4517333adb74360ba))

### Documentation

- *(adr)* ADR-057..066 — MAE long-term architecture vision + 9 closing ADRs ([c3e3dfe](https://github.com/cuttlefisch/mae/commit/c3e3dfe415615b9e930d8cd65ce8e55d4ed37fa8))
- *(adr)* Add ADR-067 — admin-enforced live-query-only KB access ([1449c9a](https://github.com/cuttlefisch/mae/commit/1449c9a39aa5727e9811055fb57398026eff3833))
- *(adr-066)* Mark Phase C confirmed green on real windows-latest CI ([3d09d06](https://github.com/cuttlefisch/mae/commit/3d09d0639e22fca1cae8a0aee1d27906f469ac09))
- *(adr-060)* Correct Phase C's design — no existing throttle, two quota keys not one ([d1c8939](https://github.com/cuttlefisch/mae/commit/d1c89395af813399410a1ce8ea4ab4ebdbcce149))
- *(adr)* Ratify ADR-057 — MAE architecture vision (issue #395) ([0cce0b8](https://github.com/cuttlefisch/mae/commit/0cce0b801082d73f96ffde52d5b63dcdbc71bf05))
- *(adr)* Correct ADR-004's WAL/busy_timeout claim for the KB store ([424c8fc](https://github.com/cuttlefisch/mae/commit/424c8fc68f9b1ee733cb6c18fc6ce90c1c4d45fc))
- *(adr)* Flag ADR-011's storage model as superseded by ADR-029 ([7d6f812](https://github.com/cuttlefisch/mae/commit/7d6f8120d566b5a35f79247067547f6add5a00a8))

### Testing

- Real subprocess/TLS e2e for headless mode + OAuth/kb-query (Phases E/F/G) ([2d96c8b](https://github.com/cuttlefisch/mae/commit/2d96c8b4613fc12a1529ac25de1471723dc4ef52))
- *(vscode)* Real mae/mae-mcp-shim binary round trip in CI ([5ba49ab](https://github.com/cuttlefisch/mae/commit/5ba49ab6721ea800be7e520c31076f7b389a0fe7))
- *(docker)* Docker-headless-e2e compose service + CI job (Phase J, #385) ([413599c](https://github.com/cuttlefisch/mae/commit/413599cd1db62888bdbc6e1938b0c1cfb99f2801))

### Miscellaneous

- Sync Cargo.lock after merging origin/main's 0.14.54 version bump ([dbda687](https://github.com/cuttlefisch/mae/commit/dbda6874acd86921cfd85a09bbc1b404d65ae95f))
- *(vscode)* Add LICENSE (GPL-3.0-or-later) + fix npm audit properly ([7c6cdc2](https://github.com/cuttlefisch/mae/commit/7c6cdc277c8fd1df1eec4155337fc03e98f3024f))
- *(vscode)* Extract "MAE for VS Code" into its own repository ([0ab3ae2](https://github.com/cuttlefisch/mae/commit/0ab3ae24f3793d2f38440ad257d671dbdcf8b82d))
- *(kb)* Regenerate ADR KB asset for ADR-062's Status flip to Accepted ([929defa](https://github.com/cuttlefisch/mae/commit/929defa783f500b262645867ce79b2a03ea2e2f7))
- Bump version to 0.14.55 ([6dffff9](https://github.com/cuttlefisch/mae/commit/6dffff90ccee993a7ed3da1f86cb086224e115de))

## [0.14.54] - 2026-07-23




### Features

- *(mcp)* Phase C — per-session permission policy + DrivenWindow isolation (#378) ([383c172](https://github.com/cuttlefisch/mae/commit/383c172f4b5badf2f9a0e005727d4c83cab4df69))
- *(mae)* Phase E — headless MAE service mode (#380) ([a756e8f](https://github.com/cuttlefisch/mae/commit/a756e8f21c370689937131ea9917318006812a67))
- *(daemon)* Phase F — OAuth 2.1 resource server (#381) ([8876896](https://github.com/cuttlefisch/mae/commit/8876896082c4072c2166af672f422a4663d64e6c))
- *(daemon)* Daemon concurrency hardening & benchmarked capacity figure (ADR-054, #379) ([74fd476](https://github.com/cuttlefisch/mae/commit/74fd4768a42c3294b15499052e1158df6798ebb2))
- *(ai)* Guidance delivery robustness — kb_export_guidance fallback exporter (ADR-050 D4, #383) ([45d78dc](https://github.com/cuttlefisch/mae/commit/45d78dc93b026bdacab5d83444ce7cfa044cc6cf))
- *(daemon)* Live scoped read-through KB query surface (ADR-053, #382) ([f8a747a](https://github.com/cuttlefisch/mae/commit/f8a747a5e8709de31bc3fcb6b7ecbf1a93019184))
- *(vscode)* MAE for VS Code extension (ADR-050 D1 full, Phase I, #384) ([6d89a49](https://github.com/cuttlefisch/mae/commit/6d89a494b4314fa078f49ee4c933fe0c45d7f67d))

### Documentation

- Cross-link RUSTSEC-2026-0215 exception to tracking issue #374 ([226829e](https://github.com/cuttlefisch/mae/commit/226829e8b860bd7e0c51fa43250a23f18d96075e))
- *(adr)* Propose ADR-050..055 for external-editor MCP pairing initiative ([af14420](https://github.com/cuttlefisch/mae/commit/af14420ba3a6de9d61eadeab934d8b3908014142))
- *(mcp)* Minimum viable local VS Code pairing + generic cross-editor docs (ADR-050, #377) ([e64f452](https://github.com/cuttlefisch/mae/commit/e64f452c1c0984cee18b5218ba617a3da770462b))

### Testing

- *(vscode)* Close the orphan-cleanup DoD gap for #384 ([8d889a2](https://github.com/cuttlefisch/mae/commit/8d889a22af6f1beb9cc74a4105dff126fef0b9de))

### Miscellaneous

- Accept RUSTSEC-2026-0215 (smallstr unmaintained, transitive via yrs) ([8c885ff](https://github.com/cuttlefisch/mae/commit/8c885ff784be1bc11b66edd82e334720f3e529f3))
- Bump version to 0.14.54 ([c451f62](https://github.com/cuttlefisch/mae/commit/c451f62bbf5a31bdc18d58fe75716121e8496b1d))

## [0.14.53] - 2026-07-22




### Features

- *(graph-view)* Chord-diagram theming polish — edge alpha, muted default color, radial labels, quieter boundary badges ([2c8b01b](https://github.com/cuttlefisch/mae/commit/2c8b01b59caa0916e326c13d99bb6d0b92b08dca))
- *(kb)* Scope-aware kb_health and kb_agenda ([fd1ec98](https://github.com/cuttlefisch/mae/commit/fd1ec987f1365c0d5773a6ae7d861d6b9e79e022))
- Enforced companion-window scope, default practices KB, graph zoom-to-fit ([c2a7207](https://github.com/cuttlefisch/mae/commit/c2a720730f7846d6ee82b43dbf729da817f29203))

### Miscellaneous

- Bump version to 0.14.53 ([c18351d](https://github.com/cuttlefisch/mae/commit/c18351d591b9455e0ec827fd3e852b25df67e122))

## [0.14.52] - 2026-07-22




### Features

- *(graph-view)* Add a chord-diagram (circular) layout mode, make it the default (#367) ([4ff44bf](https://github.com/cuttlefisch/mae/commit/4ff44bfaa3068072ffac0691446658d7a12fead3))

### Bug Fixes

- *(syntax)* Stop serving stale spans on the GUI fast path, fix AI-edit redraw, fix rename language re-detection (#355) ([1c109a8](https://github.com/cuttlefisch/mae/commit/1c109a8ad2f0c6e36f86e1a2f13835de5a344006))
- *(mcp)* Mae-mcp-shim reconnects on socket drop instead of exiting (#356) ([85318fb](https://github.com/cuttlefisch/mae/commit/85318fb17c4948ba9e06891c1db1415611ce2ee8))
- *(ai)* Extend AI-residency seed-content exemption to graph/list/health/neighborhood/graph-view tools (#361) ([4af100f](https://github.com/cuttlefisch/mae/commit/4af100f1f23651eb1ec66a065a6ea18bba667ab6))
- *(scheme)* Run-command and execute_command now dispatch Scheme-defined commands (#363) ([48a6958](https://github.com/cuttlefisch/mae/commit/48a6958c5c7f2a2d615ae7c8ec85e6aced9dd8a2))
- *(editor)* Visual-mode bulk operators now affect every cursor's own selection (#364) ([717cfdd](https://github.com/cuttlefisch/mae/commit/717cfdd2ba16a9f04eb537859e23333021cfe899))
- Satisfy is_seed field after backmerging main into chord-diagram branch ([448d62a](https://github.com/cuttlefisch/mae/commit/448d62af8cf193b6887d80217910f2dc4847856a))
- *(graph-view)* Chord layout radius grows sub-linearly, matching force mode's scale ([1aa18dc](https://github.com/cuttlefisch/mae/commit/1aa18dc2fcb7901c6e250f3f67dbc640cf9c834a))

### Miscellaneous

- Bump version to 0.14.52 ([0d1da88](https://github.com/cuttlefisch/mae/commit/0d1da88cff73972836d8da1936057299d9893d1c))

## [0.14.51] - 2026-07-22




### Miscellaneous

- Bump version to 0.14.51 ([7c470a4](https://github.com/cuttlefisch/mae/commit/7c470a4da37b0159f5bf32af543a5196b4785eef))

## [0.14.50] - 2026-07-22




### Bug Fixes

- *(ai)* Exempt seeded/built-in KB content from AI-residency gating (#358) ([cb2f5dd](https://github.com/cuttlefisch/mae/commit/cb2f5dd3cb639e8d58a3106aa3fd8b1b275fbe1e))

### Miscellaneous

- *(deps)* Bump the rust-dependencies group with 19 updates ([e1e8c5e](https://github.com/cuttlefisch/mae/commit/e1e8c5e3ebffd1b42427cc4957a246cdc8e3d31f))
- Bump version to 0.14.50 ([2b2cea1](https://github.com/cuttlefisch/mae/commit/2b2cea14be730491702fb09ce49f60df52b43bab))

## [0.14.49] - 2026-07-22




### Features

- *(kb)* Finish kb-promote AI/MCP surface + always-on AI guidance mechanism ([ac3a736](https://github.com/cuttlefisch/mae/commit/ac3a7360bc52e2d3061d5571b4dac437212e2dae))

### Bug Fixes

- *(collab)* Stop ForceSync from merge-corrupting reopened shared-file buffers (#338) ([71df97c](https://github.com/cuttlefisch/mae/commit/71df97c6b3cb43ebf7279244f79efb2c15917fe1))
- *(collab)* Stop swallowing kb-approve/set-policy/block-principal daemon rejections (#339) ([c658685](https://github.com/cuttlefisch/mae/commit/c658685e9d60f4d98cf63cbb7c42d0fbb517cbce))
- *(collab)* Surface kb-set-encryption's local validation failures (#340) ([6f4a8f9](https://github.com/cuttlefisch/mae/commit/6f4a8f9d9a90635ce19c34246910eae46c1ca4cb))
- *(collab)* Resync ALL offline-edited buffers on reconnect, not just the first (#341) ([bab6239](https://github.com/cuttlefisch/mae/commit/bab62395add595142bce505e6361b526e048f737))
- *(daemon)* Bound unauthenticated connections with a handshake timeout + max_connections cap (#342) ([26df4a9](https://github.com/cuttlefisch/mae/commit/26df4a9213d8212068e3a60355e10978146d1087))
- *(collab)* Repaint *KB Sharing*/*Collab Status* on connect/disconnect (#346) ([b820c45](https://github.com/cuttlefisch/mae/commit/b820c4547e754ebef3f9f3576285b0ed5cb08879))
- *(ui)* Cross-reference setup-daemon and collab-start command text (#347) ([b2c8cbf](https://github.com/cuttlefisch/mae/commit/b2c8cbf61b538ebe3c3d4dfd5cdb9b096475d288))
- *(ai)* Kb_search scope-aware residency check + kb_search_context scope/ranking (#350, #351) ([eb0ffcd](https://github.com/cuttlefisch/mae/commit/eb0ffcda22ee4f0c15507f6765c6680e8fd21f7b))
- *(kb)* Stop kb_search_context's hub/meta nodes from outranking specific notes (#357) ([12e6885](https://github.com/cuttlefisch/mae/commit/12e68856f587486789127d18028dfc00adf4921b))
- *(ui)* Default fuzzy finders to recently-used items on empty query (#359) ([f3efd2f](https://github.com/cuttlefisch/mae/commit/f3efd2f226b416a07f3beda42d2e355cbc5967d0))
- *(keys)* Add missing Ctrl-U (CommandPalette/Search) and fix shadowed Ctrl-J (Insert+LSP popup) (#360) ([3fef908](https://github.com/cuttlefisch/mae/commit/3fef908b719878000ba3152945bd5bee899c2e73))

### Documentation

- Fix ROADMAP.md/KB_SHARING.md citing closed #78/#157 as still open (#348) ([925fe29](https://github.com/cuttlefisch/mae/commit/925fe29e285efb9f5cac6a8dc1b9b14fa3db2963))

### Miscellaneous

- Bump version to 0.14.49 ([165b2ba](https://github.com/cuttlefisch/mae/commit/165b2ba55f3a9e747f2206209c124cac1732bf4a))

## [0.14.48] - 2026-07-21




### Features

- *(options)* Make code-action/symbol-outline popup item caps configurable ([0ff526a](https://github.com/cuttlefisch/mae/commit/0ff526ac201fbc8d32c06c8cf842b073564559ca))

### Bug Fixes

- *(core)* Wire inline_images option + add all-options-reachable test ([5e8ca22](https://github.com/cuttlefisch/mae/commit/5e8ca22456bcfbdaf14c4132f8b33ef37f6c016c))
- *(kb)* Reconcile buffer freshness after a self-inflicted KB write (#316) ([d9ca689](https://github.com/cuttlefisch/mae/commit/d9ca6896838d10e9a33257cd5901842906be884b))
- *(kb)* Body_hash/kb_record_modification misattributed changes across sibling nodes ([c79ffa7](https://github.com/cuttlefisch/mae/commit/c79ffa7b5cf48d6e95056f5d82ec65c6f3094cca))
- *(kb)* Kb_owner_of no longer lets a stranded primary node shadow its instance (#76) ([7410a82](https://github.com/cuttlefisch/mae/commit/7410a82b0fbba066cac825c5a559597c3cc36c83))
- *(editor)* Route the external-file-change warning onto the notification bus (#79) ([5195105](https://github.com/cuttlefisch/mae/commit/51951059cd9a4a4030b531d46effc90531ebc60a))
- *(collab)* Wire authorized_keys resolver into daemon collection load (#73) ([79d84d3](https://github.com/cuttlefisch/mae/commit/79d84d33fcadc3e2e4e2fdeb45ec9c8e0f58932f))
- *(sync)* Add #[must_use] to KbNodeDoc's 4 remaining CRDT setters (#273) ([3ee5cb6](https://github.com/cuttlefisch/mae/commit/3ee5cb60391a5a63a95ba1dbf8b0a4a371a45709))
- *(sync)* Surface the undecryptable op-set count instead of dropping it (#206) ([ffc6096](https://github.com/cuttlefisch/mae/commit/ffc6096eae84c2c250a4ac0fc940270a0489282f))
- *(kb)* Bounded startup-migration slice for primary-stranded federation nodes (#76) ([dacae22](https://github.com/cuttlefisch/mae/commit/dacae2252299531b7f56052c57d981b7562b7204))
- *(kb)* Add 4 uncontroversial discoverability aliases (#67) ([0dd20f2](https://github.com/cuttlefisch/mae/commit/0dd20f2b2cee56a43bb146d87a175ad9cda13483))
- *(editor)* Second notification-bus slice — save/git/DAP failures (#79) ([dc8e27d](https://github.com/cuttlefisch/mae/commit/dc8e27db290bdf871c3124655844204e8b16feef))
- *(editor)* Third notification-bus slice — KB startup/preload failures (#79) ([3b49351](https://github.com/cuttlefisch/mae/commit/3b493515147ad660245b96a42283ecba38a7df3b))
- *(daemon)* Route the version-skew warning onto the notification bus (#323) ([32b30da](https://github.com/cuttlefisch/mae/commit/32b30da3e7f19155926f96091a68b4a62044460d))
- *(sync)* Consolidate cursor-adjustment-after-remote-edit (churn hotspot) ([e951bf0](https://github.com/cuttlefisch/mae/commit/e951bf01c163a1ce4a1873d93352b8857e2b9981))
- *(gui)* Eliminate scroll pixel-precision drift + dedupe clamp math (churn hotspot) ([7bff9bf](https://github.com/cuttlefisch/mae/commit/7bff9bfa8f0de9f0f827eef1924f1eef4e7e4da5))
- *(collab)* Stop silently discarding write_framed failures on 4 security-critical ops ([26d5b25](https://github.com/cuttlefisch/mae/commit/26d5b25f61cee6c0fbb9586602d90f001dc3220e))
- *(daemon)* Stop discarding persist failures and lying about "pending" join status ([7ddbd40](https://github.com/cuttlefisch/mae/commit/7ddbd407fe513a626f31f0fd770af0b23db5a342))
- *(tests)* Stop leaking temp dirs/files from the write-failure test harness ([b245302](https://github.com/cuttlefisch/mae/commit/b2453025dd1ac661ecba88df9aa6715c1c1ff150))
- *(kb,daemon)* Harden mutex-poison-cascade risk in daemon-facing KB hot paths ([ebf1d1e](https://github.com/cuttlefisch/mae/commit/ebf1d1e0cc5fa7dba514ccb7c534d0a39bde243b))
- *(kb-manual)* Stop serving a renamed function name from the display-policy doc ([74c2d17](https://github.com/cuttlefisch/mae/commit/74c2d176c43f01ab1d76a0260637689ea3673814))
- *(kb)* Recover from sled's internal open panic instead of crashing ([9255b23](https://github.com/cuttlefisch/mae/commit/9255b23a601381e950f250542fbc1dbd9f180742))

### Refactor

- *(kb-graph)* Unify per-window graph-view state pruning into one site ([25c9f2d](https://github.com/cuttlefisch/mae/commit/25c9f2d50d35b769652c306efedb531d5d1b8552))
- *(shell)* Unify the 3 duplicated rc-sourcing implementations (#291) ([54d289b](https://github.com/cuttlefisch/mae/commit/54d289b23e1df3759cbafe943b936b871467ab10))
- *(render)* Extract duplicated hex-color-preview + contrast_fg into render_common ([8610a55](https://github.com/cuttlefisch/mae/commit/8610a5509d1b1d5fde1e0cb1d2e5fb41b0ee5609))

### Documentation

- *(kb)* Add discoverability aliases for the display-placement setting (#67) ([cfb04a3](https://github.com/cuttlefisch/mae/commit/cfb04a3a87a7dfc1653457464d35b15ec9d5a50e))
- *(shell)* Empirically verify the fish exec-tail syntax (#291) ([3779a10](https://github.com/cuttlefisch/mae/commit/3779a10419a936f916b2f33371e83f83d5a15922))
- *(roadmap)* Correct stale KB graph view wheel-zoom item ([ffd7928](https://github.com/cuttlefisch/mae/commit/ffd7928b7a79982eafa1f6ac2cf7e118e647b16b))
- *(claude-md)* Reconcile #70/#73 status in the P2P mesh initiative summary ([e50943e](https://github.com/cuttlefisch/mae/commit/e50943ea7210e1fc5c7b104eca267608350dbb3e))
- Reconcile ROADMAP.md architecture debt + tag 6 newly-oversized files ([2f3a2af](https://github.com/cuttlefisch/mae/commit/2f3a2af23ef4c7106bae6a031bc967bbd8980a29))
- Fix stale code-comment file:line refs + add missing doc cross-links ([959f48f](https://github.com/cuttlefisch/mae/commit/959f48f032df87ac58f6a6c344d8b0f547647c19))
- Fix CLAUDE.md's stale numeric claims + add missing crate row ([c5dcf1f](https://github.com/cuttlefisch/mae/commit/c5dcf1f3a841459192916394677c1928ddcfe97b))
- *(readme)* Fix stale babel/tool-count claims, restructure crate layout ([50910c7](https://github.com/cuttlefisch/mae/commit/50910c7e543f1c00ba106a739722b07e5a9a9a7f))
- *(readme)* Rebalance value communication — trim vanity counts, surface the KB+ADR story ([30650f5](https://github.com/cuttlefisch/mae/commit/30650f5c23eba776217cf7963964c616e4c0805b))
- Fix broken ADR links + a self-contradictory resolved-limitation note ([4d5566d](https://github.com/cuttlefisch/mae/commit/4d5566d27382809c7cf140626072d5e657187f96))
- Fix stale status headers + file paths in security/status docs ([836a1ea](https://github.com/cuttlefisch/mae/commit/836a1ea0b57021f58f7d79f5f050937c4793c4e7))
- Rename kb-sharing.md -> kb-sharing-setup.md to avoid a near-duplicate filename ([130f0a3](https://github.com/cuttlefisch/mae/commit/130f0a39c946fd7fbb3eac2ad6e64742c4624e26))

### Testing

- *(daemon)* Add real 3-member content-sync convergence test through the daemon protocol ([39954c8](https://github.com/cuttlefisch/mae/commit/39954c8a8621463251ffc080929cbb1a541d78dd))

### Miscellaneous

- *(gui,core)* Remove verified-dead code, narrow FrameLayout's dead_code allow ([af7c796](https://github.com/cuttlefisch/mae/commit/af7c7963d29b4f46a2cbe7a6dfd05c72d4f2ebf2))
- Bump version to 0.14.48 ([726942e](https://github.com/cuttlefisch/mae/commit/726942e5ca976cf21f3f8131275f425d5aa32adb))

## [0.14.47] - 2026-07-20




### Bug Fixes

- *(kb)* Raise CozoDB sqlite busy-retry budget for CI-level contention ([64f4b65](https://github.com/cuttlefisch/mae/commit/64f4b65c152a2456848f3ab2d52164485c01e86d))

### Miscellaneous

- Bump version to 0.14.47 ([afa6fef](https://github.com/cuttlefisch/mae/commit/afa6fefb140b76474c0e573aeb5e293151ccea0e))

## [0.14.46] - 2026-07-20




### Features

- *(config)* List every module in the init.scm template, Doom-style ([d8834f5](https://github.com/cuttlefisch/mae/commit/d8834f5dc9c8ab30386bb4fac8564c2c394df06f))
- *(kb)* :kb-set-search-scope offers to open the graph after switching ([9ea958f](https://github.com/cuttlefisch/mae/commit/9ea958f224fae91a3906f1a4ee1bcb1bc6677ae9))
- *(gui)* Expose the per-window render cache via introspect(frame) ([4a914f8](https://github.com/cuttlefisch/mae/commit/4a914f8a70f3d30cd08d9ac9032acef544ea5786))
- *(display)* Expose window split ratios as proper OptionRegistry options ([f51b9ec](https://github.com/cuttlefisch/mae/commit/f51b9ec190dfd4dce16af2d9a91a5a9665544349))

### Bug Fixes

- *(agent-cli)* Collapse Backspace guard into the match arm (clippy) ([a6a3eef](https://github.com/cuttlefisch/mae/commit/a6a3eefdf6cde3ac3d394361b09fdba36633fade))
- *(core)* Replace_contents/reload_from_disk must bump buffer generation ([962acaa](https://github.com/cuttlefisch/mae/commit/962acaa5a78c517b24050ca87022e6fd8b92b92d))
- *(kb-graph)* Default to depth 1 + add a node-count safety cap ([98e0c66](https://github.com/cuttlefisch/mae/commit/98e0c66ac228cf402013b6753c59fea8b0814629))
- *(kb)* The "open graph after switching KB" prompt could land on the wrong KB ([eb38643](https://github.com/cuttlefisch/mae/commit/eb38643cfa5b5b17967aa7e460ccacf05bbf6ed1))
- *(kb-graph)* Companion-window content leak + hub-node click freeze ([8874eab](https://github.com/cuttlefisch/mae/commit/8874eab9deb5212c82839ea09a9c5c24c420beb2))
- *(kb-graph)* Node hitboxes misaligned after toggling fullscreen overlay ([cc89f63](https://github.com/cuttlefisch/mae/commit/cc89f63a538a8c9c6e9a57a03d4833c2b36329d7))
- *(kb)* KB-preview popup reappeared immediately after Escape-dismiss ([ec93e2a](https://github.com/cuttlefisch/mae/commit/ec93e2a65a0370fe0c9b319ce86f1f657123f58d))
- OOM leak in per-window render caches + audit security/live-bug items 1-5 ([2b3bd23](https://github.com/cuttlefisch/mae/commit/2b3bd23ebab9a96c1db949a0f9189b7839420132))
- *(which-key)* Wire up which_key_separator/max_desc_length/max_height_pct/sort_order ([0eaac31](https://github.com/cuttlefisch/mae/commit/0eaac310cc1cf2ffc801436bc363a0b07c9cf00f))
- *(babel)* Run_binary reintroduced the execute_shell pipe-deadlock class ([f8252fb](https://github.com/cuttlefisch/mae/commit/f8252fbfba7fc76f4d4e59ca29a5e4e30a090188))
- *(ai)* Unrecognized register-ai-tool! permission now fails safe ([064cb1f](https://github.com/cuttlefisch/mae/commit/064cb1fcb5692b7ec8178dd1eaef5f2a09a0b5c0))
- *(pkg)* Reject unknown module.toml fields; fix agenda/tables' depends ([d5fde9b](https://github.com/cuttlefisch/mae/commit/d5fde9bb4652230f2162f76c1a139c470c3ff07e))
- *(core)* Add #[must_use] to the 3 genuinely data-loss-risky Result fns ([33d82bf](https://github.com/cuttlefisch/mae/commit/33d82bfb102aeafbb51f8d98f8f6c3c679381bf0))
- *(kb)* Org list-item :ID: drawers after the first were silently dropped ([434871a](https://github.com/cuttlefisch/mae/commit/434871aff33d89674e62598fcae2b6d821649464))

### Performance

- *(gui)* Avoid per-glyph heap allocation in canvas.rs's draw paths ([b372951](https://github.com/cuttlefisch/mae/commit/b372951eee7cc8e21d467f28c1c889d1625a52f1))

### Refactor

- *(kb)* Consolidate mae-ai's KB fallback resolution onto a shared helper ([df9f470](https://github.com/cuttlefisch/mae/commit/df9f470b86a105e8be3f5830c1313e87a1f4b681))
- *(debug)* Migrate DebugView onto the shared foldable_view helpers ([cebbd46](https://github.com/cuttlefisch/mae/commit/cebbd46cb30a8daf7895f02316ab8daeac9989c7))
- *(dispatch)* Consolidate nav.rs's 4 duplicate BufferKind scroll matches ([d2a0066](https://github.com/cuttlefisch/mae/commit/d2a006654163573bad5348a8244a6707fe6f8d33))
- *(ai)* Replace 203 near-identical ToolDefinition literals with a builder ([9074975](https://github.com/cuttlefisch/mae/commit/9074975be5ed6607b0a59fb1451f1dd278f6f295))

### Documentation

- Fix broken mae! example and stale README module count ([53e0f32](https://github.com/cuttlefisch/mae/commit/53e0f3246d9959b20b8852e1ee686f8e360232e6))
- *(readme)* Document split-ratio options, window_render cache, KB-preview suppression ([c19bb3a](https://github.com/cuttlefisch/mae/commit/c19bb3a40576827469868be82b018b37d381a3ee))

### CI

- Fix version-bump concurrency race and changelog mislabeling ([7eabc88](https://github.com/cuttlefisch/mae/commit/7eabc88a200b46de42e7383ca4a06976d78ecde8))

### Miscellaneous

- *(deps)* Bump the rust-dependencies group with 2 updates ([af467d9](https://github.com/cuttlefisch/mae/commit/af467d9debeaef5a8ed461f738eebff917c0a66b))
- Bump version to 0.14.45 ([55ca140](https://github.com/cuttlefisch/mae/commit/55ca1405eca43adcd69e96c5d689005cf6795679))
- Regenerate manual KB after scrunching/kb-switch/cache-visibility fixes ([fda9f04](https://github.com/cuttlefisch/mae/commit/fda9f04bb58f67ff14abb3928e13699668c7f328))
- *(gui)* Delete 8 genuinely dead canvas.rs methods, unmark pixel_size ([db0a262](https://github.com/cuttlefisch/mae/commit/db0a26221b3bac43b661e85ff4000d509c0c0b76))
- *(daemon)* Remove 3 stale allow(dead_code), correct a 4th's rationale ([c23d598](https://github.com/cuttlefisch/mae/commit/c23d598f79baf3b3d66b51eca69911bb68f86ce6))
- Bump version to 0.14.46 ([1a55996](https://github.com/cuttlefisch/mae/commit/1a55996645cf35f9a08b16325a8f5fba4dcd2a9e))

### Rename

- *(kb)* Kb-set-search-scope -> kb-set-scope ([0932734](https://github.com/cuttlefisch/mae/commit/09327348fb1548fdbe3e040f7a629c93af6b05d2))

## [0.14.44] - 2026-07-15




### CI

- *(deps)* Bump schneegans/dynamic-badges-action ([ad41d4a](https://github.com/cuttlefisch/mae/commit/ad41d4a861b812acde0a14ce7cfc8b3f3c958f7a))

### Miscellaneous

- *(deps)* Bump the rust-dependencies group across 1 directory with 14 updates ([99dde27](https://github.com/cuttlefisch/mae/commit/99dde27002f3c7d9449640bad112ac942b56afa3))
- *(deps)* Keep dalek trio pinned at 2/2/4 (iroh blocker), guard in dependabot ([70878c4](https://github.com/cuttlefisch/mae/commit/70878c4cb2e11dd0472884c5d2f8456526a443da))
- Bump version to 0.14.44 ([7c43a76](https://github.com/cuttlefisch/mae/commit/7c43a76c5ca54fbcc56266c71b165d1094664e78))

## [0.14.43] - 2026-07-15




### Features

- *(kb-graph-view)* Expose zoom-to-level + pin/unpin to Scheme + MCP (#322) ([c0666d0](https://github.com/cuttlefisch/mae/commit/c0666d07cc10a4b593efddc8bd5ba617dbf84f26))
- *(kb-graph-view)* Full-frame overlay mode + mouse pan; fix layout spread and hit-test radius ([e37834c](https://github.com/cuttlefisch/mae/commit/e37834cdec4cc01fa2a7fd755a3e34551b8df53a))
- *(kb-graph-view)* Relationship-aware force layout — node-kind clustering + edge-weight attraction ([0622aad](https://github.com/cuttlefisch/mae/commit/0622aadbaef994caab1a3fe2d410d731fda3fca8))
- *(kb-graph-view)* Node sizing by degree + zoom (Sigma.js/org-roam-ui-style, clamped) ([66a6082](https://github.com/cuttlefisch/mae/commit/66a6082352bb2dce9a48304629631fd275a6d146))
- *(kb-graph-view)* Zoom-dependent label LOD + viewport-bounds culling ([5c0ea67](https://github.com/cuttlefisch/mae/commit/5c0ea672d3b865877a98a002cb7c73e5508d863c))
- *(kb-graph-view)* Curved edges + animated hover/selection color tweens ([b1f0786](https://github.com/cuttlefisch/mae/commit/b1f07866f07c4ef0e655ac6b1b9dd32723b85c5b))
- *(kb-graph-view)* Node spacing, label decluttering, muted palette, Escape-dismiss ([567fd5e](https://github.com/cuttlefisch/mae/commit/567fd5e530e86e2bb07b5b2433126a5f9efee28b))
- *(kb-graph-view)* Graph view respects kb_search_scope ([3bbc182](https://github.com/cuttlefisch/mae/commit/3bbc182dd52d5176a40572a2e69c923e85357b44))
- *(kb-graph-view)* Live-tunable node color saturation cap ([9bd66a5](https://github.com/cuttlefisch/mae/commit/9bd66a5212c5c52eb66bb2cd277a33cbb080dd4d))

### Bug Fixes

- *(kb-graph-view)* Per-window viewport isolation (#321) ([74eec5e](https://github.com/cuttlefisch/mae/commit/74eec5eb5465dd2b223c485671070b333e902343))
- *(kb-graph-view)* Zoom_to reports the actual clamped value, not the raw request ([d5987cc](https://github.com/cuttlefisch/mae/commit/d5987cc0a25bab9e3e4f56fd0c04cd7141ee5a46))
- *(kb-graph-view)* Clip to window bounds, antialias nodes/edges, real font ([133e819](https://github.com/cuttlefisch/mae/commit/133e819def3c261fe5317b6b25bc94b25ac2cbb4))
- *(kb-graph-view)* Actually pull theme colors — background never worked, 6/8 themes had no palette ([9c69752](https://github.com/cuttlefisch/mae/commit/9c69752f4cc5ae8e8672cb7bc0a4091f0b6bddc1))
- *(kb-graph-view)* Live-feedback UX + performance fixes ([985ee53](https://github.com/cuttlefisch/mae/commit/985ee53fae6324fe1dafe8e4d0ca53d3538f6be6))
- *(kb-graph-view)* Settle-never-completes, drag snap-back, boundary label ([9f7fa05](https://github.com/cuttlefisch/mae/commit/9f7fa05a583cd69163f37864ebed92e36985cd77))
- *(gui)* Render cache ignores buffer_idx, serving stale content ([aac35fe](https://github.com/cuttlefisch/mae/commit/aac35fe4b3ef4ef4171d5f4921ba001b585f8689))
- *(kb)* Help/preview buffers report "no such KB node" for real nodes ([416c926](https://github.com/cuttlefisch/mae/commit/416c926221d6ca82bb78c864d3b528cda7731740))
- *(kb)* Consolidate the query-layer-fallback fix onto one shared helper ([a91796b](https://github.com/cuttlefisch/mae/commit/a91796bbb6705ba336076c40f2fe4da76c079535))

### Miscellaneous

- Regenerate manual KB to reflect this session's new graph-view options ([1106dc9](https://github.com/cuttlefisch/mae/commit/1106dc9ae2c9bbdbe5e497deb127d16280e697fd))
- Regenerate manual KB to reflect new graph-view zoom-scale option ([965c74b](https://github.com/cuttlefisch/mae/commit/965c74be29df687c816f332be74f161347b4d067))
- Regenerate manual KB after boundary-label fix ([c65a673](https://github.com/cuttlefisch/mae/commit/c65a6733837c2da170b00b0813bd815d9669ba81))
- Regenerate manual KB after graph-view spacing/scope option additions ([2f484b8](https://github.com/cuttlefisch/mae/commit/2f484b8203ed478a1172c068b8a9ce39689d396e))
- Regenerate manual KB after graph-view spacing/scope option additions ([cb4b07e](https://github.com/cuttlefisch/mae/commit/cb4b07e7159e573a3ba604f440ed31fce9f0e269))
- Regenerate manual KB after query-layer-fallback fix ([7a3bf4d](https://github.com/cuttlefisch/mae/commit/7a3bf4dc3a6e9f04e5679a5ceb953a0a0270b6bb))
- Regenerate manual KB after consolidated query-layer-fallback fix ([a94f767](https://github.com/cuttlefisch/mae/commit/a94f767a8d68769160a296194953c3cc6e4fd40e))
- Bump version to 0.14.43 ([714ab94](https://github.com/cuttlefisch/mae/commit/714ab94f8043317a896e35627afe268b7e3cae8e))

## [0.14.42] - 2026-07-13




### Features

- *(core)* Shared idle-dispatch mechanism + wire which-key idle-delay (ROADMAP #83) ([fdd0d73](https://github.com/cuttlefisch/mae/commit/fdd0d73afe92cd6303e9c52a0c04a88ca69fa4da))
- *(scheme)* Add kb-graph/kb-neighborhood/kb-related/kb-shortest-path parity ([b28e42e](https://github.com/cuttlefisch/mae/commit/b28e42ead706de6a86da66256f3e3380dd4c60e3))
- *(core)* Add BufferKind::Graph + GraphView + graph_view_ops (Part C Phase 1) ([0f99a03](https://github.com/cuttlefisch/mae/commit/0f99a0393d54423c9137f7fd1f7b4a8c1ada54a0))
- *(mae)* Add graph_layout_bridge for background KB graph layout ([bce5880](https://github.com/cuttlefisch/mae/commit/bce5880091f73b86f8d25b8b3546926b9e964234))
- *(gui,renderer)* Render the KB graph view (GUI Skia + TUI textual) ([4adbe28](https://github.com/cuttlefisch/mae/commit/4adbe280e20734e3550c0758613d44263df4f688))
- *(scheme,ai,core)* KB graph view Scheme/MCP surface, options, theme keys ([5ef168c](https://github.com/cuttlefisch/mae/commit/5ef168cfc309126ca599ce44a6da5b76bc76975e))
- *(core,mae)* Mouse click-to-navigate on the KB graph view (Part C Phase 1 item 6) ([9201fcb](https://github.com/cuttlefisch/mae/commit/9201fcb4bbceab257e3f45fe9e38ba2a6d112291))
- *(core)* Wire kb_graph_follow_current_node to auto re-center the graph view (Part C Phase 2) ([9e0e073](https://github.com/cuttlefisch/mae/commit/9e0e073a614a236bc3b4c8c8891168c7261cb0a5))
- *(gui)* KB graph view physics animation (Part C Phase 3) ([2a99ff1](https://github.com/cuttlefisch/mae/commit/2a99ff1e62430220b1efe1fa841caa0dcd1c0764))
- *(graph-view)* Add drag-to-pin and wheel-zoom to the KB graph view ([ab3bc7e](https://github.com/cuttlefisch/mae/commit/ab3bc7ef0de4504c43207a763bb0329108cad7cc))
- *(kb)* Implement KB-link hover preview state, trigger, and commands (Part D) ([c66fd31](https://github.com/cuttlefisch/mae/commit/c66fd3180623387237180c225bd19529ced111d5))
- *(kb)* Render the KB-link hover preview popup (GUI + TUI) ([18db453](https://github.com/cuttlefisch/mae/commit/18db4531628c624ea5cfd7b4f922365a8456e756))
- *(kb)* Add Scheme + MCP parity for the KB-link hover preview ([7778935](https://github.com/cuttlefisch/mae/commit/77789355a4a28ca5bee29cc9037133ce754deffb))
- *(babel)* Inherit resolved shell environment in executions and sessions ([9aa2c45](https://github.com/cuttlefisch/mae/commit/9aa2c45d3c9aa4143edb38f5c84a4dada3b270a7))
- *(kb-graph-view)* Resize adaptivity + hover/selection introspection ([073b549](https://github.com/cuttlefisch/mae/commit/073b54956fb495bb9395e65b9713da02f96faa9a))

### Bug Fixes

- *(core)* Unify AI/MCP window-driving with a first-class DrivenWindow primitive ([dd1a9ae](https://github.com/cuttlefisch/mae/commit/dd1a9ae4a99da99fff0fc72590fb73dd63c487cd))
- *(kb-graph-view)* Correct module.toml's leader-key description ([a3777d2](https://github.com/cuttlefisch/mae/commit/a3777d22a5cb7c198824e6039bcb084d20f4b673))
- *(kb-graph-view)* Apply the viewport transform in render and hit-test ([08483bc](https://github.com/cuttlefisch/mae/commit/08483bc7a0805bf2ef8bb5279294dfc25af5128d))
- *(kb)* Give each open KB node buffer a distinct, title-based name ([c7a9eb9](https://github.com/cuttlefisch/mae/commit/c7a9eb99480b1a9eb42cf90c7e13dce2848bee4e))
- *(syntax)* Don't leak regex markup into org src-block code spans ([87b1ce0](https://github.com/cuttlefisch/mae/commit/87b1ce0e407cf841c1e3716dbb7d03a323f7a1b4))
- *(babel)* Sessions handle compound statements, surface stderr, and inherit shell env ([8913572](https://github.com/cuttlefisch/mae/commit/8913572732908063c10aa72846b1b7b5c86e91c0))
- *(core)* Keep buffer mode in sync across display_buffer paths ([58e5f91](https://github.com/cuttlefisch/mae/commit/58e5f91a1a307657a591e9911d7471f6b58cd3cb))
- *(core)* Self-heal *Messages* buffer resync beyond first open ([2d4a850](https://github.com/cuttlefisch/mae/commit/2d4a85066921501a50789b678a51147b9aa0d25f))
- *(kb)* Persist and reconstruct source_file across CozoKbStore reloads ([eb6619c](https://github.com/cuttlefisch/mae/commit/eb6619cf98e72fdb85852659e323288a07679b99))
- *(babel)* Use character offsets, not byte offsets, for results/body edits ([5fb6c03](https://github.com/cuttlefisch/mae/commit/5fb6c0320ffc23a336b59d81e04b6564f7cd737a))

### Refactor

- *(canvas)* Reconcile NodeKind/KbNodeInfo with the real shared_kb::NodeKind ([994931d](https://github.com/cuttlefisch/mae/commit/994931d7f83412055aa81c5dfca9fc720fe3b98d))

### Documentation

- Document DrivenWindow, native KB graph view, and KB hover preview ([3e68793](https://github.com/cuttlefisch/mae/commit/3e68793de28b725bd8ed1003631cf6be3f33e430))

### Miscellaneous

- *(daemon)* Sync Cargo.lock to the 0.14.41 version bump ([4526a01](https://github.com/cuttlefisch/mae/commit/4526a0103f216cdeec9177a27fd864910753c566))
- Regenerate manual KB to include the new Scheme API doc entries ([5fa5cdd](https://github.com/cuttlefisch/mae/commit/5fa5cddf50ea50dae5923953af8a89e1d34c4f40))
- *(core)* Register option/state scaffolding for upcoming fixes ([674ad76](https://github.com/cuttlefisch/mae/commit/674ad76fe8b38ba481f94f6067dbb3b70c2fe8b6))
- Regenerate manual KB to reflect this session's doc/API changes ([acdbeab](https://github.com/cuttlefisch/mae/commit/acdbeab7d8750535967f59c8756f0f198ea6c19e))
- Bump version to 0.14.42 ([c51640a](https://github.com/cuttlefisch/mae/commit/c51640a2e1a795cd5885cda663be3194eda2688e))

## [0.14.41] - 2026-07-12




### Miscellaneous

- Bump version to 0.14.41 ([786c307](https://github.com/cuttlefisch/mae/commit/786c30708a12f77351d7624a60354c1549391e0e))

## [0.14.40] - 2026-07-11




### Features

- *(agent-cli)* Non-interactive --prompt mode, tool-count filtering, round diagnostics ([edbc5eb](https://github.com/cuttlefisch/mae/commit/edbc5ebc18fd05d29a64bcf54ea3fa681598e630))
- *(ai)* Relocate guardrail to mae-ai, extend to embedded delegate(), add stage tracking ([51b4c67](https://github.com/cuttlefisch/mae/commit/51b4c670e127b81295dd234df5ca4a4325ba9afc))
- *(ai)* Wire Ollama's format param for structured tool-call output ([70cbe0a](https://github.com/cuttlefisch/mae/commit/70cbe0a747e8350601cb981ff7de00755d6d23fc))
- *(kb)* Add missing_role/weakly_linked agenda filters for enrichment discovery ([b7be566](https://github.com/cuttlefisch/mae/commit/b7be56639a477d533c3fe376d68d72e223a0219d))
- *(ai)* Flip ai_editor default to mae-agent, add ai_chat_enabled gate ([5a96bb4](https://github.com/cuttlefisch/mae/commit/5a96bb472d1f996623b1f70b4ce5a1562fdb1c3a))
- *(ai)* Redirect ai-prompt to mae-agent shell when chat disabled ([d799274](https://github.com/cuttlefisch/mae/commit/d7992744627830dbff5f307f70b9a4af8ea143e1))

### Bug Fixes

- *(agent-cli,mcp)* Enforce permission tiers in --prompt mode; transmit real tool tiers over MCP ([9fdeec7](https://github.com/cuttlefisch/mae/commit/9fdeec7ba3b58f14365a0c6f558b44ebf09447ff))
- *(ai)* Close ADR-049 test/copy gaps found in self-review ([14ad72d](https://github.com/cuttlefisch/mae/commit/14ad72df0da5ffbb672b62911d1a80036047e552))
- *(ci)* Explicitly build mae-agent binary before staging artifacts ([37c22c5](https://github.com/cuttlefisch/mae/commit/37c22c58adac32121616f0cb1761644aaadb6b91))
- Address 4 concrete bugs found by the architecture audit ([48bd2bb](https://github.com/cuttlefisch/mae/commit/48bd2bbd41ebe5ca78c1d163a96f186708b743e7))
- *(daemon)* Make mae-kb's storage-sqlite requirement explicit ([14bd447](https://github.com/cuttlefisch/mae/commit/14bd447c5fe5d88f923500dd33ab20db171deb3d))
- *(kb)* Make mae-kb build standalone with default features ([49f4b1f](https://github.com/cuttlefisch/mae/commit/49f4b1f57068907d59a367cef405001dc47273b1))
- *(ci)* Repoint TCP E2E test commands to split collab_tcp_e2e_* targets ([55429eb](https://github.com/cuttlefisch/mae/commit/55429eb19bb5133a634f42abe88cad4164395168))

### Refactor

- *(render)* Extract shared remote-cursor/selection math (principle #8) ([b71bb8a](https://github.com/cuttlefisch/mae/commit/b71bb8a4945f05ea386c59a26e58bff3d0f604f1))
- *(core)* Extract shared FoldableView abstraction (DRY audit finding) ([7fc9137](https://github.com/cuttlefisch/mae/commit/7fc91373e3debdfb060fe160b6500c6d16129f15))
- *(core)* Split kb_ops.rs source into a kb_ops/ submodule (ADR none needed) ([0a91270](https://github.com/cuttlefisch/mae/commit/0a91270fb17f0989091b460cb1c5fa4aba51f944))
- *(mae)* Split main.rs into cli/gui_app/bootstrap modules ([d49f177](https://github.com/cuttlefisch/mae/commit/d49f1778b558ed65c6e47b8803b6884c6a17e132))
- *(core)* Split editor/mod.rs into themed submodules ([ebaed95](https://github.com/cuttlefisch/mae/commit/ebaed956ba1b5e7ffd4bacf3a329ff97a069aab7))
- *(kb)* Split cozo_store.rs into a cozo_store/ submodule ([1673240](https://github.com/cuttlefisch/mae/commit/16732405a93216d1da80c8688bf62743497c1c1f))
- *(sync)* Split kb.rs into a kb/ submodule ([add86af](https://github.com/cuttlefisch/mae/commit/add86aff2ec8766966b8b29d001b494ff33592f4))
- *(scheme)* Split runtime.rs's register_fn calls by category ([6d286b8](https://github.com/cuttlefisch/mae/commit/6d286b859adccb9995cf4289687a37b52063ddd8))
- *(daemon)* Split collab_handler.rs's method dispatch by domain ([7dd38a8](https://github.com/cuttlefisch/mae/commit/7dd38a8f6714b1ba0724eb6117643b971622be44))
- *(mae)* Partially split collab_bridge.rs; mark run_collab_task as accepted debt ([68f8541](https://github.com/cuttlefisch/mae/commit/68f85418f1a2d84ec3b5d16d79aaafac8583002e))

### Documentation

- *(model-support)* Real Ollama exam data for qwen3:latest and llama3-groq-tool-use:8b ([6212902](https://github.com/cuttlefisch/mae/commit/6212902177a0d1d5acb8e8bb58bbbd78bc0390d9))
- *(model-support)* Real exam data for mistral:7b, llama3.1:8b, qwen3.5:latest ([0711ad0](https://github.com/cuttlefisch/mae/commit/0711ad03b18e8ae14bd7706a157bd094ce821b6a))
- *(adr)* Add ADR-049, supersede ADR-046's rejected chat deprecation ([f430685](https://github.com/cuttlefisch/mae/commit/f43068569cc7030d0b9a24a022099de01ddae62f))
- Formalize @ai-caution tagging convention, retrofit + cross-link ([cb79975](https://github.com/cuttlefisch/mae/commit/cb799750091595fb7bad13fd7c4e6f9775baaa4e))
- Fix stale README/CLAUDE.md content, add missing @stability markers ([6410442](https://github.com/cuttlefisch/mae/commit/641044250000ecfdd973a4ec97fbff4acc3d759f))
- *(scheme)* Close the Scheme API KB-doc coverage gap (53 functions) ([2d86da2](https://github.com/cuttlefisch/mae/commit/2d86da285c9925e1957adbe4ab6ea35dc0556861))
- *(roadmap)* Check off two stale architecture-debt items ([9fd95b6](https://github.com/cuttlefisch/mae/commit/9fd95b62cebb8efafa8e0d4a697d6b30c35a73da))
- Refresh architecture-debt tracking after the file-size splitting pass ([2586c31](https://github.com/cuttlefisch/mae/commit/2586c31b595a2c0f1bd10bfdd3016a8d6119e9f1))

### Testing

- *(agent-cli)* Harden mcp_client.rs + main.rs coverage, add CI smoke check ([c8f29cc](https://github.com/cuttlefisch/mae/commit/c8f29cc4ee1c3aeba1aef3c1c20d542563dab9a4))
- *(mae)* Close the untested embedded-delegate guardrail wiring + turn-loop gaps ([dd49bae](https://github.com/cuttlefisch/mae/commit/dd49bae1e6d8ce52c5f9f65687f89d84aa84aeb1))
- *(agent-cli)* Adversarial coverage for mcp_client.rs + confirm.rs boundary matrix ([0029c76](https://github.com/cuttlefisch/mae/commit/0029c7682c3a9eaa4463ff7d2031441212815260))
- *(ai)* Adversarial coverage for residency_check.rs + execute_kb_agenda ([c526f68](https://github.com/cuttlefisch/mae/commit/c526f681e90190987efbcbf159c84d6b9385b3e4))
- *(ai)* Cover ai_chat_enabled default/redirect, fix legacy chat tests ([0d6ded1](https://github.com/cuttlefisch/mae/commit/0d6ded1106bd19e8b23fbf12df3f02069cfc1687))
- *(mae)* Split kb_graph_validation.rs (1510 lines) into 3 files by category ([1316b55](https://github.com/cuttlefisch/mae/commit/1316b55b98b49a696bea9dc94294cb97a4e7bc6d))
- *(mae)* Split collab_tcp_e2e.rs (1431 lines) into 4 files by section ([0cc72c7](https://github.com/cuttlefisch/mae/commit/0cc72c720da128e2bc192fa372ec296d658654ec))
- *(core)* Externalize kb_ops.rs's inline test module (2992 lines) ([89eb232](https://github.com/cuttlefisch/mae/commit/89eb232f5ad008bec93ff25e0e813281092da70e))
- *(scheme)* Externalize runtime.rs's inline test module ([1a08e17](https://github.com/cuttlefisch/mae/commit/1a08e179215c9c58d36ce4b2361248909f6e768a))
- *(core)* Split kb_ops_tests.rs into 8 feature-grouped files ([adb4886](https://github.com/cuttlefisch/mae/commit/adb488644333e68df167f7fbaaa6c2dbbee5f974))
- *(mae)* Split collab_bridge_tests.rs into section-grouped files ([9625906](https://github.com/cuttlefisch/mae/commit/9625906ac53305a557d1ec221d52484b6d5d51ca))
- *(daemon)* Split collab_handler_tests.rs into section-grouped files ([71e30ab](https://github.com/cuttlefisch/mae/commit/71e30abed03a7882e192b8d7871f93dad35dbf31))

### Miscellaneous

- Bump version to 0.14.40 ([b879476](https://github.com/cuttlefisch/mae/commit/b879476e3b684cc4c13b46e3f4160a166797d8ac))

## [0.14.39] - 2026-07-09




### Miscellaneous

- Bump version to 0.14.39 ([c659aa2](https://github.com/cuttlefisch/mae/commit/c659aa24283159f18c818974c538e97224bebace))

## [0.14.38] - 2026-07-08




### Features

- *(ai)* Wire ModelVerification to real self_test_suite exam data ([a0eb722](https://github.com/cuttlefisch/mae/commit/a0eb7229bac7b2857806a92ff44afd4aaaffdd36))
- *(kb)* Add AiResidency policy for sensitive KBs (ADR-048) ([44e4068](https://github.com/cuttlefisch/mae/commit/44e40684db9765ea966a340c1cc41069df0d83d2))
- *(mcp)* PSK handshake + requester identity threading (ADR-048) ([033d996](https://github.com/cuttlefisch/mae/commit/033d9960c59439937714a0fbdfc97f5845c0c7a9))
- *(agent-cli)* New mae-agent CLI/TUI harness (ADR-046) ([4b54d27](https://github.com/cuttlefisch/mae/commit/4b54d270d20bbd4d0058a381f7ff2e6f5912a48b))
- *(kb)* Molecular-note :role: classification + fix kb_add_link's ADR-030 violation ([c5865bc](https://github.com/cuttlefisch/mae/commit/c5865bc05c74215c657d70805bf284b4b80a3a05))

### Bug Fixes

- *(ai)* Delegate() Ollama dispatch bug + enforce AI-residency gate ([8a022c6](https://github.com/cuttlefisch/mae/commit/8a022c673afb4906da58e16b60450e101a2bde21))
- *(kb)* ADR-030 typed-link projection for single-user writes + pipe-display ([b57360b](https://github.com/cuttlefisch/mae/commit/b57360b31d1e85396caeb45c8732b3b8992e1273))

### Documentation

- Fix stale P2P-mesh status and MODEL_SUPPORT.md test count ([c460c53](https://github.com/cuttlefisch/mae/commit/c460c531c92654fa71a062f939c05cc690fa4b93))
- *(adr)* Add ADR-045/046/047 for the Ollama-parity AI epic ([7424ffe](https://github.com/cuttlefisch/mae/commit/7424ffe40ec0a0e2a0ce384523ca3d3f656ff878))
- *(adr)* Add ADR-048 for AI-residency policy on sensitive KBs ([f058e72](https://github.com/cuttlefisch/mae/commit/f058e72888fe6977fe8eb6ac863c1fe5cb5f8365))

### Miscellaneous

- *(deps)* Bump anyhow to 1.0.103, fixes RUSTSEC-2026-0190 ([5777724](https://github.com/cuttlefisch/mae/commit/57777246c2dfda05d2903870e01b1eb4c540c755))
- Bump version to 0.14.38 ([8a4c0e1](https://github.com/cuttlefisch/mae/commit/8a4c0e1e057ad5d7fca3151edc52808609db0a6f))

## [0.14.37] - 2026-07-08




### Bug Fixes

- *(kb)* Ghost/stale node ids survive an in-place :ID: rename ([f4fdef7](https://github.com/cuttlefisch/mae/commit/f4fdef7e3e93aefe509c0889b1d67b3487937c6c))
- *(editor)* KB node buffers bled across split windows ([ec7e973](https://github.com/cuttlefisch/mae/commit/ec7e9738f7251f5c6042ddc33f84d27ad4c1de53))
- *(kb)* Node created via SPC n f invisible to other processes/instance search ([02f9126](https://github.com/cuttlefisch/mae/commit/02f912675251bf7514d6082304a2287b4a7d95a0))
- *(kb)* Kb_id_audit missed a ghost id once its file was itself renamed away ([b00c03f](https://github.com/cuttlefisch/mae/commit/b00c03f80bcb425301011ac2d836e1ea6ef8a1a0))
- *(deps)* Pin dalek-cryptography family at v2 pending iroh's v3/v5 support ([48905eb](https://github.com/cuttlefisch/mae/commit/48905eb98f2f3d8f83f4ebb8c95fc89de99547c1))
- *(kb)* Render multi-line and native-grammar links in the KB view (#301, #302) ([2ac544e](https://github.com/cuttlefisch/mae/commit/2ac544e16f77e2dc8132149538c9c4142deb2ea5))
- *(kb)* Interim promote-to-native command for federated nodes (#303) ([90eddd0](https://github.com/cuttlefisch/mae/commit/90eddd0a9ba6515c41c866139cab744f71ef00d5))
- *(editor)* Route KB-graph links through a shared, configurable resolver (#293) ([43a4b22](https://github.com/cuttlefisch/mae/commit/43a4b22839ceb0d415a8b7d48761f88398b268bf))
- *(editor)* Consolidate org_open_link onto the KB resolver and fix its dead link lookup (#293, #304, #306) ([155bbbd](https://github.com/cuttlefisch/mae/commit/155bbbd55c48d39fd72d52969de2a94c29ce4b62))
- *(ai)* Remaining MCP wrappers return their own outcome instead of stale status (#304) ([390b13e](https://github.com/cuttlefisch/mae/commit/390b13e1b04070dc90e95b7c8d57e0a6ab153f42))
- *(editor)* Stop terminal-title spam and dedupe repeated status messages (#305) ([7eb5ac3](https://github.com/cuttlefisch/mae/commit/7eb5ac3d5bc44c652dc048b8c1f237805d92379d))
- *(babel)* Gate NeedsConfirmation execution behind a real confirm dialog (#269) ([36bffbe](https://github.com/cuttlefisch/mae/commit/36bffbe3fb0619cb2d368af4ecdf9654ff229d46))

### Documentation

- *(claude)* Add drift-signal review principle ([3e2c973](https://github.com/cuttlefisch/mae/commit/3e2c97336ccf683eda154743771a7dfa70df8cd6))

### Miscellaneous

- *(deps)* Bump the rust-dependencies group with 27 updates ([a0dc157](https://github.com/cuttlefisch/mae/commit/a0dc157308011b8d70ad2dc554e6d0fc765d280d))
- Bump version to 0.14.37 ([f04de27](https://github.com/cuttlefisch/mae/commit/f04de27d200e193b7fd11bbc7ae90e57e5b39ed4))

## [0.14.36] - 2026-07-08




### Miscellaneous

- Bump version to 0.14.36 ([d5ea469](https://github.com/cuttlefisch/mae/commit/d5ea46955da34b3bb775bf2153924c8107fdc27b))

## [0.14.35] - 2026-07-07




### Bug Fixes

- *(kb)* Stop concurrent mae processes from clobbering shared state files ([2e82e65](https://github.com/cuttlefisch/mae/commit/2e82e65608221ba2ef6bf99228c6f6c2ced08d7b))
- *(shell)* Open-ai-agent shell didn't inherit login-shell environment ([e05a2af](https://github.com/cuttlefisch/mae/commit/e05a2af136cc7c42e89573ee349860d1bea24bfe))
- *(kb)* Kb-find lazy branch only searched primary, missing federated instances ([c66197c](https://github.com/cuttlefisch/mae/commit/c66197c18089d04d813f812e2cb5ae08169428f7))
- *(collab)* SIGKILL-resilient e2e daemon lifecycle (ADR-044) ([005a17f](https://github.com/cuttlefisch/mae/commit/005a17f5e9e1d0ef43f64761cdb9a798e3611720))

### Miscellaneous

- Bump version to 0.14.35 ([2d91b59](https://github.com/cuttlefisch/mae/commit/2d91b593d88d78f928d94852050ea4dc296db9fb))

## [0.14.34] - 2026-07-07




### Features

- *(ai)* Add ai_thinking option + native Ollama provider ([325e5a7](https://github.com/cuttlefisch/mae/commit/325e5a7b2a9a66d4a79b631c2b0163b48c53f31f))

### Bug Fixes

- *(ci)* Resolve fmt, code-map, and security-advisory failures blocking PR #289 ([e77ea4b](https://github.com/cuttlefisch/mae/commit/e77ea4ba7fdba47bcc7877cba7fb8af805a6ddaf))
- *(kb)* Kb_reimport must refresh the query layer for kb-find to see it ([c539b6d](https://github.com/cuttlefisch/mae/commit/c539b6d30b8737f5d5beee40ceab459beaa23583))
- *(kb)* Federated KB instances were hardcoded to sled, blocking multi-frontend sharing ([43acda9](https://github.com/cuttlefisch/mae/commit/43acda99bfbc62116d26914f6d14c3160720c141))
- *(build)* Embedded build SHA went stale after same-branch commits ([46afe08](https://github.com/cuttlefisch/mae/commit/46afe082bd6a1611ae624b491ff616fbb0b427fa))
- *(ci)* Encrypted e2e canary oracle assumed sled's leftover LSM bytes ([112e2ad](https://github.com/cuttlefisch/mae/commit/112e2adaecf36ba8944335ddd9e0b9d55516ede0))

### Refactor

- *(cli)* Remove --connect launch flag; surface daemon-mode in init.scm ([7ee56de](https://github.com/cuttlefisch/mae/commit/7ee56ded606cc46f8dca293e3ddcd2f894993d0e))

### Miscellaneous

- *(dev)* Install rustfmt/clippy in setup-dev; wire git hooks automatically ([104a097](https://github.com/cuttlefisch/mae/commit/104a09754115a129bdfbef6082b10882616a8455))
- Bump version to 0.14.34 ([3852243](https://github.com/cuttlefisch/mae/commit/38522436f9ddc963412f0f480dd4ab78a05502d2))

## [0.14.33] - 2026-07-05




### Features

- *(kb)* Cross-instance mirror refresh (Phase 4) + multi-instance docs (Phase 5) (#288) ([b5959fc](https://github.com/cuttlefisch/mae/commit/b5959fc03632be5933ee1b47579677a7df8edeff))

### Miscellaneous

- Bump version to 0.14.33 ([8d004d5](https://github.com/cuttlefisch/mae/commit/8d004d5f569528d10278321c4a0ae3686d8b520c))

## [0.14.32] - 2026-07-05




### Features

- *(kb)* Fast bulk sled→sqlite migration + default engine → sqlite (Phase 2c) (#287) ([9c38db5](https://github.com/cuttlefisch/mae/commit/9c38db55f892af91a80f4a8f497cd6d1f814aed1))

### Miscellaneous

- Bump version to 0.14.32 ([cb1949b](https://github.com/cuttlefisch/mae/commit/cb1949b3aa1a5ca547b5b40678d655c62f1f6c11))

## [0.14.31] - 2026-07-05




### Features

- *(kb)* Sled→sqlite migration module + kb_storage_engine option (Phase 2b, opt-in) (#286) ([a5c8095](https://github.com/cuttlefisch/mae/commit/a5c8095c115461a133296c9874ab3839abc65d6e))

### Miscellaneous

- Bump version to 0.14.31 ([7f988c8](https://github.com/cuttlefisch/mae/commit/7f988c8883731216998da08ad70e79bae6444e05))

## [0.14.30] - 2026-07-05




### Features

- *(kb)* Sqlite backend foundation + multi-writer busy-retry (Phase 2a) (#285) ([a428a59](https://github.com/cuttlefisch/mae/commit/a428a59dbc60d2c492a3aa6e9dc43b4f1e274fe2))

### Miscellaneous

- Bump version to 0.14.30 ([fb21a44](https://github.com/cuttlefisch/mae/commit/fb21a443109202f3cb4e35fd6b8204a3101b2141))

## [0.14.29] - 2026-07-05




### Features

- *(kb)* Route agenda + history through the query layer (Phase 3 read unification) (#284) ([0eaab91](https://github.com/cuttlefisch/mae/commit/0eaab91f291223ffdfffdbc4e98c4d332db716fa))

### Miscellaneous

- Bump version to 0.14.29 ([139f2b8](https://github.com/cuttlefisch/mae/commit/139f2b86c1c805aac1faeecacdae3371eec8cd8b))

## [0.14.28] - 2026-07-05




### Bug Fixes

- *(kb)* Durability + startup hardening (Phase 0+1 of KB lifecycle rework) (#283) ([0311aad](https://github.com/cuttlefisch/mae/commit/0311aada51b218da8f2ffca36bd3a4552340c918))

### Miscellaneous

- Bump version to 0.14.28 ([0486d7d](https://github.com/cuttlefisch/mae/commit/0486d7d816cf75505d600151edd60b070888980a))

## [0.14.27] - 2026-07-05




### Bug Fixes

- *(kb)* Persist :kb-ingest through to the durable store (#282) ([bb87d0c](https://github.com/cuttlefisch/mae/commit/bb87d0ce9fd3d3d6e3e026db759762372a112a72))

### Miscellaneous

- Bump version to 0.14.27 ([bc06745](https://github.com/cuttlefisch/mae/commit/bc067457aa4cf8bcb683e2515af7ab25d52bf589))

## [0.14.26] - 2026-07-04




### Bug Fixes

- *(kb)* Expand ~ in :kb-ingest directory arg (#281) ([8c4a2e8](https://github.com/cuttlefisch/mae/commit/8c4a2e854a3c9c73b859971b6ba3484f41cb0249))

### Miscellaneous

- Bump version to 0.14.26 ([a135d72](https://github.com/cuttlefisch/mae/commit/a135d7298f7531ba27ee45537f94c15671b2c4c5))

## [0.14.25] - 2026-07-04




### Performance

- *(org)* Fix multi-second checklist-toggle hitch — sequential subtree scan (#280) ([09f5f34](https://github.com/cuttlefisch/mae/commit/09f5f3416a70fc531c224262f97411dda54fe04b))

### Miscellaneous

- Bump version to 0.14.25 ([a4dacfb](https://github.com/cuttlefisch/mae/commit/a4dacfbba8841104213c1f68e22109513923058b))

## [0.14.24] - 2026-07-04




### Bug Fixes

- *(keymap)* Mode-aware local leader — restore the global SPC menu in org/markdown buffers (#279) ([2bec8e7](https://github.com/cuttlefisch/mae/commit/2bec8e71166fe840ffa9544b532730884c87a3d2))

### Miscellaneous

- Bump version to 0.14.24 ([2d86c94](https://github.com/cuttlefisch/mae/commit/2d86c94afdc8476e182bd612e2057f99cb0ff324))

## [0.14.23] - 2026-07-04




### Performance

- *(gui)* Skip per-wrap-continuation-row rope walks in buffer_render (#278) ([2d67046](https://github.com/cuttlefisch/mae/commit/2d67046ae8e63e72dea56b776e3019787d6e7874))

### Miscellaneous

- Bump version to 0.14.23 ([3b0a3a4](https://github.com/cuttlefisch/mae/commit/3b0a3a4c6a712b32aaa51f0f21e6149b0ca8b819))

## [0.14.22] - 2026-07-04




### Miscellaneous

- *(ai)* Extract executor's 1346-line inline test module to a sibling file (#277) ([5fde5ba](https://github.com/cuttlefisch/mae/commit/5fde5ba04c9aa54d25f66a1d98ab4c77109cf28b))
- Bump version to 0.14.22 ([281e00d](https://github.com/cuttlefisch/mae/commit/281e00d32bd373437f3da95a1fcbfa04f51a4343))

## [0.14.21] - 2026-07-04




### Performance

- *(kb,sync)* Bulk-fetch FTS candidates (kill N+1) + debug-gate per-op sync logging (#276) ([b445127](https://github.com/cuttlefisch/mae/commit/b4451278097373794206a1c9e2d8f454ddd84037))

### Miscellaneous

- Bump version to 0.14.21 ([60f37b6](https://github.com/cuttlefisch/mae/commit/60f37b66d1737a3641d2c6602f9fadbfdedcdc95))

## [0.14.20] - 2026-07-04




### Performance

- *(daemon)* Load the KB collection once per node_update, not 4× through the gates (#275) ([16fa65f](https://github.com/cuttlefisch/mae/commit/16fa65f173332effc11e8340669877f7a10aceb2))

### Miscellaneous

- Bump version to 0.14.20 ([88e8548](https://github.com/cuttlefisch/mae/commit/88e8548c32491d59eff54764c38b17b98f24d67f))

## [0.14.19] - 2026-07-04




### Bug Fixes

- *(scheme)* Parking_lot::Mutex for shared state — remove the lock-poison cascade (#274) ([e26bbf2](https://github.com/cuttlefisch/mae/commit/e26bbf24c4bcb9e9825072d09ff4048648222929))

### Miscellaneous

- Bump version to 0.14.19 ([1d092a5](https://github.com/cuttlefisch/mae/commit/1d092a546ae39573f78ab30d414c37bba95a429b))

## [0.14.18] - 2026-07-04




### Performance

- Safe mechanical wins (render alloc, per-node query, daemon existence lookup) (#272) ([28b9c81](https://github.com/cuttlefisch/mae/commit/28b9c811095c2e6a747e7043066d84820b208bdb))

### Miscellaneous

- Bump version to 0.14.18 ([06183aa](https://github.com/cuttlefisch/mae/commit/06183aa08dba843d2d812e49aee03f685e9c4be2))

## [0.14.17] - 2026-07-04




### Features

- *(lang)* Ruby support + default LSP servers (yaml/json/toml/bash) + pyright; document C++/babel (#271) ([5177986](https://github.com/cuttlefisch/mae/commit/5177986f261cbd9cec4baee845f7630143d158ca))

### Miscellaneous

- Bump version to 0.14.17 ([a996ef2](https://github.com/cuttlefisch/mae/commit/a996ef2478c93ebf75fc8bd49f2d7c2647a5497f))

## [0.14.16] - 2026-07-04




### Security

- *(security)* V0.15 maintainability & wild-usability review (§6) (#208) ([e68b152](https://github.com/cuttlefisch/mae/commit/e68b152190c009f6840dacfb1b34bf2c98dc98b6))

### Features

- E2E member key delivery — wrap-on-approve (ADR-037/038, #151 Phase 3b PR B) (#161) ([fedc62c](https://github.com/cuttlefisch/mae/commit/fedc62c1dad64c35b05c369b78db9068526efc3b))
- Content-key rotation on member removal (ADR-037 §D3 Phase 3c, #152) (#164) ([79a29f5](https://github.com/cuttlefisch/mae/commit/79a29f5b310cf7312174aff41929bde029ad511a))
- *(daemon)* Local self-protection blocklist — enforce at every membership-derivation site (#162 A2a/A2c) (#186) ([e91b691](https://github.com/cuttlefisch/mae/commit/e91b691b7e48b3e18f8c9272e3b013df0e4087d0))
- *(editor)* Block/unblock action + parity surface for the local blocklist (#162 A2b) (#187) ([1e011b9](https://github.com/cuttlefisch/mae/commit/1e011b96472fdd362fcc272e463fa755d90f282c))
- Blocklist display + introspection — *KB Sharing* Blocked view (closes #162) (#189) ([b3988bb](https://github.com/cuttlefisch/mae/commit/b3988bb3f1bf16dcdfd363d0e01c6b806ae62c50))
- *(crypto)* Zeroize ContentKey + DH/scalar intermediates (#156 F9) (#190) ([0664e86](https://github.com/cuttlefisch/mae/commit/0664e86cd6c91dd8c8248de59814b6c22d35a287))
- *(collab)* Blank cleartext node titles in the E2e manifest (#156 F5, forward case) (#191) ([3f8ba65](https://github.com/cuttlefisch/mae/commit/3f8ba653837067661d7a082a817bbd784c6d7bf2))
- *(mcp)* Warn (not silently no-op) when key files can't be 0600'd off-unix (#158 I4) (#193) ([410f874](https://github.com/cuttlefisch/mae/commit/410f8743983bc3af7893948f3ba1db8b55f9687b))
- *(collab)* Scrub existing manifest titles when E2e is enabled on a KB (#156 F5, enable-time) (#194) ([0412d99](https://github.com/cuttlefisch/mae/commit/0412d991199440f531255f32f17334e23ff1e519))
- I1 identity key separation — published X25519 wrap key (ADR-041, #158) (#198) ([5095511](https://github.com/cuttlefisch/mae/commit/50955117bafd7966f611db8dc235104f5fa05dce))
- *(sync)* Identity key rotation — Rebind op + derive alias/retire (ADR-040, I2 PR2a) (#210) ([3504ec3](https://github.com/cuttlefisch/mae/commit/3504ec38eb841ec93813aed480f56b675dd3586b))
- *(collab)* Surface E2E caveats at the point of enable (#204, CF1) (#209) ([3f1b0e0](https://github.com/cuttlefisch/mae/commit/3f1b0e084d2b87fe54ca55737570d0ea0573f7fb))
- *(collab)* Identity-key backup advisory on first generation (#203, KL1) (#211) ([c91ba43](https://github.com/cuttlefisch/mae/commit/c91ba4333ae5ef01c89f1233d9ea4a37c3c05009))
- *(collab)* I2 PR2b — owner identity rotation ((rotate-identity)) (#216) ([68e6ca3](https://github.com/cuttlefisch/mae/commit/68e6ca3be473b846bb0fdbbc7e61552a044cdcb2))
- *(collab)* I2 PR2c-1 — daemon member-authored Rebind write gate (#213) (#218) ([88ea493](https://github.com/cuttlefisch/mae/commit/88ea493edd9ffe7828b893e319bedbea20c28482))
- *(collab)* I2 PR2c-2 — non-owner member rotation + owner reactive re-wrap (#213) (#221) ([f019ebe](https://github.com/cuttlefisch/mae/commit/f019ebe05da85ab44f889a75aa88f0dc3fc3e9b4))
- *(collab)* I2 PR3 core — recovery-key v2 derive + crypto (ADR-040 §Recovery-key) (#222) ([927a69a](https://github.com/cuttlefisch/mae/commit/927a69ad1b956e03d15c6d1b6dcd0e28e164cc70))
- *(collab)* I2 PR3b — recovery surface (daemon accept-gate + editor register/recover commands) (#223) ([dcc5cde](https://github.com/cuttlefisch/mae/commit/dcc5cdec1194c47499e7ed849cd3d3abe4d26259))
- *(collab)* Surface identity rotation/recovery in the leader menu (ADR-040) (#224) ([3878b4a](https://github.com/cuttlefisch/mae/commit/3878b4aeb1cc8c7b18302f4b577dc91c82e8face))
- *(collab)* B2 durable collection op-log persistence + robust join-decrypt (ADR-040 recovery bootstrap) (#226) ([4d305e3](https://github.com/cuttlefisch/mae/commit/4d305e338e37c7c291f9c5158d09303ad17d9af9))
- *(collab)* Wire daemon-control into --test + WIP two-daemon mesh e2e scaffold (Phase 3) ([29a0df1](https://github.com/cuttlefisch/mae/commit/29a0df1d10a91d3d0a4c6e69e420614aec77bda8))
- *(ai)* First-class MCP tools for identity rotation + recovery (ADR-040, Phase 4 parity) (#234) ([aea38f8](https://github.com/cuttlefisch/mae/commit/aea38f8209a86e8cfe8d6d5efa95d67c937d448d))
- *(collab)* Describe the setup-collab mode choices (Phase 4 UX) (#235) ([572d4db](https://github.com/cuttlefisch/mae/commit/572d4db0dcaad9110861c3f325613357f658870d))
- *(kb)* PII-safe dogfood metrics harness (Phase 7, #243) (#244) ([afb5cbc](https://github.com/cuttlefisch/mae/commit/afb5cbccf65c59d0584da98ca7e5b2b40a5eac5d))
- *(collab)* Workstream C — seed signed genesis on P2P mesh share (ADR-043, #182) (#254) ([f5034ed](https://github.com/cuttlefisch/mae/commit/f5034edd07f5bbcefd3410c710b5d45875b8131e))
- *(collab)* Workstream D pt.1 — register KB-sharing commands (parity, #248) (#257) ([f6d50e0](https://github.com/cuttlefisch/mae/commit/f6d50e0cbf5a3640ee9f693d37494b8336f6487f))
- *(collab)* Workstream E — wire collab config options + de-hardcode (#249) (#258) ([f2679b6](https://github.com/cuttlefisch/mae/commit/f2679b64edae3cc87ec8a5d3b2d7bdac923956df))
- *(cpp)* First-class C++ support — babel + clangd LSP + lldb DAP + tree-sitter highlighting (#270) ([272681a](https://github.com/cuttlefisch/mae/commit/272681ac149e0a655a402ee1b5d2d78629004645))

### Bug Fixes

- *(daemon)* Unified op-log epoch fence on every write path (#157 A1+N1) (#163) ([debeda0](https://github.com/cuttlefisch/mae/commit/debeda0c867a373a8fcfe06a517f020bed6cdddc))
- *(daemon)* Gate kb: docs on sync/update — close the fence/membership bypass (#169 M1) (#174) ([b2cf70b](https://github.com/cuttlefisch/mae/commit/b2cf70b66b164f47a887ea053e8473b4082f295a))
- *(collab)* Fail-closed E2e seal on BOTH write paths (CRITICAL #168 + #170) (#172) ([692a1ef](https://github.com/cuttlefisch/mae/commit/692a1efa45c183d343e89990af9fbf80b2ec34d9))
- *(collab)* Re-derive the content key on collection updates — deliver rotated keys to members (HIGH #173) (#175) ([8ca94e1](https://github.com/cuttlefisch/mae/commit/8ca94e1a1c68ecd9d221d19a591b1e87c82559fc))
- *(test)* Derive local_kb_client_id in the --test runner — unblock scenario KB-node sync (#166) (#177) ([e28bab2](https://github.com/cuttlefisch/mae/commit/e28bab296e8218a83478ced97abe25f4c62f7263))
- *(collab)* Joiner can decrypt E2e content — approve op-log integrity + re-seal on enable (3d gate green) (#178) ([5d78748](https://github.com/cuttlefisch/mae/commit/5d78748399324352b52966fa99b07cd78707d3ac))
- *(collab)* Fail-closed approve base — never author membership against a divergent snapshot (#179) ([77ea1f3](https://github.com/cuttlefisch/mae/commit/77ea1f300ecd030e13367bf54d62e2fec6f9562f))
- *(kb)* Route instance-prefixed kb-create to its instance, not primary (#165) (#181) ([60fdac9](https://github.com/cuttlefisch/mae/commit/60fdac930942109bcbd1d874ab8052e39b638575))
- *(collab)* Purge the pre-enable plaintext base on E2E enable — reseal-as-replace (#171) ([8936857](https://github.com/cuttlefisch/mae/commit/8936857100f747f12605d9bebc165598f261a71f))
- *(daemon)* Scrub the pre-enable manifest title from the kbc: WAL at rest (#156 F5) (#196) ([0fb9b06](https://github.com/cuttlefisch/mae/commit/0fb9b06897f82a543e1a55757fc576d1ee92d0d1))
- *(collab)* Suppress spurious join local-ahead push that breaks recovery (ADR-040 #225) (#229) ([45a91dc](https://github.com/cuttlefisch/mae/commit/45a91dc187068d4ba7a0f80c2a36ae4065952625))
- *(collab)* Close two confidence-review blockers — append-only op-log gate + raw-sync read gate (A1, A3) (#236) ([a8cb998](https://github.com/cuttlefisch/mae/commit/a8cb998e1e9c2f649fbc7e64e33673dc821e16c7))
- *(collab)* Reactive member re-wrap after the owner has itself rotated (#237) (#239) ([22e5688](https://github.com/cuttlefisch/mae/commit/22e5688842e3f7f99e856ef714f45bd754b5196e))
- *(crypto)* Use non-deprecated AEAD constructors (unblocks chacha20poly1305 bump) (#245) ([29d0190](https://github.com/cuttlefisch/mae/commit/29d019065bcd60d3bef11de0d95d934076f611ca))
- *(collab)* Forward a mesh member's wrap pubkey to the owner (#255 part 1/3) (#256) ([e466f8b](https://github.com/cuttlefisch/mae/commit/e466f8b06e2f4003e1dcf19b3b55d472487a3d1c))
- *(collab)* Checkpoint-3 remediation — dead config + timebox coverage + parity (#187) (#261) ([becc9f8](https://github.com/cuttlefisch/mae/commit/becc9f85ac7b8c9c5f0cafa14f558fd91817244e))
- *(collab)* #255 layer-3 — relay the owner's signed content op WITH its header over the mesh (#262) ([89fc8b7](https://github.com/cuttlefisch/mae/commit/89fc8b7de03cb33bebc77998aa010a087d3d3bd2))
- *(collab)* Pre-dogfood hull patch — batch-collapse bug + scale defaults + config honesty (#188) (#264) ([0e3980c](https://github.com/cuttlefisch/mae/commit/0e3980cd46979113fce36e7fc873f4bf0ad062fa))
- *(collab)* Hull-patch part 2 — the three daemon-security bugs from the review (#265) (#266) ([fdcea97](https://github.com/cuttlefisch/mae/commit/fdcea97853f9db3f5d59aa68c8dfe3f34e3586e5))

### Performance

- *(collab)* Workstream B — membership-derivation cache + O(n) causal order (ADR-042, #247) (#253) ([63d023a](https://github.com/cuttlefisch/mae/commit/63d023a9335eb3c6765be7319766e695e0ae98ea))

### Refactor

- *(collab)* Workstream A — code-review flags index + safe fixes (#246) (#252) ([e5fdfda](https://github.com/cuttlefisch/mae/commit/e5fdfda277736128f6012055f261dc4cf9e5e208))

### Documentation

- *(collab)* Bound derive_content_key to owner wraps — close #169 L1 (#182) ([f9eb270](https://github.com/cuttlefisch/mae/commit/f9eb270aefd45c127ea1ca70ee75a6522e4c7467))
- *(e2e)* Correct the re-encryption-on-enable limitation after #171 shipped (#183) ([89b86db](https://github.com/cuttlefisch/mae/commit/89b86db1dbefa446087d85961ea4139bc5e96f97))
- *(adr)* ADR-040 identity key rotation & rebind (cross-signed, history-preserving) — closes I2 design (#192) ([aa0fbd8](https://github.com/cuttlefisch/mae/commit/aa0fbd8abb941b602b0e0b89155ebf9348647099))
- *(adr)* Finalize ADR-040 (rotation, Accepted) + ADR-041 (I1 key separation) — the identity arc (#197) ([7cc5e55](https://github.com/cuttlefisch/mae/commit/7cc5e5589798d46aa140c248ec0cad9e0f6d1dc4))
- Reconcile E2E doc/code drift — wrap-key separation + F5 (CF2/CF3) (#212) ([b37791a](https://github.com/cuttlefisch/mae/commit/b37791afa87396bf7f2a967598388793fa100a27))
- *(adr-040)* Implementation addendum — PR2 splits into PR2a/PR2b/PR2c (#214) ([c63feeb](https://github.com/cuttlefisch/mae/commit/c63feeb3c7e9c3907c5969cb7fd80e45ef8a6193))
- *(collab)* Owner-mediated key recovery runbook (recovery v1, ADR-040) (#215) ([9252d7f](https://github.com/cuttlefisch/mae/commit/9252d7f51295739a1d437ae265644d8a81fe35a4))
- DAEMON_ADMIN.md — operator runbook (admin config + maintenance, #201) (#217) ([da47196](https://github.com/cuttlefisch/mae/commit/da47196010c51576ceb3afd5d64a33772af10fe6))
- *(daemon)* Backup must include collections/ + recovery/ for identity recovery (ADR-040 B2) (#227) ([23fbd61](https://github.com/cuttlefisch/mae/commit/23fbd61d7b713d1909a17f925647746707c8a96d))
- *(daemon)* P2P mesh setup runbook + troubleshooting (ADR-025, Phase 5) (#232) ([2782f92](https://github.com/cuttlefisch/mae/commit/2782f92648d357c5a41a574dcd83b4113fb599c5))
- *(manual)* In-editor concept coverage for v0.15 features (Phase 4 parity) (#233) ([968d189](https://github.com/cuttlefisch/mae/commit/968d189ea342c4442213b5e9b1c95eab0d7b256a))
- E2E user guide + RELEASING runbook + v0.15 changelog (Phase 5) (#238) ([808fa8a](https://github.com/cuttlefisch/mae/commit/808fa8a894fe73c3f18e0676e5916ab72d59ac48))
- Drop the owner-then-member rotation caveat — fixed in #239 (#242) ([2668dfd](https://github.com/cuttlefisch/mae/commit/2668dfd173152cc70281a426738dd4157359a956))
- *(collab)* Workstream F — in-manual E2E KB-sharing lessons + verifiable guard (#250) (#259) ([492f359](https://github.com/cuttlefisch/mae/commit/492f359c2bf5dc0a256e268667ee23d4daf1d151))
- *(collab)* V0.15 two-machine (alice/bob) hub test plan + per-machine note templates (#267) ([a84d017](https://github.com/cuttlefisch/mae/commit/a84d017749801d4d463d15f7ee5e14d6e7da36f3))

### Testing

- *(collab)* E2e §D3 removal+rotation gate — and fix the rotation it never fired (#184) ([18de5cb](https://github.com/cuttlefisch/mae/commit/18de5cbf4baba49651c8024c0ec87ed716541b56))
- *(collab)* At-rest title-purge oracle + honest WAL-transient note (#156 F5) (#195) ([1c1380d](https://github.com/cuttlefisch/mae/commit/1c1380d94d880e495d4add41c034b682f3f64719))
- *(sync)* Op-set reconstruction round-trips under high-clock re-seal + cross-client edit (#228) ([dca88f3](https://github.com/cuttlefisch/mae/commit/dca88f3985c35ce9445c75141a243c92ab6ecc6b))
- *(collab)* Two-daemon P2P mesh e2e gate — full convergence over iroh, in CI (ADR-025, #200) (#231) ([fc63fa2](https://github.com/cuttlefisch/mae/commit/fc63fa25ed7080c043372973d7661a94adb9e03f))

### CI

- Run the E2E encrypted KB-sharing lifecycle gate (ADR-037 Phase 3d, #153) (#180) ([cd90fcc](https://github.com/cuttlefisch/mae/commit/cd90fcc6fccf245b35f57f0ec20a0fa6a378ab95))

### Styling

- *(daemon)* Cargo fmt the #171 storage scrub test ([c003261](https://github.com/cuttlefisch/mae/commit/c0032617bc6e1620614dcadafc9b1be3f5f5465d))

### Miscellaneous

- *(pre-dogfood)* Quality pass — AI cost tables, collab status badge, daemon poison-recovery, doc/metadata drift (#268) ([b1bd6a9](https://github.com/cuttlefisch/mae/commit/b1bd6a9eea372944863777f3fe9a53778627e6ea))
- Bump version to 0.14.16 ([f270ac3](https://github.com/cuttlefisch/mae/commit/f270ac3f3e12a43789d2e790d37926a88235a942))

## [0.14.15] - 2026-06-27




### Features

- E2E enable flow — owner key lifecycle, daemon key-blind (ADR-037/038/039, #151 Phase 3b PR A) (#160) ([8dc9cc9](https://github.com/cuttlefisch/mae/commit/8dc9cc9813a04bff1551c55fc48c85b16cf3ffe8))

### Miscellaneous

- Bump version to 0.14.15 ([c1ff7c0](https://github.com/cuttlefisch/mae/commit/c1ff7c05d9253dc5aad484b889c0ebfd19dba2cc))

## [0.14.14] - 2026-06-27




### Documentation

- E2E KB-sharing security reviews, holistic reference, ADR-038/039, adversarial-testing principle (#159) ([99e6ebe](https://github.com/cuttlefisch/mae/commit/99e6ebe3dcbfb8a836d25c652d16f81072dc4300))

### Miscellaneous

- Bump version to 0.14.14 ([7be38d4](https://github.com/cuttlefisch/mae/commit/7be38d4236713d21479cca1201068a7b840a3324))

## [0.14.13] - 2026-06-27




### Features

- *(daemon)* Kb/collection_op — key-blind owner-signed collection write (ADR-037 #150, Phase 3a) (#154) ([274ff53](https://github.com/cuttlefisch/mae/commit/274ff531848c5693ff7358ea64e9083dd82dbd0b))

### Miscellaneous

- Bump version to 0.14.13 ([2ee3ff4](https://github.com/cuttlefisch/mae/commit/2ee3ff4e2f5b54100bb86c51b32cc7ef6ba4294c))

## [0.14.12] - 2026-06-26




### Features

- *(editor)* Live E2E content-encryption wiring (ADR-037, #146 Phase 2b) (#149) ([606f0bb](https://github.com/cuttlefisch/mae/commit/606f0bb6fafa848998dec23c13c0b86defb86db1))

### Miscellaneous

- Bump version to 0.14.12 ([8e1f29a](https://github.com/cuttlefisch/mae/commit/8e1f29a9803859901607ee2ceaf36dac3a1a60fc))

## [0.14.11] - 2026-06-26




### Features

- *(editor)* Seal content ops on push + content-key find-half (ADR-037, #146 Phase 2a) (#148) ([9b007fd](https://github.com/cuttlefisch/mae/commit/9b007fda55e2911969e1254d3580b61e7311ffd5))

### Miscellaneous

- Bump version to 0.14.11 ([5f9323c](https://github.com/cuttlefisch/mae/commit/5f9323cc2c33600527ea7d9fb91a6ae5f5a74c6b))

## [0.14.10] - 2026-06-26




### Features

- *(sync)* Op-set pure layer — seal/merge/open for encrypted nodes (ADR-037, #146 Phase 1) (#147) ([f76ad7c](https://github.com/cuttlefisch/mae/commit/f76ad7ca15b22d5f335b58728ffba407c9908eaa))

### Miscellaneous

- Bump version to 0.14.10 ([e409e69](https://github.com/cuttlefisch/mae/commit/e409e6918d694aefc3a8b87c70b086c0d0610552))

## [0.14.9] - 2026-06-26




### Miscellaneous

- Bump version to 0.14.9 ([c48936e](https://github.com/cuttlefisch/mae/commit/c48936ef64a62d78f08b88484bdcce76ce9fbe7e))

## [0.14.8] - 2026-06-26




### Features

- *(sync)* E2E content-encryption crypto + key-distribution foundation (ADR-037, #131) ([8829785](https://github.com/cuttlefisch/mae/commit/8829785ccf8188d04384cb7c8426ca3f9ff4cc21))

### Bug Fixes

- *(mesh)* Stop the dialer's reconnect loop on a terminal reject (#133) ([910eca5](https://github.com/cuttlefisch/mae/commit/910eca55bd7af7194aba7528dab4c42a1901dc58))

### Miscellaneous

- Bump version to 0.14.8 ([2237b9e](https://github.com/cuttlefisch/mae/commit/2237b9e26497804c27e28b172e3ee49137ca27e9))

## [0.14.7] - 2026-06-26




### Miscellaneous

- Bump version to 0.14.7 ([14920af](https://github.com/cuttlefisch/mae/commit/14920afbd6d071897d50539a67f817058f2b55a5))

## [0.14.6] - 2026-06-26




### Features

- *(mesh)* Verify relayed content ops on the dialer path (ADR-036 §D3, #91) ([669889b](https://github.com/cuttlefisch/mae/commit/669889b4fd983997650548be09c8db571bb7bf53))
- *(mesh)* Owner re-verifies a joiner's relayed content op (ADR-036 §D3, B→A, #142) ([142a9d6](https://github.com/cuttlefisch/mae/commit/142a9d66d5da189888b405073c94c72df4f91677))

### Testing

- *(mesh)* Strengthen the relay-verification oracle — selective, not assert-absence ([bc402da](https://github.com/cuttlefisch/mae/commit/bc402da7a4fd8560b26236595f41811173f578c8))

### Miscellaneous

- Bump version to 0.14.6 ([5d634ad](https://github.com/cuttlefisch/mae/commit/5d634ad498dbf97c63c6a9e888722e980294bd6e))

## [0.14.5] - 2026-06-26




### Features

- *(sync)* Kb/node_update wire serde for SignedContentOp (ADR-036 D2/D3) ([b7c98c3](https://github.com/cuttlefisch/mae/commit/b7c98c35d2d396a77f1000eae9deeb81a7068c50))
- *(daemon)* Verify signed content ops on apply for anchored KBs (ADR-036 D3, #91) ([d1f0a75](https://github.com/cuttlefisch/mae/commit/d1f0a75aef4ac001feb88044f42e13fcbba00769))
- *(sync)* Kb_node_update_request_signed — editor's signed-request builder (ADR-036, #91) ([0cf28f3](https://github.com/cuttlefisch/mae/commit/0cf28f3351eda3e6333de2b83221fac7fb23d2f0))
- *(editor)* Sign content ops on push (ADR-036 D2, #91) ([fabceed](https://github.com/cuttlefisch/mae/commit/fabceedbd78bf84755a3bf2758b0ff5fe83d5324))

### Miscellaneous

- Bump version to 0.14.5 ([33a2cc4](https://github.com/cuttlefisch/mae/commit/33a2cc468906e8ca1582f761d595a905d4fe6994))

## [0.14.4] - 2026-06-26




### Features

- *(daemon)* Quorum governance in the kb_access gate + kb/revoke (ADR-026, #132) ([1dbf487](https://github.com/cuttlefisch/mae/commit/1dbf4877615d14c289c04d415eb30effa5f628d5))

### Miscellaneous

- Bump version to 0.14.4 ([b821203](https://github.com/cuttlefisch/mae/commit/b821203463c52d36e5f0cb0b9402b9dbf09fc141))

## [0.14.3] - 2026-06-26




### Features

- *(daemon)* Durable KB docs survive idle eviction (Phase A1, ADR-032) ([bd9d7d8](https://github.com/cuttlefisch/mae/commit/bd9d7d8c6efd332daf7ad48af97bfc9acb3c6707))
- *(daemon)* Max_documents is an LRU memory bound, not a hard cap (Phase A2, ADR-032) ([94b2714](https://github.com/cuttlefisch/mae/commit/94b27141d4a7e0cd3a9acb02b6feb30d9e92d3e6))
- *(daemon)* Atomic KB checkpoint — content-hashed CRDT capture (Phase A3, ADR-032) ([61ef903](https://github.com/cuttlefisch/mae/commit/61ef9030a252971b240720335ce3fb6490504c95))
- *(daemon)* KB backup / restore / export (Phase A4, ADR-032) ([4652253](https://github.com/cuttlefisch/mae/commit/4652253cf6f5c089ab2cab19df54c9a689f6002d))
- *(daemon)* Snapshot integrity — content-hash + verify on load (Phase A5, ADR-032) ([9549243](https://github.com/cuttlefisch/mae/commit/954924305cea1380aa2915f4bed1fa2fbdea93a2))
- *(daemon)* Projector core — node CRDT doc → cozo (Phase B1, ADR-029) ([8cbe791](https://github.com/cuttlefisch/mae/commit/8cbe791513819792430abef6a5d9f57bade1ef97))
- *(daemon)* Live change feed — doc_store → projector (Phase B2, ADR-029) ([716ccf2](https://github.com/cuttlefisch/mae/commit/716ccf2107a621a305b240f45e308d8185665a97))
- *(daemon)* Per-KB projection router + collection projection (Phase B3, ADR-029) ([ee2e9fd](https://github.com/cuttlefisch/mae/commit/ee2e9fd4cc7981cd18fe79f5036ad3312c160962))
- *(daemon)* Rebuild-from-CRDT self-heal path (Phase B4, ADR-029) ([6dd595e](https://github.com/cuttlefisch/mae/commit/6dd595ee49ac1a5e7ff48e22f00ee14152b50727))
- *(kb)* Phase D1 — host the primary KB on the daemon (route writes), ADR-029 ([787533c](https://github.com/cuttlefisch/mae/commit/787533cbb22f762ad2746ef0b9a5afd172fa2cd8))
- *(kb)* In-text link weight/confidence grammar (Phase C1, ADR-030) ([14d2074](https://github.com/cuttlefisch/mae/commit/14d207451a1146ce1b751677a2e002feb909cdfa))
- *(kb,daemon)* Projector wires the typed link graph from text (Phase C2, ADR-030) ([5c30fe1](https://github.com/cuttlefisch/mae/commit/5c30fe1b85758b12429d0b507cbe175613b7dc2c))
- *(editor)* Conceal the {w= c=} link attribute group in the rendered view (Phase C3, ADR-030) ([bad7866](https://github.com/cuttlefisch/mae/commit/bad78669967f7c89c5ecf7db749530cd2d7e8ede))
- *(kb)* ADR-030 link grammar — orderless key-value attrs in the target ([2c6f462](https://github.com/cuttlefisch/mae/commit/2c6f462e741e37d1627771761204cf8d1712be92))
- *(kb)* Phase D1.1 — route KB create/delete through the daemon CRDT ([86dc943](https://github.com/cuttlefisch/mae/commit/86dc94308a7138e4a58f0e9389d3f324c23c9827))
- *(kb)* Phase D2 — daemon read RPCs + introspection + LRU completion (ADR-029) ([dcd523a](https://github.com/cuttlefisch/mae/commit/dcd523aa12c059b33d070b4de1f214de49d9468b))
- *(kb)* Phase D3a — daemon-aware thin startup + lazy edit hydration (ADR-029) ([965efc5](https://github.com/cuttlefisch/mae/commit/965efc5c1d35a263931d7ae51cc9b80d9e7c73e0))
- *(kb)* Phase D3b (part) — invalidate the daemon read cache on remote KB edits ([fe454fc](https://github.com/cuttlefisch/mae/commit/fe454fcd769e66adec7cf1c8ed547e716bcf847f))
- *(kb)* Phase D3b — daemon-hydration retire (true thin client, ADR-029) ([e330599](https://github.com/cuttlefisch/mae/commit/e330599245c31ba694c453f2aad18a52ac419ec5))
- *(kb)* Close thin-client read-routing gap (agenda + search + health) [#118] (#120) ([be20639](https://github.com/cuttlefisch/mae/commit/be20639172661057be054be37ade42259a638b77))
- *(daemon)* Report version in daemon/status (ADR-035 version-skew foundation) (#123) ([da6df06](https://github.com/cuttlefisch/mae/commit/da6df06afc154372895a848f5de693bf841df30c))
- *(daemon)* Daemon_mode behavior-set option (off/on-demand/shared) — ADR-035 (#122) ([ea29a94](https://github.com/cuttlefisch/mae/commit/ea29a94806edfa68c48dde0305ab93a85b043378))
- *(daemon)* Editor-side version-skew check on daemon attach (ADR-035) (#124) ([378272e](https://github.com/cuttlefisch/mae/commit/378272e12d8936ae17d561fa7643dad3d2713318))
- *(daemon)* On-demand auto-spawn of a co-located mae-daemon (ADR-035 PR B) [#119] (#125) ([d22b67b](https://github.com/cuttlefisch/mae/commit/d22b67bc370634048b774536a4b41543913f7eed))
- *(daemon)* DaemonRequirement capability model + human/AI/Scheme parity (ADR-035 PR C) [#119] (#126) ([7f84981](https://github.com/cuttlefisch/mae/commit/7f849811c021e5900ab3448ff8e8a6aef68866c5))
- *(daemon)* Proactive daemon-state notifications (ADR-035 PR C-b) (#128) ([f32a257](https://github.com/cuttlefisch/mae/commit/f32a2573475c9b2d96e92aeccd9235dfb3106d32))
- *(daemon)* Session-long supervision of the on-demand daemon (ADR-035 PR B2) (#129) ([d59bf57](https://github.com/cuttlefisch/mae/commit/d59bf572754c692c19371f405c957059d967b9b3))
- *(sync)* Derive quorum governance from the signed op-log (ADR-026 §A4, #132) ([87630e8](https://github.com/cuttlefisch/mae/commit/87630e816e74c1fb0c21d229aab84f955083dcb1))
- *(sync)* Signed content ops — peer-verifiable authorship layer (ADR-036, #91) ([2463095](https://github.com/cuttlefisch/mae/commit/2463095181bbeb7f911daf4e93d12abc8b8ed997))

### Bug Fixes

- *(kb)* Phase D3c — pre-connect edit window + thin-mirror reroute + experimental flag ([d6db523](https://github.com/cuttlefisch/mae/commit/d6db52394f701bf5c8606165b5d781cecf7620db))

### Documentation

- *(adr)* Finalize ADR-030 link grammar — orderless key-value attrs in the target ([2a2b8df](https://github.com/cuttlefisch/mae/commit/2a2b8df06ce7bbb1f0382e91a351bf5f155b1b87))
- *(adr)* ADR-035 — editor↔daemon boundary + daemon_mode (daemon is optional) ([6e2181e](https://github.com/cuttlefisch/mae/commit/6e2181e47cff6d8ef55953de0a1ee6f6a16cd5e8))
- *(adr)* Reconcile ADR-014 + ADR-031 with ADR-035 (daemon optional, in-process floor) (#121) ([4c08871](https://github.com/cuttlefisch/mae/commit/4c088717769a354dd9f0b05221bf220b6c8a8971))
- *(adr)* ADR-036 signed content ops + ADR-037 E2E content encryption ([f195435](https://github.com/cuttlefisch/mae/commit/f195435485d27016d6d40692abc11d6142535b3e))

### Testing

- *(kb)* Migrate kb_graph_validation fixtures to ADR-030 link grammar ([ce971a7](https://github.com/cuttlefisch/mae/commit/ce971a76dd966f725ca8272976a9292ba1fd2de4))
- *(sync)* Concurrent + shuffled-order quorum/governance tests (no cherry-picks) ([3d3de60](https://github.com/cuttlefisch/mae/commit/3d3de601109a570065cd7c768ee3911ede598c31))
- *(daemon)* Reliable real-daemon lifecycle harness + e2e (ADR-035, #136) ([8db27b1](https://github.com/cuttlefisch/mae/commit/8db27b116c7a4506db16d6b432a42ace8b619332))

### Build System

- Disable incremental compilation to stop unbounded target/ growth (#130) ([0d06c21](https://github.com/cuttlefisch/mae/commit/0d06c21784bfaa85d8b21ed6f90d3cf4f2be3b48))

### CI

- Harden cargo dependency fetching against HTTP/2 network flakes (#127) ([4046116](https://github.com/cuttlefisch/mae/commit/404611669ec031480179ddbcb8a61b3175e67572))

### Styling

- *(daemon)* Rustfmt the Phase A files (CI fmt fix) ([c8b254a](https://github.com/cuttlefisch/mae/commit/c8b254a289be23e6391e147b998681f127788cce))

### Miscellaneous

- Bump version to 0.14.3 ([ea8ebc2](https://github.com/cuttlefisch/mae/commit/ea8ebc2c727c71c2c6f52aec9a1bbdeaee0361b1))

## [0.14.2] - 2026-06-25




### Features

- *(p2p)* JoinTicket — the shareable "magnet link" bootstrap primitive (ADR-025) ([99ce402](https://github.com/cuttlefisch/mae/commit/99ce4022abb948074dffe703de5e7c0cd40191ee))
- *(p2p)* Mint_ticket — build a JoinTicket from the live mesh endpoint (ADR-025) ([e422ed8](https://github.com/cuttlefisch/mae/commit/e422ed871a28459f190592099cd6ba9e9e8f4055))
- *(p2p)* P2p/mint_ticket control method on the KB socket (ADR-025, #101) ([5232260](https://github.com/cuttlefisch/mae/commit/5232260b3bed41a8f96983e2732893202cc24762))
- *(p2p)* DaemonControl trait — single backend for the P2P share surfaces ([e3a7333](https://github.com/cuttlefisch/mae/commit/e3a73337bf437d49e2ab769432e70e7d093a7e68))
- *(p2p)* Wire the editor's daemon control channel for P2P share ([e030b2f](https://github.com/cuttlefisch/mae/commit/e030b2f58786ffdb72005b7003aff3650a34f251))
- *(p2p)* Kb-share-p2p across command, MCP tool & Scheme primitive ([61e2cf0](https://github.com/cuttlefisch/mae/commit/61e2cf05867d028ab11c22ec6d2529667466e0d0))
- *(p2p)* Mae kb-share-p2p CLI — the 4th parity surface (ADR-025) ([8ae36ce](https://github.com/cuttlefisch/mae/commit/8ae36cec64d93d8069dd15247304d3dac6c43aae))
- *(p2p)* P2p/join_ticket — accept a magnet link, record the dial target (#101) ([88c5be8](https://github.com/cuttlefisch/mae/commit/88c5be8d67fbef1c40f88eec8e7b6ed7abfb2fb1))
- *(p2p)* Mae setup-collab --p2p — enable the daemon mesh (ADR-025/#94) ([b33bd9d](https://github.com/cuttlefisch/mae/commit/b33bd9def19910df59bde023a57636a92275f5c9))
- *(p2p)* Per-KB TransportPolicy on KbCollectionDoc (P2a, ADR-018/025) ([5b87eef](https://github.com/cuttlefisch/mae/commit/5b87eef3521af5e5074ba7c44b5f6fdb2516e572))
- *(p2p)* Enforce per-KB transport policy in kb_access (P2a, ADR-018/025) ([b3406ba](https://github.com/cuttlefisch/mae/commit/b3406ba48c344e9fc5476bfbf99a4f6b6fa8ab16))
- *(p2p)* Live-reload the mesh access gate per accept (P2a/I-10) ([d1799c4](https://github.com/cuttlefisch/mae/commit/d1799c4faa2a9fa2af5de81f88738643b7b58812))
- *(p2p)* Configurable mesh connection-trust gate (P2a, ADR-025) ([d6b0d7b](https://github.com/cuttlefisch/mae/commit/d6b0d7bf4204cfc9c96565719e8e36e9d80af0ec))
- *(p2p)* Kb/share establishes transport exposure (P2a slice 5, ADR-018/025) ([68ca225](https://github.com/cuttlefisch/mae/commit/68ca225f35334891f04a1ef6252df17c6e098517))
- *(p2p)* Signed membership op — the ADR-026 cryptographic foundation (P2b) ([3fa43b0](https://github.com/cuttlefisch/mae/commit/3fa43b0fd6624e15f95cf268de1aaf26afb8e6bc))
- *(p2p)* SignedMembershipOp record — op-log entry type (P2b-2, ADR-026) ([634ecf7](https://github.com/cuttlefisch/mae/commit/634ecf71631004fa26058087bfbff156dc45daf3))
- *(sync)* Signed membership op-log storage + two-phase append (P2P 2b-2) ([cfb04ce](https://github.com/cuttlefisch/mae/commit/cfb04ce54fe8aa64590e41599db7d60c9156b21e))
- *(sync)* Derive_valid_members — peer-side membership replay (P2P 2b-3) ([a3c5c5a](https://github.com/cuttlefisch/mae/commit/a3c5c5aeea1472f094ec01a976375d2becef6b09))
- *(sync)* Strong-removal resolver — concurrent membership convergence (P2P 2b-4) ([63d8630](https://github.com/cuttlefisch/mae/commit/63d8630587b0ceea21ff8570f77572f68a7d5cbd))
- *(sync)* Inviter-removal cascade policy (P2P 2b-5) ([9d3de2d](https://github.com/cuttlefisch/mae/commit/9d3de2d821072ff507fde1d0c4b5d1295f826655))
- *(sync)* Local blocklist + MembershipView options (P2P 2b-5b/1) ([46cf861](https://github.com/cuttlefisch/mae/commit/46cf86103f3a72c6c7c929088e134870afcd7e9c))
- *(sync)* Quorum governance — m-of-n co-signed removal (P2P 2b-5b/2) ([a33535b](https://github.com/cuttlefisch/mae/commit/a33535b068cd7ef5cc9a8e48c502db331683c8c7))
- *(daemon)* Sign membership ops into the op-log on add/remove (P2P 2b-6a) ([338a72f](https://github.com/cuttlefisch/mae/commit/338a72f5cd1fc2f84d0653562e8e5fb64480a4e4))
- *(daemon)* Sign approvals into the membership op-log (P2P 2b-6b) ([ec3a712](https://github.com/cuttlefisch/mae/commit/ec3a712427ad0c4aea85b6c77c95eda419f2446a))
- *(daemon)* Kb_access verifies membership from the signed op-log (P2P 2b-6c) ([4405d54](https://github.com/cuttlefisch/mae/commit/4405d54697b180a94f2a70686b294e3b893e1add))
- *(daemon)* Outbound mesh dialer — dial, verify, anchor, pull a KB (P2P 2c-1) ([742fd5d](https://github.com/cuttlefisch/mae/commit/742fd5dde81dbd1a7c55350de791a819e0c0adf2))
- *(daemon)* Background dialer drains join tickets + retries (P2P 2c-2) ([41d3867](https://github.com/cuttlefisch/mae/commit/41d38675e4c39e0c1748d7a92c9e6ed2ec6aef7f))
- *(collab)* Kb-join — full 4-surface P2P join parity (P2P 2a) ([f7323ae](https://github.com/cuttlefisch/mae/commit/f7323ae5719de1f783cd303e38d9ac16d87162d8))
- *(daemon)* Live mesh sync — persistent sessions + inbound apply (P2P 2c-3a) ([f399dc3](https://github.com/cuttlefisch/mae/commit/f399dc3b08e2121c6a2812cb782fd15a4a5e82c9))
- *(daemon)* Bidirectional live mesh sync — outbound forwarding (P2P 2c-3b) ([aded14b](https://github.com/cuttlefisch/mae/commit/aded14bab82dae9c33c72f604b4e0d8cbbe7cd84))
- *(collab)* Close the join→subscribe missed-edit window (P2P 2c-3c) ([f80ff48](https://github.com/cuttlefisch/mae/commit/f80ff4897147f475b379fbf7e6f78d4cfdfbfb8d))
- *(p2p)* Kb-share-p2p establishes the mesh share before minting (Phase 2a) ([fdec8d8](https://github.com/cuttlefisch/mae/commit/fdec8d8fa990e7e8c6025da697d219ca461d07b4))
- *(p2p)* Seed node content in p2p/share_kb from the daemon KB store ([8954d65](https://github.com/cuttlefisch/mae/commit/8954d657f4121098f5a4bae9639ab8ca1357441d))

### Bug Fixes

- *(daemon)* Clients default to the daemon's actual socket path (precheck #2) ([70236ef](https://github.com/cuttlefisch/mae/commit/70236ef84f1e27c2bd22694773b710345ef2752e))
- *(daemon)* Surface [collab.p2p] in --check-config + refresh kb-join message (precheck #1/#4) ([4d47b35](https://github.com/cuttlefisch/mae/commit/4d47b3593f9c2c5c9e64d602d51b4d86fee5734c))

### Documentation

- *(adr)* Discovery lifecycle, join tickets + address-spoofing threat model ([e093351](https://github.com/cuttlefisch/mae/commit/e0933513c644894874392d2c5b7b3473edcc9817))
- *(adr)* Pin CLI/editor/Scheme/MCP action parity for the P2P workflow ([b9a43f7](https://github.com/cuttlefisch/mae/commit/b9a43f71c241d766eba0104c1d7a3135606121d9))
- *(adr)* Pin hub + mesh coexistence as a supported invariant (ADR-025) ([0d0094c](https://github.com/cuttlefisch/mae/commit/0d0094c78734ec6ac207a46132b3ad9e302a2da4))
- *(adr)* Pin Phase-2a mesh access mechanisms (ADR-018/025) ([5642df5](https://github.com/cuttlefisch/mae/commit/5642df53332acc8960680379aaeb14947508d5ed))
- *(adr)* Op-log membership model + mobility + data lifecycle (P2P 2b design) ([51bb05c](https://github.com/cuttlefisch/mae/commit/51bb05c3a6da0ce1e1ddf7269c18992374d2775e))
- *(p2p)* Status + two-machine (alice/bob) testing guide + repro helpers ([272e18e](https://github.com/cuttlefisch/mae/commit/272e18e0e7d195a00be355c1512dfa0dc6d8edaa))
- *(adr)* Refresh status lines to reflect shipped P2P phases (accuracy pass) ([1ae1dd3](https://github.com/cuttlefisch/mae/commit/1ae1dd3301b911ed3721e9432c7519960c71ce0d))
- *(p2p)* Prime the alice/bob note files with the S1–S13 scenario tables ([813162e](https://github.com/cuttlefisch/mae/commit/813162ea5ea33076ab2e7f21394667df53ecee69))
- *(p2p)* Add note-file rotation to the testing protocol (week+ rounds) ([0c8d6a7](https://github.com/cuttlefisch/mae/commit/0c8d6a7ef7246883ed9dcc41ff00f5dd4d5c410c))
- *(p2p)* Record p2p/share_kb (share-then-mint) + two-daemon validation ([3fdbf0d](https://github.com/cuttlefisch/mae/commit/3fdbf0deec5cfb6b23eeb89d4bd10b24ce298bb8))
- *(p2p)* Note node-content seeding follow-up + --policy in the test guide ([23b73e1](https://github.com/cuttlefisch/mae/commit/23b73e16f76e4499b9136496063cb2fe33d27bc4))
- *(p2p)* Document the two node-content paths + seeding precondition ([70e6d31](https://github.com/cuttlefisch/mae/commit/70e6d3194f456511573e6eef0e8bd5cdd46c9232))
- *(adr)* KB data-architecture redesign — ADR-029..034 + multi-peer arch doc ([21b0802](https://github.com/cuttlefisch/mae/commit/21b08023bc1a5cb06f958ed27726c983ba3ff63f))
- *(arch)* Fix mermaid rendering in kb-multi-peer (sections 3 + 7) ([7b1452b](https://github.com/cuttlefisch/mae/commit/7b1452b813480d6c2806991531a09a078fbb91b6))

### Testing

- *(mdns)* Real round-trip + deterministic parse coverage (replaces no-op tests) ([6c12c7c](https://github.com/cuttlefisch/mae/commit/6c12c7cc6c5559650771feac4b7e30f5889f5d6a))

### Miscellaneous

- Bump version to 0.14.2 ([3921d03](https://github.com/cuttlefisch/mae/commit/3921d03a9e0dc44707b84870c38314942a17f8c5))

## [0.14.1] - 2026-06-24




### Features

- *(collab)* Epoch-fence hardening — unpredictable token + persist across remove/re-add (#72) ([a08141e](https://github.com/cuttlefisch/mae/commit/a08141eb5d5749442e935a9e93684a8c645cddef))
- *(p2p)* Iroh transport foundation — validated endpoint from trusted-peer identity (#88) ([6bbe95a](https://github.com/cuttlefisch/mae/commit/6bbe95a39208d1e22eeba2a79753c5fabb0b9f5d))
- *(p2p)* Mesh accept loop + authorized_keys gate (P1/#88) ([2a384a3](https://github.com/cuttlefisch/mae/commit/2a384a39e7f9500cdd5d1cad9331c1f01fea740d))
- *(p2p)* Activate the mesh from daemon startup behind [collab.p2p] (P1/#94) ([c922155](https://github.com/cuttlefisch/mae/commit/c92215560d708e85a9d1fa3e3006515a1b739921))

### Refactor

- *(collab)* Extract oversized inline test modules to sibling files (#70) ([d4a171a](https://github.com/cuttlefisch/mae/commit/d4a171aaed9b3596a726795beb1655a2ad43e302))

### Documentation

- *(adr)* P2P daemon-mesh design — ADR-025/026/027 (transport, integrity, observability) ([76a87c2](https://github.com/cuttlefisch/mae/commit/76a87c2401f729ecb73974f07209a3a2e532590d))
- *(adr)* P2P config/install/activation (ADR-025) + epic #94 ([8a82b20](https://github.com/cuttlefisch/mae/commit/8a82b2094061235781df505cfa2770a7b4c106a6))
- *(claude)* Point status + ADR index at the P2P daemon-mesh initiative ([061f175](https://github.com/cuttlefisch/mae/commit/061f175e258545852548b47180b9a747c2a941b4))

### Testing

- *(p2p)* Prove iroh streams round-trip Content-Length framing (P1/#88) ([da16471](https://github.com/cuttlefisch/mae/commit/da16471af659eacfac43c0fc0c2b5bc13f1f7519))

### CI

- Stop recompiling the mae crate for the install-artifacts step ([4af6daa](https://github.com/cuttlefisch/mae/commit/4af6daa7fd22d1528032099b959bf33398fb94bd))
- Keep the cheap mae-mcp build (shim) in the artifacts step ([443a608](https://github.com/cuttlefisch/mae/commit/443a6083cbcc4f52e32f028764f00043f06ae6d8))
- *(deps)* Bump actions/checkout ([256400b](https://github.com/cuttlefisch/mae/commit/256400b9572aa4513e15cc7ffe2db372c61de586))

### Styling

- *(collab)* Drop leading blank line in collab_handler_tests.rs (fmt --check) ([70f20c2](https://github.com/cuttlefisch/mae/commit/70f20c29c3a69b92e0b4b3a6c12b538eaa389ae2))

### Miscellaneous

- *(deps)* Coordinated RustCrypto realignment + rand 0.10 (#87, #51) ([b48c371](https://github.com/cuttlefisch/mae/commit/b48c37142291ade3b955d21146fba2171952290d))
- Bump version to 0.14.1 ([e6f4398](https://github.com/cuttlefisch/mae/commit/e6f439868e0defc6fb57cdf861df939e269775b1))

## [0.14.0] - 2026-06-24




### Features

- *(collab)* Trusted-keys keystore + multi-key PSK auth (mae-mcp) ([36ac2fc](https://github.com/cuttlefisch/mae/commit/36ac2fcb443ffecc5147c35fc13a631017a1532e))
- *(collab)* Wire trusted-keys keystore into daemon + editor ([d410e84](https://github.com/cuttlefisch/mae/commit/d410e841dc1f695270862fed7f32aa80416352d9))
- *(collab)* Ed25519 identity + KeyAuth signed-challenge handshake (ADR-017 phase 1) ([1841107](https://github.com/cuttlefisch/mae/commit/1841107dc373160a2a8f15f0da557efa2a6c7086))
- *(collab)* Daemon 'key' auth mode + admin CLI (ADR-017 phase 2) ([e00b150](https://github.com/cuttlefisch/mae/commit/e00b1508c26b55a7f83a1439d3abb6001aa94bd0))
- *(collab)* Native mTLS transport from Ed25519 identities (ADR-017 phase 0) ([886cdaa](https://github.com/cuttlefisch/mae/commit/886cdaa28aef664672186e464a70349630d240ae))
- *(collab)* Daemon mTLS accept + session identity binding (ADR-017 phase 1) ([9f3b533](https://github.com/cuttlefisch/mae/commit/9f3b5335cbf2ae94f37831ad06b45fdadede213e))
- *(collab)* Editor key-mode options + peer identity CLI (ADR-017 phase 2a) ([db30451](https://github.com/cuttlefisch/mae/commit/db30451ba186f0e5516ed8d3d19ef6921d1ba264))
- *(collab)* Editor mTLS client transport + mTLS e2e test (ADR-017 phase 2b) ([447fc44](https://github.com/cuttlefisch/mae/commit/447fc441e29a4b57fb90c36c683a943cf16c3eb8))
- *(collab)* Strict identity binding on the daemon (ADR-017 phase 3) ([5432219](https://github.com/cuttlefisch/mae/commit/5432219fadb9804b7a319453a81e0c0fdc1c7e0f))
- *(collab)* Per-KB membership enforcement (ADR-017 phase 4) ([1e3028d](https://github.com/cuttlefisch/mae/commit/1e3028d30b77445231b9100e56e8638fcd9ece0f))
- *(collab)* Editor KB membership commands (ADR-017 phase 4 editor) ([5b19f78](https://github.com/cuttlefisch/mae/commit/5b19f782dc125326d40c5f62ee80907e53d504d8))
- *(collab)* Interactive TOFU host-key prompt (ADR-017 phase 2c) ([99881c2](https://github.com/cuttlefisch/mae/commit/99881c268de9bcf7f7b09b6d888b3e6c1b3ab8cc))
- *(collab)* Mae setup-collab + opt-in SSH identity reuse (ADR-017) ([b88c759](https://github.com/cuttlefisch/mae/commit/b88c75965e7f0aaf1a2d9fc871bc1c263837f26f))
- *(collab)* `:kb-share <name>` to share a specific KB instance (I-4) ([b111b9e](https://github.com/cuttlefisch/mae/commit/b111b9e6ae694bc641f5209d4c75f93a0f083186))
- *(collab)* ADR-018 Phase 0 — principal accessors + authorized_keys hardening ([863d854](https://github.com/cuttlefisch/mae/commit/863d854369e3b1cdda5d6356150911d80bbf6cf9))
- *(collab)* ADR-018 Phase 1 — KbCollectionDoc v2 schema (owner/roles/policy/pending) ([caad6eb](https://github.com/cuttlefisch/mae/commit/caad6eb076499b1186c08953d328dc89dceb61fb))
- *(collab)* ADR-018 Phase 2+3 — kb_access engine + identity-anchored handlers ([9b72494](https://github.com/cuttlefisch/mae/commit/9b724945ac830d20bd8697e9e3a5cb3c6f1e6c6a))
- *(collab)* ADR-018 Phase 4 — daemon CLI revoke-by-fingerprint + label-uniqueness ([7147f75](https://github.com/cuttlefisch/mae/commit/7147f75b7e50f6eb18c04fa944b4c0821a426238))
- *(collab)* ADR-018 Phase 5 — editor commands for roles, policy, approve ([335eeee](https://github.com/cuttlefisch/mae/commit/335eeeeef92e0ac0ae3db2e03a12273ca5f29b13))
- *(collab)* ADR-018 Phase 6 — KbCollectionDoc.migrate_if_legacy ([4d107c3](https://github.com/cuttlefisch/mae/commit/4d107c30faf45d99bf675314d25667b56cb1adf9))
- *(collab)* ADR-019 Phase 0+1 — observability + durable emit gate ([23b73f1](https://github.com/cuttlefisch/mae/commit/23b73f15d89f6f636eef3ff93f6224f70466a877))
- *(collab)* ADR-019 Phase 2+4 — joined KBs are first-class instances; receive routes to owner ([35aafc2](https://github.com/cuttlefisch/mae/commit/35aafc200c9e1d49c593c55c73b36125aea19c04))
- *(collab)* ADR-019 Phase 3 — reconnect/startup reconstruction of shared-KB sync ([e6a4c45](https://github.com/cuttlefisch/mae/commit/e6a4c4584ad89b414e132c11cbd4b3a25359ea9f))
- *(collab)* ADR-022 W1 — N-peer reconcile harness + crash-safe reconcile primitive + B-17 fix ([4a99039](https://github.com/cuttlefisch/mae/commit/4a990394b0db2ddecc63a0e235d7f776120dfae1))
- *(collab)* ADR-022 W2 — SV-reconcile on KB (re)join, end to end ([945f294](https://github.com/cuttlefisch/mae/commit/945f294e9898d455fbd27599843c3513cd18dc0b))
- *(collab)* Kb_add_member / kb_remove_member tools (AI peer drives membership) ([7cf979b](https://github.com/cuttlefisch/mae/commit/7cf979b1078cd594398108bb905c97d53fae0009))
- *(collab)* B-19 epoch-fenced-rebase primitives (ADR-023) + unit tests ([35611be](https://github.com/cuttlefisch/mae/commit/35611bee8e01b96760eb30f0df883a1f2284c3f8))
- *(collab)* B-19 epoch fence — daemon enforcement + editor rotation (ADR-023) ([e95bb3c](https://github.com/cuttlefisch/mae/commit/e95bb3cc2e1d7df2667a292a78115ffa2447f40a))
- *(ui)* ADR-024 NotificationCenter attention bus — core + routing (phase 1) ([d7c61fd](https://github.com/cuttlefisch/mae/commit/d7c61fd20eda563be0d16f9966ce1ad89c424823))
- *(ui)* Mode-line attention badge for the notification bus (ADR-024 phase 2) ([a4e2277](https://github.com/cuttlefisch/mae/commit/a4e2277709eb900779d77d6171d027261c7718ce))
- *(ui)* *Notifications* magit-style attention buffer (ADR-024 phase 3) ([55b7dda](https://github.com/cuttlefisch/mae/commit/55b7ddab2958a1a4fb447fc7fdf46ebc92c894c6))
- *(collab)* R1 — kb/node_fetch RPC + async adopt-and-re-author (ADR-024, fixes 8d) ([2b9d77a](https://github.com/cuttlefisch/mae/commit/2b9d77aa851b17d5022cd325a0ec409cc8c2a842))
- *(collab)* R2 fenced-edit notification + R3 MCP notify tools (ADR-024) ([fb2db10](https://github.com/cuttlefisch/mae/commit/fb2db1022d2b0df8d8ff0473232de7af9aefd184))
- *(ui)* R4 — generalized modal reply + TOFU migration (ADR-024) ([e8474bd](https://github.com/cuttlefisch/mae/commit/e8474bd644f6f69302177ea4a96d0ab3f82141f2))
- *(collab)* R5 — no silent overwrite of divergent local edits on (re)join (ADR-024) ([03d5e5a](https://github.com/cuttlefisch/mae/commit/03d5e5a557acf3f52f219f19fa1c1d7493e9b7ae))
- *(modules)* Required/core module tier — auto-enable regardless of (mae!) block ([9bbe252](https://github.com/cuttlefisch/mae/commit/9bbe252934cbb20cabe15945639f02930e8e2ab9))
- *(collab)* B-22c — bus Accept/Reject actions on the host-key TOFU prompt ([7fe4f93](https://github.com/cuttlefisch/mae/commit/7fe4f93497bc15291e689d2beb9af05fd75469a4))
- *(collab)* Connect-config liveness guards + build-SHA observability (Wave 1: C2, C3) ([2c5c269](https://github.com/cuttlefisch/mae/commit/2c5c2692a895bd4a4151b001b5339c1635f3cce2))
- *(collab)* Relearn KB authorization epoch on live kbc broadcast (Wave 2: C1) ([f72f54a](https://github.com/cuttlefisch/mae/commit/f72f54a3e4dec169b5b508bf9d073191d72a7853))
- *(collab)* KB-sharing introspection snapshot — buffer/MCP/Scheme single source of truth (P0) ([b94784b](https://github.com/cuttlefisch/mae/commit/b94784b82b9c97cfb50185a087004349bdcb249e))
- *(collab)* Magit-style *KB Sharing* management buffer (P1) ([04dedc7](https://github.com/cuttlefisch/mae/commit/04dedc7f1c7a0e754bbc393feddc03d0943c2c44))
- *(collab)* Scheme/MCP/command parity + fingerprint-free member picking (P2) ([fc28d61](https://github.com/cuttlefisch/mae/commit/fc28d616c6863fc4b7d3cf7430675c7dbe57c2dc))
- *(collab)* Pending-request notifications + configurable fence resolution (P4) ([7a16eb6](https://github.com/cuttlefisch/mae/commit/7a16eb6e2a80dff03465c89495a70b38ac2574b1))

### Bug Fixes

- *(collab)* Re-stamp KB creator to authenticated identity + two-editor membership e2e ([8ebf673](https://github.com/cuttlefisch/mae/commit/8ebf673a4fc661dcd36d759dd461828724279c13))
- *(collab)* Avoid port collisions in e2e tests + guard setup-collab against 0.0.0.0 ([7644794](https://github.com/cuttlefisch/mae/commit/7644794d7f4e0f02684eab51300582b5373803a9))
- *(collab)* Make trusted-peer e2e cross-platform (macOS + Linux) ([a8ac842](https://github.com/cuttlefisch/mae/commit/a8ac8424996cdb4f46bac52d9e18d5ea131b3355))
- *(mouse)* Clamp double-click word-select offset; guard word_start_backward (I-1) ([a57455f](https://github.com/cuttlefisch/mae/commit/a57455f9d413d7f28ef477c1ca8c5323b4dbb2a6))
- *(collab)* Resolve named KB by uuid for share; kb-join/leave honor <id> (I-5/I-6) ([281d2ae](https://github.com/cuttlefisch/mae/commit/281d2aeda2253c10b029132f8372ebd1149ff42f))
- *(collab)* ADR-018 — owner-gate raw kbc: sync/update (membership-smuggling defense) ([774af1b](https://github.com/cuttlefisch/mae/commit/774af1b9a15759bf37717cd08833abf5b72ed223))
- *(collab)* I-10 — daemon honors authorize/revoke live (no restart) ([2792908](https://github.com/cuttlefisch/mae/commit/27929083116c1be3b23e497cbc9073b77653257d))
- *(collab)* I-9 — shared-KB edits resolve + propagate (federation-aware writes) ([697b901](https://github.com/cuttlefisch/mae/commit/697b901522ee0b80c98986a14977cc5102d2b446))
- *(collab)* B-1 — kb-join surfaces joined / pending / denied distinctly ([43f6c5a](https://github.com/cuttlefisch/mae/commit/43f6c5a542d60925cb8bd1194d88afcc30ac576e))
- *(collab)* ADR-019 Phase 5 — robustness/parity/UX tail ([cf673b7](https://github.com/cuttlefisch/mae/commit/cf673b7c205d093eb3c2128c97f2745313ca5dba))
- *(collab)* ADR-019 — reconnect re-subscribe skips primary KB (Collab Status launch popup) ([5d903d3](https://github.com/cuttlefisch/mae/commit/5d903d3322dd25ad69430d23af02f3ccf17d4e88))
- *(collab)* ADR-020 Phase 1 — never silently lose a kb/node_update (+ daemon liveness) ([0865b4d](https://github.com/cuttlefisch/mae/commit/0865b4d8d06706d5cfe64b6899de92a6482047c3))
- *(collab)* ADR-020 Phase 2 — merge-on-join instead of overwrite ([4d72ed4](https://github.com/cuttlefisch/mae/commit/4d72ed413cc011ebed04829782bca5880b78a93a))
- *(collab)* ADR-020 Phase 3 — durable joined instance + disk-first loader (B-10) ([1f4a699](https://github.com/cuttlefisch/mae/commit/1f4a6993f09ebc326c15d0e33ab76be14387582c))
- *(collab)* ADR-020 — kb/node_update is a request, not a dropped notification (B-8 root cause) ([95295a2](https://github.com/cuttlefisch/mae/commit/95295a2ba1615eeb11b8f2fd8fff4af92dc2ccad))
- *(collab)* ADR-020 B-13 — joined member must live-subscribe to KB node docs (receive path) ([4602ce4](https://github.com/cuttlefisch/mae/commit/4602ce4bed218ad6fb3f1d083278f26456110789))
- *(collab)* ADR-020 B-14 + B-15 — KB node edits now actually merge across peers ([490d9a3](https://github.com/cuttlefisch/mae/commit/490d9a3cfcdfdc3a6bca1cf260e5fa3762654cdc))
- *(collab)* ADR-020 B-16 — canonical persisted lineage on share + stable per-peer client_id ([1652fcf](https://github.com/cuttlefisch/mae/commit/1652fcf4817bdcaf4b3bd49eae31bc5cbd76923f))
- *(collab)* ADR-020 B-12 — owner re-share preserves membership (no silent revoke on restart) ([c87ae3a](https://github.com/cuttlefisch/mae/commit/c87ae3a4ec4d19bb1c7e9b579ed52ecaf1a85d94))
- *(collab)* Observability — pending_kb_updates now reflects the DURABLE offline queue (bob's yellow flag) ([6a1a560](https://github.com/cuttlefisch/mae/commit/6a1a5604785904a6dc108ef925b34764858c36de))
- *(collab)* Per-launch auto-connect overrides win over init.scm (found in T3c live run) ([91a5201](https://github.com/cuttlefisch/mae/commit/91a520151a4eac68844829e4bd2fbe8b957f9364))
- *(collab)* B-18 — node tags now CRDT-sync (KbNodeDoc::set_tags + upsert wiring) ([97af88d](https://github.com/cuttlefisch/mae/commit/97af88df937107bf53d7769f65f74512d566c828))
- *(collab)* B-20 — fence stale-epoch contiguous continuations of a canonical client (ADR-023) ([d934d68](https://github.com/cuttlefisch/mae/commit/d934d6875207133601b1e5a451d1e01a2879b21a))
- *(collab)* B-21 — runtime collab_host_key_policy now honored by the connect path ([8fe5a73](https://github.com/cuttlefisch/mae/commit/8fe5a73e12385cdd50daff08ad053cfcab721398))
- *(gui)* B-22 — host-key TOFU modal renders + captures input (runtime + focus) ([371803c](https://github.com/cuttlefisch/mae/commit/371803c8d3adda7c7e6e12651fa4e0df6f4ae486))
- *(gui,tui)* B-22a — render the mini-dialog modal whenever present (both backends) ([b09becd](https://github.com/cuttlefisch/mae/commit/b09becdc2e09b4a1480a96f7d732bb6456498c08))
- *(gui,tui)* B-23 — content-adaptive MiniDialog sizing (shared), full fingerprint readable ([c976c1f](https://github.com/cuttlefisch/mae/commit/c976c1f80bfc284cefe4df4e625322aa7e840216))
- *(collab)* Non-blocking background mDNS discovery (P3) ([761af26](https://github.com/cuttlefisch/mae/commit/761af26cb2a9d520af406d3b9e9f88d1ae013239))
- *(collab)* Audit fix-now — KB-sharing span computer + daemon hardening + onboarding docs ([42916b5](https://github.com/cuttlefisch/mae/commit/42916b51bdcc16c14e8617d33eddeb4003632a00))

### Refactor

- *(render)* Unify overlay priority in one place so GUI/TUI can't diverge (B-22a root cause) ([65c2281](https://github.com/cuttlefisch/mae/commit/65c22813c060f05303ce4b944ad033b72107e34c))

### Documentation

- *(adr)* ADR-017 asymmetric peer auth (SSH-style keys + TOFU) ([e890935](https://github.com/cuttlefisch/mae/commit/e8909357fa62fd8c4492bf73541f6ea975e4f0d1))
- *(collab)* Fix COLLABORATION.md bugs + trusted-peer (key mode) guide ([6760303](https://github.com/cuttlefisch/mae/commit/6760303b7e3a3fea93200dd22836b595e95a5288))
- Trusted-peer collaboration testing plan (automated + 2-machine live) ([fd2e35a](https://github.com/cuttlefisch/mae/commit/fd2e35a5bdd70df74a95be72a3c3d5ae4f2e59cc))
- Add build/setup + key-setup section to the collab testing plan ([daadc94](https://github.com/cuttlefisch/mae/commit/daadc94b3a625cc956771f6ec4f32c09aaa13c11))
- *(collab)* Live-run coordination board for the two-machine validation ([b947a52](https://github.com/cuttlefisch/mae/commit/b947a52e293ee2589a1aca8b33d7aa4d50c784e6))
- *(collab)* Require accept-new policy + rebuild instructions for both machines (#66) ([30776b9](https://github.com/cuttlefisch/mae/commit/30776b962a3b2cd3f36ade8b2d24a8ca1b83d486))
- *(collab)* Start bob-side test notes log (issues from the live run) ([6b2b3cf](https://github.com/cuttlefisch/mae/commit/6b2b3cfc0384f09dae621c80f829411b753feb53))
- *(collab)* Restructure bob notes — tag every observation with its test step ([985fa41](https://github.com/cuttlefisch/mae/commit/985fa4147c0b2f441718166231ec6ce1662d3d40))
- *(collab)* Alice-side test notes — record I-1 rope panic + cross-ref bob ([6c048bc](https://github.com/cuttlefisch/mae/commit/6c048bc7f175addc5cdc55f7e7e9bf90aaa2722d))
- *(collab)* Run 2 — bidirectional CRDT sync confirmed; I-1/I-2/I-7 resolved ([b6fdc8b](https://github.com/cuttlefisch/mae/commit/b6fdc8bd07890049bf96e124bf6e8c2e174cce3e))
- *(collab)* Alice Run-2 notes — T2.5 convergence ✅, I-2 reattributed ([82df2ff](https://github.com/cuttlefisch/mae/commit/82df2ff157e8e4df75e14018cdc6673f07ecbf6e))
- *(collab)* T2.5 simultaneous-edit convergence confirmed (Run 2) ([13a18f1](https://github.com/cuttlefisch/mae/commit/13a18f18a84e2073b4e9a176d10bfb532c4e3aec))
- *(collab)* Reconcile I-2 with alice's notes + record her I-3 follow-up ([1304d67](https://github.com/cuttlefisch/mae/commit/1304d67f799edb93089d7a5b0f42a2fd4604693f))
- *(collab)* ADR-018 + COLLABORATION.md — identity-anchored KB access control ([76ed368](https://github.com/cuttlefisch/mae/commit/76ed368276807ae2c47be106c51952bf1958f14e))
- *(collab)* ADR-018 — in-editor KB concept for human + AI peers ([585f799](https://github.com/cuttlefisch/mae/commit/585f799cd5d9cce0c0c7025d227a7f815b298d02))
- *(collab)* Alice notes — ADR-018 done + the new live T2.6 flow for bob ([2ce3ebf](https://github.com/cuttlefisch/mae/commit/2ce3ebf7407765694450822d8121c6ca542395f4))
- *(collab)* Run 3 (ADR-018 T2.6) — membership + replication validated; B-1/B-2/B-3 logged ([ce878b2](https://github.com/cuttlefisch/mae/commit/ce878b2c9d08eaec50ed3b75b66ee34905301081))
- *(collab)* T2.6 revoke+restrictive denial confirmed; B-1 upgraded, B-4 added ([8509b97](https://github.com/cuttlefisch/mae/commit/8509b9704674863bb1462d0cee32a537e4afd533))
- *(collab)* Run 3 live T2.6 — steps 1-5 PASS over mTLS; I-8 (federation write gap) logged ([47cf7eb](https://github.com/cuttlefisch/mae/commit/47cf7eba8d908e8f41c1f5aad53002310b931a49))
- *(collab)* T2.6 restrictive deny-by-default PASSES live; I-9 (edit propagation broken) logged ([e3429cc](https://github.com/cuttlefisch/mae/commit/e3429ccba5bc3969ea63cd31c5a9118cc89e685b))
- *(collab)* I-10 (once-at-startup auth, no live revoke) logged; pivot to bug-fix pass ([1b8a729](https://github.com/cuttlefisch/mae/commit/1b8a729916f84dad2985c4edfe9ff50e7527e2e8))
- *(collab)* Mark I-8/I-9/I-10/B-1 fixed; resume plan for both machines ([c16468b](https://github.com/cuttlefisch/mae/commit/c16468b329caad77e9e5a32dbf0147489f77c522))
- *(collab)* B-5 (kb_join stalls on malformed KB row) + B-6 (KB store not XDG-first) ([9dc858e](https://github.com/cuttlefisch/mae/commit/9dc858e7a5eae2a4211938ff18917b689a298f17))
- *(collab)* Run 4 — B-1 + I-9 fixes verified; editor propagation paused (alice planning fixes) ([9e46c1c](https://github.com/cuttlefisch/mae/commit/9e46c1c9a14ced66c1ec684bced3d1474dbbb805))
- *(collab)* ADR-019 Phase 6 — ADR doc + restart-survival e2e ([fb5c455](https://github.com/cuttlefisch/mae/commit/fb5c45599a2ee3574d1fc771f0d2f527342ce82e))
- *(collab)* ADR-019 landed — durable shared-KB sync; both machines rebuild to resume ([5ccb6fe](https://github.com/cuttlefisch/mae/commit/5ccb6fefd35bee88b6eae67cf359d5c5596bd17f))
- *(collab)* Run 5 (ADR-019) — B-3 resolved; B-8 editor edit doesn't enqueue/propagate ([91db1f1](https://github.com/cuttlefisch/mae/commit/91db1f1d8c0e3591d65518358e7b41c70d289198))
- *(collab)* B-10 — joined KB instance has dir=""; likely B-8 root cause + restart-survival bug ([2a79040](https://github.com/cuttlefisch/mae/commit/2a79040ccdae018cfc64e3cb058b49d702b26b2a))
- *(collab)* Holistic design guidance for alice — shared KB as durable replicated CRDT artifact ([18f9a97](https://github.com/cuttlefisch/mae/commit/18f9a97adebeae0feb742da656b152269a10bc7d))
- *(collab)* ADR-020 Phase 0 — durable replicated KB artifact + observability seam ([b93498d](https://github.com/cuttlefisch/mae/commit/b93498d1f0976b9568a79cbd7691360655a0f899))
- *(collab)* Stage 1 (ADR-020 P0–3) landed + pushed — bob pickup + live finding ([aaf33f8](https://github.com/cuttlefisch/mae/commit/aaf33f82187c05f87f2392eb3bf733691f6a49dc))
- *(collab)* Bob Stage-1 baseline — B-10 fixed (3 nodes reload), B-8 on_save hypothesis ([8e648b4](https://github.com/cuttlefisch/mae/commit/8e648b4afe9e8e158bae2625f4e11a2ec1c46222))
- *(collab)* Step 1 (alice→bob receive) FAIL — B-8 confirmed from owner side ([569d4dd](https://github.com/cuttlefisch/mae/commit/569d4dd5872374557488d7d48f98232bc746249b))
- *(collab)* B-8 root cause + fix landed (95295a2b) — bob rebuild + re-run Step 1 ([9a3b973](https://github.com/cuttlefisch/mae/commit/9a3b973349566a250d7d25d5726845c52d75d766))
- *(collab)* B-8 fix build — new B-12 (approve→no auto-subscribe) + Phase-2 merge confirmed ([5ca2c8f](https://github.com/cuttlefisch/mae/commit/5ca2c8f3cf18513fc66aa1f2672a219a477d48d1))
- *(collab)* Step 1 re-run — B-8 EMIT FIXED, new B-13 (join doesn't subscribe to node docs) ([b69d4b5](https://github.com/cuttlefisch/mae/commit/b69d4b5d483df90bded5c4b23ef4c22aeeba8540))
- *(collab)* B-13 narrowed to member-side-only — daemon delivery confirmed ([76d3461](https://github.com/cuttlefisch/mae/commit/76d34612a926d2272ccf984a06c6135963e94377))
- *(collab)* B-13 fix landed (4602ce4b) — bob rebuild instructions + B-12 re-approve heads-up ([ab19fb1](https://github.com/cuttlefisch/mae/commit/ab19fb14b0c8d463b781e01ab276fad6e4d24015))
- *(collab)* B-13 CONFIRMED fixed (build verified) — new B-14 no-op CRDT merge ([d06e5d2](https://github.com/cuttlefisch/mae/commit/d06e5d25c0b71c9ec9956828700c0eeb24d85ab8))
- *(collab)* B-14+B-15 fixed (490d9a3c) — bob rebuild + re-join to adopt lineage ([8d1e040](https://github.com/cuttlefisch/mae/commit/8d1e040f92296825ad89b8266dd6445f70e2657b))
- *(collab)* ✅ STEP 1 (alice→bob receive) PASSES on B-14+B-15 build ([f77706a](https://github.com/cuttlefisch/mae/commit/f77706a3d15ea338195da542e2c0f1e37af8e5a9))
- *(collab)* Step 2 (bob→alice) — emit GREEN at bob+daemon, owner-side merge fails (B-16) ([16b8a86](https://github.com/cuttlefisch/mae/commit/16b8a863072c7e7b11b0e57c6760156bbcd3c94a))
- *(collab)* KB-sync bug chain + testing-methodology lessons (for write-up + robust e2e) ([decf6ba](https://github.com/cuttlefisch/mae/commit/decf6ba2bae2a15d8ed8756f33e92852b8d7f90a))
- *(collab)* B-16 fixed (1652fcf4) — bob rebuild + bidirectional re-test (owner lineage + client_id) ([4a33016](https://github.com/cuttlefisch/mae/commit/4a330168831fefe0252ee8fa9947ee1828b90311))
- *(collab)* ✅✅ BIDIRECTIONAL Stage-1 KB sync CONFIRMED (B-16 build) ([a49e54f](https://github.com/cuttlefisch/mae/commit/a49e54f93ec3e2a3c3553bb5caae0fd86f96c0e1))
- *(adr)* ADR-021 — durable, auditable membership & policy (compliance foundation) ([ca08e52](https://github.com/cuttlefisch/mae/commit/ca08e52a660735afe7d9723917dc7183b0109745))
- *(collab)* Bidirectional GREEN + B-12 deployed — remaining manual CRDT test matrix for bob ([3a67a54](https://github.com/cuttlefisch/mae/commit/3a67a54c8982e362f056d44997c39c2f76e9b894))
- *(collab)* ✅ T1 (B-12 owner-restart) PASS — membership preserved + bidirectional intact ([3578acd](https://github.com/cuttlefisch/mae/commit/3578acd9d96691face904ea000258655abb2b46c))
- *(collab)* T2 restart-survival PASS (bob) — disk-first reload + auto-rejoin + bob->alice emit ([8635951](https://github.com/cuttlefisch/mae/commit/8635951e1412f4698ab576ffa58151209b258dec))
- *(collab)* T1 + T2 PASS — results log (cross-validated daemon ⇄ bob startup) ([d86fd4f](https://github.com/cuttlefisch/mae/commit/d86fd4fc2572d09b2efc3b6f573c42328a59ab36))
- *(collab)* T3 offline-merge — ready-to-run procedure (roles, steps, daemon signals, pass criteria) ([71f86d8](https://github.com/cuttlefisch/mae/commit/71f86d82013f0e5634a85b4c5c74d575ad40d8b1))
- *(collab)* ✅ T3 offline-merge PASS (bob) + yellow flag (offline-pending not durable/observable) ([a53368b](https://github.com/cuttlefisch/mae/commit/a53368b89dd3d0eec0adac89ab3f6d7320ba6074))
- *(collab)* T3 PASS + yellow-flag fix recorded; T3b ready-to-run (offline edit survives editor restart) ([9c58dfd](https://github.com/cuttlefisch/mae/commit/9c58dfdbe6869f6c76f3377708be477e41b5e54d))
- *(collab)* ✅ T3b PASS — offline edit survives full editor restart + yellow flag CLOSED ([268847e](https://github.com/cuttlefisch/mae/commit/268847e15e5814096493220f2be3e0f4b9437f34))
- *(collab)* ✅ T3b PASS — offline edit survives editor restart; T3c (crash) is next ([81d3145](https://github.com/cuttlefisch/mae/commit/81d314543c3b2d7633e18f9d1be64432d7d80697))
- *(collab)* T3c — kill-9 crash CHARACTERIZATION procedure (observe content/intent durability + adopt-clobber) ([5b67d2e](https://github.com/cuttlefisch/mae/commit/5b67d2e99b1b8f2634706012415d6745fea10cb6))
- *(collab)* 🔬 T3c kill-9 characterization — no clobber this run, but flush-window NOT stressed ([27ffc6d](https://github.com/cuttlefisch/mae/commit/27ffc6d5078fc84d12f93e6f2c29f474dd9166ff))
- *(adr)* ADR-022 — crash-safe convergent KB sync (state-vector reconcile, N peers) ([b41b321](https://github.com/cuttlefisch/mae/commit/b41b321bada8e362fed4e180392447eecfc71c08))
- *(collab)* ADR-022 as-built impl + B-17 lesson (the harness's first catch) ([cb3a65e](https://github.com/cuttlefisch/mae/commit/cb3a65eb17a58ca51f99ae4f41e5238b2a23bcd9))
- *(collab)* T3c-stress READY-TO-RUN (PASS/FAIL) — ADR-022 crash-safety, 2-machine ([a8650ea](https://github.com/cuttlefisch/mae/commit/a8650ea8881e04d59a6f22c1e550a551d84a9225))
- *(collab)* ✅ T3c-stress PASS (ADR-022 build) — clean pre-connect capture + auto-connect finding ([cbcdb4d](https://github.com/cuttlefisch/mae/commit/cbcdb4d4471c7ff860d7d84661c47ccf33a54cb6))
- *(collab)* T3c-stress LIVE RESULT — PASS (2 runs), ADR-022 crash-safety ([800a048](https://github.com/cuttlefisch/mae/commit/800a048feef3b4f24989f62bad8ae989eb514917))
- *(collab)* T4–T7 live matrix READY-TO-RUN (PASS/FAIL) — concurrent / multi-field / daemon-restart / roles ([d90561e](https://github.com/cuttlefisch/mae/commit/d90561ec0249625e17fbcc03be64f9a19f6fd505))
- *(collab)* T4 concurrent same-node — bob converged title (alice verify byte-for-byte) ([83a1b7c](https://github.com/cuttlefisch/mae/commit/83a1b7cc59e4ff74e993e8775a10afae14606337))
- *(collab)* ✅ T4 PASS — concurrent same-node convergence (byte-identical both machines) ([b2cdf79](https://github.com/cuttlefisch/mae/commit/b2cdf79b8a3910ecfc10b54da849ba6e2ac0d046))
- *(collab)* T5 — body/title multi-field PASS, NEW B-18 (tags YArray does not CRDT-sync) ([2816b82](https://github.com/cuttlefisch/mae/commit/2816b82a884cd90e8ca46f506b6d43390b50fe78))
- *(collab)* B-18 marked PROVISIONAL — step-3 tags run was muddled, controlled re-run protocol ([d041d77](https://github.com/cuttlefisch/mae/commit/d041d77769d63a508b9dac445281a266fa5d5dea))
- *(collab)* ✅ B-18 CONFIRMED (clean run) — tags YArray do not CRDT-sync ([3eba14a](https://github.com/cuttlefisch/mae/commit/3eba14a70f1834e95efafcc79b1d831cbf33a6f7))
- *(collab)* T5 verdict — title/body PASS, tags=B-18 (found live, fixed 97af88df) ([5736599](https://github.com/cuttlefisch/mae/commit/5736599aa7b572d0e72db84456c1f928ff7c1870))
- *(collab)* B-18 fix re-verify — alice->bob tags STILL no-op (likely alice send-side), bob send OK ([a131e9e](https://github.com/cuttlefisch/mae/commit/a131e9ed269496961e1a0605898697f07db7664e))
- *(collab)* ✅ B-18 FIX VERIFIED (alice->bob) — tags now CRDT-sync, root was alice old build ([53538a9](https://github.com/cuttlefisch/mae/commit/53538a9e9cf25f8bba8dae8b8995131c54e530f7))
- *(collab)* ✅ T5 FULL PASS — B-18 fix verified live both directions ([5fa8b05](https://github.com/cuttlefisch/mae/commit/5fa8b0503d4a75daeba672bc290b8d57513095f2))
- *(collab)* ✅ T6 daemon-restart survival — bob side PASS (alice to confirm WAL recovery + receive) ([d3ac2cf](https://github.com/cuttlefisch/mae/commit/d3ac2cfa8422563eb8f2d4fbd276030799620b4c))
- *(collab)* ✅ T6 PASS — ungraceful (kill -9) daemon restart, WAL crash-recovery ([46903d9](https://github.com/cuttlefisch/mae/commit/46903d97247aeef180ba82ca5e24e68a8d633418))
- *(collab)* ✅ T7 roles/policy PASS + 🎉 LIVE MATRIX T1–T7 COMPLETE ([14de807](https://github.com/cuttlefisch/mae/commit/14de8079842069436bd777eca75d2e9485df73e6))
- *(collab)* ✅ T7 PASS — roles/policy enforcement; FULL T1–T7 matrix green + B-19 security finding ([073b7c6](https://github.com/cuttlefisch/mae/commit/073b7c64b5e581aec1d081513ac087efebce08e4))
- *(adr)* ADR-023 — secure write-access for membership-gated KBs (epoch-fenced rebase) ([69a9400](https://github.com/cuttlefisch/mae/commit/69a9400bf7d22d35daaf47a2afc513afac94ea0a))
- *(collab)* Step 8 — B-19 epoch-fence live test plan + bob's next-test pointer ([a452969](https://github.com/cuttlefisch/mae/commit/a4529693b81ee8bad576a6ffc11a041c4025e24c))
- *(collab)* Step 8 — adapt to live state (collabtest KB, bob leftover-editor reset, manual connect) ([98c6368](https://github.com/cuttlefisch/mae/commit/98c6368079ea5eb7d2b4a6b80fe5972e3a318700))
- *(collab)* Step 8/B-19 epoch fence LIVE (steps 2-4 PASS) + UX hiccup flagged ([05dcfce](https://github.com/cuttlefisch/mae/commit/05dcfce04b2b9df2e02d54691aaf36aab9a0fb79))
- *(collab)* Step 8 8d BLOCKED — post-grant re-author also fenced (stale op persists), need adopt path ([38cdb35](https://github.com/cuttlefisch/mae/commit/38cdb35bb961e88bd652e6e660ccb9c3115d2c52))
- *(collab)* UX story — magit-style collab conflict/divergence buffer (CRDT-lifecycle review) ([12038f0](https://github.com/cuttlefisch/mae/commit/12038f0c290f775daee55eb329cefcab79f60725))
- *(collab)* Step 9 — ADR-024 notification-bus resolution UX live test plan ([37e1823](https://github.com/cuttlefisch/mae/commit/37e1823491d6d618dbc9351e2251423f1d9c322e))
- *(collab)* Step 9 readiness — 2 concerns for alice (notifications module not auto-enabled; staging A/B) ([8ce8b06](https://github.com/cuttlefisch/mae/commit/8ce8b06a7a7a7cde70e1cd84aee04f8656b9ca37))
- *(collab)* Step 9 GO — alice's answers (required-module tier done; Option A staging) ([6e55e99](https://github.com/cuttlefisch/mae/commit/6e55e9927ab20e6fd68ad4a0e8c91cdd28f105aa))
- *(collab)* ✅ Step 9 9a+9b PASS — 8d closed live via ADR-024 Keep-mine ([8afb0f5](https://github.com/cuttlefisch/mae/commit/8afb0f57893c70830e9e2f4a65d082ab0db3d7f5))
- *(collab)* 🛑 Step 9c — B-20: viewer-era edit CASCADES on demote->re-promote (epoch fence hole) ([897b949](https://github.com/cuttlefisch/mae/commit/897b949b65468e85414e84a354cd8e6389bd96fe))
- *(collab)* B-20 FIXED + 9c re-test plan for bob (no editor rebuild — daemon-side fix) ([1fc2627](https://github.com/cuttlefisch/mae/commit/1fc26276357b2cc21a32d0e8346be8d86deed1f7))
- *(collab)* Step 9c re-test STEP A — bob local state clean (B-20 fix daemon-side, no rebuild) ([f37aae7](https://github.com/cuttlefisch/mae/commit/f37aae7976155658d04beb82640e5d560595329f))
- *(collab)* 9c re-test GREEN-LIT — bob role=Editor(epoch4) confirmed + corrected Step B ([5511304](https://github.com/cuttlefisch/mae/commit/5511304d1e6e892ff9487936e874a08c336a0005))
- *(collab)* ✅ Step 9c RE-RUN — B-20 fix verified live (continuation fenced, no cascade) + Accept-remote ([597c66d](https://github.com/cuttlefisch/mae/commit/597c66d40875497ac54b2c73cf742732529dd512))
- *(collab)* Step 9c CLOSED (both sides) + proposed 9d (TOFU/R4 modal) strategy + open Qs for alice ([5d8f34f](https://github.com/cuttlefisch/mae/commit/5d8f34f735afe879d2e9ba9d61a87e2a737e1d94))
- *(collab)* 9c CLOSED (alice WAL throughout-proof) + 9d TOFU/R4 answers + green-light ([bdf059e](https://github.com/cuttlefisch/mae/commit/bdf059eff496d6328a04eb00c035b2a935b2ec37))
- *(collab)* 🛑 9d BLOCKED — B-21: runtime collab_host_key_policy not honored by connect path ([4e8a7de](https://github.com/cuttlefisch/mae/commit/4e8a7de5f383d3194cabd6d8b213d1ad67a38751))
- *(collab)* B-21 FIXED (8fe5a73e) + updated 9d instructions for bob (rebuild + runtime :set) ([e5538ae](https://github.com/cuttlefisch/mae/commit/e5538aefbda71b46f7a055207c74d4b2c073f2b2))
- *(collab)* 9d — B-21 fix CONFIRMED (runtime prompt honored) + 🛑 B-22 GUI TOFU modal render/focus bug ([cc52c7b](https://github.com/cuttlefisch/mae/commit/cc52c7bc2cc467368a7f13c9307b9fa7ab9c0778))
- *(collab)* B-22 fixed (render+focus, 371803c8) + 9d re-test for bob; B-22c deferred ([5337cb5](https://github.com/cuttlefisch/mae/commit/5337cb584759aafa863df576f178c7beefe981d5))
- *(collab)* 9d re-test — B-22b (focus) FIXED, but B-22a (modal render) STILL BROKEN ([f2e3e9d](https://github.com/cuttlefisch/mae/commit/f2e3e9d8b7c1775948b975f0dd27d7560aee4ae5))
- *(collab)* B-22a addendum — stall confined to the pending-prompt window (paint gated on resolution) ([ffa90d1](https://github.com/cuttlefisch/mae/commit/ffa90d12bc9815b6b0af25f1f85a7630bf032f86))
- *(collab)* B-22a code trace — TOFU modal DOES reuse MiniDialog; bug is repaint-scheduling ([101ded4](https://github.com/cuttlefisch/mae/commit/101ded4f63e257641605266fe158966ba3311804))
- *(collab)* 9d FUNCTIONALLY PASSES (accept+reject proven) + B-22c done; B-22a tracked ([33fbdd3](https://github.com/cuttlefisch/mae/commit/33fbdd3c7edc80b4833bcc18211f47b711a8fd55))
- *(collab)* B-22a experiment + interpretation matrix for bob (instrumentation c7a4bc49) ([fe14986](https://github.com/cuttlefisch/mae/commit/fe149869b63827afd5bdc341e14e1fe4bc8c567b))
- *(collab)* ✅ B-22a experiment DECISIVE — frame paints, modal OVERLAY skipped (render-path bug) ([c4c8901](https://github.com/cuttlefisch/mae/commit/c4c89011a575f2e6aa9bad072d857a2ec6903e55))
- *(collab)* B-22a fixed (render-path, both backends) + architectural overlay unification ([f526aef](https://github.com/cuttlefisch/mae/commit/f526aef0dc431c5dc1b4c6d3b80099639c79c2d4))
- *(collab)* ✅ B-22a FIXED (modal renders) + 🛑 NEW B-23 (modal doesn't size to contents, fp truncated) ([3d95153](https://github.com/cuttlefisch/mae/commit/3d95153609bc54ac6ea577b2205140911fc20065))
- *(collab)* B-23 STRUCTURAL fix recommendation — content-adaptive MiniDialog sizing, shared backends ([595c64d](https://github.com/cuttlefisch/mae/commit/595c64dd8e12e8b5d745312bd24886e7901f166e))
- *(collab)* B-23 FIXED (shared content-adaptive dialog sizing, c976c1f8) + rebuild-to-confirm ([a66449f](https://github.com/cuttlefisch/mae/commit/a66449f817e081b481d5ad0fd6cab12680620053))
- *(collab)* ✅✅ B-23 FIXED + 9d (TOFU/R4) FULL PASS — B-22/B-23 modal arc CLOSED ([a903927](https://github.com/cuttlefisch/mae/commit/a90392767fed83d993d1154384697a868d2930f8))
- *(collab)* Automated-coverage map + residual-manual flagging (Wave 3: A3) ([4c580f1](https://github.com/cuttlefisch/mae/commit/4c580f1848c549dc771b9e6e8a7b94b94872e2f9))
- *(collab)* KB-sharing management UX, scripting/AI guide, recovery (P5) ([ad8c45e](https://github.com/cuttlefisch/mae/commit/ad8c45ec61fe9b12fa1ae4e98955f48640a3f302))
- *(claude)* Refresh status line — KB sharing user-ready, v0.14.0 pending ([f473407](https://github.com/cuttlefisch/mae/commit/f4734072dd313119d551a412d2f2a586cc4d3d63))
- Sweep static docs for outdated config-topology + stale facts ([3342e0c](https://github.com/cuttlefisch/mae/commit/3342e0c32544155622b2bd520b7e095096e36f34))
- *(manual)* Sweep kb_seed manual for config-topology + stale facts + API arity ([7b82b73](https://github.com/cuttlefisch/mae/commit/7b82b732e79822c65e71d159f2816697ecb88561))
- *(ai)* Bring AI prompts + agent guidance up to date for v0.14.0 collab ([8ef9f46](https://github.com/cuttlefisch/mae/commit/8ef9f46dcd1fee2edcebb4e0c3f7da8a371146bd))
- *(roadmap)* Cross-ref deferred work to issues, reorganize, correct already-done items ([507ad4c](https://github.com/cuttlefisch/mae/commit/507ad4c8551ebb44ef5f1b033ff8ea906b76a5ca))

### Testing

- *(collab)* Add collabtest KB fixture + wire into membership e2e ([ca141cb](https://github.com/cuttlefisch/mae/commit/ca141cb323f1d13858b7c289e62f9aec69957816))
- *(collab)* Drop stray KB instance marker from collabtest fixture ([d352be2](https://github.com/cuttlefisch/mae/commit/d352be2655f40a56e0e9b4904152aa0c6552cea0))
- *(collab)* ADR-018 — membership e2e to invite→pending→approve (principal-keyed) ([6e13c9c](https://github.com/cuttlefisch/mae/commit/6e13c9ce239630bcf1806ff11af8a2d7ee6f9d75))
- *(collab)* B-8 repro (passes) + analysis — gate logic correct, bug is live-state-specific ([d9f7fbd](https://github.com/cuttlefisch/mae/commit/d9f7fbd0460daff4439aa5df7bbefbcdf3add29b))
- *(collab)* ADR-020 B-16 — production-fidelity two-peer CRDT tests (catch dummy/hardcoded params) ([02546bf](https://github.com/cuttlefisch/mae/commit/02546bf91e8644ee774a432030c0aff54a0c5ff0))
- *(collab)* W3 — collab_tcp_e2e uses shared wire builders (kills the B-8 trap) ([75c213a](https://github.com/cuttlefisch/mae/commit/75c213a285610c9a16e4e388c8ad48b0b0a982f3))
- *(collab)* W3 — revive network_e2e (dead gate → self-contained, runs in CI) ([a1e5fde](https://github.com/cuttlefisch/mae/commit/a1e5fde7c5b6ab2bb07096a14907cbc17332cb8b))
- *(collab)* T5 tags (B-18) + T6 KB-node restart WAL recovery e2e (ADR-023) ([fac0095](https://github.com/cuttlefisch/mae/commit/fac009596fc24c37821c2b5283a2fef63b221771))
- *(collab)* Automate manual fence/config/surface tests + fix non-UX bugs (Wave 1) ([9d7db62](https://github.com/cuttlefisch/mae/commit/9d7db622cece9028363b6ee7cac0516b166999de))
- *(collab)* Bridge adopt round-trip + real-daemon concurrent convergence (Wave 2: A2b, A5) ([0589518](https://github.com/cuttlefisch/mae/commit/0589518bae428c31d19770807a9bdae7981f3592))
- *(collab)* Lock primary KB store XDG-first contract (Wave 4: B5 / B-6) ([f036f20](https://github.com/cuttlefisch/mae/commit/f036f2050d9a016f4a46b752e4f41e080377f03b))
- *(collab)* Security-negative coverage — MITM no-overwrite + unauthorized-peer e2e (Wave 3: A4) ([b72e1eb](https://github.com/cuttlefisch/mae/commit/b72e1eb3bbca4f65c30a8e518d08b0b004fb5a0b))
- *(collab)* Daemon membership/policy gate audit + KB-sharing coverage map (P6) ([01b1a6d](https://github.com/cuttlefisch/mae/commit/01b1a6dbc26d3c339c6af6a242a0efe813df8c23))

### CI

- Run trusted-peer mTLS + per-KB membership e2e in the e2e job ([2b2f2de](https://github.com/cuttlefisch/mae/commit/2b2f2dee2dff6aff502775aab72a7917c23b96b6))

### Styling

- *(test)* Rustfmt network_e2e (drop leftover blank lines from the sed transform) ([b2ab2be](https://github.com/cuttlefisch/mae/commit/b2ab2be22a8b545ccc204162b9f994165a707700))

### Miscellaneous

- Sync Cargo.lock workspace versions to 0.13.12 ([88fd2d3](https://github.com/cuttlefisch/mae/commit/88fd2d306f18adc322f2a03411e106b76ca882d2))
- *(collab)* Retire B-22a instrumentation → clean collab-target host-key lifecycle tracing ([0f3750e](https://github.com/cuttlefisch/mae/commit/0f3750e9e40ef78f673c8fc488fda76017d8756c))
- *(deps)* Bump safe Rust dependencies (supersedes #68's non-crypto bumps) ([91be0a9](https://github.com/cuttlefisch/mae/commit/91be0a9a0d3754865ffc4af6013ee679c7dba855))
- *(skills)* Share the comprehensive /mae-audit command with all contributors ([e4a7a9e](https://github.com/cuttlefisch/mae/commit/e4a7a9e189d60e160852a4e5be5250f989e1c946))
- Bump version to 0.14.0 ([aeb1cdc](https://github.com/cuttlefisch/mae/commit/aeb1cdce17d228701073148545e7ed9e44f1dadf))

### Debug

- *(collab)* B-22a instrumentation — trace host-key prompt forwarder→delivery→paint ([c7a4bc4](https://github.com/cuttlefisch/mae/commit/c7a4bc49f5de51c8e1e7964a4ca1cdbd1f3d5c95))

## [0.13.12] - 2026-06-15




### Bug Fixes

- *(kb)* ADR-015 node links to existing concept:keymap-inheritance ([e287d24](https://github.com/cuttlefisch/mae/commit/e287d24419adfc67c0893bcf3d6c374123a8948e))

### CI

- *(release)* Point the Homebrew tap update at the renamed cask (mae-app) ([c28fad9](https://github.com/cuttlefisch/mae/commit/c28fad9729082b8cb9ef03aee7407dd19fa895cf))
- *(release)* Deploy tap bump via auto-merged PR (through branch protection) ([4e9d5f6](https://github.com/cuttlefisch/mae/commit/4e9d5f65d35bf8b0dd7c00d57538575d9718c5e8))

### Miscellaneous

- Bump version to 0.13.12 ([a1b33bd](https://github.com/cuttlefisch/mae/commit/a1b33bdd4280f397c2cc0113613bb33e5e966a90))

## [0.13.11] - 2026-06-14




### Features

- *(keymap)* Data-driven keymap registry + Scheme context API (Phase 1a, ADR-015) ([053eb22](https://github.com/cuttlefisch/mae/commit/053eb224d02a7afbc68171db264b7900adf3333d))
- *(keymap)* Shared navigation context for read-only nav buffers (Phase 1b) ([bb58240](https://github.com/cuttlefisch/mae/commit/bb5824087f03fb4bcfea5bf2e6ad926fc928d6e7))
- *(pkg)* `mae prune-shadows` — remove stale on-disk module copies ([cdc0c06](https://github.com/cuttlefisch/mae/commit/cdc0c06733bc7bbf458c5aa5dafa731151f443fc))

### Bug Fixes

- *(modules)* Keymap flavor & dependency-closure bugs that brick the leader menu ([817cc2d](https://github.com/cuttlefisch/mae/commit/817cc2dc9cad82448c02c3dea1bc7645158539e3))
- *(upgrade)* Self-heal Homebrew formula link so `mae upgrade` can't strand the CLI off PATH ([5a81fa4](https://github.com/cuttlefisch/mae/commit/5a81fa4eea3e232f8bc900bbfb6a184afc4d3452))
- *(pkg)* Unify reload pipeline + keymap_flavor authority + warn on stale shadow (C1/H2/H3/H4) ([d39c0ff](https://github.com/cuttlefisch/mae/commit/d39c0ff01e761defc71a9c79c0b3cf7abf30c22a))

### Refactor

- *(keymap)* Single layered resolution chain for dispatch + display (Phase 0, ADR-015) ([e1e58e0](https://github.com/cuttlefisch/mae/commit/e1e58e00a51e7e22af4ea36119d267fab5a5790b))

### Documentation

- *(adr)* ADR-015 keymap resolution chain, ADR-016 artifact interaction model ([0f62018](https://github.com/cuttlefisch/mae/commit/0f620181b146ab571c1d69c802f49dfc0447fad2))
- *(kb)* Mirror ADR-015/016 as concept KB nodes + KB Source headers ([286bf56](https://github.com/cuttlefisch/mae/commit/286bf569202f9aaaba02bd2bd7df85b6ad8ac28a))

### Miscellaneous

- Sync Cargo.lock to 0.13.10 after backmerging main ([c62ad34](https://github.com/cuttlefisch/mae/commit/c62ad348644c8d1619ffd8deff4590b9b5260191))
- Bump version to 0.13.11 ([ec7e56d](https://github.com/cuttlefisch/mae/commit/ec7e56d0b33aa7faaaf74294695349482ae24e4f))

## [0.13.10] - 2026-06-14




### Features

- *(cli)* Channel-aware `mae upgrade` self-upgrade (Doom-style) ([3f05d8c](https://github.com/cuttlefisch/mae/commit/3f05d8cd0ab6061274b4ba45270f9aad9bcf56d5))

### Bug Fixes

- *(kb)* Clippy needless-borrow in tests + regenerate code map ([4e7b333](https://github.com/cuttlefisch/mae/commit/4e7b33398dab70e2dc31a47ae5162433a247b169))

### Miscellaneous

- Sync Cargo.lock to 0.13.9 after merging release bump ([c0c51f7](https://github.com/cuttlefisch/mae/commit/c0c51f7ecf6d9a5ba414f734db573c40d955a178))
- Bump version to 0.13.10 ([4954c45](https://github.com/cuttlefisch/mae/commit/4954c45f07340df0a67884e4396c7ad390fefbb9))

## [0.13.9] - 2026-06-14




### Features

- *(kb)* Orderless field-weighted relevance ranker (search_ranked) ([fdbc6ee](https://github.com/cuttlefisch/mae/commit/fdbc6ee4b9f5fcaf3d9439e2d6a04c0f78596975))
- *(kb)* Route search through the relevance ranker + graded dipstick harness ([37f0a2c](https://github.com/cuttlefisch/mae/commit/37f0a2c11e5d0fce61e2709c184b59b78df90df0))
- *(kb)* Tune search_ranked to top-1 1.00 on graded dipstick ([45d7e61](https://github.com/cuttlefisch/mae/commit/45d7e6105750e4a3438bdbaf7d55960640f50d2f))
- *(kb)* KbScope-aware federated search + enriched kb_search output ([dac76b2](https://github.com/cuttlefisch/mae/commit/dac76b23c19f5b0df604de0b2c3400106cfa43d3))
- *(kb)* Recency sort + session visit tracking (Phase 3) ([eb05ff9](https://github.com/cuttlefisch/mae/commit/eb05ff9a6a3b9de6f7f2aefb2cf72a67e1e156c8))
- *(kb)* Graph relatedness — KnowledgeBase/Cozo `related` + kb_related tool (Phase 4) ([8bd763f](https://github.com/cuttlefisch/mae/commit/8bd763f344fe374736680b67019135d73c04901d))
- *(kb)* "Related" section in the KB buffer (Phase 2) ([2f13d1a](https://github.com/cuttlefisch/mae/commit/2f13d1afd0fbe287fac09b815846b3cf8b49d780))
- *(kb)* Kb_search_scope config option + honor it in kb_search (Phase 5, config surface) ([32babe7](https://github.com/cuttlefisch/mae/commit/32babe715556135a91a9cfe87cc5f4c66793a382))
- *(modules)* Embed built-in modules in the binary (always-present baseline) ([d92fec2](https://github.com/cuttlefisch/mae/commit/d92fec2df7a7d45e7dfa6b562530ef8edcb6b2b1))
- *(modules)* Live reload-modules / mae-reload command ([e073633](https://github.com/cuttlefisch/mae/commit/e07363360512468db14cffe7748bbe0e0f513b3c))
- *(keymaps)* Non-modal keybind flavor + transient leader keypad + live switching ([dfa0a24](https://github.com/cuttlefisch/mae/commit/dfa0a24d87bababafa64a12283d769a5af789f47))
- *(keymaps)* Guided flavor picker (dashboard quick-action) + GEMINI/manual sync ([d8bed02](https://github.com/cuttlefisch/mae/commit/d8bed02f722e8b1611631d5e9763290ff8c00740))
- *(test-runner)* File-boundary state isolation + leak detection ([9441536](https://github.com/cuttlefisch/mae/commit/9441536adf4afad8fc3ad36e64dcb9dd04fa2769))
- *(kb)* Guided KB-search-scope picker (Phase 5 UI) ([04e6c36](https://github.com/cuttlefisch/mae/commit/04e6c3663d823f516df4e00b03d965f4fce6c355))
- *(kb)* Lazy completion at scale + contract-aligned vector stub (Phase 6) ([70e9b56](https://github.com/cuttlefisch/mae/commit/70e9b566a70ca34c6c38b4f980942a56b95c0f15))

### Bug Fixes

- *(ci)* Collab-start test checks all keymaps + refresh code map ([567d16f](https://github.com/cuttlefisch/mae/commit/567d16f2a71a53831d9b6f05c0bf84c8ebfc92ae))
- *(e2e)* Hook dispatch order + test isolation for flavor/mode/options ([7e96429](https://github.com/cuttlefisch/mae/commit/7e96429535efef23107a1e48a5f1e73aa60571a1))

### Refactor

- *(paths)* Consolidate path resolvers into pkg::paths ([0b8bf5d](https://github.com/cuttlefisch/mae/commit/0b8bf5d32abaae93752030b9c27fa29e062bcbcb))
- *(keymaps)* Single source of truth — remove duplicated kernel leader tree ([072b4e2](https://github.com/cuttlefisch/mae/commit/072b4e245d483aee0af7b499708ca7f132151778))

### Documentation

- Update grading dipstick header for expanded metrics + perf companion ([6f7b7b5](https://github.com/cuttlefisch/mae/commit/6f7b7b58d2613d6c1a941c3e1321f152c08f7c5b))
- Document test isolation + clean env for e2e in CLAUDE.md ([74b53dc](https://github.com/cuttlefisch/mae/commit/74b53dc654c13bde67ed1ce8fff0aaf089a9072c))

### Testing

- *(kb)* Richer accuracy metrics + performance/scale validation (#38) ([7b013ae](https://github.com/cuttlefisch/mae/commit/7b013ae0423283a5ec12fe814640105bc2f0cc72))
- Regression coverage for module architecture; update CLAUDE/GEMINI ([6229ec2](https://github.com/cuttlefisch/mae/commit/6229ec232aea66d53fefb246f50cb293248ed739))

### Miscellaneous

- Bump version to 0.13.9 ([24b9387](https://github.com/cuttlefisch/mae/commit/24b93874b3e0c2d09bfa4ee8ed15ab6e87f18189))

## [0.13.8] - 2026-06-14




### Bug Fixes

- *(kb,modules,daemon)* Kb_view_query parse error, module discovery, daemon naming ([78ade4a](https://github.com/cuttlefisch/mae/commit/78ade4a867937a96bae41af1bfefdd39cb7e5c24))
- *(modules)* Unify discovery path, add macOS data dir, make load failures loud ([d2fe898](https://github.com/cuttlefisch/mae/commit/d2fe8980aa344b28b2f4817515a07bbf7cca415c))

### Miscellaneous

- Bump version to 0.13.8 ([8cc99ce](https://github.com/cuttlefisch/mae/commit/8cc99ce275dd56d5d31936eb8ec9e667f386e704))

## [0.13.7] - 2026-06-13




### Styling

- Rustfmt the macOS doc/test/font changes (+ Cargo.lock version sync) ([d0a81b1](https://github.com/cuttlefisch/mae/commit/d0a81b1bb9360ab906344b48fd0c9d4894ecd67b))

### Miscellaneous

- Bump version to 0.13.7 ([d43888c](https://github.com/cuttlefisch/mae/commit/d43888c30391c520ac9c33a8f945b3936574dfa1))

## [0.13.6] - 2026-06-12




### Features

- *(gui)* Launch GUI by default when a display is available, else TUI ([6d5458c](https://github.com/cuttlefisch/mae/commit/6d5458cc38e4370d97a1d5d9525d7961668bb9eb))
- *(gui)* Load a bundled font from MAE_FONT_DIR (font-agnostic) ([972fd07](https://github.com/cuttlefisch/mae/commit/972fd07caaefd6fd626abfe2cc5c2a6a12e9a2bf))
- *(gui)* Bundle a license-clean JetBrains Mono Nerd Font in MAE.app ([905879f](https://github.com/cuttlefisch/mae/commit/905879f817ad5ddd7bda25cdbbcef5129950ba2d))

### Bug Fixes

- *(gui)* PATH resolution, manual KB loading, and unified Scheme eval ([8c39a76](https://github.com/cuttlefisch/mae/commit/8c39a76471c35144ff94c7d0cb89e3d6a6bedc19))
- *(gui)* Fall back to system monospace font instead of failing to launch ([68ea980](https://github.com/cuttlefisch/mae/commit/68ea980c67448205332f2f51f6a17fbe969614ef))
- *(scheme)* Dismantle long cons chains iteratively to avoid stack overflow ([236ecf9](https://github.com/cuttlefisch/mae/commit/236ecf9321cdd9c44bb8da95292951448f2ce04c))
- *(kb)* Normalize watcher paths so macOS FSEvents events match seeded keys ([48178db](https://github.com/cuttlefisch/mae/commit/48178db7f7afa0116e58dbc29cb1e9ad3d9022fd))
- *(macos)* App bundle case collision, TERM leak, PATH gaps ([c4e387d](https://github.com/cuttlefisch/mae/commit/c4e387d2739c36fd8bb13e2fd13ccb19060de1fd))

### Documentation

- GUI-by-default launch, font/icon config, init.scm-primary config surface ([7a0ec90](https://github.com/cuttlefisch/mae/commit/7a0ec90b5b98d531392ee81c4c0c67885252ba7c))

### Testing

- Harden + speed up the suite on macOS (first macOS test run) ([383a952](https://github.com/cuttlefisch/mae/commit/383a95244929abde088ea3a0e244a350eb623b8a))

### Build System

- *(release)* Ship the GUI-capable binary as the macOS formula `mae` ([060f7dc](https://github.com/cuttlefisch/mae/commit/060f7dc3ad344b08ee7ca7f4ffd4ffae745dd73e))

### Miscellaneous

- Bump version to 0.13.6 ([420ab09](https://github.com/cuttlefisch/mae/commit/420ab0919afb03324813c9e9867ccabf6c8fd1f2))

## [0.13.5] - 2026-06-12




### Features

- *(macos)* Homebrew tap + quarantine/PATH fixes ([18e7ad5](https://github.com/cuttlefisch/mae/commit/18e7ad5196350a9e081f188fbab2c5e62e2e4b72))
- Interactive setup wizard + config consolidation ([eb291c3](https://github.com/cuttlefisch/mae/commit/eb291c33acc48e4eb26d48c6002f3e4eedea1f27))

### Bug Fixes

- *(ci)* Wait for CI to pass before tagging release ([1aec578](https://github.com/cuttlefisch/mae/commit/1aec57897008f6479973b09691c78103831ce5b4))

### Miscellaneous

- Bump version to 0.13.5 ([ad1924f](https://github.com/cuttlefisch/mae/commit/ad1924f1efbf6ae14ba292dee0ec3d4e68bfe8df))

## [0.13.4] - 2026-06-12




### Bug Fixes

- *(install)* Module manifest filename is module.toml not manifest.toml ([d7eb698](https://github.com/cuttlefisch/mae/commit/d7eb698204428911a6d492ef045250ede4f22282))

### Documentation

- Remove stale mae-state-server references ([117c901](https://github.com/cuttlefisch/mae/commit/117c901ccefbc5c2dcbda7eb210bb94379616cee))

### Miscellaneous

- Bump version to 0.13.4 ([5dbc552](https://github.com/cuttlefisch/mae/commit/5dbc552507656d1a26502e669a2bd5e5cfd1efb5))

## [0.13.3] - 2026-06-11




### Bug Fixes

- *(ci)* Use RELEASE_PAT for GitHub release creation ([bca2d20](https://github.com/cuttlefisch/mae/commit/bca2d204856fe43bfeed5f5e4d73fd253354f076))
- *(install)* Consolidate mae-state-server into mae-daemon + fix macOS bundle ([7ac86b9](https://github.com/cuttlefisch/mae/commit/7ac86b9c6b02d78c10cf058e702ed5d720f8d4fa))
- *(ci)* Daemon binary lookup for separate workspace ([bbe74fd](https://github.com/cuttlefisch/mae/commit/bbe74fd468c446e95db87f207c8ee1c681430a13))
- *(ci)* Add --config/--bind/--data-dir CLI flags to daemon ([752598c](https://github.com/cuttlefisch/mae/commit/752598ceb8b2c2363ce45d6d7d6731cac967a713))
- *(install)* MacOS launchd support, per-step validation, unified install flow ([8e07f9d](https://github.com/cuttlefisch/mae/commit/8e07f9dd064b64eee7a747bd8567f2d47a60d454))

### CI

- *(deps)* Bump peter-evans/create-or-update-comment ([12eec27](https://github.com/cuttlefisch/mae/commit/12eec2715be028157349a4ede6adbda62a6ea888))

### Miscellaneous

- Bump version to 0.13.3 ([b7219af](https://github.com/cuttlefisch/mae/commit/b7219af6f26f2a74c9ec97a7390780f0ce4b8fb4))

## [0.13.1] - 2026-06-10




### Features

- *(deps)* Upgrade yrs 0.27, rand 0.9, rusqlite 0.40 ([a85fd07](https://github.com/cuttlefisch/mae/commit/a85fd07862f410d7b917243fdeb7b2cbdcec84c4))

### Miscellaneous

- *(ci)* Ignore incompatible dep versions in dependabot ([3da2ca3](https://github.com/cuttlefisch/mae/commit/3da2ca3e42d7e579ed368c31de15d213ffeae4d2))
- *(ci)* Add weekly compat check for blocked dep upgrades ([3fb03f0](https://github.com/cuttlefisch/mae/commit/3fb03f0cc7229ee6b7d2aeba7d1529f1e6260165))
- Bump version to 0.13.1 ([0815704](https://github.com/cuttlefisch/mae/commit/08157047bf882a3e2d551976b52f0204fa99ae47))

## [0.13.0] - 2026-06-10




### Features

- Binary architecture split — editor + daemon workspaces (ADR-014) ([ac263f7](https://github.com/cuttlefisch/mae/commit/ac263f722275c0ab3bb1272075f1db14681aa7a6))
- LRU cache query layer + daemon client for editor-daemon integration ([100eb50](https://github.com/cuttlefisch/mae/commit/100eb5007555951b32bf34fa9d9dead3cd301705))
- Wire daemon connection at editor startup + config.toml support ([9d606b7](https://github.com/cuttlefisch/mae/commit/9d606b709e634071f474d2468a7c0a5e0ac97d08))
- AI hygiene daemon — deterministic KB quality assessment ([30c8deb](https://github.com/cuttlefisch/mae/commit/30c8deb35bcf903158c2c08e4e17470b288c90f6))
- Complete release pipeline + install script for v0.13.0 ([b8a7ba4](https://github.com/cuttlefisch/mae/commit/b8a7ba499dfb6883788f1e0661bded262f1781a8))
- Install script upgrade/uninstall support + CI validation ([df9fd34](https://github.com/cuttlefisch/mae/commit/df9fd34a997d96a10d16219f8cddc27fdeaaf481))
- *(ci)* Add release:none label to skip version bump ([29cf4e5](https://github.com/cuttlefisch/mae/commit/29cf4e5d48ac63a2b8d20168fad0265fa5fcf535))

### Bug Fixes

- Daemon/LRU code quality — NodeKind consistency, logging, missing override ([f6be9ea](https://github.com/cuttlefisch/mae/commit/f6be9ea1fcad1a8bae33e67cbecd62ee844164be))
- LRU cache correctness + daemon client hardening from code review ([1a5bf4d](https://github.com/cuttlefisch/mae/commit/1a5bf4d727fadc7ab85a5ad9c93dad270b4e0308))
- Audit findings — cap unbounded params, log errors, avoid extra alloc ([270dbbc](https://github.com/cuttlefisch/mae/commit/270dbbc481974006f9617bf7a813ab3c1a591eb0))
- Dockerfile paths for shared/ crates + upgrade lru to 0.16 ([60f32df](https://github.com/cuttlefisch/mae/commit/60f32df1b5ac966552d8cf410ba6d0dc06f83cc7))
- *(ci)* Build state-server + mcp-shim before staging artifacts ([0d5fe9e](https://github.com/cuttlefisch/mae/commit/0d5fe9e562ef0726bd6a649829e489c64fc727e1))
- *(ci)* Split cargo build commands for different packages ([7dfb1ab](https://github.com/cuttlefisch/mae/commit/7dfb1ab398bdbc77d0a579c0d6a8efb71285733d))
- *(ci)* Badges workflow — add gui feature, fix tokei paths ([4ca8625](https://github.com/cuttlefisch/mae/commit/4ca862523fc84756d07ef4c8ae4ef915f64692cb))
- *(release)* Build mae-mcp package in macOS release job ([0c82d7a](https://github.com/cuttlefisch/mae/commit/0c82d7aa28b46976d8768d2ef6e965a79dddda64))
- *(release)* Build Linux tarball with GUI (glibc) + fix download guide ([04d1310](https://github.com/cuttlefisch/mae/commit/04d13107e8def607ce6d8b2058e4ec749b67b9b8))

### CI

- Add install script validation job ([4e0ab23](https://github.com/cuttlefisch/mae/commit/4e0ab23f42510ce6657a151b701c494ac277d6c6))
- Reuse pre-built artifacts in install-test job ([528f678](https://github.com/cuttlefisch/mae/commit/528f67861bbd588d4b4eb3906aedd39eae26e70e))

### Miscellaneous

- Bump version to 0.11.5 ([a28d8a2](https://github.com/cuttlefisch/mae/commit/a28d8a2ba7aa1a7dc19346026501bad1f03b5c0a))
- CI/CD, Makefile, release pipeline for daemon + ADR-014 ([2532d8c](https://github.com/cuttlefisch/mae/commit/2532d8c302ad7d11ffcd6a09ad8fadfb0f1659a7))
- Revert version to 0.12.0 + update version-bump workflow ([1710aec](https://github.com/cuttlefisch/mae/commit/1710aec1c42e20638a1a0aeb65aee2826714c2b8))
- Bump version to 0.13.0 ([8c30de5](https://github.com/cuttlefisch/mae/commit/8c30de557b1b9770a3c878c2b0f9ff14aa94e25e))

## [0.11.4] - 2026-06-04




### Features

- *(kb)* Ex-commands, Scheme wrappers, Babel Datalog, docs + self-test updates ([fa59e09](https://github.com/cuttlefisch/mae/commit/fa59e09860dd6577d51ae14e82b24ced6e587375))
- *(kb)* Persistent graph KB — remove rusqlite, pre-built manual KB ([773bb37](https://github.com/cuttlefisch/mae/commit/773bb37217424a2222cac41c0da97f29fd40b44f))
- *(kb)* CozoDB-direct ingestion pipeline with IngestMode + content hash tracking ([06d91d5](https://github.com/cuttlefisch/mae/commit/06d91d5330443030bb7f38a6cf16f82ce4bc7db1))
- *(kb)* Scale validation test + ADR-012 Phase 2/3 docs ([66dd16f](https://github.com/cuttlefisch/mae/commit/66dd16fe17cd6747a3ece6fbbf2fa93f57c1d997))
- *(kb)* CozoDB-first query layer — KbQueryLayer trait + 46 migration sites ([b89cee6](https://github.com/cuttlefisch/mae/commit/b89cee6269092db2cd0f7498b9c81d5a4415883e))

### Bug Fixes

- *(kb)* Update_crdt_doc missing origin_instance + relax FTS threshold ([0dd479c](https://github.com/cuttlefisch/mae/commit/0dd479cd62561b3e2adea4be7e6643a0e49aaf95))
- *(release)* Bundle manual KB, modules, and sample config in all release artifacts ([2903920](https://github.com/cuttlefisch/mae/commit/29039200993bab7da042b18c7f95ce37eb4948d1))
- *(kb)* Federated query layer, batch loading, org heading conventions ([8bae241](https://github.com/cuttlefisch/mae/commit/8bae241848e4d6bdb7cfe8aa059c01c9afc22709))

### Documentation

- Update ROADMAP + CLAUDE.md for v0.12.0 persistent KB completion ([caddc38](https://github.com/cuttlefisch/mae/commit/caddc387e5d5dff62f7e4650914c1254e7b67374))
- ADR-013 KB query architecture + fix nightly CI + roadmap binary review ([b252812](https://github.com/cuttlefisch/mae/commit/b252812ca423be54770110a6c14b902403e52936))

### CI

- Skip CozoDB integration tests on nightly Rust ([74a93d0](https://github.com/cuttlefisch/mae/commit/74a93d0f26263206d4eb3d269df8348c5947b251))
- *(deps)* Bump the ci-dependencies group with 2 updates ([2813128](https://github.com/cuttlefisch/mae/commit/2813128f97bf1c764c97e3cfc998e88744cd49a6))

### Miscellaneous

- Bump version to 0.12.0 + changelog + roadmap update ([feadcb7](https://github.com/cuttlefisch/mae/commit/feadcb70ca9ecaeffc363f3ecca6d2e3e7d315aa))
- *(deps)* Update compatible dependencies via cargo update ([0d4a3d8](https://github.com/cuttlefisch/mae/commit/0d4a3d8da187a0eb7d78debae74aa2f9bce4c08b))
- Bump version to 0.11.4 ([267e70a](https://github.com/cuttlefisch/mae/commit/267e70a3ac906308d72e144e42f0f2e2947d4462))

## [0.11.3] - 2026-06-01




### Features

- *(kb)* CozoDB-primary graph KB with enhanced schema (v0.12.0 Phases A-F,H) ([f211998](https://github.com/cuttlefisch/mae/commit/f2119989af341128d799f791783edc980f1785c5))
- *(kb)* HNSW vector embeddings + GraphRAG query template (Phase G) ([fbdcfb5](https://github.com/cuttlefisch/mae/commit/fbdcfb5a9cda9038df7a018d73d9386796d87202))
- *(kb)* Seed 6 pre-built view flavors (Phase H complete) ([37ac4c7](https://github.com/cuttlefisch/mae/commit/37ac4c79dd7317c08966d808524f06a9dba366cb))
- *(kb)* AI tools for graph KB + Phase I validation suite ([5abfd01](https://github.com/cuttlefisch/mae/commit/5abfd01abec63e39d18a697209ddc1c437106e97))

### Bug Fixes

- *(ci)* Bump test timeout 20m→30m ([c5707d1](https://github.com/cuttlefisch/mae/commit/c5707d17afd6d05d43a06d58ce672ee1ab20a72f))
- *(release)* Use ditto for macOS .app zip to preserve metadata ([e913a4b](https://github.com/cuttlefisch/mae/commit/e913a4b44eef16db31b0caf1937b30a6f124a0f3))
- *(release)* Include mae-state-server in macOS GUI zip + improve release notes ([4007e99](https://github.com/cuttlefisch/mae/commit/4007e99b5e0128970042cc69d9fb87760eddac72))
- *(kb)* Migrate ~230 seed nodes to correct NodeKind + fix 12 broken links ([2c55e06](https://github.com/cuttlefisch/mae/commit/2c55e06ea1c8b52befaca513d38599f785cc1c14))

### Documentation

- Update ROADMAP + ADR-011 for v0.12.0 CozoDB-primary graph KB ([a80bfae](https://github.com/cuttlefisch/mae/commit/a80bfae529362bccacae3ff353140a23bc79b072))

### Miscellaneous

- Bump version to 0.11.3 ([0c6bb76](https://github.com/cuttlefisch/mae/commit/0c6bb76cc06c49b986d77339c0389453f2603b24))

## [0.11.2] - 2026-06-01




### Features

- KbStore trait + SQLite-first persistence (ADR-011) ([923cbf6](https://github.com/cuttlefisch/mae/commit/923cbf6a21651758204ae4a87824bc3ade0f19b6))
- CozoKbStore — graph-native KB backend behind feature flag ([6f5dc2a](https://github.com/cuttlefisch/mae/commit/6f5dc2a0d7750db764fe2bd01483d13f27a8edef))
- Graph-native AI tools + KbStore trait graph extensions ([ad17dee](https://github.com/cuttlefisch/mae/commit/ad17deeffdbcd3dbe4b15a5840fbd448c18f89dd))

### Bug Fixes

- AI tool parity, code quality, docs — v0.11.1 stability ([b424e8a](https://github.com/cuttlefisch/mae/commit/b424e8a7b36fbd709f4e1a426c923fec73072d22))
- CozoDB Tantivy FTS — post-query verification for sled backend ([7ae2233](https://github.com/cuttlefisch/mae/commit/7ae22337652c7348dc989b0f37a35a08082d9d63))
- *(ci)* Cargo-deny all-features + cozo dep cleanup ([14c8377](https://github.com/cuttlefisch/mae/commit/14c83773af692c8aa5fbba50ac3afec159f73615))
- *(ci)* Add rollup job for "stable / check" branch protection ([0435e59](https://github.com/cuttlefisch/mae/commit/0435e59a924ef43d8467cdce3d5181496e5f5323))
- *(ci)* Remove rollup job, fix branch protection instead ([5ff8f08](https://github.com/cuttlefisch/mae/commit/5ff8f08968835f19411034f89cb69afd4e1a56e8))
- Revert version 0.12.0→0.11.1, patch-only default for version bumps ([7a5d2fc](https://github.com/cuttlefisch/mae/commit/7a5d2fcc1942ea8d2becb7107b1aebc1a12c0ffc))
- *(ci)* Nightly tests use debug profile to avoid 20min timeout ([d97945d](https://github.com/cuttlefisch/mae/commit/d97945d329b10e684c54a5e269cf02d7494eb4e1))

### Documentation

- Update ROADMAP — Phase A+B complete, CozoDB backend shipped ([c788d86](https://github.com/cuttlefisch/mae/commit/c788d863e54f434303e21b64d9ac83eb40073c2a))
- Pre-release KB + config validation for v0.11.1 multi-user testing ([78872b6](https://github.com/cuttlefisch/mae/commit/78872b6f001fa739ccd944ebab788c964469d6ed))
- Fix stale auth claims — PSK shipped in v0.11.0, not "no auth" ([1284ff3](https://github.com/cuttlefisch/mae/commit/1284ff3de405868d837a9b2bf1bcbd08c6731fc0))
- *(CLAUDE.md)* Add 8 missing crates to layout table (12→20) ([71b89b4](https://github.com/cuttlefisch/mae/commit/71b89b4843c6c8d875ff2ad4768628981fbbc3f2))

### Testing

- KB lifecycle E2E suite (24 Rust + 3 Scheme tests) ([b3d6edc](https://github.com/cuttlefisch/mae/commit/b3d6edc045460c6aa1c125000aca16fe0945154c))

### CI

- Add CozoDB backend tests + KB lifecycle E2E to CI ([69b9f27](https://github.com/cuttlefisch/mae/commit/69b9f27bd3db6214126fd7d813e96904c16fa1b4))
- Remove redundant gui job — stable/test already builds GUI binary ([d54b10f](https://github.com/cuttlefisch/mae/commit/d54b10f2431a460d8531a878bc90cfe82f19525d))
- Run tests in release profile to eliminate redundant build ([8f855f8](https://github.com/cuttlefisch/mae/commit/8f855f828757069929785f2cc21932a02fa4c16d))

### Miscellaneous

- Bump version to 0.12.0 ([f2c836d](https://github.com/cuttlefisch/mae/commit/f2c836dfa6303972bcd1c1345b90e71ca229e822))
- Bump version to 0.11.2 ([91aa06a](https://github.com/cuttlefisch/mae/commit/91aa06a895d46e987c7ee35ef34d0e836a23aad4))

## [0.11.0] - 2026-05-30




### Features

- PSK auth, KB data dir standardization, project pruning ([fffa39f](https://github.com/cuttlefisch/mae/commit/fffa39ff33852aeae4643ff35dfcac6b3a37e31a))
- KB sharing E2E — bridge, intent wiring, scoped join/leave, 8 TCP tests ([710d223](https://github.com/cuttlefisch/mae/commit/710d2235b9262f9af9ace0106be99640f5849b68))
- Phase 4 — continuous KB sync (local edits → CRDT updates) ([09731d3](https://github.com/cuttlefisch/mae/commit/09731d359c30c1b0bde1eedeca26f303f556b28e))
- Phase 8 — offline KB sync queue + status line indicators ([4ae5de4](https://github.com/cuttlefisch/mae/commit/4ae5de42e30b15aa909e55c64899d28acb9986c8))
- Phase 6 — mDNS discovery for P2P KB sharing ([39256ec](https://github.com/cuttlefisch/mae/commit/39256ecc2f4c026846b7e972c9eb44deb4fb9175))
- Client PSK auth, mDNS wiring, collab-discover command, network docs ([893ab07](https://github.com/cuttlefisch/mae/commit/893ab07afd5d698eb4bacd8b362b1265c740d3f4))
- Mae-canvas crate scaffold (KB graph visualization) ([8ab7438](https://github.com/cuttlefisch/mae/commit/8ab743860b931260e6d4890b28a958480eaa812a))

### Bug Fixes

- Nightly clippy for_kv_map lint, CI TCP E2E timeout ([3791180](https://github.com/cuttlefisch/mae/commit/3791180e4508d0af6ca339230d14f9c5af7ad134))
- Nightly clippy useless_format lint in mae-ai self_test ([2fcb812](https://github.com/cuttlefisch/mae/commit/2fcb8124173f5f4809b8e8aed07674dd9b79b6e9))
- Include mae-state-server in macOS release artifact ([2a33b8a](https://github.com/cuttlefisch/mae/commit/2a33b8a1f359d1f06dc79875b9eca64762fb4ac5))

### Documentation

- Mark 5 fixed bugs in ROADMAP, update version + test count ([0824ec0](https://github.com/cuttlefisch/mae/commit/0824ec06777c2f68ddc1b496fdf4fb46e85a268b))
- Phase 7 — KB sharing user guide + ROADMAP update ([90077a3](https://github.com/cuttlefisch/mae/commit/90077a31500761e9885c147821b213441b86606a))

### CI

- Drop redundant check matrix + reuse binary artifact for e2e ([c00d1fd](https://github.com/cuttlefisch/mae/commit/c00d1fd00a6ce72895b2ba4bad22ca725df442ba))

### Miscellaneous

- Bump version to 0.11.0 ([57a2e9a](https://github.com/cuttlefisch/mae/commit/57a2e9a0917a695e1065b26c28478104791e93c7))

## [0.10.6] - 2026-05-28




### Bug Fixes

- *(collab)* FNV-1a client_id hash + comprehensive test gap closure ([5d9b7f9](https://github.com/cuttlefisch/mae/commit/5d9b7f921ffc03c09bd4ed1fa4be54ee19c405a3))
- *(collab)* Cursor drift on remote edits — adjust offset by edit position ([01f11fc](https://github.com/cuttlefisch/mae/commit/01f11fc9a692c6ca665b047f002bb22876391850))
- *(collab)* Awareness not rendering — JSON format mismatch + missing subscription ([a42130d](https://github.com/cuttlefisch/mae/commit/a42130dc9e3e80712bd33d194dda2bf984fa5053))

### Testing

- *(collab)* Add 12 round-trip deserialization tests for protocol features ([a225146](https://github.com/cuttlefisch/mae/commit/a2251462884cbb7b0195948f9951e34be209f970))

### CI

- Add 20m timeout to cargo test step (prevents nightly hangs) ([dfddb57](https://github.com/cuttlefisch/mae/commit/dfddb57950f8eed130b1d129656f608cb03dfde9))
- Remove e2e dependency on full check matrix (runs in parallel now) ([5987b4e](https://github.com/cuttlefisch/mae/commit/5987b4ebb1b527fc7d65ea86d3aca0dd4c49c057))

### Miscellaneous

- Bump version to 0.10.6 ([d3c5ec4](https://github.com/cuttlefisch/mae/commit/d3c5ec41520b62b1d81a8823f8de0f1c70f1f4e1))

## [0.10.5] - 2026-05-27




### Features

- *(scheme)* Phase 13a+13b — mae-scheme reader, compiler, and bytecode VM ([3f1ad27](https://github.com/cuttlefisch/mae/commit/3f1ad2767c5ba0bff6c3316cbe5cae12db609bf8))
- *(scheme)* Phase 13c — R7RS standard library (48 new tests) ([ddd45e6](https://github.com/cuttlefisch/mae/commit/ddd45e63b301a5cfd5a2b5e1e36aa472165c4632))
- *(scheme)* Phase 13d — hygienic macros, module system, R7RS hardening ([88ad180](https://github.com/cuttlefisch/mae/commit/88ad180ccaf18d13c93c72502ef57d573f1aa282))
- *(scheme)* R7RS compliance hardening — ~40 missing functions, 22 new tests ([2177b38](https://github.com/cuttlefisch/mae/commit/2177b38d597008fb0a3c8bb22983af5b0d9b8661))
- *(scheme)* R7RS compliance — multi-list map, ports, binary I/O, let-values ([7c9a332](https://github.com/cuttlefisch/mae/commit/7c9a332f3fb4c3c232a697536b9829e779c490fa))
- *(scheme)* R7RS compliance — file I/O, cond-expand, time, process context ([37a29e5](https://github.com/cuttlefisch/mae/commit/37a29e52b9fc267258c366a0a0c07a15a4e07a77))
- *(scheme)* Read, eval, define-values, vector->list ranges ([8917167](https://github.com/cuttlefisch/mae/commit/8917167d6a22dac3af671bd4319f334410f6226a))
- *(scheme)* R7RS edge cases — case-insensitive ops, with-exception-handler, UTF-8 fixes ([10649ba](https://github.com/cuttlefisch/mae/commit/10649ba9c02b403382b0e3b45022aba8632c189b))
- *(scheme)* Close R7RS gaps — floor/, truncate/, rationalize, let-syntax, port close ([9add982](https://github.com/cuttlefisch/mae/commit/9add9829285cc4d406f4bace8023b0b4de77e955))
- *(scheme)* (scheme inexact) + (scheme file) libraries, 292 R7RS tests ([c827dd3](https://github.com/cuttlefisch/mae/commit/c827dd3425280f13e570f4d230665fc704f55158))
- *(scheme)* Include, load, load_paths + library system tests (Phase 13d) ([1a3d53a](https://github.com/cuttlefisch/mae/commit/1a3d53ad6080016b51ee95b8fcadabd793e763ea))
- *(scheme)* Blocking sleep-ms + timing test (Phase 13f foundation) ([00c82fc](https://github.com/cuttlefisch/mae/commit/00c82fca782ce555b0506bb8cba44b914f510705))
- *(scheme)* Top-level load + with-output-to-file, 300 R7RS tests ([4c01ec9](https://github.com/cuttlefisch/mae/commit/4c01ec9adda24aa72b6ced8f94edddc650780ab0))
- *(scheme)* Cond => arrow clause + map shortest-list, 428 tests ([cc5feb9](https://github.com/cuttlefisch/mae/commit/cc5feb9a8dcdd806f4cb29027adcfee4f7af058d))
- *(scheme)* 105 IO/port test fixtures + remove dead write-char ([a9a126a](https://github.com/cuttlefisch/mae/commit/a9a126a7cafba794bb8c00a600f506d95e5bf9c8))
- *(scheme)* Eval, call-with-values, radix/exactness, spec stances + 362 R7RS tests ([172aa9d](https://github.com/cuttlefisch/mae/commit/172aa9d77949dad9cefc388c257990c0cd57b820))
- *(scheme)* Structural PartialEq, force/delay-force, min/max inexact + 409 R7RS tests ([694cb9e](https://github.com/cuttlefisch/mae/commit/694cb9e3d6e2072f2afb99e43ea95fc2c28754ce))
- *(scheme)* Comprehensive R7RS test coverage — 431 compliance tests ([7be0924](https://github.com/cuttlefisch/mae/commit/7be0924d8b7c2e93fa0a8e808bfc8a87ce627593))
- *(scheme)* GC observability, Trace completeness, cycle-risk documentation ([64005a3](https://github.com/cuttlefisch/mae/commit/64005a3bfe947c460a2f442cff7b93ea1d404bdd))
- *(scheme)* R7RS §6.11 exception system (Chibi-Scheme pattern) ([7ba1909](https://github.com/cuttlefisch/mae/commit/7ba19094a575b7e8a79dcbdff6c117ec2bf59773))
- *(scheme)* Eliminate spec-lawyering — proper char-ready?, rationalize, ellipsis, stdin ([be7abb0](https://github.com/cuttlefisch/mae/commit/be7abb06a4568aea0395276d0bb9c7d6ffb6466e))
- *(scheme)* Phase 13e — replace Steel with mae-scheme VM + purge all references ([0b15c06](https://github.com/cuttlefisch/mae/commit/0b15c0684df5c699bc598c80f4be040fa0b7eea0))
- *(scheme)* Phase 13f — async/yield infrastructure + (mae async) library ([a0e8732](https://github.com/cuttlefisch/mae/commit/a0e8732ef7ed0210fdc0fe56370722adf14d3533))
- *(test)* Auto-flush wrappers + consolidated test architecture ([24f7772](https://github.com/cuttlefisch/mae/commit/24f77721e86ab6e2484ab90eb60132e175503111))
- *(scheme)* Phase 13g — in-process Scheme LSP (Swank-style) ([29be90d](https://github.com/cuttlefisch/mae/commit/29be90d854035895eae4e194396659a306429c21))
- *(scheme)* Source maps + go-to-definition for Scheme LSP ([2b1faeb](https://github.com/cuttlefisch/mae/commit/2b1faebdd62aeaf89c974db01501ba9dfbc0e28f))
- *(scheme)* Phase 13g — DAP infrastructure (breakpoints, stepping, debug mode) ([91d0a14](https://github.com/cuttlefisch/mae/commit/91d0a1465983b77664411a1ccd574ab2892b6c72))
- *(scheme)* Phase 13g — Scheme DAP bridge (in-process debugger) ([effb9b4](https://github.com/cuttlefisch/mae/commit/effb9b48baf328f9c7fa7d07f8fcce1b049a6086))
- *(scheme)* Phase 13h — introspection + observability ([6085b63](https://github.com/cuttlefisch/mae/commit/6085b632744bb0d667e7de99eb484930975030ad))
- *(scheme)* Yield-tick + await-hook primitives, MCP eval_scheme fix, set_mode resilience ([39caf8e](https://github.com/cuttlefisch/mae/commit/39caf8ef46ee17e05e10be362041bbd4f564ad16))
- *(hooks)* Wire all 25 well-known hooks + event-driven test primitives ([c551562](https://github.com/cuttlefisch/mae/commit/c5515627f5f82ac555965fa96275f411ff12f061))
- *(collab)* Doc-scoped event broadcasting + E2E test hardening ([b3fe991](https://github.com/cuttlefisch/mae/commit/b3fe991ef0e61909a08bd3fad49153d33e8eb009))
- *(scheme)* Phase 13i — R7RS library system + proper §5.6 isolation ([002432d](https://github.com/cuttlefisch/mae/commit/002432d10da10312efd608b67ada232ff56d2485))

### Bug Fixes

- *(scheme)* Let/let* stack corruption + Phase 13d derived expressions ([0207360](https://github.com/cuttlefisch/mae/commit/0207360bd6b4dca4203936b24ee47ad6a4297a01))
- *(scheme)* TCO for and/or + 24 stress tests (287 total R7RS tests) ([fd16796](https://github.com/cuttlefisch/mae/commit/fd1679602a3e9ab67fb0cafe1eec683b6f6e37e0))
- *(scheme)* 3 critical compiler bugs + 128 torture/benchmark tests ([f7e48db](https://github.com/cuttlefisch/mae/commit/f7e48dbe3c9843fb6822f10eafe160c453be9794))
- *(scheme)* Closed port operations now error properly ([003976b](https://github.com/cuttlefisch/mae/commit/003976b9eb91b5fa973b98638936afc87f7e0fa5))
- *(scheme)* R7RS compliance — dynamic-wind+call/cc, file-error?, member/assoc comparator, port redirection ([0ee15c8](https://github.com/cuttlefisch/mae/commit/0ee15c841715cffa67a2ec1bba416cf930b604ce))
- *(scheme)* 16 audit fixes — binary ports, parameterize, record-type, overflow safety ([1e3b882](https://github.com/cuttlefisch/mae/commit/1e3b882bc3f3451874e3bce6986b73ba6a9b30f8))
- *(scheme)* VM foreign fn arity check + 215 branch-level tests (1115 R7RS) ([cdfab9f](https://github.com/cuttlefisch/mae/commit/cdfab9f46f7a909912aa32fd9d721a0c52911a57))
- *(scheme)* Consolidate yield primitives + replace static sleeps in E2E tests ([79dbe76](https://github.com/cuttlefisch/mae/commit/79dbe768b3421cccfe011af5f462437d13aafb96))
- *(test)* Split write-file + file-exists? into separate test steps ([711e590](https://github.com/cuttlefisch/mae/commit/711e59000e43113f8854fccaca7d62a672fa02ac))
- *(collab)* Prevent CRDT undo update loss + Docker E2E orchestration ([c9b0a06](https://github.com/cuttlefisch/mae/commit/c9b0a06bc773f99a33113966c9580569e516b5c5))
- *(collab)* CRDT undo cursor positioning + undo stack size limit API ([fb5120b](https://github.com/cuttlefisch/mae/commit/fb5120be8f8fc9c90526a223b79b149ea54b46ac))
- *(crdt)* UTF-16 offset encoding + content hash modified flag + cursor drift ([92a20b8](https://github.com/cuttlefisch/mae/commit/92a20b8625991dbfb1c38d65c1a40affa6fc6314))

### Refactor

- *(core)* Extract LspContext from Editor struct (22 fields) ([9618461](https://github.com/cuttlefisch/mae/commit/9618461c2f54308c6cbd41945d4db884d27c8be0))

### Documentation

- *(scheme)* Document exception system architecture in SPEC_STANCES.md ([b8bb2eb](https://github.com/cuttlefisch/mae/commit/b8bb2eb677fb8b528c83d38ecaab4cff98222955))
- *(collab)* Update E2E README — yield primitives now drain events ([9e6f004](https://github.com/cuttlefisch/mae/commit/9e6f0047adda1892b7b97b5edc10116740372035))
- *(scheme)* Phase 13j — ADR-009 + EXTENSION_GUIDE + ROADMAP completion ([04c12d4](https://github.com/cuttlefisch/mae/commit/04c12d457fe7b53a3eb3f083eae4b04a0fdeaab6))

### Testing

- *(scheme)* 60 edge-case tests + 7 R7RS compliance fixes ([65c1cbc](https://github.com/cuttlefisch/mae/commit/65c1cbc9ac135d9441594c62c20c8ac51ca4bb7c))
- *(scheme)* Phase 13g — E2E + performance tests for LSP & DAP ([f038045](https://github.com/cuttlefisch/mae/commit/f0380452798f7f8bbccb5816f7b7d0ef89d12381))
- *(collab)* Close test gaps — multi-doc, WAL recovery, corrupted state, stress ([463e859](https://github.com/cuttlefisch/mae/commit/463e859ce444b7b1f0374513aa5b299ac3df5590))
- Awareness E2E + editor test coverage gaps (marks, hooks, macros, surround, windows) ([dc13e13](https://github.com/cuttlefisch/mae/commit/dc13e13b2f64590dbdf95d1b021a46851cbb04bd))

### CI

- *(scheme)* Add R7RS compliance CI job, un-ignore fib(30) benchmark ([3a45208](https://github.com/cuttlefisch/mae/commit/3a45208aed10d267d2a6911ead0d04915eb5ec0a))
- Add missing test suites — IO ports, collab-local, timeout bump ([98e78f3](https://github.com/cuttlefisch/mae/commit/98e78f384932f800fde200b15490231081d320cf))

### Miscellaneous

- Bump version to 0.10.5 ([c57d52d](https://github.com/cuttlefisch/mae/commit/c57d52d91e6258a6ea3ac861310297b19165081e))

## [0.10.4] - 2026-05-24




### Features

- *(sync)* Per-user CRDT undo via yrs UndoManager ([9d8f169](https://github.com/cuttlefisch/mae/commit/9d8f169aaf3bdede4ca7bc269fa5736fb23dcbef))
- *(collab)* Awareness protocol — cursor/selection/presence sharing ([b6d3c1c](https://github.com/cuttlefisch/mae/commit/b6d3c1cbc4df8fa78d94707c59b8dbee64c8c3bc))
- *(collab)* Long-lived session tests + debug observability ([5fa8d3c](https://github.com/cuttlefisch/mae/commit/5fa8d3c5c0624d5f8a187d1583b1390e7f1b4ead))

### Bug Fixes

- *(collab)* Buffer status indicators, save guard, sharer notifications, reconnect backoff ([8de53b8](https://github.com/cuttlefisch/mae/commit/8de53b817c36fd4ec3302162d40d682f9118013e))
- *(docker)* Add 7 missing crates to Dockerfile, fix collab E2E test_smoke ([b765978](https://github.com/cuttlefisch/mae/commit/b765978fc2857ea3bf332a9e6c6f4712ca92335b))
- *(collab)* Remove (load) from undo E2E tests, fix verifier mounts ([ba3faa2](https://github.com/cuttlefisch/mae/commit/ba3faa2947bff91fb649d78ea919471e4b550205))
- *(gui)* Nightly clippy redundant reference + collab diagnostic logging ([968c6d6](https://github.com/cuttlefisch/mae/commit/968c6d649acfd0da589150705d00fa4ae61229ed))
- *(collab)* Headless test runner missing drain_and_broadcast — local CRDT edits never forwarded ([5cf6250](https://github.com/cuttlefisch/mae/commit/5cf6250c9ba23769c8ef826c3e60afd4298d8300))
- *(docker)* Pre-create /workspace and /shared with mae user ownership ([d1f5395](https://github.com/cuttlefisch/mae/commit/d1f5395323d1a495b2f9a9e460746c35b0afdcba))
- *(collab)* Use server-resolved doc_id in join response, not client-supplied ([8b0e3fd](https://github.com/cuttlefisch/mae/commit/8b0e3fdaaabeb58bf72951ce3e036ea1adde839f))
- *(collab)* Read_message partial-peek framing, intent drain ordering, diagnostics ([4be3b61](https://github.com/cuttlefisch/mae/commit/4be3b61b429114ae07d4a1650488728771777fbb))
- *(collab)* Structured diagnostic logging + healthcheck noise fix ([2b0887c](https://github.com/cuttlefisch/mae/commit/2b0887c3d9f645611bb11f7036fafbe1d6d64f36))
- *(collab)* Cross-client crosstalk, ForceSync undo wipe, Docker E2E orchestration ([afae68a](https://github.com/cuttlefisch/mae/commit/afae68af5929af1a1aaee93fef91b8a48d0116d4))
- *(test)* Buffer-drain-updates always-accumulate + Docker orchestration cleanup ([baa30ca](https://github.com/cuttlefisch/mae/commit/baa30ca458b734508091a7fd4b5f713a6ca333c1))
- *(ci)* Remove stale mae-test-fixtures exclude from Dockerfile, wire collab_heartbeat_interval option ([59e681c](https://github.com/cuttlefisch/mae/commit/59e681c03f1a6e49b1721be1ed10a4c3f65a5053))
- *(ci)* Replace flaky verifier poll with docker compose wait ([ba8b12e](https://github.com/cuttlefisch/mae/commit/ba8b12e6885f046f5605ce46b342e871a574367b))
- *(ci)* Foreground docker-compose + 13 protocol E2E coverage tests ([6e47801](https://github.com/cuttlefisch/mae/commit/6e47801663609a9c0fc3793d3996dbb8e36f84ad))
- *(collab)* Awareness notification parse error + seq_tracker seeding + observability ([3ef1055](https://github.com/cuttlefisch/mae/commit/3ef1055de92ab2c91989fe0213eec7339dd8cb67))
- *(crdt)* Vim-style undo grouping for CRDT sync ([12f8ce4](https://github.com/cuttlefisch/mae/commit/12f8ce454a417dcd2993eda85ca5b4d269921089))

### Refactor

- *(core)* Extract DapContext sub-struct from Editor (2 fields) ([7ba8242](https://github.com/cuttlefisch/mae/commit/7ba82425334022550da49cc30dca2708a4b6a0d3))

### Documentation

- Update ROADMAP — editor struct at ~69 fields after 6 extractions ([3201b92](https://github.com/cuttlefisch/mae/commit/3201b92d1ce8ecb50979cbee09d0177c6b0e37dd))
- Mark collab bugs 2-4 + E8 complete, clarify Bug 1 status in ROADMAP ([1c16230](https://github.com/cuttlefisch/mae/commit/1c162308024bf761dd491796830c7d26714e08a4))
- Update SYNC_PROTOCOL known-limitations, refresh RoamNotes test infra ([ca5879f](https://github.com/cuttlefisch/mae/commit/ca5879fb9300ef8459379d4cffd2bc5451e36f43))

### Testing

- *(collab)* Two-client CRDT undo E2E test in Docker ([69c746b](https://github.com/cuttlefisch/mae/commit/69c746bd92de180f7d5894867da36fb260df152d))
- Collab hardening — 21 new tests, encode_diff API, v0.10.4 ([36dd0b7](https://github.com/cuttlefisch/mae/commit/36dd0b77731d71415b17a90dacc6fbf7bca17fc0))

### CI

- Re-enable collab Docker E2E tests ([1e8c6bc](https://github.com/cuttlefisch/mae/commit/1e8c6bc85ed081f0d44ccefe4174396150158553))
- Unify local/remote CI — include mae-gui in workspace, 15m collab timeout ([de157c9](https://github.com/cuttlefisch/mae/commit/de157c966d5baa9cbe330976d77fef6f6d1ec3e2))
- Disable Docker collab E2E (blocked on Phase 13 Scheme runtime) ([068309a](https://github.com/cuttlefisch/mae/commit/068309a07c7052c7f1bb4e7429f15948ebdce00e))

### Miscellaneous

- Bump version to 0.10.4 ([2bd047e](https://github.com/cuttlefisch/mae/commit/2bd047e0038e664b7d9b3bfee130403bc6c257c0))

### Roadmap

- Networked feature E2E coverage gate — no ship without tests ([167df56](https://github.com/cuttlefisch/mae/commit/167df5613120aa71f96f8b25dc634afb73eb8aee))
- Track cursor drift, modified flag, Docker E2E timeout bugs ([f6e17fa](https://github.com/cuttlefisch/mae/commit/f6e17fa68af0096e4953f1e63e95abe4255d32f7))

## [0.10.3] - 2026-05-20




### Miscellaneous

- Bump version to 0.10.3 ([5dc980d](https://github.com/cuttlefisch/mae/commit/5dc980d9c792636107adc13126dcb4a2eccdc063))

## [0.10.2] - 2026-05-20




### Features

- Collaborative editing — scalability, UX commands, AI tools, observability ([7e200e3](https://github.com/cuttlefisch/mae/commit/7e200e3f8d8c307e8bb603b909496a6ace753e50))
- Observability, KB docs, E2E tests for collaborative editing ([31058c2](https://github.com/cuttlefisch/mae/commit/31058c227537eabd9ab50ccc1ce04fbb9f1e9eed))
- KB CRDT integration — schema v7, Node↔KbNodeDoc bridge ([34dc95a](https://github.com/cuttlefisch/mae/commit/34dc95a92d5efb22cb69f4683ea365762b3f35f1))
- Add `make install-upgrade` + help/KB terminology audit ([dd32984](https://github.com/cuttlefisch/mae/commit/dd32984821c8d130e8e95ed3ec1376a01d06e836))
- Collab correctness + save protocol + org rendering fixes ([9fc93e7](https://github.com/cuttlefisch/mae/commit/9fc93e7abfe38b5cc2f1e95380698c6eba748d2c))
- 3-tier collab E2E test suite + 4 bug fixes + MCP shim framing ([c85dbd3](https://github.com/cuttlefisch/mae/commit/c85dbd346bdb62ff6a7299ee35f39152e050a56b))
- CRDT test primitives + editor tests + testing framework docs ([8db90c9](https://github.com/cuttlefisch/mae/commit/8db90c90f0a2f9b55f6c4ca46fd89f65f3813ea5))
- Scheme test library v2 — 310 tests, CI integration, CRDT lifecycle, user story E2E ([242d45f](https://github.com/cuttlefisch/mae/commit/242d45f0c35a7f9ee977d3e2decc6685af3b8849))
- CRDT robustness hardening — ADR-008, runtime limits, CI fixes, 3,629 tests ([3e51263](https://github.com/cuttlefisch/mae/commit/3e51263d33b86813c0889a1d0d6dda829fa07565))
- Join-save model, suffix matching, CI warning fixes — 3,639 tests ([782d54f](https://github.com/cuttlefisch/mae/commit/782d54fb524921dd5bd247ff983bf7ea10862cb1))
- KB search body matching + recency sorting (kb_search_sort option) ([cb37a20](https://github.com/cuttlefisch/mae/commit/cb37a2060645b3ee40961cf8add2d644fe352473))
- Save protocol wiring + disconnect lifecycle + stub audit — collab data model v2 ([ca6c202](https://github.com/cuttlefisch/mae/commit/ca6c202a87c890a8f32a44673d5ab9c630c9093c))
- Protocol resilience — gap detection, heartbeat, offline recovery, git identity ([b8d4b6a](https://github.com/cuttlefisch/mae/commit/b8d4b6aa5953dd593952703dbe07325bf7433d09))
- Benchmark suite + dispatch/ui.rs split — foundational testing + architecture ([0829dd5](https://github.com/cuttlefisch/mae/commit/0829dd5d7d18a1db1b1b4dcb6a2141dd87a9ce52))

### Bug Fixes

- CI state-server test (binary crate, no --lib) + regenerate code map ([597c91d](https://github.com/cuttlefisch/mae/commit/597c91da8bc36d76519a1ae263960d134b11d36d))
- :q closes window not app, C-c cancels not kills, keymap-doom auto-loads ([c3aa80b](https://github.com/cuttlefisch/mae/commit/c3aa80b0b5eba17824c3caeb5d97da49716a3782))
- Add missing untracked files (collab_bridge.rs, mae-connect.desktop) ([8d66046](https://github.com/cuttlefisch/mae/commit/8d66046f21a38079011ba3843cb14e8a98efec21))
- Share duplication, echo filtering, peer count + README refresh ([0d19003](https://github.com/cuttlefisch/mae/commit/0d19003a33d251622d469cdff4f98a6129500539))
- 4 MCP protocol bugs + architecture spec ([d0bf7f0](https://github.com/cuttlefisch/mae/commit/d0bf7f06e1fe9b7a131fa866b140905f57d9a9af))
- MCP shim stdio framing + protocol version negotiation ([d3aa424](https://github.com/cuttlefisch/mae/commit/d3aa424bde38ce543d3fb19614b94ed82707558a))
- Org-mode parity restoration + 46 regression guard tests ([12abab8](https://github.com/cuttlefisch/mae/commit/12abab8003895763bf3c8a411bc0168d17c669d0))
- File mode system — lang module auto-load + language detection + describe-mode parity ([4183c14](https://github.com/cuttlefisch/mae/commit/4183c14e242edb7537d4e7198d0bd47d4ab7289a))
- Scheme test framework — 5 gap fixes + execute-ex + Rust-side iteration ([13518ad](https://github.com/cuttlefisch/mae/commit/13518ad80776f572f520815678121cf67184c802))
- CI workflow YAML — restore newlines after exclude removal ([f44656b](https://github.com/cuttlefisch/mae/commit/f44656b06fd3a15b2ad49cc2d2c8aa7a8716d071))
- CI consolidation (17→13 jobs) + docker e2e /sync permissions ([1cbbf86](https://github.com/cuttlefisch/mae/commit/1cbbf860c3945ba670a497b91406c547fdb3e225))
- Collab test failures (get-option freshness, missing option arms) + tiered CI ([1b47fcd](https://github.com/cuttlefisch/mae/commit/1b47fcdc62f851ed671bac2367999ea965a028a5))
- Keybinding conflicts (kernel→module migration) + buffer-text freshness for collab E2E ([e9f7569](https://github.com/cuttlefisch/mae/commit/e9f75698abaaa65130c6000740a60f6e58d4bea9))
- Split collab E2E test steps for pending op ordering ([ab7bff5](https://github.com/cuttlefisch/mae/commit/ab7bff53dc3eec3fb6647f81273ba4fb23ec7bcc))
- *(gui)* Suppress field_reassign_with_default in cursor test ([39e8a0a](https://github.com/cuttlefisch/mae/commit/39e8a0a7513afa8518c78bac61118ca076d4879f))

### Refactor

- Extract CollabState + ShellIntents sub-structs from Editor (30 fields) ([2e17808](https://github.com/cuttlefisch/mae/commit/2e17808effa410a0df0968817706336161b0a222))
- Extract ViState + AiState sub-structs from Editor (75 fields) ([d344094](https://github.com/cuttlefisch/mae/commit/d3440949ff405bebebf739238339406c41f8e20c))
- *(core)* Standardize test variable names to `editor` ([7561af3](https://github.com/cuttlefisch/mae/commit/7561af3133e179dbafadfd292567575491348a5c))
- *(core)* Extract KbContext sub-struct from Editor (21 fields) ([19283ae](https://github.com/cuttlefisch/mae/commit/19283aee777ea0450e1fb50bdb0c4df21b00df34))

### Documentation

- Update ROADMAP — mark 7 completed items + document extraction roadmap ([e08a5f1](https://github.com/cuttlefisch/mae/commit/e08a5f13a6ffc2a1cdefc7ba0d6e6f371a642170))
- Update ROADMAP — editor struct at ~40 fields after 4 extractions ([649914f](https://github.com/cuttlefisch/mae/commit/649914fcd200e3bef1f5c2e2c1b60a169720dcde))
- Add naming conventions to CONTRIBUTING.md ([dd41a12](https://github.com/cuttlefisch/mae/commit/dd41a128f69d06a42d2d91b8ea7ac0c41ba3aa4e))

### Testing

- MCP protocol audit — 8 new tests + header guard + code-map precommit ([7867846](https://github.com/cuttlefisch/mae/commit/7867846bfed942ac47af74112996e4501dc82cbf))
- Collab E2E — save round-trip, heartbeat, reconnect re-share ([ec5c06e](https://github.com/cuttlefisch/mae/commit/ec5c06eaa2231805f405aae842c387ab21038c41))
- TCP E2E — offline reconnect resync + peer notifications ([a5ec70f](https://github.com/cuttlefisch/mae/commit/a5ec70fcdf8a717c26c2b4e139847d2b8ead1d44))

### CI

- Add GUI tests + clippy to CI, include in badge count ([bbebf1a](https://github.com/cuttlefisch/mae/commit/bbebf1aeb891ffcdc84112e38f6669c2f569d8f8))
- Disable docker collab E2E during struct-extraction refactor ([f09ef0a](https://github.com/cuttlefisch/mae/commit/f09ef0ab3856f3d547d5ac6e5b10bd27dd0b4716))
- *(deps)* Bump schneegans/dynamic-badges-action ([eab0672](https://github.com/cuttlefisch/mae/commit/eab067296d8a4a23d50895f1403920fe1ece5c79))

### Miscellaneous

- Backmerge main + update README screenshot ([9f67929](https://github.com/cuttlefisch/mae/commit/9f67929bfcfb0688b22ea569f8db89981b9d8952))
- Update CLAUDE.md, ADR-006, code map for collab features ([f21a68f](https://github.com/cuttlefisch/mae/commit/f21a68fe4d04e2e96c074eb1bd0726e5f0b45744))
- Regenerate code map after KB CRDT integration ([53f7d7c](https://github.com/cuttlefisch/mae/commit/53f7d7c41a85e1f8fecd35ae0f1352a08277f118))
- Regenerate code map after terminology audit ([85a3f89](https://github.com/cuttlefisch/mae/commit/85a3f892519d5591cbee2c44a757e77f893ccc01))
- *(deps)* Bump the rust-dependencies group with 12 updates ([fb4c506](https://github.com/cuttlefisch/mae/commit/fb4c5066868a912a28b3cf12198d5aee33eb5e79))
- Bump version to 0.10.2 ([8eee744](https://github.com/cuttlefisch/mae/commit/8eee74465af45405789de19b1d006cc31de17535))

## [0.10.1] - 2026-05-15




### Bug Fixes

- CI code-map job — check freshness instead of auto-push ([5e7686e](https://github.com/cuttlefisch/mae/commit/5e7686e066c002db0c9b5757e2b3614e8d579e5e))
- Run code-map freshness check on PRs too, not just main pushes ([1ea3dc7](https://github.com/cuttlefisch/mae/commit/1ea3dc762d736d068d8586ea096a01b1dcf16fc2))
- Remove dead code-map auto-push from release workflow ([9445297](https://github.com/cuttlefisch/mae/commit/9445297be8bdbc864c735331be9303f4c642ce64))

### Refactor

- Extract badges job to own workflow (main-push only) ([5521533](https://github.com/cuttlefisch/mae/commit/5521533562bda62965cab5b9fa35804c782c319f))

### Miscellaneous

- Bump version to 0.10.1 ([5a00884](https://github.com/cuttlefisch/mae/commit/5a0088469db75110fa09e120f36e53f19f73cb1c))

## [0.10.0] - 2026-05-15




### Features

- KB node creation UX — org-roam parity (SPC n c, SPC n i) ([7d4a0f3](https://github.com/cuttlefisch/mae/commit/7d4a0f37c242e78f0906b2eb5479f60659fd6e58))
- Replaceable window policy + buffer type audit (Doom real-buffer-p parity) ([eb751b9](https://github.com/cuttlefisch/mae/commit/eb751b909b86f259f1ca7b5264595c83194e9dfc))
- Full property drawer parsing + schema v5 (Part 1) ([9f265cd](https://github.com/cuttlefisch/mae/commit/9f265cd63843bd1cd63c8204ad9eaa9e9fa26343))
- Write-through safety — kb_write_guard anti-cascade (Part 2) ([2b7a0e5](https://github.com/cuttlefisch/mae/commit/2b7a0e5f72c3a13413a2486ccfc438d3d0d73a46))
- Activity tracking + activity-sorted search (Part 3) ([b7da1be](https://github.com/cuttlefisch/mae/commit/b7da1be944a57e7f1014efd3c61d33266f8cd8be))
- Org-dailies core — chain-fill, navigation, audit report (Part 4) ([40cd8d2](https://github.com/cuttlefisch/mae/commit/40cd8d22f0733af79a34aca0e41ebf3382338f33))
- Dailies module — SPC n d keybindings + concept:dailies help node (Part 5) ([e23bd5a](https://github.com/cuttlefisch/mae/commit/e23bd5a26801a2db3021c3031865b37fae5e3cd7))
- KB integrity pipeline — stale detection, link validation, orphan cleanup, metrics (Part 6) ([649790b](https://github.com/cuttlefisch/mae/commit/649790b979b94e8898c251c96a87866e0f90d72d))
- Keymap flavor infrastructure + keybinding reference docs ([7064565](https://github.com/cuttlefisch/mae/commit/70645652636e018fa5d7b18098a28207f23a7a35))
- Which-key scrolling, height config, sort order, group labels ([ca9170d](https://github.com/cuttlefisch/mae/commit/ca9170d15adde565246af1a919f081440dd08d55))
- Server-client M1 — multi-client MCP, file safety, KB WAL, ADRs ([6d77970](https://github.com/cuttlefisch/mae/commit/6d77970315c6d39ed04590dc09a0e98e0626a98b))
- Mae-sync crate — yrs CRDT text bridge + KB node schema (20 tests) ([a089796](https://github.com/cuttlefisch/mae/commit/a08979676db339ae5d554f59eb2d8ac88115822b))
- Phase B — wire TextSync into Buffer for collaborative edits ([5f09553](https://github.com/cuttlefisch/mae/commit/5f09553f71a88e31abd4fd4150ad6eb1c58fb73c))
- Phase C — MCP sync method handlers (pull-based) ([70c7811](https://github.com/cuttlefisch/mae/commit/70c7811cea80b3d9217528be5f1698e6d7b58d2b))
- Phase D — push-based sync event broadcasting (11 tests) ([0bab9d2](https://github.com/cuttlefisch/mae/commit/0bab9d22081891845587ee8aef7329a1692badb6))
- Generalize MCP transport for TCP + pub API for state-server ([32a4bf8](https://github.com/cuttlefisch/mae/commit/32a4bf8cae081d26152a5deb2580f013017d875b))
- Mae-state-server — collaborative state server with WAL persistence ([56fbc71](https://github.com/cuttlefisch/mae/commit/56fbc7185b817c2bbf3d09b531f49e721634a020))

### Bug Fixes

- Kernel dailies bindings + set-group-name Scheme API + introspect version ([d812323](https://github.com/cuttlefisch/mae/commit/d812323cc9ad6f643d4aaca10a3bdce412b24a41))
- Audit fixes for sync/MCP push architecture ([9c7fd7e](https://github.com/cuttlefisch/mae/commit/9c7fd7e8224c19712a6564d0f9f125c732ac2bdc))

### Testing

- Multi-client MCP integration tests ([b65e6c8](https://github.com/cuttlefisch/mae/commit/b65e6c860e19cf8dcb6735a4096e2df9196f7079))
- Fill M1 hardening coverage gaps (9 new tests) ([f81c6bc](https://github.com/cuttlefisch/mae/commit/f81c6bc769f613fe412a6515dcc3f76096534dd4))

### Miscellaneous

- Regenerate code map (new activity + properties APIs) ([27e1697](https://github.com/cuttlefisch/mae/commit/27e1697c3a5039f165ef28c625f856f36f41b271))
- Regenerate code map (dailies + keymap_query APIs) ([ad1eac1](https://github.com/cuttlefisch/mae/commit/ad1eac109b93151f3c0b3ea126d5845da6a7b1dc))
- Regenerate code map (KB integrity + keymap flavor APIs) ([0bd289e](https://github.com/cuttlefisch/mae/commit/0bd289ea3fee20d2da2e0a53179d67067053ef3b))
- Bump version to 0.10.0 ([2d94d0e](https://github.com/cuttlefisch/mae/commit/2d94d0e12d22bcf111e9496b5e3222fc2d770e35))

## [0.9.0] - 2026-05-14




### Features

- Memory synthesis, network status, verifier agent, org↔markdown conversion, splash image sizing ([7266a37](https://github.com/cuttlefisch/mae/commit/7266a372ba6602efefaa7a988e0304874efb8553))
- MCP client, tool search, model exam, verifier agent + fix shell buffer corruption ([8ac8803](https://github.com/cuttlefisch/mae/commit/8ac8803d1ae356401f51ea6618f072cad065f7cb))
- Model exam persistence, docs refresh, CI fix ([496ef35](https://github.com/cuttlefisch/mae/commit/496ef35771248b46f79b4c79b24053bc37f0332b))
- Unified test system (sandbox+grading) + LSP readiness probe ([bc201e9](https://github.com/cuttlefisch/mae/commit/bc201e9abe70b7ce337b03cec4c67857e9f2defe))

### Bug Fixes

- Resolve splash-art image paths relative to module dir ([71f2fe1](https://github.com/cuttlefisch/mae/commit/71f2fe1d597a40ade3e1d385a9e08bc1ca7f3c0f))
- Splash screen polish — centering, unicode width, mode guard ([87f10fb](https://github.com/cuttlefisch/mae/commit/87f10fb67084299e03a78b425077075123a0e7ae))
- Add missing splash options + image_natural_size method ([f11d48f](https://github.com/cuttlefisch/mae/commit/f11d48fa9d0a5c0a019e33af7a46db17ab49abbd))
- Tolerate missing `id` in exam tool_calls + first exam result ([cbb668e](https://github.com/cuttlefisch/mae/commit/cbb668eba7f05c8ea47fc552180d174be7ce63d3))
- Shell exit orphan windows, buffer readiness mode sync, dead hooks ([5757b86](https://github.com/cuttlefisch/mae/commit/5757b865566fa41d55786c2aabb859d0005f82a6))
- Agent shell steals conversation window + self-test oscillation abort ([8a52851](https://github.com/cuttlefisch/mae/commit/8a528517a9d8010404c2217d125eb23ca0f9aaab))
- Self-test reliability — sandbox confinement, LSP readiness, shell lifecycle ([0fa4654](https://github.com/cuttlefisch/mae/commit/0fa465496d9768db9fa542d24e6fc44cd03ec079))
- Anchor-first project detection + persistent project list ([12fa240](https://github.com/cuttlefisch/mae/commit/12fa240cea1dc5697b74daa345cd574006ddf06e))
- Safe project pruning + interactive project-forget (SPC p D) ([ba34aef](https://github.com/cuttlefisch/mae/commit/ba34aefc472f3484c113b395e92de5ec35d6e13d))

### Miscellaneous

- Update Cargo.lock after main backmerge ([d9c8279](https://github.com/cuttlefisch/mae/commit/d9c827951e6eb42413570cfdb179c099c8d6cf5d))
- Regenerate code map after project detection changes ([057a07c](https://github.com/cuttlefisch/mae/commit/057a07cb77bd2a50724ba0ba7720282451ff51ba))
- Pre-release polish — docs, fragility markers, module READMEs ([511da23](https://github.com/cuttlefisch/mae/commit/511da23e3718537c17d3d5d5640b749dc3a06cf7))
- Bump version to 0.9.0 ([048337f](https://github.com/cuttlefisch/mae/commit/048337f6d76cae70ef9558d64ad30dbd25b63c00))

## [0.8.3] - 2026-05-13




### Miscellaneous

- Bump version to 0.8.3 ([94844fa](https://github.com/cuttlefisch/mae/commit/94844fa4f4965dda64e0426cb41da97032e5c503))

## [0.8.2] - 2026-05-13




### Features

- Module system foundation (M1-M3) — manifest, resolver, loader, CLI, 3 extractions ([16c2c74](https://github.com/cuttlefisch/mae/commit/16c2c740d25990a9219c352cdb194d4711b1de2b))
- Extract 5 Tier 1 modules — search, registers, macros, tables, multicursor ([7910936](https://github.com/cuttlefisch/mae/commit/79109367d3bf4a623c1fc85f59756a55df7101b1))
- Add `mae pkg info` and `mae pkg create` CLI subcommands ([102b178](https://github.com/cuttlefisch/mae/commit/102b1789851a6193a314a50325ef19f3818a24f5))
- Extract file-tree module (Tier 2) — first module-owned keymap ([71248ef](https://github.com/cuttlefisch/mae/commit/71248ef6650114eee471acfe48edc01fdeaca75e))
- M4 — describe-mode command, deprecation mechanism ([db0824e](https://github.com/cuttlefisch/mae/commit/db0824efcd0857e2b22a8acaeeddf9525fd2d1dc))
- Declarative package management, module KB nodes, CI badges (M1-M5) ([5d707ef](https://github.com/cuttlefisch/mae/commit/5d707ef183657ec05d957565f05e8b60ee8298fd))
- KB federation lifecycle — live watching, edit-source, RAG, keybindings (W1-W6) ([88978d4](https://github.com/cuttlefisch/mae/commit/88978d41b44b983d3603915bed9f81e0b12e2fa6))
- KB UX improvements — Obsidian/org-roam reading parity ([e7acbe4](https://github.com/cuttlefisch/mae/commit/e7acbe46ebf9474a94c63694c310f760a0118d8a))
- Scheme API gaps, advice system, module extraction, reliability (A-F) ([663bb79](https://github.com/cuttlefisch/mae/commit/663bb79642fe1a1387de67c20ee7b95a973abcd4))
- Babel crate extraction, persistent sessions, language backends, edit-special (G1-G4d) ([a757ad2](https://github.com/cuttlefisch/mae/commit/a757ad24389cc167d94c0a23970cd05b5b63f150))
- Doom Parity Tier 1 — snippets, format, make, lookup, spell crates + mod.rs split (H1-H6) ([45dcf01](https://github.com/cuttlefisch/mae/commit/45dcf019a610293fa671a25fc99565d810976c60))
- Tool validation, ai-status!, register-ai-tool! (I1-I3) ([5efadd3](https://github.com/cuttlefisch/mae/commit/5efadd36c0eb3048d95205311dd0b11e01b89543))
- Model table expansion, source metadata, module template, planner-compact (I5-I8) ([a732612](https://github.com/cuttlefisch/mae/commit/a7326122210b2848ef8430b21706603807492379))
- God-file splits + module system hardening (J1-J5) ([2c2c0f4](https://github.com/cuttlefisch/mae/commit/2c2c0f400cfe843b94ec9302d5253403a7014b79))
- Custom splash art, local package source, e2e CI (K1-K4) ([5ed1b1e](https://github.com/cuttlefisch/mae/commit/5ed1b1ef0990fec75c6453793879b22a3d85d573))

### Bug Fixes

- Shell-select buffer exit path + KB fuzzy search, window groups, AI tools ([ce2471e](https://github.com/cuttlefisch/mae/commit/ce2471e3e863358036a796a856c86eb5870fe7f7))

### Performance

- Streaming save, async git diff, autosave cadence (I4) ([8f0c8d0](https://github.com/cuttlefisch/mae/commit/8f0c8d0cc0e9373ff49706692e150d7fd6903dcd))

### Documentation

- Add module system KB nodes and extension authoring guide ([81008d3](https://github.com/cuttlefisch/mae/commit/81008d335efc52553b636c79e01340385bc37bd5))

### CI

- *(deps)* Bump the ci-dependencies group with 2 updates ([418ac68](https://github.com/cuttlefisch/mae/commit/418ac6846112f52afa0bc7775c74cb69687e8b6c))

### Miscellaneous

- Track .claude/commands/ skills in git, keep settings.local.json ignored ([389a72d](https://github.com/cuttlefisch/mae/commit/389a72d8e714a698b0db9be0068f878c667f3008))
- Bump MSRV to 1.95, Dockerfile to rust:1.95 ([147a930](https://github.com/cuttlefisch/mae/commit/147a930a11654502bb2d57213355c6def640a6d8))
- *(deps)* Bump the rust-dependencies group with 14 updates ([0bae408](https://github.com/cuttlefisch/mae/commit/0bae408d92ea90c07580e34e535006986df9e1c6))
- Bump MSRV to 1.95 (sysinfo 0.39.1 requires it) ([780d642](https://github.com/cuttlefisch/mae/commit/780d642b62aad22f7c2f3b84895f00c0b3e26771))
- Bump version to 0.8.2 ([6472c4d](https://github.com/cuttlefisch/mae/commit/6472c4d6f4e2ffd209c1280ae3882eab42fa00bb))

## [0.8.1] - 2026-05-11




### CI

- Drop macOS x86_64 from release matrix, add version bump guidance ([ca17004](https://github.com/cuttlefisch/mae/commit/ca170041098fb602d578043924efdc3dc7c4a799))

### Miscellaneous

- Bump version to 0.8.1 ([2a41bd5](https://github.com/cuttlefisch/mae/commit/2a41bd5a187e8fca15214001f51890d95b00c8c4))

## [0.8.0] - 2026-05-11




### Bug Fixes

- Nightly clippy unnecessary_sort_by in perf.rs ([1298cc9](https://github.com/cuttlefisch/mae/commit/1298cc9a1e5d2496496147a921fa4b75acb785f4))
- Make versioned file parsing idempotent and forward-compatible ([bb201e6](https://github.com/cuttlefisch/mae/commit/bb201e6e9660b8a90e458afe23c28bd2281f1797))

### Documentation

- FOSS project readiness + onboarding polish ([6a8d87b](https://github.com/cuttlefisch/mae/commit/6a8d87bf36068800f6ebedd488a38afb392c542a))
- Remove manual test plan from tracked files ([578cd4b](https://github.com/cuttlefisch/mae/commit/578cd4bbcfb7bc8910cfc05c4e905bba587d6f77))
- Accuracy audit + release pipeline fixes ([2d11cf4](https://github.com/cuttlefisch/mae/commit/2d11cf40230dd121fb61cd3287805809ce198047))
- Pre-merge audit — kb_seed split, dispatch headers, security posture ([51e69c2](https://github.com/cuttlefisch/mae/commit/51e69c2e113d82edd078fbeaee11124bc7902b5f))

### CI

- Add GUI build job to validate release pipeline ([b6e3cb8](https://github.com/cuttlefisch/mae/commit/b6e3cb8c9879a5e91ddbc07f79c49b68ab1fe4ca))
- Add containerized development and release validation ([a478739](https://github.com/cuttlefisch/mae/commit/a47873918ee3fc0f5078bd4df706ee62514f7250))

### Miscellaneous

- Bump version to 0.8.0 ([b21fefe](https://github.com/cuttlefisch/mae/commit/b21fefebe2e5ecd19704fcf3a53f0378ea729f3d))

## [0.6.1] - 2026-05-06




### Features

- M1 scheme docs + progressive tutorial + bugfixes (2,260 tests) ([f752f85](https://github.com/cuttlefisch/mae/commit/f752f85a929397f7cea3debd9d6798e3d3de1d02))
- M2 contextual help — namespace fallback, Tab completion, which-key docs (2,264 tests) ([aad77d2](https://github.com/cuttlefisch/mae/commit/aad77d20bb385789ae812bd011d9d8daebf45ea2))
- M3 user help nodes — ~/.config/mae/help/*.org + :help-edit (2,265 tests) ([de65887](https://github.com/cuttlefisch/mae/commit/de65887614ccef292f72a5bce3a42e32ec33960c))
- Layered init.scm + after-load hook (2,269 tests) ([05da79f](https://github.com/cuttlefisch/mae/commit/05da79ffd58c32eb7e819da045a1e18d57f86837))
- Config health — --debug-init, :describe-configuration, audit_configuration MCP tool (2,276 tests) ([27ce130](https://github.com/cuttlefisch/mae/commit/27ce13081e5125a54c3dacdc8300481c3616af43))
- Display policy — enum-based buffer placement + conversation protection (2,286 tests) ([1a697a6](https://github.com/cuttlefisch/mae/commit/1a697a64563e8175cd4a7679b32d9e36c21f1b6c))
- Mouse focus — click-to-focus, scroll-under-mouse, idle deferred work (2,299 tests) ([d51b666](https://github.com/cuttlefisch/mae/commit/d51b666f21cbf01759d01af531c61159a17ec6c0))
- Rich content, multi-cursor, scroll fixes, TUI shift normalization (2,389 tests) ([2e0dfbe](https://github.com/cuttlefisch/mae/commit/2e0dfbe9b70eb8b933b281dd34bc2c36efa1ac17))
- Smooth sub-line scrolling past images + viewport clipping (2,389 tests) ([d589100](https://github.com/cuttlefisch/mae/commit/d58910049f56cfa4b43ff3e24990c82a7dac5158))
- Native SVG rendering via skia svg::Dom, remove resvg (2,389 tests) ([024d31e](https://github.com/cuttlefisch/mae/commit/024d31e54bb528db96a5ebca155ddc107dbfe21c))
- Org core interactivity, large doc perf, heading statistics cookies ([3db92c3](https://github.com/cuttlefisch/mae/commit/3db92c386f794dde39bcf340a75192007b30d566))
- Viewport-local syntax spans, configurable perf thresholds (2,464 tests) ([9853db2](https://github.com/cuttlefisch/mae/commit/9853db2c4c3728ec456cec0515ff910ee2c03a6a))
- Per-window render caching, redraw level fixes, frame profiling (2,475 tests) ([c975b37](https://github.com/cuttlefisch/mae/commit/c975b37129e2d93e018fe0862cbd666c0632c833))
- Inertial (kinetic) scrolling for GUI trackpad/mouse ([c82aced](https://github.com/cuttlefisch/mae/commit/c82aced5950cb1dac649fe6cb8b8d831879b2c5a))
- I3-style window movement (SPC w H/J/K/L) ([f4caf6e](https://github.com/cuttlefisch/mae/commit/f4caf6e3257aeea6cdfc36e007568e8ccae48ac5))
- AI target tool, undo-aware modified flag, popup split positioning ([1f5f482](https://github.com/cuttlefisch/mae/commit/1f5f4823cd779fc9042e3b3c1e3f48d9def1375a))
- LSP+DAP polish, agent shell CWD fix ([6cebd07](https://github.com/cuttlefisch/mae/commit/6cebd071ad797cdef415466599e07c40a7d2288a))
- Babel/export/federation + AI agent target dispatch ([984709a](https://github.com/cuttlefisch/mae/commit/984709af78a579d996c4db5b47028f3e394c45df))
- --clean/-q flag + dashboard screenshot for README ([3d677a8](https://github.com/cuttlefisch/mae/commit/3d677a8241f14610efc635088d2f4aec34781c88))
- PR polish — onboarding, config accuracy, AI-unconfigured UX ([c75dc32](https://github.com/cuttlefisch/mae/commit/c75dc3292ceac19d646c207c689e7f15a4361180))
- CI gap closure + macOS release binaries ([11fa418](https://github.com/cuttlefisch/mae/commit/11fa4182af889835908f2dd3c03991d50ed6c975))

### Bug Fixes

- AI chat viewport scrolls past response — use real output window height ([c391b21](https://github.com/cuttlefisch/mae/commit/c391b2109359501d714419cc1427f87fe18fed3c))
- Shell UX — auto-scroll on input, C-y paste, bracketed paste ([eae0272](https://github.com/cuttlefisch/mae/commit/eae0272a19bd7d4a8b925a9fd945100136f3f458))
- Per-window inertia, shell scroll, terminal normal-mode keys, viewport height ([5720480](https://github.com/cuttlefisch/mae/commit/57204807cac9f71ba943b45cc330178b9dd3f59b))
- Nightly clippy iter_kv_map in kb todo_nodes() ([a90a0b8](https://github.com/cuttlefisch/mae/commit/a90a0b8a7c31cc22af104a1d284e1e576f83fe54))

### Performance

- Eliminate rope.line() bottleneck in compute_layout() ([58a09fa](https://github.com/cuttlefisch/mae/commit/58a09faf4de2617b5ca76e671574bd336194ae7a))

### Documentation

- V0.7.0 version bump + README rewrite for technical reviewers ([585c323](https://github.com/cuttlefisch/mae/commit/585c32348070352f1d68f29f8115c13cc846738b))

### Miscellaneous

- *(deps)* Update skia-safe requirement in the rust-dependencies group ([26fb773](https://github.com/cuttlefisch/mae/commit/26fb7737d3ae8c327ae791d881cb6339fc14d685))
- Bump version to 0.6.1 ([190ecd1](https://github.com/cuttlefisch/mae/commit/190ecd1fb2d352fdd724ccfa37c7a1a4e7ad29a5))

## [0.6.0] - 2026-05-02




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

### Documentation

- *(v0.6.0)* KB nodes for org/markdown structural editing, ROADMAP update ([bcdcdc3](https://github.com/cuttlefisch/mae/commit/bcdcdc398438dd7231130e85c43e397e220a6512))
- Update ROADMAP + KB nodes for v0.6.0 Round 3 features ([b165f2c](https://github.com/cuttlefisch/mae/commit/b165f2c757a961e993facfd5a1ce0fba1438148b))
- Update README badges and feature list for v0.6.0 ([55a9077](https://github.com/cuttlefisch/mae/commit/55a9077eb71fe7a914798f7e5ece10f5f24e01aa))

### Testing

- *(v0.6.0)* AI guidance self-test category (keybindings, windows, themes) ([3e22ad9](https://github.com/cuttlefisch/mae/commit/3e22ad948d875b164d32e549adc0a0ab90fb8d43))
- *(v0.6.0)* Regression tests for keymap fallback, change markers, link rendering ([5048a0a](https://github.com/cuttlefisch/mae/commit/5048a0a5abfcdcea0c941a31e88c029b18da8e41))

### Miscellaneous

- Bump version to 0.6.0 ([1dd80fa](https://github.com/cuttlefisch/mae/commit/1dd80fad425632c8ffab099b20707374e9f49f4a))

## [0.5.1] - 2026-04-28




### Features

- *(v0.5.1)* Ghost cursor fix, status bar overhaul, vim parity, AI help ([479e5fd](https://github.com/cuttlefisch/mae/commit/479e5fd62e5c814f59e6e3a25dd86a17444ee5bd))
- *(gui)* Org heading tiered scaling, cursor/cursorline fixes, roadmap additions ([7a6807e](https://github.com/cuttlefisch/mae/commit/7a6807e1c59c73d933af617bcc1754fe8a373df4))
- *(gui)* Pixel-based variable-height line rendering ([69801c3](https://github.com/cuttlefisch/mae/commit/69801c3221f005f64068971623d648fa6e55b69c))

### Bug Fixes

- *(ci)* Version-bump workflow — target root Cargo.toml, handle tag conflicts ([f612752](https://github.com/cuttlefisch/mae/commit/f6127520bedf1fd33f107ed7850721bdeba83cf3))
- *(v0.5.1)* Hardening, config error surfacing, docs update (1,673 tests) ([a94893d](https://github.com/cuttlefisch/mae/commit/a94893da69c1a7d6119882de56136422139812a7))
- *(v0.5.1)* Block visual I, undo grouping, search perf, range :s, :set completion ([71dcee3](https://github.com/cuttlefisch/mae/commit/71dcee331c851a1309721a1533865d50f64b6c13))
- *(v0.5.1)* Substitute undo grouping, search highlight drift, command cursor ([a358fd4](https://github.com/cuttlefisch/mae/commit/a358fd4157e70b4d68d99a005bb8b9e0ce1eb0a7))
- *(v0.5.1)* Debounced syntax reparse, HighlightConfig cache, deduplicated render path ([7c7a68e](https://github.com/cuttlefisch/mae/commit/7c7a68e5f72f1847e1620495ee523f18cde95012))
- *(v0.5.1)* Cached lazy theme resolution, scaled heading overflow, roadmap updates ([cda8475](https://github.com/cuttlefisch/mae/commit/cda847541f674264b97c423014804f1914600ffe))
- *(ci)* Use RELEASE_PAT for version bump workflow ([5835548](https://github.com/cuttlefisch/mae/commit/5835548e66693a91c2d2b940dc1705e32d5c76d2))

### Miscellaneous

- Bump version to 0.5.1 ([585a7a0](https://github.com/cuttlefisch/mae/commit/585a7a0f1f0a60377d7a0d715ec7df0660286bea))

## [0.5.0] - 2026-04-26




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

### Documentation

- Update ROADMAP, README, CLAUDE.md for v0.4.1 modularization ([6e815e1](https://github.com/cuttlefisch/mae/commit/6e815e17ccc900b3e162c3b5bfc1bebe2ec33d45))
- Update GEMINI.md with v0.5.0 test count and missing crates ([a704de4](https://github.com/cuttlefisch/mae/commit/a704de4ddbebf6c72dd44fe39232261a785dfafd))
- *(kb)* Fix DAP tool names, add tool architecture + missing self-test categories ([8a5d7df](https://github.com/cuttlefisch/mae/commit/8a5d7dfd6502f666f813a560afcf2e10fdb5d296))
- *(roadmap)* Update v0.5.0 items, mark completed v0.6.0, fix tool counts ([b8790f9](https://github.com/cuttlefisch/mae/commit/b8790f912ac65e936a57dc0fbf8e5079240d662e))
- Expand Getting Started with prerequisites + AI setup, add CONTRIBUTING.md ([3f0325e](https://github.com/cuttlefisch/mae/commit/3f0325e79972e52e9004e537d1209dc2605fe13c))
- LSP self-test retry guidance, dev dependencies in CLAUDE.md ([6f1b915](https://github.com/cuttlefisch/mae/commit/6f1b915a4f974d2840568d7bf2e8afeb843b75c0))
- Update test counts (1,641), LOC badge (~82k), v0.5.0 summary ([dd9db98](https://github.com/cuttlefisch/mae/commit/dd9db98bd2e12833ff9873b8640c2f98f3edf4fa))
- Mark C-o insert mode as complete in ROADMAP ([1945763](https://github.com/cuttlefisch/mae/commit/1945763facb481714e8e74fd18a1704f42e3b5ef))

### Testing

- *(ai)* Add regression tests for mid-flight compaction, UI events, and log_activity ([029023e](https://github.com/cuttlefisch/mae/commit/029023e46574cbf59f2aa2a021917970bcad52b6))

### Build System

- Add setup-dev script + make target for dev dependency installation ([9fe969b](https://github.com/cuttlefisch/mae/commit/9fe969b7283531ca711fe3c05659107d484f66b2))

### Miscellaneous

- Bump version to v0.4.1, add .mae to gitignore ([a4b7795](https://github.com/cuttlefisch/mae/commit/a4b7795e7881b867695cba72a4a3a417d87511fa))

## [0.4.0] - 2026-04-21




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

### Miscellaneous

- *(deps)* Bump the rust-dependencies group with 10 updates ([a7da3fd](https://github.com/cuttlefisch/mae/commit/a7da3fd11ea006a53dd2f1a3af082e23ec530cb0))

## [0.3.0] - 2026-04-20




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

### Performance

- Cap TUI render rate at 60fps with deferred frame timer ([48d8b5c](https://github.com/cuttlefisch/mae/commit/48d8b5c212a57cdb86f5d82642210224cb034c4e))

### Refactor

- Simplify splash to bat-only, art infra for future PR ([9d1b514](https://github.com/cuttlefisch/mae/commit/9d1b5144bc5750dfb0d800b1ee27205f54651d6e))
- Extract shared event loop helpers, simplify review, roadmap update ([1a794ca](https://github.com/cuttlefisch/mae/commit/1a794cab6e57381b3dfdcb7b292d28925374199d))

### Documentation

- Update roadmap with Phase 3g hardening, editor history lessons, and revised priorities ([5003224](https://github.com/cuttlefisch/mae/commit/5003224a1b65c38eb57b7c14badcbef2fe15fb9e))
- Add repo badges and update test count to 1,303 ([325f733](https://github.com/cuttlefisch/mae/commit/325f7338214f206010b9346a9674b2f72d1f1b40))
- Update ROADMAP + CLAUDE.md — 1,509 tests, v0.3.0 status ([338072c](https://github.com/cuttlefisch/mae/commit/338072cc8f8871967c78026e8355aadc4fd58b35))
- Update ROADMAP — 5 GUI features were already implemented ([74c7da5](https://github.com/cuttlefisch/mae/commit/74c7da5fa248c61393b6e865078738ac88f8bd84))

### CI

- Add GitHub Actions CI, release workflow, README, and changelog config ([e8442bb](https://github.com/cuttlefisch/mae/commit/e8442bb70921be87c3e07e329bcfa79c22a7c70f))
- Add semantic version bumping on PR merge ([69cd7b8](https://github.com/cuttlefisch/mae/commit/69cd7b8d6996f250817bb2eae3803469cbaa5961))
- *(deps)* Bump the ci-dependencies group with 2 updates ([e907ab8](https://github.com/cuttlefisch/mae/commit/e907ab8e1341b318abff25f47167f19bbc6a9735))

### Styling

- Cargo fmt ([5c8f6fe](https://github.com/cuttlefisch/mae/commit/5c8f6fe98ca2db7c4d921ce65ce68aa94bb560fd))

### Miscellaneous

- Fix clippy warnings and apply cargo fmt ([a6db8ca](https://github.com/cuttlefisch/mae/commit/a6db8cadd9318c7d0c750b1d6790c19c81a2dbf3))
- Update ROADMAP — Phase 3f M3 complete (conversation persistence) ([df87f8f](https://github.com/cuttlefisch/mae/commit/df87f8f4b405033e6bbdb7063dba6570dca1340a))
- Update ROADMAP — macros done, all Tier 1 self-hosting blockers complete ([437ae19](https://github.com/cuttlefisch/mae/commit/437ae19917e49468399dd868fa8dcfe05124434c))
- Update CLAUDE.md — current phase status and next targets ([af75252](https://github.com/cuttlefisch/mae/commit/af7525288df33d22c8ec6c89aff63a848e85b193))
- *(deps)* Update toml requirement from 0.8 to 1.1 ([6270aac](https://github.com/cuttlefisch/mae/commit/6270aacfd76e4caaf5f0a71ff4bb2bd0a854199d))
- Group dependabot updates into batched PRs ([c1096c8](https://github.com/cuttlefisch/mae/commit/c1096c899f23166d6e7d9c378321ba58bdfb7b5e))

### Infra

- Add pre-commit hook for cargo fmt check ([6384406](https://github.com/cuttlefisch/mae/commit/63844067ebcd2ffa8cd7769faadd1cdfc01152e8))

### Tmp

- Add theme test file for diff testing ([8c603ae](https://github.com/cuttlefisch/mae/commit/8c603aeecb7e64d2f8bf6ec8a457f251d2730f72))


