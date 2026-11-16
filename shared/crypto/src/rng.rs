//! Where every random byte comes from.
//!
//! Natively: rand's thread RNG (seeded from the OS), as before this code
//! moved out of mae-sync. On wasm32: the module's one import,
//! `mae_crypto_host.random_fill(ptr, len) -> u32` (0 = filled), which the
//! loader backs with `crypto.getRandomValues`. There is no other source and
//! no fallback: a host that cannot fill fails the operation.
//!
//! @ai-caution: [security] Never add a fallback (a fixed seed, a clock, a
//! counter) for when the host cannot supply randomness. A member key, a
//! salt or a nonce drawn from a predictable source is worse than an error.

/// The host could not supply random bytes, or supplied all zeros.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandomUnavailable;

impl std::fmt::Display for RandomUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no secure random source is available")
    }
}

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "mae_crypto_host")]
unsafe extern "C" {
    /// Fills `len` bytes at `ptr` from the host's CSPRNG; 0 on success.
    fn random_fill(ptr: *mut u8, len: usize) -> u32;
}

/// Fills `dest` with random bytes, or fails.
///
/// A fill of 16 bytes or more that comes back all zeros, or identical to the
/// previous such fill (on this thread; the WASM module has one), is refused:
/// odds 2^-128 for a real source. The first is what a host stub that reports
/// success without writing produces (`dest` starts zeroed here); the second,
/// a host that returns a constant or replays a buffer. A source that varies
/// but is predictable is not detectable and not detected.
pub fn fill(dest: &mut [u8]) -> Result<(), RandomUnavailable> {
    fill_from(dest, source)
}

fn fill_from(
    dest: &mut [u8],
    source: impl FnOnce(&mut [u8]) -> Result<(), RandomUnavailable>,
) -> Result<(), RandomUnavailable> {
    dest.fill(0);
    let filled = source(dest).is_ok();
    if !filled || (dest.len() >= 16 && (dest.iter().all(|b| *b == 0) || repeats(dest))) {
        dest.fill(0);
        return Err(RandomUnavailable);
    }
    Ok(())
}

thread_local! {
    /// A digest of the last fill of 16 bytes or more: a digest, so a seed
    /// drawn here is not kept in memory as itself.
    static LAST: std::cell::Cell<[u8; 16]> = const { std::cell::Cell::new([0u8; 16]) };
}

/// Whether `fill` is identical to the previous fill of 16 bytes or more,
/// recording it as the previous one.
fn repeats(fill: &[u8]) -> bool {
    use sha2::{Digest, Sha256};
    let mut d = [0u8; 16];
    d.copy_from_slice(&Sha256::digest(fill)[..16]);
    LAST.with(|last| last.replace(d) == d)
}

#[cfg(target_arch = "wasm32")]
fn source(dest: &mut [u8]) -> Result<(), RandomUnavailable> {
    // SAFETY: `dest` is a writable region of `dest.len()` bytes; the host
    // writes at most that range (it is handed exactly that view).
    if unsafe { random_fill(dest.as_mut_ptr(), dest.len()) } == 0 {
        Ok(())
    } else {
        Err(RandomUnavailable)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn source(dest: &mut [u8]) -> Result<(), RandomUnavailable> {
    use rand::Rng;
    rand::rng().fill_bytes(dest);
    Ok(())
}

/// `N` random bytes for an operation whose signature is infallible
/// (`content_crypto::encrypt` and friends). Without a source it panics,
/// which in the WASM module (`panic = "abort"`) traps: the operation does
/// not happen, and nothing is produced with a predictable nonce or key.
pub(crate) fn array<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    if fill(&mut out).is_err() {
        panic!("no secure random source");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_are_not_zero_and_not_repeated() {
        let a: [u8; 32] = array();
        let b: [u8; 32] = array();
        assert_ne!(a, [0u8; 32]);
        assert_ne!(a, b);
    }

    /// A host that returns the same bytes twice is refused the second time;
    /// a varying one is not.
    #[test]
    fn a_fill_identical_to_the_previous_one_is_refused() {
        let constant = |d: &mut [u8]| {
            d.fill(0x5a);
            Ok(())
        };
        let mut a = [0u8; 32];
        assert_eq!(fill_from(&mut a, constant), Ok(()));
        let mut b = [0u8; 32];
        assert_eq!(fill_from(&mut b, constant), Err(RandomUnavailable));
        assert_eq!(b, [0u8; 32], "the repeated bytes are not handed on");
        for n in 1..=4u8 {
            let mut c = [0u8; 16];
            assert_eq!(
                fill_from(&mut c, |d| {
                    d.fill(n);
                    Ok(())
                }),
                Ok(())
            );
        }
        // Short fills are not compared (a 4-byte value may repeat).
        let mut s = [0u8; 4];
        assert_eq!(
            fill_from(&mut s, |d| {
                d.fill(7);
                Ok(())
            }),
            Ok(())
        );
        assert_eq!(
            fill_from(&mut s, |d| {
                d.fill(7);
                Ok(())
            }),
            Ok(())
        );
    }

    /// A host that reports success without writing, or that fails, gives
    /// nothing usable: both are errors, not zeros handed on.
    #[test]
    fn a_source_that_writes_nothing_or_fails_is_refused() {
        let mut buf = [7u8; 32];
        assert_eq!(fill_from(&mut buf, |_| Ok(())), Err(RandomUnavailable));
        assert_eq!(buf, [0u8; 32], "a refused fill leaves nothing behind");
        assert_eq!(
            fill_from(&mut buf, |_| Err(RandomUnavailable)),
            Err(RandomUnavailable)
        );
        // A source that writes and then reports failure is a failure too,
        // and what it wrote is not handed on.
        assert_eq!(
            fill_from(&mut buf, |d| {
                d.fill(9);
                Err(RandomUnavailable)
            }),
            Err(RandomUnavailable)
        );
        assert_eq!(buf, [0u8; 32]);
        // Short fills (below 16 bytes) may legitimately be all zero.
        let mut short = [0u8; 4];
        assert_eq!(fill_from(&mut short, |_| Ok(())), Ok(()));
        // A real fill passes.
        assert_eq!(
            fill_from(&mut buf, |d| {
                d.fill(9);
                Ok(())
            }),
            Ok(())
        );
    }
}
