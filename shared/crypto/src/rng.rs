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
/// A fill of 16 bytes or more that comes back all zeros is refused (odds
/// 2^-128 for a real source): it is what a host stub that reports success
/// without writing produces, and `dest` starts zeroed here. A source that
/// writes constant non-zero bytes is not detectable and not detected.
pub fn fill(dest: &mut [u8]) -> Result<(), RandomUnavailable> {
    fill_from(dest, source)
}

fn fill_from(
    dest: &mut [u8],
    source: impl FnOnce(&mut [u8]) -> Result<(), RandomUnavailable>,
) -> Result<(), RandomUnavailable> {
    dest.fill(0);
    let filled = source(dest).is_ok();
    if !filled || (dest.len() >= 16 && dest.iter().all(|b| *b == 0)) {
        dest.fill(0);
        return Err(RandomUnavailable);
    }
    Ok(())
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
