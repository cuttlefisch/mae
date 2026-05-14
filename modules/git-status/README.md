# Module: git-status

Magit-style git status buffer keybindings and SPC g leader.

## Info

| Field | Value |
|-------|-------|
| Category | tools |
| Version | 0.1.0 |
| Dependencies | None |

## Keybindings

### Normal mode (SPC g leader)

| Key | Command | Description |
|-----|---------|-------------|
| `SPC g s` / `SPC g g` | `git-status` | Open git status buffer |
| `SPC g b` | `git-blame` | Show git blame |
| `SPC g d` | `git-diff` | Show git diff |
| `SPC g l` | `git-log` | Show git log |
| `SPC g c` | `git-commit` | Commit staged changes |
| `SPC g S` | `git-stage-all` | Stage all changes |
| `SPC g U` | `git-unstage-all` | Unstage all changes |

### Git status buffer (git-status keymap)

#### Navigation

| Key | Command | Description |
|-----|---------|-------------|
| `j` | `move-down` | Move down |
| `k` | `move-up` | Move up |
| `n` | `git-next-hunk` | Jump to next hunk |
| `p` | `git-prev-hunk` | Jump to previous hunk |
| `G` | `move-to-last-line` | Go to last line |
| `g g` | `move-to-first-line` | Go to first line |
| `Tab` | `git-toggle-fold` | Toggle section fold |
| `Enter` | `git-status-open` | Open file at point |

#### Stage / Unstage

| Key | Command | Description |
|-----|---------|-------------|
| `s` | `git-stage` | Stage hunk or file |
| `u` | `git-unstage` | Unstage hunk or file |
| `S` | `git-stage-all` | Stage all |
| `U` | `git-unstage-all` | Unstage all |
| `x` | `git-discard` | Discard changes |

#### Commit

| Key | Command | Description |
|-----|---------|-------------|
| `c c` | `git-commit` | Commit staged changes |
| `c a` | `git-amend` | Amend last commit |

#### Log

| Key | Command | Description |
|-----|---------|-------------|
| `l l` | `git-log` | Show git log |

#### Push / Pull / Fetch

| Key | Command | Description |
|-----|---------|-------------|
| `P p` / `P u` | `git-push` | Push to remote |
| `f f` | `git-fetch` | Fetch from remote |
| `F p` / `F u` | `git-pull` | Pull from remote |

#### Branch

| Key | Command | Description |
|-----|---------|-------------|
| `b b` | `git-branch-switch` | Switch branch |
| `b n` | `git-branch-create` | Create new branch |
| `b d` | `git-branch-delete` | Delete branch |

#### Stash

| Key | Command | Description |
|-----|---------|-------------|
| `z z` | `git-stash-push` | Push stash |
| `z p` | `git-stash-pop` | Pop stash |
| `z a` | `git-stash-apply` | Apply stash |
| `z d` | `git-stash-drop` | Drop stash |

#### Misc

| Key | Command | Description |
|-----|---------|-------------|
| `g r` | `git-status` | Refresh status |
| `q` / `Escape` | `kill-buffer` | Close status buffer |
| `?` | `show-buffer-keys` | Show all keybindings |

### Mode menu (SPC m, inside git-status buffer)

| Key | Command | Description |
|-----|---------|-------------|
| `SPC m s` | `git-stage` | Stage |
| `SPC m u` | `git-unstage` | Unstage |
| `SPC m S` | `git-stage-all` | Stage all |
| `SPC m U` | `git-unstage-all` | Unstage all |
| `SPC m x` | `git-discard` | Discard |
| `SPC m c` | `git-commit` | Commit |
| `SPC m a` | `git-amend` | Amend |
| `SPC m p` | `git-push` | Push |
| `SPC m f` | `git-fetch` | Fetch |
| `SPC m F` | `git-pull` | Pull |
| `SPC m r` | `git-status` | Refresh |

## Configuration

No module-specific options.
