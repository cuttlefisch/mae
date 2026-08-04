#!/bin/sh
# new-workspace.sh — create an isolated MAE workspace for parallel development.
#
#   scripts/new-workspace.sh <name> [base-branch]
#
# Creates a SEPARATE CLONE at $MAE_WORKSPACE_ROOT/<name> (default:
# ../mae-ws/<name>), on a fresh branch off `base-branch` (default: main), with
# every per-clone protection configured.
#
# ---------------------------------------------------------------------------
# Why a separate clone and NOT `git worktree`
# ---------------------------------------------------------------------------
#
# Worktrees look like the right tool — they are cheap and share the object
# store — but they share one thing that makes them unsafe for parallel builds:
# **`.git/config` is shared with the main checkout.**
#
# On 2026-08-03/04 that shared config cost a day. Git exports an absolute
# `GIT_DIR` into hooks when a command runs from a linked worktree, every child
# process inherits it, and a build launched under it made skia's
# `git-sync-deps` bypass its own `is_git_toplevel()` guard and run
# `git remote set-url origin <skia mirror>` — against the SHARED main
# `.git/config`. `origin` was silently repointed at a Skia dependency mirror
# four times; `git push` then prompted for Google credentials and `origin/main`
# pointed at an unrelated project's history.
#
# A separate clone has its own `.git/config`. A rewrite in one workspace cannot
# reach another, and cannot reach the primary checkout. That isolation is the
# whole point of this script. (The environment scrub in `.githooks/pre-commit`
# fixes the root cause; separate clones mean a future leak of the same shape is
# contained to one workspace instead of hitting all of them.)
#
# ---------------------------------------------------------------------------
# What a fresh clone does NOT inherit, and why this script exists
# ---------------------------------------------------------------------------
#
# Three protections live in `.git/config`, which is per-clone and cannot be
# committed. A plain `git clone` silently has NONE of them:
#
#   sync-deps.disable       skia's own opt-out; the backstop if a GIT_DIR
#                           leak ever recurs
#   remote.origin.pushurl   `set-url` rewrites only `remote.origin.url`, so a
#                           pinned pushurl is what actually stops the
#                           credential prompt if the URL is rewritten
#   core.hooksPath          points at the machine-local hook chain (secret
#                           scanning) which then delegates to .githooks
#
# `make setup-hooks` sets the first two and preserves an existing non-default
# hooksPath. This script runs it, then verifies the result rather than assuming
# it worked.
#
# Portable per CLAUDE.md principle #13: POSIX sh, no GNU-only flags.

set -eu

PRIMARY="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
ROOT="${MAE_WORKSPACE_ROOT:-$(dirname "$PRIMARY")/mae-ws}"
UPSTREAM="${MAE_UPSTREAM_URL:-git@github.com:cuttlefisch/mae.git}"

usage() {
    cat >&2 <<EOF
usage: $0 <name> [base-branch]

  <name>         workspace directory name, also used as the branch suffix
  [base-branch]  branch to start from (default: main)

env:
  MAE_WORKSPACE_ROOT   where workspaces live (default: $(dirname "$PRIMARY")/mae-ws)
  MAE_UPSTREAM_URL     clone URL (default: $UPSTREAM)

example:
  $0 fix-597-git-staging
  $0 spike-p2p main
EOF
    exit 2
}

[ $# -ge 1 ] || usage
NAME="$1"
BASE="${2:-main}"
DEST="$ROOT/$NAME"

case "$NAME" in
    */*|.|..) echo "error: <name> must be a plain directory name" >&2; exit 2 ;;
esac
if [ -e "$DEST" ]; then
    echo "error: $DEST already exists" >&2
    exit 1
fi

mkdir -p "$ROOT"

echo "==> cloning into $DEST"
# --reference-if-able reuses the primary clone's object store so this is fast
# and cheap; --dissociate then copies what it needs so the new workspace does
# NOT break if the primary is later moved, gc'd or deleted. Independence is
# worth the extra disk — a workspace that silently depends on another checkout
# is the class of coupling this script exists to remove.
git clone --reference-if-able "$PRIMARY" --dissociate "$UPSTREAM" "$DEST"

cd "$DEST"

echo "==> creating branch from origin/$BASE"
git fetch origin "$BASE" --quiet
# Explicit base. Never a bare `git checkout -b`, which silently branches off
# whatever HEAD happens to be — that is how a feature branch once ended up
# rooted on a long-running integration branch carrying ~100 unmerged commits.
git checkout -q -b "$NAME" "origin/$BASE"

echo "==> configuring per-clone protections"
make setup-hooks

# Inherit the machine-local hook chain if the primary has one and setup-hooks
# left the default in place (it only claims an unset or already-.githooks value).
PRIMARY_HOOKS="$(git -C "$PRIMARY" config --get core.hooksPath || true)"
MINE="$(git config --get core.hooksPath || true)"
if [ -n "$PRIMARY_HOOKS" ] && [ "$PRIMARY_HOOKS" != ".githooks" ] && [ "$MINE" = ".githooks" ]; then
    git config core.hooksPath "$PRIMARY_HOOKS"
    echo "    core.hooksPath inherited from the primary clone: $PRIMARY_HOOKS"
fi

echo "==> verifying"
fail=0
check() {
    actual="$(git config --get "$1" || true)"
    if [ -z "$actual" ]; then
        echo "    MISSING  $1" >&2
        fail=1
    else
        echo "    ok       $1 = $actual"
    fi
}
check sync-deps.disable
check remote.origin.pushurl
check core.hooksPath

# The isolation property this whole script is about: prove this workspace's git
# dir is its OWN, not shared with the primary. A `git worktree` would resolve to
# the primary's `.git`, which is the coupling that let a skia build rewrite the
# main checkout's remote.
#
# NB compare ABSOLUTE git dirs. `git config --show-origin` reports a path
# relative to the repo root (`file:.git/config`) when run from inside it, so a
# substring match against $DEST silently never matches — a check that cannot
# pass is no better than one that cannot fail.
mine="$(git rev-parse --absolute-git-dir)"
theirs="$(git -C "$PRIMARY" rev-parse --absolute-git-dir 2>/dev/null || echo '')"
if [ "$mine" = "$theirs" ]; then
    echo "    FAIL     git dir is SHARED with the primary clone: $mine" >&2
    echo "             a remote rewrite here would hit $PRIMARY too." >&2
    fail=1
else
    echo "    ok       git dir is workspace-local ($mine)"
fi

[ "$fail" -eq 0 ] || { echo "workspace created but NOT fully protected — fix the above" >&2; exit 1; }

cat <<EOF

Workspace ready:

    cd $DEST

  branch:  $NAME  (from origin/$BASE)
  config:  isolated — a remote rewrite here cannot reach $PRIMARY

  Each workspace builds into its own target/. A full debug build of this
  workspace is large (cargo's default dev profile keeps full debuginfo), so
  prefer 'cargo check' while iterating and 'rm -rf target/debug' when done.
EOF
