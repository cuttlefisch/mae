//! mae-org's WASM facade (ADR-112 D3, D5).
//! @stability: experimental
//!
//! The ABI is six exported functions over linear memory, no bindings
//! generator (see `js/mae-org.mjs`, which drives it):
//!
//! 1. `mae_org_alloc(len) -> ptr`: the caller reserves `len` bytes and
//!    writes the request's UTF-8 there;
//! 2. `mae_org_call(op, ptr, len) -> len`: runs operation `op`
//!    ([`api::Op`]) on those bytes, frees them, and keeps the response; its
//!    length is returned;
//! 3. `mae_org_result_ptr() -> ptr`: where the response is; the caller
//!    copies `len` bytes out;
//! 4. `mae_org_result_free()`: drops the response;
//! 5. `mae_org_live_bytes() -> n`: bytes allocated and not yet freed (a
//!    counting allocator), for the leak test;
//! 6. `mae_org_seed(k0, k1)`: the keys of the module's hash maps, 128 bits
//!    from the host's secure randomness. Called once per instance, before
//!    any operation (every one but `version` refuses to run unseeded).
//!
//! The module is single-threaded and not re-entrant (no callback crosses
//! it), so one response slot suffices. All `unsafe` is in this file, each
//! block with its argument.

pub mod api;
mod custom_id;
mod headline_properties;
pub mod json;
pub mod syntax;

#[cfg(target_arch = "wasm32")]
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The system allocator, counting live bytes. Only the WASM module counts:
/// a native build (its tests) keeps the default allocator.
#[cfg(target_arch = "wasm32")]
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method forwards to `System` with the caller's arguments
// unchanged, so `System`'s guarantees hold; the counter is a side effect.
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

thread_local! {
    static RESULT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Reserves `len` bytes for a request; the pointer goes to `mae_org_call`.
#[no_mangle]
pub extern "C" fn mae_org_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Runs `op` on the `len` request bytes at `ptr` (from `mae_org_alloc(len)`,
/// written by the caller), frees them, and returns the response's length.
///
/// # Safety
/// `ptr` must come from `mae_org_alloc(len)` with this same `len`, hold
/// `len` initialised bytes, and not be used again.
#[no_mangle]
pub unsafe extern "C" fn mae_org_call(op: u32, ptr: *mut u8, len: usize) -> usize {
    // SAFETY: by the contract above, `ptr` is a `Vec<u8>` allocation of
    // capacity `len` whose `len` bytes the caller initialised; rebuilding
    // the Vec takes ownership back, and dropping it frees the request.
    let bytes = unsafe { Vec::from_raw_parts(ptr, len, len) };
    let request = String::from_utf8_lossy(&bytes);
    let response = api::call(op, &request).into_bytes();
    let n = response.len();
    RESULT.with(|r| *r.borrow_mut() = response);
    n
}

/// Where the last response is (valid until the next call or free).
#[no_mangle]
pub extern "C" fn mae_org_result_ptr() -> *const u8 {
    RESULT.with(|r| r.borrow().as_ptr())
}

/// Drops the last response.
#[no_mangle]
pub extern "C" fn mae_org_result_free() {
    RESULT.with(|r| *r.borrow_mut() = Vec::new());
}

/// Seeds the keys of mae-org's hash maps (`mae_org::seeded`) with 128 bits
/// the host draws from a secure source (the loader: `crypto.getRandomValues`).
/// Every operation but `version` refuses to run until this is called: the
/// module's own source of randomness is fixed, and maps keyed by document
/// text must not be predictable.
#[no_mangle]
pub extern "C" fn mae_org_seed(k0: u64, k1: u64) {
    mae_org::seeded::seed(k0, k1);
}

/// Bytes allocated and not freed (0 natively, where nothing counts).
#[no_mangle]
pub extern "C" fn mae_org_live_bytes() -> usize {
    LIVE.load(Ordering::Relaxed)
}
