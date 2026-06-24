pub(super) const KEY_NORMAL: &str = "** Normal-mode keys (summary)\n\n\
*** Movement\n\
- `h j k l` — left / down / up / right\n\
- `w` / `b` / `e` — next word / previous word / end of word (see [[cmd:move-word-forward]])\n\
- `0` / `$` — start / end of line\n\
- `gg` / `G` — first / last line\n\
- `f<char>` — find char on line\n\n\
*** Operators (compose with any motion)\n\
- `d{motion}` — delete (e.g. `dw`, `dG`, `dgg`, `d%`, `d}`)\n\
- `c{motion}` — change (delete + enter insert)\n\
- `y{motion}` — yank (copy)\n\
- `dd` / `cc` / `yy` — linewise specials\n\
- `di(` / `ca\"` / `yi{` — text objects\n\n\
*** Editing\n\
- `i` / `a` — enter insert mode (before / after cursor) ([[cmd:enter-insert-mode]])\n\
- `o` / `O` — open line below / above ([[cmd:open-line-below]])\n\
- `u` / `C-r` — undo / redo ([[cmd:undo]], [[cmd:redo]])\n\n\
*** Leader keys (SPC)\n\
See [[key:leader-keys]] for the full SPC leader reference.\n\n\
*** Windows, buffers, files\n\
- `:e <path>` — open file\n\
- `:ls` — list buffers ([[cmd:switch-buffer]])\n\
- `C-^` — switch to alternate buffer\n\n\
*** Help\n\
- `:help` — open this page\n\
- `:describe-command <name>` — show docs for any command\n\n\
See also: [[index]], [[concept:mode]]\n";

pub(super) const KEY_LEADER: &str = "** SPC Leader Bindings (Doom Emacs style)\n\n\
These bindings live in the shared `leader` keymap (the single source of truth for the\n\
which-key menu) and appear in every keymap flavor. The default `doom` flavor opens the\n\
keypad with `SPC` in normal mode; the `nonmodal` (CUA) flavor opens it with `C-;` in\n\
insert mode. Switch flavors live with `:keymap-set-flavor <name>`.\n\
Press the leader key to see the which-key popup showing available sub-keys.\n\n\
*** SPC SPC — Command Palette\n\
Fuzzy-search all commands (like Doom's `M-x` or VSCode's `Ctrl-Shift-P`).\n\n\
*** SPC / — Project Search\n\
Quick shortcut for `project-search` (ripgrep in project root).\n\n\
*** SPC b — +buffer\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `s` | [[cmd:save]] | Save current buffer |\n\
| `b` | [[cmd:switch-buffer]] | Switch buffer (fuzzy) |\n\
| `d` | [[cmd:kill-buffer]] | Kill buffer |\n\
| `n` | [[cmd:next-buffer]] | Next buffer |\n\
| `p` | [[cmd:prev-buffer]] | Previous buffer |\n\
| `l` | [[cmd:alternate-file]] | Alternate file |\n\
| `m` | [[cmd:view-messages]] | Messages buffer |\n\
| `N` | [[cmd:new-buffer]] | New buffer |\n\
| `D` | [[cmd:force-kill-buffer]] | Force kill |\n\
| `o` | [[cmd:kill-other-buffers]] | Kill other buffers |\n\
| `S` | [[cmd:save-all-buffers]] | Save all |\n\
| `r` | [[cmd:revert-buffer]] | Revert from disk |\n\n\
*** SPC f — +file\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `f` | [[cmd:find-file]] | Open file picker |\n\
| `d` | [[cmd:file-browser]] | Directory browser |\n\
| `s` | [[cmd:save]] | Save |\n\
| `r` | [[cmd:recent-files]] | Recent files |\n\
| `y` | [[cmd:yank-file-path]] | Yank file path |\n\
| `R` | [[cmd:rename-file]] | Rename file |\n\
| `S` | [[cmd:save-as]] | Save as |\n\
| `c` | [[cmd:edit-config]] | Edit config |\n\n\
*** SPC p — +project\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `f` | [[cmd:project-find-file]] | Find file in project |\n\
| `s` | [[cmd:project-search]] | Grep in project |\n\
| `d` | [[cmd:project-browse]] | Browse project dir |\n\
| `r` | [[cmd:project-recent-files]] | Recent project files |\n\n\
*** SPC w — +window\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `v` | [[cmd:split-vertical]] | Vertical split |\n\
| `s` | [[cmd:split-horizontal]] | Horizontal split |\n\
| `q` | [[cmd:close-window]] | Close window |\n\
| `h/j/k/l` | focus-{dir} | Move focus |\n\n\
*** SPC s — +search/syntax\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `s` | [[cmd:search-buffer]] | Search in buffer |\n\
| `n` | [[cmd:syntax-select-node]] | Select syntax node |\n\
| `e` | [[cmd:syntax-expand-selection]] | Expand selection |\n\
| `c` | [[cmd:syntax-contract-selection]] | Contract selection |\n\
| `p` | [[cmd:project-search]] | Project search |\n\
| `h` | [[cmd:clear-search-highlight]] | Clear highlights |\n\n\
*** SPC c — +code\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `d` | [[cmd:lsp-goto-definition]] | Go to definition |\n\
| `r` | [[cmd:lsp-find-references]] | Find references |\n\
| `k` | [[cmd:lsp-hover]] | Hover info |\n\
| `x` | [[cmd:lsp-show-diagnostics]] | Diagnostics |\n\
| `a` | [[cmd:lsp-code-action]] | Code action |\n\
| `R` | [[cmd:lsp-rename]] | Rename symbol |\n\
| `f` | [[cmd:lsp-format]] | Format |\n\n\
*** SPC g — +git\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `s` | [[cmd:git-status]] | Git status |\n\
| `b` | [[cmd:git-blame]] | Git blame |\n\
| `d` | [[cmd:git-diff]] | Git diff |\n\
| `l` | [[cmd:git-log]] | Git log |\n\n\
*** SPC t — +toggle\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `t` | [[cmd:cycle-theme]] | Cycle theme |\n\
| `s` | [[cmd:set-theme]] | Set theme |\n\
| `l` | [[cmd:toggle-line-numbers]] | Line numbers |\n\
| `r` | [[cmd:toggle-relative-line-numbers]] | Relative numbers |\n\
| `w` | [[cmd:toggle-word-wrap]] | Word wrap |\n\
| `F` | [[cmd:toggle-fps]] | FPS overlay |\n\n\
*** SPC a — +ai\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `p` | [[cmd:ai-prompt]] | AI prompt |\n\
| `a` | [[cmd:open-ai-agent]] | Launch agent in shell |\n\
| `c` | [[cmd:ai-cancel]] | Cancel AI |\n\n\
*** Org-mode\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `TAB` | [[cmd:org-cycle]] | Three-state fold cycle |\n\
| `M-h` / `M-Left` | [[cmd:org-promote]] | Promote heading |\n\
| `M-l` / `M-Right` | [[cmd:org-demote]] | Demote heading |\n\
| `M-k` / `M-Up` | [[cmd:org-move-subtree-up]] | Move subtree up |\n\
| `M-j` / `M-Down` | [[cmd:org-move-subtree-down]] | Move subtree down |\n\
| `S-Left` | [[cmd:org-todo-prev]] | Prev TODO state |\n\
| `S-Right` | [[cmd:org-todo-next]] | Next TODO state |\n\
| `Enter` | [[cmd:org-open-link]] | Follow link |\n\n\
*** SPC m — +mode (org)\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `s n` | [[cmd:org-narrow-subtree]] | Narrow to subtree |\n\
| `s w` | [[cmd:org-widen]] | Widen (restore full buffer) |\n\n\
*** Markdown\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `TAB` | [[cmd:md-cycle]] | Three-state fold cycle |\n\
| `M-h` / `M-Left` | [[cmd:md-promote]] | Promote heading |\n\
| `M-l` / `M-Right` | [[cmd:md-demote]] | Demote heading |\n\
| `M-k` / `M-Up` | [[cmd:md-move-subtree-up]] | Move subtree up |\n\
| `M-j` / `M-Down` | [[cmd:md-move-subtree-down]] | Move subtree down |\n\n\
*** SPC m — +mode (markdown)\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `s n` | [[cmd:md-narrow-subtree]] | Narrow to subtree |\n\
| `s w` | [[cmd:md-widen]] | Widen (restore full buffer) |\n\n\
*** SPC h — +help\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `h` | [[cmd:help]] | Help index |\n\
| `k` | [[cmd:describe-key]] | Describe key |\n\
| `c` | [[cmd:describe-command]] | Describe command |\n\
| `s` | [[cmd:help-search]] | Search help |\n\
| `o` | [[cmd:describe-option]] | Describe option |\n\n\
*** SPC d — +debug\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `d` | [[cmd:debug-start]] | Start debug |\n\
| `s` | [[cmd:debug-self]] | Self-debug |\n\
| `b` | [[cmd:debug-toggle-breakpoint]] | Toggle breakpoint |\n\
| `c` | [[cmd:debug-continue]] | Continue |\n\
| `p` | [[cmd:debug-panel]] | Debug panel |\n\
| `n` | [[cmd:debug-step-over]] | Step over |\n\
| `i` | [[cmd:debug-step-into]] | Step into |\n\
| `o` | [[cmd:debug-step-out]] | Step out |\n\n\
*** SPC o — +open\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `t` | [[cmd:terminal]] | Open terminal |\n\
| `r` | [[cmd:terminal-reset]] | Reset terminal |\n\
| `c` | [[cmd:terminal-close]] | Close terminal |\n\n\
*** SPC n — +notes\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `f` | [[cmd:kb-find]] | Search KB nodes |\n\n\
*** SPC e — +eval\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `l` | [[cmd:eval-line]] | Eval line |\n\
| `b` | [[cmd:eval-buffer]] | Eval buffer |\n\
| `o` | [[cmd:open-scheme-repl]] | REPL |\n\
| `s` | [[cmd:send-to-shell]] | Send line to shell |\n\
| `S` | [[cmd:send-region-to-shell]] | Send region to shell |\n\n\
*** SPC q — +quit\n\
| Key | Command | Description |\n\
|-----|---------|-------------|\n\
| `q` | [[cmd:quit]] | Quit |\n\
| `Q` | [[cmd:force-quit]] | Force quit |\n\n\
See also: [[key:normal-mode]], [[index]]\n";
