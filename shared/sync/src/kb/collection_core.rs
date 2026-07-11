//! `KbCollectionDoc`: creation and basic CRUD — construction, node manifest
//! (add/remove/list), and legacy member add/remove. See `collection_roles.rs`
//! for ownership/RBAC, `collection_oplog.rs` for the signed membership
//! op-log, and `collection_crypto.rs` for E2E crypto authoring.

use yrs::updates::encoder::Encode;
use yrs::{Array, ArrayPrelim, Map, MapPrelim, Out, ReadTxn, Transact};

use super::*;
use crate::text::{new_doc, new_doc_with_client_id};

impl KbCollectionDoc {
    /// Create a new collection document.
    pub fn new(name: &str, creator: &str) -> Self {
        let doc = new_doc();
        {
            let root = doc.get_or_insert_map(COLLECTION_MAP);
            let mut txn = doc.transact_mut();
            root.insert(&mut txn, COLL_NAME_KEY, name);
            root.insert(&mut txn, COLL_CREATOR_KEY, creator);
            root.insert(&mut txn, COLL_NODES_KEY, MapPrelim::default());
            let members = root.insert(&mut txn, COLL_MEMBERS_KEY, ArrayPrelim::default());
            members.push_back(&mut txn, creator);
        }
        Self { doc }
    }

    /// Create a new collection document with a specific client ID.
    pub fn new_with_client_id(name: &str, creator: &str, client_id: u64) -> Self {
        let doc = new_doc_with_client_id(client_id);
        {
            let root = doc.get_or_insert_map(COLLECTION_MAP);
            let mut txn = doc.transact_mut();
            root.insert(&mut txn, COLL_NAME_KEY, name);
            root.insert(&mut txn, COLL_CREATOR_KEY, creator);
            root.insert(&mut txn, COLL_NODES_KEY, MapPrelim::default());
            let members = root.insert(&mut txn, COLL_MEMBERS_KEY, ArrayPrelim::default());
            members.push_back(&mut txn, creator);
        }
        Self { doc }
    }

    /// Load from encoded bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SyncError> {
        let doc = new_doc();
        let update =
            yrs::Update::decode_v1(bytes).map_err(|e| SyncError::Encoding(e.to_string()))?;
        {
            let mut txn = doc.transact_mut();
            txn.apply_update(update)
                .map_err(|e| SyncError::Encoding(e.to_string()))?;
        }
        Ok(Self { doc })
    }

    /// Encode full state.
    pub fn encode_state(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&yrs::StateVector::default())
    }

    /// State vector for sync.
    pub fn state_vector(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.state_vector().encode_v1()
    }

    /// Apply a remote update.
    pub fn apply_update(&mut self, update: &[u8]) -> Result<(), SyncError> {
        let update =
            yrs::Update::decode_v1(update).map_err(|e| SyncError::Encoding(e.to_string()))?;
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update)
            .map_err(|e| SyncError::Encoding(e.to_string()))?;
        Ok(())
    }

    /// Get KB name.
    pub fn name(&self) -> String {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_NAME_KEY)
            .map(|v| v.to_string(&txn))
            .unwrap_or_default()
    }

    /// Get creator name.
    pub fn creator(&self) -> String {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_CREATOR_KEY)
            .map(|v| v.to_string(&txn))
            .unwrap_or_default()
    }

    /// Add a node to the collection manifest. Returns encoded update.
    pub fn add_node(&mut self, node_id: &str, title: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YMap(nodes)) = root.get(&txn, COLL_NODES_KEY) {
            nodes.insert(&mut txn, node_id, title);
        }
        txn.encode_update_v1()
    }

    /// #156 F5: blank every cleartext node title in the manifest in ONE transaction —
    /// used when E2e is enabled on an EXISTING KB so the key-blind daemon stops holding
    /// plaintext titles (the real title lives encrypted in the node op-set; the manifest
    /// only needs the `node_id`). Returns the encoded delta, or an **empty `Vec`** when
    /// there was nothing to blank (no nodes, or all titles already empty) — idempotent.
    pub fn blank_node_titles_delta(&mut self) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        let Some(Out::YMap(nodes)) = root.get(&txn, COLL_NODES_KEY) else {
            return Vec::new();
        };
        let mut with_titles: Vec<String> = Vec::new();
        for (k, v) in nodes.iter(&txn) {
            if !v.to_string(&txn).is_empty() {
                with_titles.push(k.to_string());
            }
        }
        if with_titles.is_empty() {
            return Vec::new();
        }
        for id in &with_titles {
            nodes.insert(&mut txn, id.as_str(), "");
        }
        txn.encode_update_v1()
    }

    /// Remove a node from the collection manifest. Returns encoded update.
    pub fn remove_node(&mut self, node_id: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YMap(nodes)) = root.get(&txn, COLL_NODES_KEY) {
            nodes.remove(&mut txn, node_id);
        }
        txn.encode_update_v1()
    }

    /// List all nodes in the collection: (node_id, title) pairs.
    pub fn list_nodes(&self) -> Vec<(String, String)> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        match root.get(&txn, COLL_NODES_KEY) {
            Some(Out::YMap(nodes)) => nodes
                .iter(&txn)
                .map(|(k, v)| (k.to_string(), v.to_string(&txn)))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Get number of nodes in the collection.
    pub fn node_count(&self) -> u32 {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        match root.get(&txn, COLL_NODES_KEY) {
            Some(Out::YMap(nodes)) => nodes.len(&txn),
            _ => 0,
        }
    }

    /// Re-stamp the authoritative creator and ensure they are a member.
    /// Used by the daemon to bind a shared collection to the AUTHENTICATED peer
    /// identity (ADR-017 strict binding), overriding the client-supplied creator.
    /// Returns the encoded update.
    pub fn set_creator(&mut self, creator: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        root.insert(&mut txn, COLL_CREATOR_KEY, creator);
        if let Some(Out::YArray(members)) = root.get(&txn, COLL_MEMBERS_KEY) {
            let already = members.iter(&txn).any(|v| v.to_string(&txn) == creator);
            if !already {
                members.push_back(&mut txn, creator);
            }
        }
        txn.encode_update_v1()
    }

    /// Add a member to the collection. Returns encoded update.
    pub fn add_member(&mut self, user_name: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YArray(members)) = root.get(&txn, COLL_MEMBERS_KEY) {
            // Check for duplicates
            let already = members.iter(&txn).any(|v| v.to_string(&txn) == user_name);
            if !already {
                members.push_back(&mut txn, user_name);
            }
        }
        txn.encode_update_v1()
    }

    /// Remove a member from the collection. Returns encoded update.
    pub fn remove_member(&mut self, user_name: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YArray(members)) = root.get(&txn, COLL_MEMBERS_KEY) {
            let idx = members
                .iter(&txn)
                .position(|v| v.to_string(&txn) == user_name);
            if let Some(idx) = idx {
                members.remove(&mut txn, idx as u32);
            }
        }
        txn.encode_update_v1()
    }

    /// List all members.
    pub fn members(&self) -> Vec<String> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        match root.get(&txn, COLL_MEMBERS_KEY) {
            Some(Out::YArray(arr)) => arr.iter(&txn).map(|v| v.to_string(&txn)).collect(),
            _ => Vec::new(),
        }
    }
}
