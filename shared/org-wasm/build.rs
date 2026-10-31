//! The WASM module's stack: 4 MiB rather than rustc's 1 MiB default, so the
//! renderer's recursion (bounded by mae-org's RENDER_MAX_DEPTH and the
//! parser's nesting cap) has the same headroom as a native test thread.
//! Only for wasm32: a native link must not see the flag.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.starts_with("wasm32") {
        println!("cargo:rustc-link-arg-cdylib=-zstack-size=4194304");
    }
}
