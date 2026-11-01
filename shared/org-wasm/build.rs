//! The WASM module's shadow stack: 4 MiB rather than rustc's 1 MiB default,
//! so the renderer's recursion (bounded by mae-org's RENDER_MAX_DEPTH and
//! the parser's nesting cap) has the same headroom as a native test thread;
//! and FIRST in linear memory (`--stack-first`, which rustc already passes
//! for wasm32, stated here so it cannot silently change), so an overflow
//! runs below address 0 and traps instead of writing over static data or
//! the heap. `js/test.mjs` reads both back from the built module. Only for
//! wasm32: a native link must not see the flags.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.starts_with("wasm32") {
        println!("cargo:rustc-link-arg-cdylib=-zstack-size=4194304");
        println!("cargo:rustc-link-arg-cdylib=--stack-first");
    }
}
