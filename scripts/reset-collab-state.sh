#!/bin/sh
# reset-collab-state.sh — move MAE collab + KB state aside (timestamped backup),
# never deletes. Use between two-machine test scenarios so a poisoned store from a
# prior build can't silently stall the next run (see docs/collab-kb-sync-testing-
# lessons.md, B-5). Cross-OS (Linux + macOS), XDG-first per CLAUDE.md #13.
#
# Usage:
#   scripts/reset-collab-state.sh            # back up collab/ + kb/ subdirs
#   scripts/reset-collab-state.sh --list     # list current backups, do nothing
#
# It does NOT touch config (init.scm / daemon.toml) or authorized_keys — only the
# collab session/identity store and the KB CRDT store, so the editor + daemon
# re-register cleanly on next launch while old state is preserved for forensics
# under <name>.backup.<timestamp>.

set -eu

TS="$(date -u +%Y%m%d-%H%M%S)"

# Candidate mae data dirs, XDG-first (honor XDG_DATA_HOME on every platform, then the
# platform default). We act on every candidate that exists + is distinct, so a
# machine using the macOS Library path AND one using XDG are both handled.
DIRS="$HOME/.local/share/mae"
[ -n "${XDG_DATA_HOME:-}" ] && DIRS="$XDG_DATA_HOME/mae $DIRS"
[ "$(uname -s)" = "Darwin" ] && DIRS="$DIRS $HOME/Library/Application Support/mae"

if [ "${1:-}" = "--list" ]; then
    found=0
    for base in $DIRS; do
        for b in "$base"/collab.backup.* "$base"/kb.backup.*; do
            [ -e "$b" ] && { echo "$b"; found=1; }
        done
    done
    [ "$found" -eq 1 ] || echo "(no backups found)"
    exit 0
fi

seen=" "
moved=0
for base in $DIRS; do
    case "$seen" in *" $base "*) continue ;; esac   # de-dup (XDG may equal default)
    seen="$seen$base "
    [ -d "$base" ] || continue
    for sub in collab kb; do
        if [ -d "$base/$sub" ]; then
            mv "$base/$sub" "$base/$sub.backup.$TS"
            echo "moved: $base/$sub -> $base/$sub.backup.$TS"
            moved=1
        fi
    done
done

if [ "$moved" -eq 1 ]; then
    echo "done. Restore by moving a *.backup.$TS dir back to its original name."
else
    echo "nothing to reset (no collab/ or kb/ state found under the mae data dir)."
fi
