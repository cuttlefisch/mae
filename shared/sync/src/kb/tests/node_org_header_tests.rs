//! The `org_header` field (property order, title and filetags line presence)
//! and the explicit creation time a seed gives a node.

use super::*;

/// Six keys in an order no sort produces, one of them mixed-case: what the
/// drawer of a seeded page looks like.
fn header() -> OrgHeader {
    OrgHeader {
        property_order: ["ID", "zeta", "KIND", "Alpha", "mIxEd_Case", "beta"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        has_title_line: true,
        has_filetags_line: false,
    }
}

#[test]
fn a_node_without_a_header_reads_none_and_reading_writes_nothing() {
    let node = KbNodeDoc::new("n", "T", "B", &[]);
    let before = node.state_vector();
    assert_eq!(node.org_header(), None);
    assert_eq!(node.materialize().org_header, None);
    assert_eq!(node.state_vector(), before, "a read must not author an op");
}

#[test]
fn the_header_round_trips_through_bytes_with_order_and_key_case() {
    for (has_title_line, has_filetags_line) in
        [(true, true), (false, false), (true, false), (false, true)]
    {
        let mut node = KbNodeDoc::new("n", "", "B", &[]);
        let h = OrgHeader {
            has_title_line,
            has_filetags_line,
            ..header()
        };
        let _ = node.set_org_header(&h);
        let back = KbNodeDoc::from_bytes(&node.encode()).unwrap();
        assert_eq!(back.org_header(), Some(h));
    }
}

/// A peer that receives the update sees the same header: the field crosses the
/// wire, which is what lets any peer export the KB.
#[test]
fn the_header_reaches_a_peer_through_an_update() {
    let mut a = KbNodeDoc::new("n", "T", "B", &[]);
    let mut b = KbNodeDoc::from_bytes(&a.encode()).unwrap();
    let update = a.set_org_header(&header());
    b.apply_update(&update).unwrap();
    assert_eq!(b.org_header(), Some(header()));
}

#[test]
fn an_unchanged_header_writes_no_op() {
    let mut node = KbNodeDoc::new("n", "T", "B", &[]);
    let _ = node.set_org_header(&header());
    let sv = node.state_vector();
    let _ = node.set_org_header(&header());
    assert_eq!(node.state_vector(), sv);
}

/// Kills: leaving `org_header` out of `version()`. A reader polling `list`
/// must see a change of order or of a line flag as a change.
#[test]
fn version_changes_with_the_order_and_with_each_line_flag() {
    let mut node = KbNodeDoc::new("n", "T", "B", &[]);
    let bare = node.version();
    let _ = node.set_org_header(&header());
    let with = node.version();
    assert_ne!(bare, with);
    let mut swapped = header();
    swapped.property_order.swap(1, 2);
    let mut titled = header();
    titled.has_title_line = false;
    let mut tagged = header();
    tagged.has_filetags_line = true;
    for other in [swapped, titled, tagged] {
        let _ = node.set_org_header(&other);
        assert_ne!(node.version(), with, "{other:?}");
    }
}

/// Kills: `reborn` not copying the header (rebirth would silently drop the
/// order an export needs), and `rebirth_hash` not covering it.
#[test]
fn rebirth_keeps_the_header_and_its_hash_covers_it() {
    let mut node = KbNodeDoc::new("n", "T", "B", &[]);
    let plain_hash = node.rebirth_hash();
    let _ = node.set_org_header(&header());
    assert_ne!(node.rebirth_hash(), plain_hash);
    let reborn = node.reborn(7);
    assert_eq!(reborn.org_header(), Some(header()));
    assert_eq!(reborn.rebirth_hash(), node.rebirth_hash());
}

/// Kills: `new_created_at` stamping the clock instead of the given time.
#[test]
fn a_given_creation_time_is_the_one_stamped() {
    for t in [0, 1_234_567_890, 1_600_000_000] {
        let node = KbNodeDoc::new_created_at("n", "T", "B", &[], t);
        assert_eq!(node.created_at(), Some(t));
        let back = KbNodeDoc::from_bytes(&node.encode()).unwrap();
        assert_eq!(back.created_at(), Some(t));
    }
}
