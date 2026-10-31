#!/bin/sh
# Regenerate the Emacs org-element goldens for mae-org's conformance test
# (ADR-112 D5). Goldens are checked in so CI needs no Emacs; run this only
# when the fixture list, a fixture, or the pinned Org version changes, and
# review the golden diff like code.
#
# Usage (from anywhere):  shared/org/conformance/regen.sh
#
# Requires: emacs (batch). The Org version used is recorded in
# goldens/ORG_VERSION; the test reports it. POSIX sh, no GNU-only flags
# (principle #13).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
list="$here/corpus.txt"
out="$here/goldens"

command -v emacs >/dev/null 2>&1 || { echo "regen.sh: emacs not found" >&2; exit 1; }

manifest=$(mktemp "${TMPDIR:-/tmp}/mae-org-manifest.XXXXXX")
trap 'rm -f "$manifest"' EXIT INT TERM

rm -rf "$out"
mkdir -p "$out"
count=0
while IFS= read -r rel || [ -n "$rel" ]; do
    case "$rel" in ''|'#'*) continue ;; esac
    [ -f "$root/$rel" ] || { echo "regen.sh: listed fixture missing: $rel" >&2; exit 1; }
    printf '%s\t%s\n' "$root/$rel" "$out/$rel.jsonl" >> "$manifest"
    count=$((count + 1))
done < "$list"

emacs -Q --batch -l "$here/org-element-dump.el" --manifest "$manifest"
emacs -Q --batch --eval '(progn (require (quote org)) (princ (format "%s\n" org-version)))' > "$out/ORG_VERSION"
echo "regen.sh: $count goldens written with Org $(cat "$out/ORG_VERSION")"
