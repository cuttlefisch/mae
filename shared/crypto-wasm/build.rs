//! The WASM module's shadow stack goes FIRST in linear memory
//! (`--stack-first`, which rustc already passes for wasm32, stated here so
//! it cannot silently change): an overflow runs below address 0 and traps
//! instead of writing over static data or the heap, where key material
//! lives. 1 MiB, rustc's default, stated for the same reason; the scrub in
//! src/lib.rs relies on no operation reaching anywhere near it. Only for
//! wasm32: a native link must not see the flags.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.starts_with("wasm32") {
        println!("cargo:rustc-link-arg-cdylib=-zstack-size=1048576");
        println!("cargo:rustc-link-arg-cdylib=--stack-first");
    }
}
