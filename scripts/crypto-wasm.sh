#!/bin/sh
# SPIKE: MAE's envelope crypto as a WASM module (shared/crypto-wasm).
# Not wired into make, CI or a release.
#
#   scripts/crypto-wasm.sh build   # the artifact, its size and sha256
#   scripts/crypto-wasm.sh repro   # two builds in separate target dirs; same bytes?
#   scripts/crypto-wasm.sh test    # WASM <-> native interop, tamper, entropy (needs node)
#
# Same reproducibility recipe as scripts/org-wasm.sh, plus one cfg: on
# wasm32-unknown-unknown getrandom has no default source, and the `custom`
# backend routes it to the module's one host import (src/lib.rs).
set -eu

cmd=${1:-build}
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
CARGO=${CARGO:-cargo}
NODE=${NODE:-node}
cargo_home=${CARGO_HOME:-$HOME/.cargo}
target_dir=${CRYPTO_WASM_TARGET_DIR:-$root/target}

artifact() {
    echo "$1/wasm32-unknown-unknown/wasm/mae_crypto_wasm.wasm"
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

build() {
    # Unit separator between flags, so a path with a space stays one flag.
    flags=$(printf '%s\037%s\037%s' "--remap-path-prefix=$root=/mae" \
        "--remap-path-prefix=$cargo_home=/cargo" '--cfg=getrandom_backend="custom"')
    CARGO_ENCODED_RUSTFLAGS="$flags" CARGO_TARGET_DIR="$1" \
        "$CARGO" build --locked -p mae-crypto-wasm --lib \
        --target wasm32-unknown-unknown --profile wasm
}

case "$cmd" in
    build)
        build "$target_dir"
        a=$(artifact "$target_dir")
        echo "$a: $(wc -c <"$a" | tr -d ' ') bytes, sha256 $(sha256 "$a")"
        ;;
    repro)
        build "$target_dir"
        second="$target_dir/crypto-wasm-repro"
        rm -rf "$second"
        build "$second"
        a=$(sha256 "$(artifact "$target_dir")")
        b=$(sha256 "$(artifact "$second")")
        echo "first build:  $a"
        echo "second build: $b"
        [ "$a" = "$b" ] || { echo "not reproducible" >&2; exit 1; }
        ;;
    test)
        build "$target_dir"
        CARGO_TARGET_DIR="$target_dir" "$CARGO" build --locked --release -p mae-crypto-wasm --bin mae-crypto-native
        MAE_CRYPTO_WASM=$(artifact "$target_dir") \
            MAE_CRYPTO_NATIVE="$target_dir/release/mae-crypto-native" \
            "$NODE" shared/crypto-wasm/js/test.mjs
        ;;
    *)
        echo "usage: $0 build|repro|test" >&2
        exit 2
        ;;
esac
