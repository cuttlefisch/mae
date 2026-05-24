# MAE Keybinding Reference

> Generated from kernel defaults + doom flavor. Run `:describe-bindings` for live state.

## Keymap Flavors

MAE supports keybinding "flavors" — selectable base keymap sets controlled by the
`keymap_flavor` option (default: `"doom"`). Set via `:set keymap_flavor doom` or
`config.toml`:

```toml
[editor]
keymap_flavor = "doom"
```

Available flavors:
- **doom** (default) — SPC leader key, vi motions, operator-pending, Doom Emacs-style groups
- **vim-pure** — vi motions + operators, no SPC leader (use `:` commands instead) *(planned)*
- **emacs** — Non-modal, C-x prefix tree, M-x command palette *(planned)*
- **minimal** — Arrow-only navigation, function key toolbar *(planned)*

---

## 1. Kernel Bindings (always active)

These bindings are compiled into the Rust binary and active regardless of flavor.

### Normal Mode — Core

| Key | Command | Description |
|-----|---------|-------------|
| `Esc` | `enter-normal-mode` | Return to normal mode |
| `i` | `enter-insert-mode` | Enter insert mode |
| `a` | `enter-insert-mode-after` | Insert after cursor |
| `A` | `enter-insert-mode-eol` | Insert at end of line |
| `o` | `open-line-below` | Open line below |
| `O` | `open-line-above` | Open line above |
| `:` | `enter-command-mode` | Command line |
| `v` | `enter-visual-char` | Visual char mode |
| `V` | `enter-visual-line` | Visual line mode |
| `C-v` | `enter-visual-block` | Visual block mode |

### Normal Mode — Motions

| Key | Command |
|-----|---------|
| `h/j/k/l` | Move left/down/up/right |
| Arrow keys | Move left/down/up/right |
| `w/b/e` | Word forward/backward/end |
| `W/B/E` | WORD forward/backward/end |
| `0/$` | Line start/end |
| `^/_` | First non-blank |
| `gg/G` | File start/end |
| `{/}` | Paragraph backward/forward |
| `%` | Matching bracket |
| `f/F/t/T` | Find/till char forward/backward |
| `;/,` | Repeat find / reverse |
| `H/M/L` | Screen top/middle/bottom |
| `gj/gk` | Display line down/up |
| `g0/g$` | Display line start/end |

### Normal Mode — Editing

| Key | Command |
|-----|---------|
| `x` | Delete char forward |
| `X` | Delete char backward |
| `dd` | Delete line |
| `D` | Delete to line end |
| `d` | Operator: delete |
| `c` | Operator: change |
| `y` | Operator: yank |
| `di/da/ci/ca/yi/ya` | Text objects (inner/around) |
| `cc/C` | Change line / to end |
| `yy/Y` | Yank line |
| `p/P` | Paste after/before |
| `r` | Replace char |
| `s/S` | Substitute char/line |
| `J` | Join lines |
| `>>/<<` | Indent/dedent |
| `~` | Toggle case |
| `gUU/guu` | Uppercase/lowercase line |
| `u/C-r` | Undo/redo |
| `.` | Dot repeat |
| `ZZ/ZQ` | Save+quit / force quit |

### Normal Mode — Scroll

| Key | Command |
|-----|---------|
| `C-u/C-d` | Half page up/down |
| `C-f/C-b` | Full page down/up |
| `C-e/C-y` | Scroll line down/up |
| `zz/zt/zb` | Center/top/bottom |
| `za/zM/zR` | Toggle/close all/open all folds |

### Normal Mode — LSP

| Key | Command |
|-----|---------|
| `gd` | Go to definition |
| `gr` | Find references |
| `K` | Hover info |
| `]d/[d` | Next/prev diagnostic |

### Normal Mode — Misc

| Key | Command |
|-----|---------|
| `gf` | Go to file under cursor |
| `gx` | Open link at cursor |
| `gl` | Edit link at cursor |
| `gi` | Re-insert at last position |
| `gv` | Reselect visual |
| `C-g` | File info |
| `C-6` | Alternate file |
| `C-=/C--/C-0` | Font zoom in/out/reset |

### Normal Mode — Window (C-w prefix)

| Key | Command |
|-----|---------|
| `C-w v/s` | Split vertical/horizontal |
| `C-w q` | Close window |
| `C-w h/j/k/l` | Focus left/down/up/right |
| `C-w +/-/=` | Grow/shrink/balance |

### Capture Mode

| Key | Command |
|-----|---------|
| `C-c C-c` | Finalize capture |
| `C-c C-k` | Abort capture |

### Insert Mode

| Key | Command |
|-----|---------|
| `Esc` | Return to normal |
| Arrow keys | Movement |
| `Tab` | Accept LSP completion |
| `C-n/C-p` | Next/prev completion |

### Visual Mode

All normal motions plus:

| Key | Command |
|-----|---------|
| `d/x` | Delete selection |
| `y` | Yank selection |
| `c` | Change selection |
| `>/<` | Indent/dedent |
| `J` | Join lines |
| `p/P` | Paste over |
| `o` | Swap selection ends |
| `u/U` | Lower/uppercase |
| `I/A` | Block insert/append |
| `i/a` | Inner/around object |

### Shell Insert

| Key | Command |
|-----|---------|
| `C-\ C-n` | Exit to shell-normal |
| `C-y` | Paste |

---

## 2. Doom Flavor (SPC Leader Groups)

### `SPC SPC` — Command palette
### `SPC :` — Command mode

### `SPC b` — +buffer

| Key | Command |
|-----|---------|
| `SPC b s` | Save |
| `SPC b b` | Switch buffer |
| `SPC b d/k` | Kill buffer |
| `SPC b n/p` | Next/prev buffer |
| `SPC b l/a` | Alternate file |
| `SPC b m` | View messages |
| `SPC b N` | New buffer |
| `SPC b D` | Force kill |
| `SPC b i` | File info |
| `SPC b o` | Kill other buffers |
| `SPC b S` | Save all |
| `SPC b r` | Revert buffer |

### `SPC f` — +file

| Key | Command |
|-----|---------|
| `SPC f f` | Find file |
| `SPC f d` | File browser |
| `SPC f s` | Save |
| `SPC f r` | Recent files |
| `SPC f y` | Yank file path |
| `SPC f R` | Rename file |
| `SPC f n` | New buffer |
| `SPC f c` | Edit config |
| `SPC f C` | Copy file |
| `SPC f P` | Edit settings |
| `SPC f S` | Save as |
| `SPC f D` | Delete file |

### `SPC w` — +window

| Key | Command |
|-----|---------|
| `SPC w v/s` | Split vertical/horizontal |
| `SPC w q/d` | Close window |
| `SPC w h/j/k/l` | Focus |
| `SPC w H/J/K/L` | Move window |
| `SPC w +/-/=` | Grow/shrink/balance |
| `SPC w m` | Maximize |
| `SPC w w` | Focus next |

### `SPC a` — +ai

| Key | Command |
|-----|---------|
| `SPC a a` | Open AI agent |
| `SPC a p` | AI prompt |
| `SPC a c` | Cancel AI |
| `SPC a m` | Set AI mode |
| `SPC a P` | Set AI profile |
| `SPC a n` | Ping AI |
| `SPC a v` | Verify |

### `SPC h` — +help

| Key | Command |
|-----|---------|
| `SPC h h` | Help |
| `SPC h k` | Describe key |
| `SPC h c` | Describe command |
| `SPC h o` | Describe option |
| `SPC h t` | Tutor |
| `SPC h s` | Help search |
| `SPC h b/f` | Help back/forward |
| `SPC h q` | Help close |
| `SPC h l` | Help reopen |
| `SPC h d` | Dashboard |
| `SPC h B` | Describe bindings |
| `SPC h m` | Describe mode |
| `SPC h D` | Describe display policy |

### `SPC c` — +code (LSP)

| Key | Command |
|-----|---------|
| `SPC c d` | Go to definition |
| `SPC c r` | Find references |
| `SPC c k` | Hover |
| `SPC c x` | Show diagnostics |
| `SPC c a` | Code action |
| `SPC c R` | Rename |
| `SPC c f/F` | Format / range format |
| `SPC c s` | LSP status |
| `SPC c o` | Symbol outline |

### `SPC l` — +lsp

| Key | Command |
|-----|---------|
| `SPC l p` | Peek definition |
| `SPC l r` | Peek references |

### `SPC n` — +notes

| Key | Command |
|-----|---------|
| `SPC n f` | KB find |
| `SPC n v` | KB view |
| `SPC n e` | Edit source |
| `SPC n c` | KB create |
| `SPC n D` | KB delete |
| `SPC n r` | Register KB |
| `SPC n R` | Reimport KB |
| `SPC n i` | Insert link |
| `SPC n s` | Finalize capture |
| `SPC n k` | Abort capture |
| `SPC n C` | Cleanup orphans |
| `SPC n I` | KB instances |
| `SPC n h` | KB health |
| `SPC n d t` | Daily: today |
| `SPC n d y` | Daily: yesterday |
| `SPC n d d` | Daily: go to date |
| `SPC n d p/n` | Daily: prev/next |

### `SPC p` — +project

| Key | Command |
|-----|---------|
| `SPC p f` | Find file in project |
| `SPC p s` | Project search |
| `SPC p d` | Browse project |
| `SPC p r` | Recent files |
| `SPC p p` | Switch project |
| `SPC p a` | Add project |
| `SPC p D` | Forget project |
| `SPC p c` | Clean project |

### `SPC e` — +eval

| Key | Command |
|-----|---------|
| `SPC e l` | Eval line |
| `SPC e b` | Eval buffer |
| `SPC e o` | Scheme REPL |
| `SPC e s` | Send to shell |
| `SPC e r` | Eval region (visual) |
| `SPC e S` | Send region to shell (visual) |

### `SPC s` — +search/syntax

| Key | Command |
|-----|---------|
| `SPC s n` | Select syntax node |
| `SPC s e` | Expand selection |
| `SPC s c` | Contract selection |

### `SPC o` — +open

| Key | Command |
|-----|---------|
| `SPC o t` | Terminal |
| `SPC o T` | Terminal here |
| `SPC o r` | Terminal reset |
| `SPC o c` | Terminal close |

### `SPC t` — +toggle

| Key | Command |
|-----|---------|
| `SPC t t` | Cycle theme |
| `SPC t S` | Set theme |
| `SPC t l` | Line numbers |
| `SPC t r` | Relative line numbers |
| `SPC t w` | Word wrap |
| `SPC t i` | Inline images |
| `SPC t s` | Scrollbar |
| `SPC t F` | FPS overlay |
| `SPC t D` | Debug mode |
| `SPC t d` | LSP diagnostics inline |

### `SPC q` — +quit

| Key | Command |
|-----|---------|
| `SPC q q` | Quit |
| `SPC q Q` | Force quit |
| `SPC q s` | Save and quit |
| `SPC q S` | Save all and quit |

### `SPC x` — Scratch buffer

---

## 3. Module Overlay Bindings

These bindings are added by Scheme modules loaded at startup.

### Git Status (`modules/git-status/`)
`SPC g` prefix — git operations (stage, commit, push, diff, log, blame, etc.)

### Org Mode (`modules/org/`)
`SPC m` local leader — heading manipulation, export, TODO cycling

### Markdown (`modules/markdown/`)
`SPC m` local leader — heading manipulation, promote/demote

### Debug (`modules/debug/`)
`SPC d` prefix — breakpoints, step, continue, debug panel

### Agenda (`modules/agenda/`)
`SPC o a/A` — open agenda / demo agenda

### File Tree (`modules/file-tree/`)
`SPC f t` — toggle file tree; tree-specific keymap (j/k/Enter/q/etc.)

### Search (`modules/search/`)
`/`, `?`, `n`, `N`, `*`, `#`, `gn`, `gN` — incremental search

### Marks & Jumps (`modules/marks-jumps/`)
`m`, `'`, `C-o`, `C-i`, `g;`, `g,` — marks, jump list, change list

### Macros (`modules/macros/`)
`q`, `@` — record/replay macros

### Registers (`modules/registers/`)
`"` — register selection prefix

### Surround (`modules/surround/`)
`ys`, `cs`, `ds`, visual `S` — vim-surround operations

### Multicursor (`modules/multicursor/`)
`SPC m` prefix — add cursors, align, skip

### Dailies (`modules/dailies/`)
`SPC n d` prefix — daily journal notes

### Tables (`modules/tables/`)
Org/markdown table editing bindings
