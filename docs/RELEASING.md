# Releasing MAE

The release pipeline is **label-driven for version bumps** and **tag-driven for artifact builds**.
This document is the maintainer runbook: how the version number moves, how to cut a release, and
how to verify it.

## How versioning works (automatic)

`.github/workflows/version-bump.yml` runs when a PR merges to `main` and reads the PR's labels:

| PR label | Effect on `VERSION` |
|---|---|
| `release:none` | **no bump** — the version stays put (use for docs/tests/refactors) |
| `patch` (or none of the below, default) | `x.y.Z+1` |
| `minor` | `x.Y+1.0` |
| `major` | `X+1.0.0` |

On a bump it writes the new number to `VERSION`, updates the root `Cargo.toml` (and the workspace
members / `install.sh` per the workflow), and commits it. **Consequence:** during feature work,
label every PR `release:none` so the version only moves on the deliberate release PR.

> Current baseline: `VERSION` = `0.14.15`. The v0.15.0 release is a **`minor`** bump
> (`0.14.15` → `0.15.0`).

## How artifacts get built (automatic)

`.github/workflows/release.yml` triggers on a pushed tag matching `v*` and builds four artifacts,
then creates the GitHub Release with generated notes:

| Artifact | File | Contents |
|---|---|---|
| Linux CLI/TUI+GUI | `mae-linux-x86_64.tar.gz` | GUI-capable `mae` + `mae-daemon` + `mae-mcp-shim` + modules + manual KB + configs |
| Linux GUI | `mae-linux-x86_64-gui.AppImage` | self-contained GUI AppImage |
| macOS app | `MAE-macos-aarch64.zip` | `MAE.app` + `mae-daemon` + `mae-mcp-shim` + `install.sh` |
| macOS CLI | `mae-macos-aarch64.tar.gz` | GUI-capable `mae` (`-nw` for terminal) + services + modules + manual KB |

The tag is the single source of truth for a release — the build does not re-derive the version.

## Cutting a release

1. **Curate the changelog.** Move the `[Unreleased]` section of `CHANGELOG.md` into a dated
   `[X.Y.Z]` heading. Keep it honest and user-facing: features, the **limitations** (hub-GA /
   mesh-beta framing, the E2E caveats from [`E2E_USER_GUIDE.md`](E2E_USER_GUIDE.md) §7), and any
   upgrade/backup notes.

2. **Green `main`.** Confirm all required checks pass on `main` — `stable / {test,clippy,fmt}` **and
   `collab / docker e2e`** (the encrypted + §D3-removal + rotate + recover + mesh gate set, required
   as of v0.15). Run the local gates if in doubt:
   ```bash
   make ci-all                              # both workspaces, GUI included
   scripts/collab-encrypted-e2e.sh          # + MAE_E2E_REMOVAL / MAE_E2E_ROTATE / MAE_E2E_RECOVER=1
   scripts/collab-p2p-mesh-e2e.sh           # two-daemon mesh convergence
   ```

3. **Bump the version.** Merge the release PR with the `minor` label (for 0.15.0), letting
   `version-bump.yml` write `VERSION` + `Cargo.toml`. (Or, for a manual cut, set `VERSION`
   yourself and update `Cargo.toml` to match, then commit.)

4. **Rebuild the manual KB** if concepts changed this cycle: `make manual-kb` (bakes
   `assets/mae-manual.cozo` from `kb_seed`). The release artifacts ship the baked manual.

5. **Tag and push.** From the release commit on `main`:
   ```bash
   git tag v0.15.0
   git push origin v0.15.0
   ```
   `release.yml` builds the four artifacts and publishes the GitHub Release.

## Verifying a release

- **CI:** confirm all four `release.yml` build jobs are green and the Release has all four assets
  attached.
- **Install lifecycle:** download an artifact and run the bundled `install.sh` in a clean HOME —
  it places binaries in `~/.local/bin/`, modules, the manual KB, config scaffolding, and the
  service units (systemd on Linux, launchd on macOS). The `container / smoke + new-user` and
  `install / script validation` CI jobs exercise the pristine first-run path.
- **Smoke:** `mae --version` matches the tag; `mae-daemon --check-config` is clean; `:help` opens
  and `concept:kb-sharing` covers the E2E/rotation/recovery/mesh material.

## Post-release

- Open a fresh `## [Unreleased]` section at the top of `CHANGELOG.md`.
- Move any release-cycle findings that shipped as documented limitations (see the confidence-review
  tracker) into the appropriate follow-up milestone so nothing is silently dropped.
