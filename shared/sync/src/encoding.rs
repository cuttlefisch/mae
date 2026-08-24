//! Encoding helpers for yrs updates over JSON-RPC transport.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use yrs::{updates::decoder::Decode, Doc, ReadTxn, Transact};

use crate::SyncError;

/// Encode binary update as base64 (for JSON-RPC transport).
pub fn update_to_base64(update: &[u8]) -> String {
    STANDARD.encode(update)
}

/// Decode base64 back to binary update.
pub fn base64_to_update(encoded: &str) -> Result<Vec<u8>, SyncError> {
    STANDARD
        .decode(encoded)
        .map_err(|e| SyncError::Encoding(format!("base64 decode: {e}")))
}

/// Encode state vector as base64.
pub fn state_vector_to_base64(sv: &[u8]) -> String {
    STANDARD.encode(sv)
}

/// Compute a diff: given a remote state vector, encode what this doc has that they don't.
pub fn encode_diff(doc: &Doc, remote_sv: &[u8]) -> Result<Vec<u8>, SyncError> {
    let sv = yrs::StateVector::decode_v1(remote_sv)
        .map_err(|e| SyncError::Encoding(format!("state vector decode: {e}")))?;
    let txn = doc.transact();
    Ok(txn.encode_state_as_update_v1(&sv))
}

/// Validate that bytes are a well-formed yrs update.
pub fn validate_update(bytes: &[u8]) -> Result<(), SyncError> {
    yrs::Update::decode_v1(bytes).map_err(|e| SyncError::Encoding(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::{updates::encoder::Encode, GetString, Text, Transact};

    #[test]
    fn base64_roundtrip() {
        let data = b"hello world binary \x00\x01\xff";
        let encoded = update_to_base64(data);
        let decoded = base64_to_update(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn encode_diff_produces_valid_update() {
        let doc_a = Doc::with_client_id(1);
        let doc_b = Doc::with_client_id(2);

        // A has some content
        {
            let text = doc_a.get_or_insert_text("t");
            let mut txn = doc_a.transact_mut();
            text.insert(&mut txn, 0, "hello");
        }

        // B is empty — get its state vector
        let sv_b = {
            let txn = doc_b.transact();
            txn.state_vector().encode_v1()
        };

        // Compute diff from A's perspective
        let diff = encode_diff(&doc_a, &sv_b).unwrap();
        assert!(!diff.is_empty());

        // Apply diff to B — should give B the content
        let update = yrs::Update::decode_v1(&diff).unwrap();
        {
            let mut txn = doc_b.transact_mut();
            txn.apply_update(update).unwrap();
        }

        let text = doc_b.get_or_insert_text("t");
        let txn = doc_b.transact();
        assert_eq!(text.get_string(&txn), "hello");
    }

    #[test]
    fn validate_update_rejects_garbage() {
        assert!(validate_update(b"not a valid update").is_err());
    }

    #[test]
    fn validate_update_accepts_valid() {
        let doc = Doc::new();
        let text = doc.get_or_insert_text("t");
        let update = {
            let mut txn = doc.transact_mut();
            text.insert(&mut txn, 0, "test");
            txn.encode_update_v1()
        };
        assert!(validate_update(&update).is_ok());
    }

    #[test]
    fn decode_empty_state_vector() {
        let result = yrs::StateVector::decode_v1(&[]);
        assert!(
            result.is_err(),
            "empty bytes should not decode as a valid StateVector"
        );
    }

    #[test]
    fn decode_truncated_update() {
        let doc = Doc::with_client_id(1);
        let text = doc.get_or_insert_text("t");
        let update = {
            let mut txn = doc.transact_mut();
            text.insert(&mut txn, 0, "truncation test");
            txn.encode_update_v1()
        };
        assert!(update.len() >= 2, "update must be long enough to truncate");
        let truncated = &update[..update.len() / 2];
        assert!(
            validate_update(truncated).is_err(),
            "truncated update should fail validation"
        );
    }

    #[test]
    fn encode_decode_large_state_vector() {
        let doc = Doc::new();
        // Create 100 distinct client IDs making edits by merging updates from
        // separate per-client docs into one doc.
        for client_id in 1u64..=100 {
            let client_doc = Doc::with_client_id(client_id);
            let text = client_doc.get_or_insert_text("shared");
            {
                let mut txn = client_doc.transact_mut();
                text.insert(&mut txn, 0, &format!("c{client_id} "));
            }
            // Encode the client's full state as an update and apply to the main doc.
            let client_update = {
                let txn = client_doc.transact();
                txn.encode_state_as_update_v1(&yrs::StateVector::default())
            };
            let update = yrs::Update::decode_v1(&client_update).unwrap();
            let mut txn = doc.transact_mut();
            txn.apply_update(update).unwrap();
        }

        // Encode state vector, round-trip through base64, decode back.
        let sv_bytes = {
            let txn = doc.transact();
            txn.state_vector().encode_v1()
        };
        assert!(!sv_bytes.is_empty());

        let encoded = state_vector_to_base64(&sv_bytes);
        let decoded_bytes = base64_to_update(&encoded).unwrap();
        assert_eq!(decoded_bytes, sv_bytes);

        // Verify the decoded bytes parse as a valid StateVector.
        let sv_decoded = yrs::StateVector::decode_v1(&decoded_bytes).unwrap();
        // The state vector should contain entries for all 100 client IDs.
        for client_id in 1u64..=100 {
            assert!(
                sv_decoded.get(&yrs::block::ClientID::new(client_id)) > 0,
                "state vector missing clock for client {client_id}"
            );
        }
    }

    #[test]
    fn validate_update_rejects_random_bytes() {
        // Deterministic pseudo-random bytes (LCG with fixed seed — no external deps).
        let mut state: u64 = 0xdeadbeef_cafebabe;
        let mut bytes = vec![0u8; 256];
        for b in bytes.iter_mut() {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *b = (state >> 33) as u8;
        }
        assert!(
            validate_update(&bytes).is_err(),
            "pseudo-random bytes should not be a valid yrs update"
        );
    }

    /// lib0 varint encoding of `n` as an unsigned integer: 7 bits per byte,
    /// high bit = "more bytes follow". Written out rather than pulled from a
    /// helper so the payload below is auditable by eye.
    fn varint(mut n: u64) -> Vec<u8> {
        let mut out = Vec::new();
        while n >= 0x80 {
            out.push((n as u8 & 0x7F) | 0x80);
            n >>= 7;
        }
        out.push(n as u8);
        out
    }

    /// A hostile state vector is FIVE BYTES and used to abort the whole process.
    ///
    /// `StateVector::decode` reads an attacker-controlled `u32` length prefix.
    /// Through yrs 0.27.3 it then called `HashMap::with_capacity_and_hasher(len, ..)`
    /// unguarded, so a varint declaring `u32::MAX` entries requested a ~100 GB
    /// allocation -> `handle_alloc_error` -> **`abort()`, non-unwinding**. Not a
    /// panic: `catch_unwind` could not have contained it.
    ///
    /// This reaches the daemon over `sync/diff`, which is a *read* request: it needs
    /// no write authority and bypasses the signature check, the ADR-023 epoch fence
    /// and the message-size cap by construction (a 5-byte payload is under any cap).
    ///
    /// Fixed upstream in yrs 0.27.4 (y-crdt PR #639) by a fallible `try_reserve`.
    /// This test is the regression guard on that pin: if a future bump regresses it,
    /// the test process aborts rather than failing, which is itself the signal.
    #[test]
    fn hostile_state_vector_length_prefix_is_a_decode_error_not_an_abort() {
        let doc = Doc::with_client_id(1);
        for declared in [u32::MAX as u64, u32::MAX as u64 / 2, 1 << 24] {
            let bomb = varint(declared);
            assert!(
                bomb.len() <= 5,
                "the whole point is that this is tiny: {} bytes",
                bomb.len()
            );
            let err = encode_diff(&doc, &bomb);
            assert!(
                err.is_err(),
                "a state vector declaring {declared} entries must be refused, not allocated"
            );
        }
    }

    /// The same length-prefix class on the *update* path, which is what
    /// `validate_update` gates before a WAL append. `Any::decode`'s nested map and
    /// array sites had the same unguarded `with_capacity` through 0.27.3.
    ///
    /// Truncated-but-well-formed prefixes matter more than `b"garbage"`: garbage
    /// fails at byte 0 and proves nothing about what a *plausible* payload does.
    #[test]
    fn truncated_and_overlong_updates_are_refused_without_aborting() {
        // A valid update, then every truncation of it. Each must be refused
        // cleanly -- no abort, no panic, no silent Ok.
        let doc = Doc::with_client_id(1);
        let text = doc.get_or_insert_text("t");
        let mut txn = doc.transact_mut();
        text.insert(&mut txn, 0, "the quick brown fox");
        let valid = txn.encode_update_v1();
        drop(txn);
        assert!(
            validate_update(&valid).is_ok(),
            "control: the update is valid"
        );

        for cut in 1..valid.len() {
            let _ = validate_update(&valid[..cut]);
        }

        // A declared-huge block count with no blocks behind it.
        let mut bomb = varint(u32::MAX as u64);
        bomb.extend_from_slice(&[0x00]);
        assert!(
            validate_update(&bomb).is_err(),
            "an update declaring u32::MAX blocks must be refused"
        );
    }
}
