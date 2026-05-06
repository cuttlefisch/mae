# Manual Test Plan: Fresh User Onboarding

Test the full clone-to-first-edit experience from a fresh Linux user account.
This validates build, install, config, and `mae doctor` behavior without
any existing `~/.config/mae/` or `~/.local/bin/mae`.

---

## 0. Create a Test User

```sh
sudo useradd -m -s /bin/bash maetest
sudo passwd maetest
# Install rustup as maetest (needed for build)
sudo -iu maetest bash -c 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y'
```

System deps (clang, fontconfig-devel, freetype-devel) are already installed
system-wide from your main account. If testing TUI-only, skip GUI deps.

---

## 1. Clone & Doctor (First Contact)

```sh
sudo -iu maetest
git clone git@github.com:cuttlefisch/mae.git ~/mae && cd ~/mae
make doctor
```

**Expected:**
- [ ] `make doctor` exits 0
- [ ] Reports `rustc`, `cargo` with green checkmarks
- [ ] Reports `clang`, `fontconfig`, `freetype` status (green if installed, yellow warning if not)
- [ ] Reports optional tools (lldb, rust-analyzer) — warnings are OK, not errors

---

## 2. Build

### 2a. TUI-only build (no clang/skia dependency)

```sh
make build-tui
```

**Expected:**
- [ ] Builds without errors
- [ ] `./target/release/mae --version` prints version

### 2b. GUI build (requires clang + fontconfig + freetype)

```sh
make build
```

**Expected:**
- [ ] Builds without errors (may take several minutes for skia)
- [ ] `./target/release/mae --version` prints version

---

## 3. Install

```sh
make install
```

**Expected:**
- [ ] Binary installed to `~/.local/bin/mae`
- [ ] `mae --version` works (assumes `~/.local/bin` is on PATH — if not, test with full path)
- [ ] Desktop launcher installed to `~/.local/share/applications/mae.desktop`
- [ ] SVG icon installed to `~/.local/share/icons/hicolor/scalable/apps/mae.svg`

---

## 4. `mae doctor` (Post-Install)

```sh
mae doctor
```

**Expected:**
- [ ] Prints colored output with section headers (Build Prerequisites, Configuration, AI Provider, LSP Servers, DAP Adapters)
- [ ] Build Prerequisites: all green (rustc, cargo found)
- [ ] Configuration: yellow warning for missing config.toml and init.scm (expected — fresh user)
- [ ] AI Provider: yellow warning (no API key set — expected)
- [ ] LSP/DAP: status matches what's actually installed
- [ ] Exit code 0 (warnings don't cause failure, only errors do)

---

## 5. Config Init

```sh
mae --init-config
```

**Expected:**
- [ ] Creates `~/.config/mae/config.toml` with commented template
- [ ] Creates `~/.config/mae/init.scm` with sample config
- [ ] Runs first-run wizard (interactive prompts for provider/model)
  - If `MAE_SKIP_WIZARD=1` is set, wizard is skipped
- [ ] Running `mae doctor` again shows green checkmarks for config.toml and init.scm

### 5a. Config Validation

```sh
mae --check-config
```

**Expected:**
- [ ] Prints `mae: config OK`
- [ ] Exit code 0

```sh
mae --check-config --report
```

**Expected:**
- [ ] Prints JSON health report to stdout
- [ ] Includes `ai`, `lsp`, `dap`, `options`, `display_policy` sections

### 5b. Intentionally Break Config

```sh
echo "invalid toml {{{{" >> ~/.config/mae/config.toml
mae --check-config
```

**Expected:**
- [ ] Prints error message
- [ ] Exit code 1
- [ ] `mae doctor` shows red cross for config.toml parse error

Restore:
```sh
mae --init-config --force
```

---

## 6. First Launch (No AI Key)

```sh
mae
```

**Expected:**
- [ ] Opens dashboard (splash screen) without errors
- [ ] Status bar shows version, mode (NORMAL), no LSP/AI status
- [ ] `:q` exits cleanly
- [ ] No crash, no panic

### 6a. Open a File

```sh
mae ~/.config/mae/config.toml
```

**Expected:**
- [ ] File opens with content visible
- [ ] Syntax highlighting works (TOML)
- [ ] `hjkl` navigation works
- [ ] `i` enters insert mode, `Esc` returns to normal
- [ ] `:w` saves, `:q` quits

### 6b. GUI Launch

```sh
mae --gui ~/.config/mae/config.toml
```

**Expected:**
- [ ] Window opens with Skia-rendered text
- [ ] Font rendering is correct (monospace, no tofu)
- [ ] Mouse click positions cursor
- [ ] `Ctrl-+` / `Ctrl--` zoom works
- [ ] Close window exits cleanly

---

## 7. AI Setup

Set an API key and test:

```sh
export ANTHROPIC_API_KEY=sk-ant-...   # or any provider
mae doctor
```

**Expected:**
- [ ] AI Provider section shows green checkmark for the set key
- [ ] Warning gone for that provider

### 7a. AI Conversation

```sh
mae
# Inside the editor:
# SPC a p  →  opens AI conversation
# Type a prompt, press Enter
```

**Expected:**
- [ ] AI responds (streaming output visible)
- [ ] Tool calls displayed inline
- [ ] `Esc` cancels in-flight request
- [ ] No crash on network error (if key is invalid, shows error in status bar)

### 7b. AI Self-Test (Optional — costs API tokens)

```sh
mae --self-test introspection
```

**Expected:**
- [ ] Runs introspection test category
- [ ] Prints pass/fail summary
- [ ] Exit code 0 on pass, 1 on fail

---

## 8. Help & Tutorial

```sh
mae
# Inside:
# :tutor        →  opens interactive tutorial
# :help         →  opens help index
# SPC h h       →  same as :help
# SPC SPC       →  command palette
```

**Expected:**
- [ ] `:tutor` opens tutorial buffer with 12 lessons
- [ ] Tab/Enter follows links between lessons
- [ ] `:help` opens help index, fuzzy search works
- [ ] `SPC SPC` opens command palette, typing filters

---

## 9. Contributing Docs Validation

These checks confirm the docs are accurate for a new contributor:

- [ ] `CONTRIBUTING.md` links to `docs/CODE_MAP.md` — file exists
- [ ] `CONTRIBUTING.md` links to `docs/terminology.md` — file exists
- [ ] `CONTRIBUTING.md` links to `docs/TOOL_ADDITION_CHECKLIST.md` — file exists
- [ ] `CONTRIBUTING.md` links to `ROADMAP.md#known-bugs` — anchor exists
- [ ] `README.md` "Known Bugs" link resolves to ROADMAP.md section
- [ ] `make ci` instructions in CONTRIBUTING.md match actual Makefile targets
- [ ] Commit message format in CONTRIBUTING.md matches git-cliff config

---

## 10. GitHub Templates Render Check

After push, verify on GitHub:

- [ ] New Issue → shows "Bug Report" form (not freeform)
- [ ] Bug Report form has: Description, Steps, Backend dropdown, OS dropdown, Doctor Output, Additional Context
- [ ] "Feature Request" link in issue picker routes correctly
- [ ] Blank issues are disabled
- [ ] New PR → template auto-populates with Summary/Test Plan/Checklist
- [ ] `SECURITY.md` shows in repo's Security tab

---

## 11. Cargo.lock Reproducibility

```sh
# In a fresh clone:
cargo build --release 2>&1 | head -5
```

**Expected:**
- [ ] No `Updating crates.io index` fetch needed (lockfile pins versions)
- [ ] Build uses exact versions from Cargo.lock
- [ ] `cargo build` is deterministic across machines

---

## 12. Cleanup

```sh
exit  # leave maetest session
sudo userdel -r maetest
```

---

## Quick Checklist (Summary)

| # | Test | Pass? |
|---|------|-------|
| 1 | `make doctor` runs from fresh clone | |
| 2 | `make build-tui` succeeds | |
| 3 | `make build` succeeds (GUI) | |
| 4 | `make install` installs binary + desktop file | |
| 5 | `mae doctor` reports correct status | |
| 6 | `mae --init-config` creates config + init.scm | |
| 7 | `mae --check-config` validates config | |
| 8 | `mae` launches dashboard, `:q` exits | |
| 9 | `mae --gui` launches GUI window | |
| 10 | AI conversation works with API key | |
| 11 | `:tutor` and `:help` open correctly | |
| 12 | GitHub templates render on push | |
| 13 | Cargo.lock provides reproducible builds | |
