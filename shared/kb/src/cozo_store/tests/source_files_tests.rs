use super::*;

#[test]
fn record_source_file_retracts_ids_dropped_from_a_still_existing_path() {
    // Full-directory `:kb-reimport`'s counterpart to the watcher-path fix:
    // a file's id-set can change (in-place `:ID:` rename) without the file
    // ever being deleted, so deletion-detection keyed on "path vanished"
    // (federation.rs) never fires — record_source_file itself must diff
    // old vs. new ids and retract the difference.
    let (_tmp, store) = make_store();
    let old_node = Node::new("user:t-jenkinsp", "jenkinsp", NodeKind::Note, "Jenkins");
    store.insert_node(&old_node).unwrap();
    store
        .record_source_file("jenkinsp.org", "hash1", 1, &["user:t-jenkinsp".to_string()])
        .unwrap();
    assert!(store.get_node("user:t-jenkinsp").unwrap().is_some());

    // Same path, renamed id — as if the file's :ID: was hand-edited and the
    // directory got re-walked without the path itself ever changing.
    let new_node = Node::new("user:t-jenkins", "jenkins", NodeKind::Note, "Jenkins");
    store.insert_node(&new_node).unwrap();
    store
        .record_source_file("jenkinsp.org", "hash2", 2, &["user:t-jenkins".to_string()])
        .unwrap();

    assert!(
        store.get_node("user:t-jenkinsp").unwrap().is_none(),
        "the old id must be retracted once the file no longer produces it"
    );
    assert!(
        store.get_node("user:t-jenkins").unwrap().is_some(),
        "the current id must remain"
    );
    assert_eq!(
        store.get_source_file_node_ids("jenkinsp.org").unwrap(),
        vec!["user:t-jenkins".to_string()]
    );
}
