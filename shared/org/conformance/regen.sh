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
#
# Emacs is BOUNDED: on some inputs org-element never terminates (e.g.
# `<<<x>>>\n\nx[[x]]', where it allocates without limit), so an unbounded
# run can take the machine down. Every Emacs runs under a virtual-memory cap
# (`ulimit -v', MAE_ORG_ORACLE_VMEM_KB, default 2 GiB) and a time limit
# (`timeout', else `gtimeout', else none; MAE_ORG_ORACLE_TIMEOUT seconds per
# file, default 60). Files run in chunks (a chunk gets five files' time);
# when a chunk does not finish, its remaining files run one at a time. A
# file that hits a limit gets the golden {"type":"#nonterminating",...}:
# "oracle: Emacs did not terminate", reported, not an error, and its
# differences must be registered in divergences.toml like any other.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
list="$here/corpus.txt"
out="$here/goldens"
dump="$here/org-element-dump.el"
vmem_kb=${MAE_ORG_ORACLE_VMEM_KB:-2097152}
secs=${MAE_ORG_ORACLE_TIMEOUT:-60}
chunk_size=25

command -v emacs >/dev/null 2>&1 || { echo "regen.sh: emacs not found" >&2; exit 1; }

if command -v timeout >/dev/null 2>&1; then
    timeout_cmd=timeout
elif command -v gtimeout >/dev/null 2>&1; then
    timeout_cmd=gtimeout
else
    timeout_cmd=
    echo "regen.sh: warning: no timeout/gtimeout; Emacs runs without a time limit" >&2
fi
if ! (ulimit -v "$vmem_kb") 2>/dev/null; then
    echo "regen.sh: warning: ulimit -v unsupported here; Emacs runs without a memory cap" >&2
    vmem_kb=
fi

# bounded SECONDS EMACS-ARGS...: one Emacs under the caps; its exit status.
bounded() {
    limit=$1
    shift
    (
        [ -z "$vmem_kb" ] || ulimit -v "$vmem_kb"
        if [ -n "$timeout_cmd" ]; then
            exec "$timeout_cmd" "$limit" emacs "$@"
        else
            exec emacs "$@"
        fi
    )
}

tmp=$(mktemp -d "${TMPDIR:-/tmp}/mae-org-regen.XXXXXX")
trap 'rm -rf "$tmp"' EXIT INT TERM

rm -rf "$out"
mkdir -p "$out"
all="$tmp/all"
: > "$all"
count=0
while IFS= read -r rel || [ -n "$rel" ]; do
    case "$rel" in ''|'#'*) continue ;; esac
    [ -f "$root/$rel" ] || { echo "regen.sh: listed fixture missing: $rel" >&2; exit 1; }
    mkdir -p "$(dirname "$out/$rel.jsonl")"
    printf '%s\t%s\n' "$root/$rel" "$out/$rel.jsonl" >> "$all"
    count=$((count + 1))
done < "$list"

# One file on its own; a run that dies under the caps makes it
# #nonterminating. Out of memory is read from the message, not the status:
# measured, Emacs exits 3 (the dump's handler), 1 or 255 depending on where
# the exhaustion strikes. The preflight below rules out a broken dump script.
one() {
    src=$1
    dst=$2
    status=0
    bounded "$secs" -Q --batch -l "$dump" "$src" > "$dst.tmp" 2> "$tmp/err" || status=$?
    if [ "$status" -eq 0 ]; then
        mv "$dst.tmp" "$dst"
        return
    fi
    rm -f "$dst.tmp"
    if grep -q "Memory exhausted" "$tmp/err"; then
        why=memory
    elif [ "$status" -eq 124 ]; then
        why=time
    else
        why="exit $status"
    fi
    printf '{"type":"#nonterminating","limit":"%s"}\n' "$why" > "$dst"
    nonterminating=$((nonterminating + 1))
    echo "regen.sh: oracle: Emacs did not terminate ($why): $src" >&2
}

# Preflight: the dump must work on a trivial file, or every file would be
# recorded as nonterminating.
printf '* a\n' > "$tmp/preflight.org"
bounded "$secs" -Q --batch -l "$dump" "$tmp/preflight.org" > "$tmp/preflight.jsonl"
grep -q '"type":"#file"' "$tmp/preflight.jsonl" || { echo "regen.sh: the dump script fails on a trivial file" >&2; exit 1; }

nonterminating=0
split -l "$chunk_size" "$all" "$tmp/chunk."
for chunk in "$tmp"/chunk.*; do
    bounded $((secs * 5)) -Q --batch -l "$dump" --manifest "$chunk" >/dev/null 2>&1 || true
    while IFS="$(printf '\t')" read -r src dst; do
        [ -f "$dst" ] || one "$src" "$dst"
    done < "$chunk"
done

emacs -Q --batch --eval '(progn (require (quote org)) (princ (format "%s\n" org-version)))' > "$out/ORG_VERSION"
echo "regen.sh: $count goldens written with Org $(cat "$out/ORG_VERSION"); Emacs did not terminate on $nonterminating"
