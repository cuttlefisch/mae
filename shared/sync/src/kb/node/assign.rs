//! Setting a node's whole content to a target, through each field's own
//! setter: the daemon's form of ADR-092's one write path, for a writer that
//! holds a target [`MaterializedNode`] rather than an editor `Node` (a seed
//! update today; ADR-116 D6's rollback is the same operation).
//!
//! Text fields go through `reconcile_text_ref` (a character-level diff, never a
//! wholesale replace), properties per key, and every scalar only when it
//! differs, so an unchanged field authors no operation and peers converge on
//! the result like any edit.
//!
//! @ai-caution: [crdt] Lists (tags, aliases, links) are diffed by value first,
//! which is what concurrent editors need. Their ORDER is content for an
//! `org-literal-v1` node (the exporter writes `#+filetags:` in tag order), so
//! when the by-value diff leaves the order different from the target the list
//! is cleared and refilled. That is a wholesale rewrite of ONE list, made only
//! on a reorder; a writer that cannot accept it for a shared document must not
//! call this.

use super::super::MaterializedNode;
use super::KbNodeDoc;

impl KbNodeDoc {
    /// Make this document's content `target` and return the update that does
    /// it (only the operations this call authored). `id` and `created_at` are
    /// facts about the node, not content a write changes: both are left as
    /// they are. An absent `org_header` in `target` leaves the recorded one.
    #[must_use = "dropping this update silently prevents the content change from syncing to peers"]
    pub fn assign(&mut self, target: &MaterializedNode) -> Vec<u8> {
        let before = self.state_vector();
        if self.title() != target.title {
            let _ = self.set_title(&target.title);
        }
        if self.body() != target.body {
            let _ = self.set_body(&target.body);
        }
        self.assign_tags(&target.tags);
        self.assign_aliases(&target.aliases);
        self.assign_links(&target.links);
        let _ = self.set_kind(target.kind.as_deref());
        let _ = self.set_todo_state(target.todo_state.as_deref());
        let _ = self.set_priority(target.priority.as_deref());
        let _ = self.set_source_version(target.source_version);
        let _ = self.set_source(target.source.as_deref());
        let _ = self.set_properties(&target.properties);
        if let Some(header) = &target.org_header {
            let _ = self.set_org_header(header);
        }
        self.encode_diff(&before).unwrap_or_default()
    }

    fn assign_tags(&mut self, tags: &[String]) {
        let _ = self.set_tags(tags);
        if self.tags() != tags {
            let _ = self.set_tags(&[]);
            let _ = self.set_tags(tags);
        }
    }

    fn assign_aliases(&mut self, aliases: &[String]) {
        let _ = self.set_aliases(aliases);
        if self.aliases() != aliases {
            let _ = self.set_aliases(&[]);
            let _ = self.set_aliases(aliases);
        }
    }

    /// Links have no list setter: remove every one, then add the target's,
    /// and only when they differ at all.
    fn assign_links(&mut self, links: &[String]) {
        if self.links() == links {
            return;
        }
        for l in self.links() {
            let _ = self.remove_link(&l);
        }
        for l in links {
            let _ = self.add_link(l);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::super::super::OrgHeader;
    use super::*;

    fn doc() -> KbNodeDoc {
        let mut d = KbNodeDoc::new_created_at("n", "Old", "line one\nline two\n", &[], 7);
        let _ = d.set_tags(&["a".into(), "b".into(), "c".into()]);
        let _ = d.set_properties(&HashMap::from([("ID".into(), "n".into())]));
        let _ = d.add_link("x");
        let _ = d.set_todo_state(Some("TODO"));
        d
    }

    fn target(tags: &[&str]) -> MaterializedNode {
        let mut t = doc().materialize();
        t.title = "New".into();
        t.body = "line one\nline 2\nline three\n".into();
        t.tags = tags.iter().map(|s| s.to_string()).collect();
        t.links = Vec::new();
        t.todo_state = None;
        t.kind = Some("Concept".into());
        t.aliases = vec!["al".into()];
        t.properties = HashMap::from([("ID".into(), "n".into()), ("K".into(), "v".into())]);
        t.org_header = Some(OrgHeader {
            property_order: vec!["ID".into(), "K".into()],
            has_title_line: true,
            has_filetags_line: false,
        });
        t
    }

    /// Every field reaches the target, including a reordered tag list, and a
    /// peer holding the old document converges by applying the update alone.
    /// Kills: dropping any field, or diffing tags by value only (which keeps
    /// the old order).
    #[test]
    fn assign_reaches_the_target_and_a_peer_converges() {
        for tags in [&["c", "a"][..], &["b", "a", "c"], &[], &["z", "a"]] {
            let mut d = doc();
            let mut peer = KbNodeDoc::from_bytes(&d.encode()).unwrap();
            let t = target(tags);
            let update = d.assign(&t);
            assert_eq!(d.materialize(), t, "{tags:?}");
            peer.apply_update(&update).unwrap();
            assert_eq!(peer.materialize(), t, "{tags:?}");
        }
    }

    /// The body changes by a character-level diff: the unchanged first line
    /// keeps its CRDT items, so the update is far smaller than the text.
    #[test]
    fn text_is_diffed_not_replaced() {
        let mut d = doc();
        let mut t = d.materialize();
        t.body = format!("{}x\n", "long unchanged prefix ".repeat(200));
        let _ = d.assign(&t);
        let mut again = d.materialize();
        again.body.push_str("y\n");
        let update = d.assign(&again);
        assert!(update.len() < 200, "{} bytes", update.len());
    }

    /// Assigning the content a document already has authors nothing.
    #[test]
    fn assigning_the_same_content_is_a_no_op() {
        let mut d = doc();
        let sv = d.state_vector();
        let same = d.materialize();
        let _ = d.assign(&same);
        assert_eq!(d.state_vector(), sv);
    }

    /// `created_at` and `id` are never moved.
    #[test]
    fn identity_and_creation_time_stay() {
        let mut d = doc();
        let mut t = target(&["a"]);
        t.created_at = Some(99);
        t.id = "other".into();
        let _ = d.assign(&t);
        assert_eq!((d.id().as_str(), d.created_at()), ("n", Some(7)));
    }
}
