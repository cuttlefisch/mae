//! A collection's node list and member list come out in one order, whatever
//! decode of the same bytes reads them. A yrs map iterates in its `HashMap`'s
//! order, seeded per instance: measured before the fix, 20 decodes of one
//! 40-node collection listed it in 20 different orders, so `kb/query.search`
//! (which examines the first `max_scan_nodes`) searched a different subset on
//! every request.

use mae_sync::kb::{KbCollectionDoc, Role};

/// The collection, built in an order that is neither sorted nor reversed, so
/// insertion order cannot pass for key order.
fn collection() -> Vec<u8> {
    let mut c = KbCollectionDoc::new_owned("kb-order", "SHA256:owner", "owner");
    for i in (0..40).map(|i| (i * 17) % 40) {
        c.add_node(&format!("node-{i:02}"), &format!("Title {i}"));
        c.upsert_member(&format!("SHA256:member-{i:02}"), "m", Role::Viewer);
    }
    c.encode_state()
}

#[test]
fn every_decode_lists_nodes_and_members_in_key_order() {
    let bytes = collection();
    for _ in 0..20 {
        let d = KbCollectionDoc::from_bytes(&bytes).unwrap();
        let nodes: Vec<String> = d.list_nodes().into_iter().map(|(id, _)| id).collect();
        let mut sorted = nodes.clone();
        sorted.sort();
        assert_eq!(nodes.len(), 40);
        assert_eq!(nodes, sorted, "nodes in node-id order");

        let members: Vec<String> = d
            .member_roles()
            .into_iter()
            .map(|m| m.fingerprint)
            .collect();
        let mut sorted = members.clone();
        sorted.sort();
        assert!(members.len() >= 40);
        assert_eq!(members, sorted, "members in fingerprint order");
    }
}
