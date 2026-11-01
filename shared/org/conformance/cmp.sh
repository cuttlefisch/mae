#!/bin/sh
# cmp.sh — does NEEDLE show in Emacs's export and in which mae-org channels?
#
# Usage: shared/org/conformance/cmp.sh NEEDLE FILE...
#
# For each FILE: Emacs 30.2's ox-html body export (conformance/leak-export.el,
# bounded: systemd-run --user --scope MemoryMax=3G where systemd is there,
# else ulimit -v; always under timeout; HOME a scratch dir under target/)
# and the mae-org probe (examples/probe.rs `grep`), side by side:
#
#   == FILE: emacs=SHOWN|hidden|ERROR <msg>|NO RESULT | mae: N channel(s) ...
#
# leak-export.el exports an id: link as a link to an opaque target with its
# description, as the leak oracle does. VERBOSE=1 also prints Emacs's output
# and every MAE hit.
#
# Environment: PROBE (default target/debug/examples/probe; build it with
# `cargo build -p mae-org --example probe`), EMACS_TIMEOUT (seconds,
# default 60, at most 120). POSIX sh; no GNU-only flags.
set -eu

if [ "$#" -lt 2 ]; then
  echo "usage: $0 NEEDLE FILE..." >&2
  exit 2
fi
needle=$1
shift

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
probe=${PROBE:-$root/target/debug/examples/probe}
limit=${EMACS_TIMEOUT:-60}
if [ "$limit" -gt 120 ]; then limit=120; fi
if [ ! -x "$probe" ]; then
  echo "no probe at $probe: cargo build -p mae-org --example probe" >&2
  exit 2
fi

work=$root/target/leak-cmp/run-$$
home=$root/target/leak-cmp/emacs-home
mkdir -p "$work/in" "$work/out" "$home"
trap 'rm -rf "$work"' EXIT INT TERM

i=0
for f in "$@"; do
  i=$((i + 1))
  name=$(printf 'f%05d' "$i")
  cp "$f" "$work/in/$name.org"
done

el=$here/leak-export.el
if command -v systemd-run >/dev/null 2>&1 &&
  systemd-run --user --scope -q true >/dev/null 2>&1; then
  systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0 \
    --setenv=HOME="$home" timeout "$limit" \
    emacs -Q --batch -l "$el" "$work/in" "$work/out" >"$work/emacs.log" 2>&1 || true
else
  (
    # `ulimit -v` is not POSIX (SC3045), but dash, bash, ksh and zsh all
    # have it; where it is missing Emacs runs uncapped, with a warning.
    # shellcheck disable=SC3045
    ulimit -v 3145728 2>/dev/null || echo "cmp.sh: no ulimit -v here; Emacs has no memory cap" >&2
    HOME=$home
    export HOME
    if command -v timeout >/dev/null 2>&1; then t=timeout
    elif command -v gtimeout >/dev/null 2>&1; then t=gtimeout
    else t=""
    fi
    if [ -n "$t" ]; then
      "$t" "$limit" emacs -Q --batch -l "$el" "$work/in" "$work/out"
    else
      emacs -Q --batch -l "$el" "$work/in" "$work/out"
    fi
  ) >"$work/emacs.log" 2>&1 || true
fi

i=0
for f in "$@"; do
  i=$((i + 1))
  base=$work/out/$(printf 'f%05d' "$i")
  if [ -f "$base.html" ]; then
    if grep -qiF -- "$needle" "$base.html"; then e=SHOWN; else e=hidden; fi
    out=$base.html
  elif [ -f "$base.error" ]; then
    e="ERROR $(head -n 1 "$base.error" | cut -c 1-120)"
    out=$base.error
  else
    e="NO RESULT (hang or crash: see the log)"
    out=$work/emacs.log
  fi
  hits=$("$probe" grep "$f" "$needle")
  summary=$(printf '%s\n' "$hits" | tail -n 1)
  printf '== %s: emacs=%s | mae: %s\n' "$f" "$e" "$summary"
  if [ "${VERBOSE:-0}" = 1 ]; then
    echo "--- emacs:"
    cat "$out"
    echo "--- mae:"
    printf '%s\n' "$hits"
  fi
done
