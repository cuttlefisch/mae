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
}
