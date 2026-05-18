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
}
