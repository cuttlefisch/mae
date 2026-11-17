//! mae-crypto's WASM facade: MAE's member-key, identity-challenge, key-backup
//! and content crypto for a browser client, as MAE's own Rust code.
//! @stability: experimental
//!
//! The ABI is five exported functions over linear memory, no bindings
//! generator (`js/mae-crypto.mjs` drives it), the shape of mae-org-wasm's:
//!
//! 1. `mae_crypto_alloc(len) -> ptr`: the caller reserves `len` bytes and
//!    writes the request's UTF-8 JSON there;
//! 2. `mae_crypto_call(op, ptr, len) -> len`: runs operation `op`
//!    ([`api::Op`]) on those bytes, wipes and frees them, and keeps the JSON
//!    response, whose length it returns;
//! 3. `mae_crypto_result_ptr() -> ptr`: where the response is;
//! 4. `mae_crypto_result_free()`: wipes and drops the response;
//! 5. `mae_crypto_live_bytes() -> n`: bytes allocated and not freed (the
//!    leak test).
//!
//! One import: `mae_crypto_host.random_fill(ptr, len) -> u32` (0 = filled;
//! mae-crypto's `rng`), which the loader backs with `crypto.getRandomValues`.
//!
//! Secrets in linear memory: every heap block is zeroed when it is freed
//! (the allocator below), and the shadow stack an operation used is zeroed
//! when it returns ([`scrub_stack`]). So after a call, the only secret bytes
//! left in the module are the member keys it holds by handle (until
//! `forget`) and the response until `mae_crypto_result_free`. The loader's
//! JS-side copies are outside this module's reach.
//!
//! The module is single-threaded and not re-entrant (no callback crosses
//! it). All `unsafe` is in this file and mae-crypto's `rng`, each block with
//! its argument.

pub mod api;
mod keystore;

#[cfg(target_arch = "wasm32")]
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use zeroize::Zeroize;

/// The system allocator, counting live bytes and zeroing every block it
/// frees. Only the WASM module: a native build keeps the default allocator.
#[cfg(target_arch = "wasm32")]
struct Wiping;

static LIVE: AtomicUsize = AtomicUsize::new(0);

/// Zeroes `len` bytes at `ptr` in a way the optimiser may not drop as a
/// dead store before a free.
///
/// # Safety
/// `ptr..ptr+len` must be writable.
#[cfg(target_arch = "wasm32")]
unsafe fn wipe(ptr: *mut u8, len: usize) {
    // SAFETY: by the contract above.
    unsafe { std::ptr::write_bytes(ptr, 0, len) };
    std::sync::atomic::compiler_fence(Ordering::SeqCst);
    std::hint::black_box(ptr);
}

// SAFETY: alloc and dealloc forward to `System` with the caller's arguments
// unchanged; dealloc first zeroes the block it is handed, which the caller
// owns. realloc is alloc + copy + dealloc, each upholding its contract.
#[cfg(target_arch = "wasm32")]
unsafe impl GlobalAlloc for Wiping {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded; the caller upholds `alloc`'s contract.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` is a live block of `layout.size()` bytes the caller
        // owns and is giving up.
        unsafe { wipe(ptr, layout.size()) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: forwarded; `ptr` came from this allocator with `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new: usize) -> *mut u8 {
        // Never `System.realloc`: it may move the block and free the old
        // one unwiped.
        // SAFETY: `new` is non-zero and, rounded to `layout.align()`, does
        // not overflow (realloc's contract), so the layout is valid.
        let new_layout = unsafe { Layout::from_size_align_unchecked(new, layout.align()) };
        // SAFETY: a valid, non-zero layout.
        let p = unsafe { self.alloc(new_layout) };
        if !p.is_null() {
            // SAFETY: both blocks are live and distinct; the copy is within
            // the smaller of the two; the old block is then freed (wiped).
            unsafe {
                std::ptr::copy_nonoverlapping(ptr, p, layout.size().min(new));
                self.dealloc(ptr, layout);
            }
        }
        p
    }
}

#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOCATOR: Wiping = Wiping;

thread_local! {
    static RESULT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Reserves `len` bytes for a request; the pointer goes to `mae_crypto_call`.
#[unsafe(no_mangle)]
pub extern "C" fn mae_crypto_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Runs `op` on the `len` request bytes at `ptr`, wipes and frees them, and
/// returns the response's length (always a JSON response, errors included).
///
/// # Safety
/// `ptr` must come from `mae_crypto_alloc(len)` with this same `len`, hold
/// `len` initialised bytes, and not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mae_crypto_call(op: u32, ptr: *mut u8, len: usize) -> usize {
    // SAFETY: by the contract above, a `Vec<u8>` allocation of capacity
    // `len` whose bytes the caller initialised; rebuilding takes ownership.
    let mut req = unsafe { Vec::from_raw_parts(ptr, len, len) };
    let response = api::call(op, &req).into_bytes();
    scrub_stack();
    req.zeroize();
    drop(req);
    let n = response.len();
    RESULT.with(|r| {
        let mut r = r.borrow_mut();
        r.zeroize();
        *r = response;
    });
    n
}

/// Zeroes the shadow stack the operation just used: `ZeroizeOnDrop` wipes a
/// key where it is dropped, not the copies a move leaves in old frames. The
/// operation's frames sat where this one's array now is; 64 KiB is well
/// above the depth any operation reaches (argon2's blocks are on the heap).
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
