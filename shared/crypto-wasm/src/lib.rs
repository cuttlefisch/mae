//! SPIKE: MAE's ADR-037 envelope crypto (`mae_sync::content_crypto`) as a
//! WASM module for a browser client. Not built by any release or CI job.
//! @stability: experimental
//!
//! The crypto is not reimplemented: `content_crypto.rs` is mae-sync's own
//! file, compiled here by `#[path]`, because mae-sync as a whole does not
//! build for wasm32-unknown-unknown (see Cargo.toml). A real crate would
//! move that file into a small crate both depend on.
//!
//! The ABI mirrors mae-org-wasm's, without JSON:
//!
//! 1. `mae_crypto_alloc(len) -> ptr`: reserve `len` request bytes;
//! 2. `mae_crypto_call(op, ptr, len) -> n`: run `op` ([`api`]); the request
//!    is wiped and freed; `n >= 0` is the response length, `n < 0` an error;
//! 3. `mae_crypto_result_ptr() -> ptr`: the response;
//! 4. `mae_crypto_result_free()`: wipe and drop the response;
//! 5. `mae_crypto_live_bytes() -> n`: bytes allocated and not freed.
//!
//! One import: `mae_crypto_host.random_fill(ptr, len) -> u32` (0 = filled),
//! which the loader backs with `crypto.getRandomValues`. It is getrandom's
//! custom backend, so every random byte the crypto draws (rand's ThreadRng
//! seed and reseeds) comes from the host's CSPRNG on demand. A host that
//! cannot supply it fails the call: getrandom errors, rand panics, and the
//! module traps (panic = abort). Nothing falls back to a fixed seed.

pub mod api;

#[path = "../../sync/src/content_crypto.rs"]
pub mod content_crypto;

define_call!(crate::content_crypto);

#[cfg(target_arch = "wasm32")]
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use zeroize::Zeroize;

#[cfg(target_arch = "wasm32")]
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method forwards to `System` unchanged; the counter is a
// side effect.
#[cfg(target_arch = "wasm32")]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded; the caller upholds `alloc`'s contract.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: forwarded; `ptr` came from this allocator with `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new: usize) -> *mut u8 {
        // SAFETY: forwarded; the caller upholds `realloc`'s contract.
        let p = unsafe { System.realloc(ptr, layout, new) };
        if !p.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            LIVE.fetch_add(new, Ordering::Relaxed);
        }
        p
    }
}

#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "mae_crypto_host")]
unsafe extern "C" {
    /// Fills `len` bytes at `ptr` from the host's CSPRNG; 0 on success.
    fn random_fill(ptr: *mut u8, len: usize) -> u32;
}

/// getrandom's custom backend (`--cfg getrandom_backend="custom"`).
///
/// # Safety
/// getrandom passes a writable region of `len` bytes.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
unsafe extern "Rust" fn __getrandom_v03_custom(
    dest: *mut u8,
    len: usize,
) -> Result<(), getrandom::Error> {
    // SAFETY: `dest..dest+len` is writable (getrandom's contract); the host
    // writes exactly that range or reports failure.
    if unsafe { random_fill(dest, len) } == 0 {
        Ok(())
    } else {
        Err(getrandom::Error::new_custom(1))
    }
}

thread_local! {
    static RESULT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Reserves `len` bytes for a request.
#[unsafe(no_mangle)]
pub extern "C" fn mae_crypto_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Runs `op` on the request at `ptr`, wipes and frees it, and returns the
/// response length (or a negative error code).
///
/// # Safety
/// `ptr` must come from `mae_crypto_alloc(len)` with this same `len`, hold
/// `len` initialised bytes, and not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mae_crypto_call(op: u32, ptr: *mut u8, len: usize) -> i32 {
    // SAFETY: by the contract above, a `Vec<u8>` allocation of capacity
    // `len` whose bytes the caller initialised; rebuilding takes ownership.
    let mut req = unsafe { Vec::from_raw_parts(ptr, len, len) };
    let out = call(op, &req);
    scrub_stack();
    req.zeroize();
    drop(req);
    RESULT.with(|r| r.borrow_mut().zeroize());
    match out {
        Ok(bytes) => {
            let n = bytes.len() as i32;
            RESULT.with(|r| *r.borrow_mut() = bytes);
            n
        }
        Err(code) => code,
    }
}

/// Zeroes the shadow stack the operation just used. `ZeroizeOnDrop` wipes a
/// key where it is dropped, not the copies a move leaves behind: measured
/// (`js/test.mjs`, residue check), `unwrap` left one content-key copy on the
/// shadow stack without this. The operation's frames sit below this one,
/// so a zeroed array here overwrites them; 64 KiB is well above the depth
/// any operation reaches.
#[inline(never)]
fn scrub_stack() {
    let mut pad = [0u8; 64 * 1024];
    pad.zeroize();
    std::hint::black_box(&pad);
}

/// Where the last response is (valid until the next call or free).
#[unsafe(no_mangle)]
pub extern "C" fn mae_crypto_result_ptr() -> *const u8 {
    RESULT.with(|r| r.borrow().as_ptr())
}

/// Wipes and drops the last response.
#[unsafe(no_mangle)]
pub extern "C" fn mae_crypto_result_free() {
    RESULT.with(|r| {
        let mut r = r.borrow_mut();
        r.zeroize();
        *r = Vec::new();
    });
}

/// Bytes allocated and not freed (0 natively, where nothing counts).
#[unsafe(no_mangle)]
pub extern "C" fn mae_crypto_live_bytes() -> usize {
    LIVE.load(Ordering::Relaxed)
}
