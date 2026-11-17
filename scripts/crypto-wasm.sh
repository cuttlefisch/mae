#!/bin/sh
# mae-crypto's WASM module (shared/crypto-wasm): build, prove the build
# reproducible, and test it against the native build.
#
#   scripts/crypto-wasm.sh build   # the artifact, its size and sha256
#   scripts/crypto-wasm.sh repro   # build twice in separate target dirs; same bytes?
#   scripts/crypto-wasm.sh test    # size budget, ABI, native-vs-WASM differential,
#                                  # interop, entropy, leak and residue (needs node)
#
# The same reproducibility recipe as scripts/org-wasm.sh: one codegen unit,
# LTO, no debug info or symbols (Cargo.toml's `wasm` profile), Cargo.lock
# honoured (--locked), and the two machine-specific paths compiled into panic
# locations (this checkout and CARGO_HOME) remapped to fixed names. No
# getrandom backend flag: the WASM build has no getrandom at all, randomness
# is the module's one import (shared/crypto/src/rng.rs).
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
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        echo "neither sha256sum nor shasum found" >&2
        exit 1
    fi
}

build() {
    # Unit separator between flags, so a path with a space stays one flag.
    flags=$(printf '%s\037%s' "--remap-path-prefix=$root=/mae" "--remap-path-prefix=$cargo_home=/cargo")
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
        if [ "$a" != "$b" ]; then
            echo "the WASM build is not reproducible" >&2
            exit 1
        fi
        echo "reproducible: $(wc -c <"$(artifact "$target_dir")" | tr -d ' ') bytes, sha256 $a"
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
