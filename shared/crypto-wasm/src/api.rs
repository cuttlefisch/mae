//! The operations, as bytes in and bytes out. Every key and public key is
//! 32 bytes at the head of the request; the rest is the operand.
//!
//! | op | request                    | response            |
//! |----|----------------------------|---------------------|
//! | 1  | key ‖ plaintext            | nonce ‖ ct ‖ tag    |
//! | 2  | key ‖ sealed               | plaintext           |
//! | 3  | content key ‖ wrap pub     | eph pub ‖ sealed    |
//! | 4  | ed25519 seed ‖ wrapped     | content key         |
//! | 5  | ed25519 seed               | wrap pub            |
//!
//! Errors are negative codes, the same on both sides of the interop test.

pub const SEAL: u32 = 1;
pub const OPEN: u32 = 2;
pub const WRAP: u32 = 3;
pub const UNWRAP: u32 = 4;
pub const WRAP_PUBLIC: u32 = 5;

pub const E_DECRYPT: i32 = -1;
pub const E_MALFORMED: i32 = -2;
pub const E_BAD_KEY: i32 = -3;
pub const E_SHORT: i32 = -4;
pub const E_OP: i32 = -5;

/// Defines `call(op, request) -> Result<Vec<u8>, i32>` over a module with
/// `content_crypto`'s API. Used twice: over this crate's `#[path]` copy (the
/// WASM module) and over `mae_sync::content_crypto` (the native interop
/// binary), so the two sides cannot drift in framing.
#[macro_export]
macro_rules! define_call {
    ($($m:ident)::+) => {
        pub fn call(op: u32, req: &[u8]) -> Result<Vec<u8>, i32> {
            use $crate::api::*;
            use $($m)::+ as cc;
            use zeroize::Zeroize;
            fn err(e: cc::CryptoError) -> i32 {
                match e {
                    cc::CryptoError::Decrypt => E_DECRYPT,
                    cc::CryptoError::Malformed => E_MALFORMED,
                    cc::CryptoError::BadKey => E_BAD_KEY,
                }
            }
            if !(SEAL..=WRAP_PUBLIC).contains(&op) {
                return Err(E_OP);
            }
            if req.len() < 32 {
                return Err(E_SHORT);
            }
            let (head, rest) = req.split_at(32);
            let mut k: [u8; 32] = head.try_into().map_err(|_| E_SHORT)?;
            let out = match op {
                SEAL => Ok(cc::encrypt(&cc::ContentKey::from_bytes(k), rest)),
                OPEN => cc::decrypt(&cc::ContentKey::from_bytes(k), rest).map_err(err),
                WRAP => match <[u8; 32]>::try_from(rest) {
                    Ok(public) => {
                        cc::wrap_to_member(&cc::ContentKey::from_bytes(k), &public).map_err(err)
                    }
                    Err(_) => Err(E_SHORT),
                },
                UNWRAP => cc::unwrap_as_member(rest, &k)
                    .map(|ck| ck.as_bytes().to_vec())
                    .map_err(err),
                _ => {
                    if rest.is_empty() {
                        Ok(cc::wrap_public_for(&k).to_vec())
                    } else {
                        Err(E_SHORT)
                    }
                }
            };
            k.zeroize();
            out
        }
    };
}
