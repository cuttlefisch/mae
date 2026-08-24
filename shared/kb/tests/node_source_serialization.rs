//! `NodeSource`'s serialized form is a compatibility surface (#710).
//!
//! These strings are persisted in the Cozo row AND now cross the wire in the
//! node CRDT payload, so renaming one silently reclassifies existing nodes and
//! drops provenance arriving from a peer on an older build.

/// An unrecognised provenance from a newer peer must not be coerced into a known
/// variant. Same reasoning: a peer knowing something this build does not is not
/// permission to guess.
#[test]
fn an_unknown_provenance_value_is_not_coerced() {
    use mae_kb::NodeSource;
    assert_eq!(NodeSource::from_str_opt("seed"), Some(NodeSource::Seed));
    assert_eq!(
        NodeSource::from_str_opt("some_future_variant"),
        None,
        "an unknown provenance must be reported as unknown, not guessed"
    );
    // Round-trip every variant through its serialized form, so the persisted
    // strings cannot drift from the parser.
    for v in [
        NodeSource::Seed,
        NodeSource::UserOrg,
        NodeSource::Manual,
        NodeSource::Federation,
        NodeSource::Promoted,
    ] {
        assert_eq!(
            NodeSource::from_str_opt(v.as_str()),
            Some(v),
            "'{}' does not parse back to itself",
            v.as_str()
        );
    }
}
