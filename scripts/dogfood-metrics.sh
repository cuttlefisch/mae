#!/usr/bin/env bash
# PII-SAFE dogfood metrics wrapper (plan Phase 7 / issue #243).
#
# Ingests an org-mode KB directory and prints ONLY quantitative metrics — counts, sizes, ratios,
# timings — via shared/kb/examples/dogfood_metrics.rs. It never emits node titles, bodies, tags,
# ids, filenames, or link targets, so the output is safe to paste into an issue when validating
# ingestion at real RoamNotes scale.
#
#   scripts/dogfood-metrics.sh <org-dir>     print PII-safe quant metrics for an org KB
#   scripts/dogfood-metrics.sh --self-test   verify the harness works + leaks no content
#
# Portable (principle #13): POSIX-ish bash, no GNU-only tooling.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

run_metrics() {
  # mae-sync is optional in mae-kb behind the `crdt` feature; the lib needs it to compile.
  cargo run --quiet -p mae-kb --features crdt --example dogfood_metrics -- "$1"
}

if [ "${1:-}" = "--self-test" ]; then
  fixture="tests/fixtures/kb/collabtest"
  echo "dogfood self-test: ingesting $fixture" >&2
  out="$(run_metrics "$fixture")"
  echo "$out"

  # (a) The metrics match the known fixture shape (3 nodes, 0 links).
  echo "$out" | grep -q "^total_nodes 3$" || { echo "FAIL: expected total_nodes 3" >&2; exit 1; }
  echo "$out" | grep -q "^total_links 0$" || { echo "FAIL: expected total_links 0" >&2; exit 1; }

  # (b) PII-ABSENCE: none of the fixture's node-title tokens may appear in the output. This is
  # the adversarial guard — if the harness ever regresses to printing content, this fails.
  for token in Collab Alpha Beta Overview Fixture Instance; do
    if echo "$out" | grep -qi "$token"; then
      echo "FAIL: content token '$token' leaked into metrics output (PII)!" >&2
      exit 1
    fi
  done

  echo "dogfood self-test PASS: metrics correct + no content leaked" >&2
  exit 0
fi

if [ -z "${1:-}" ]; then
  echo "usage: $0 <org-dir>          print PII-safe quant metrics for an org KB" >&2
  echo "       $0 --self-test        verify the harness (uses the repo fixture)" >&2
  exit 2
fi

run_metrics "$1"
