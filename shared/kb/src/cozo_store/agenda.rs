//! Agenda queries (Phase E): todo/priority/tag/stale/orphan/dead-end/
//! missing-role/weakly-linked/custom filters over the node relation.

use super::util::{btree_params, cozo_err, dv_str, row_to_node};
use super::*;

impl CozoKbStore {
    /// Run an agenda query with the given filter.
    pub fn agenda_query(&self, filter: &AgendaFilter) -> Result<Vec<Node>, KbStoreError> {
        let query = match filter {
            AgendaFilter::Todo(None) => {
                "?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt] := *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt}, todo_state != ''".to_string()
            }
            AgendaFilter::Todo(Some(state)) => {
                // Bound, not interpolated. The previous `state.replace('\'', "")`
                // stripped single quotes only -- see `mae_kb::ident`'s header for
                // what that inconsistency cost at the one site that used double
                // quotes. Cozo binds data values natively; use it.
                return self.run_agenda_bound(
                    "?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt] := *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt}, todo_state = $v",
                    dv_str(state),
                );
            }
            AgendaFilter::Priority(min_pri) => {
                // This one had NO escaping at all, not even the partial kind.
                return self.run_agenda_bound(
                    "?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt] := *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt}, priority <= $v",
                    dv_str(&min_pri.to_string()),
                );
            }
            AgendaFilter::Tag(tag) => {
                return self.run_agenda_bound(
                    "?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt] := *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt}, str_includes(tags_json, $v)",
                    dv_str(tag),
                );
            }
            AgendaFilter::Stale(days) => {
                let cutoff = self.now_epoch() - (*days as i64 * 86400);
                format!(
                    "?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt] := *nodes{{id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt, updated_at}}, updated_at < {cutoff}, title != ''"
                )
            }
            AgendaFilter::Orphan => {
                // Nodes with no incoming or outgoing links
                "has_links[id] := *links{src: id}\nhas_links[id] := *links{dst: id}\n?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt] := *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt}, not has_links[id], title != ''".to_string()
            }
            AgendaFilter::DeadEnd => {
                // Nodes with no outgoing links
                "has_outgoing[id] := *links{src: id}\n?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt] := *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt}, not has_outgoing[id], title != ''".to_string()
            }
            AgendaFilter::MissingRole => {
                // Nodes with no `:role:` property set — the projected `properties_json` blob
                // never contains a "role" key. Mirrors the substring-`str_includes` style
                // already used by AgendaFilter::Tag, just negated (see `!` prefix-negation
                // support in CozoScript).
                "?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt] := *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt}, !str_includes(properties_json, '\"role\"'), title != ''".to_string()
            }
            AgendaFilter::WeaklyLinked(n) => {
                // Nodes with fewer than N outgoing typed links. Generalizes DeadEnd's
                // "no outgoing links" (n=0 case) into a per-node outgoing-link count via a
                // grouped `count()` aggregation, with a zero-count arm for nodes that have
                // no *links row at all (count() only sees nodes with >=1 outgoing edge).
                format!(
                    "has_outgoing[id] := *links{{src: id}}\nlink_count[id, count(dst)] := *links{{src: id, dst}}\nnode_link_count[id, cnt] := link_count[id, cnt]\nnode_link_count[id, cnt] := *nodes{{id, title}}, not has_outgoing[id], title != '', cnt = 0\n?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt] := *nodes{{id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt}}, node_link_count[id, cnt], cnt < {n}, title != ''"
                )
            }
            AgendaFilter::Custom(q) => q.clone(),
        };

        let result = self.run_immut(&query).map_err(cozo_err)?;
        Ok(Self::rows_to_nodes(&result))
    }

    /// Run an agenda query whose only variable part is a **data value**, bound
    /// as `$v` rather than interpolated into the script.
    ///
    /// @ai-caution: [datalog-injection] Every agenda filter that takes a
    /// caller-supplied value routes through here. Before this existed, three of
    /// them did `x.replace('\'', "")` -- which strips single quotes only -- and
    /// `Priority` did nothing at all. Cozo binds data values natively
    /// (`run_immut_params`); there is no reason for any of them to build a
    /// script string. Identifier positions, which binding cannot reach, use
    /// `mae_kb::ident` instead.
    fn run_agenda_bound(&self, script: &str, value: DataValue) -> Result<Vec<Node>, KbStoreError> {
        let result = self
            .run_immut_params(script, btree_params([("v", value)]))
            .map_err(cozo_err)?;
        Ok(Self::rows_to_nodes(&result))
    }

    /// Shared row->Node conversion: tolerate a malformed row by skipping it
    /// (ADR-019 / B-5) rather than aborting the whole load, which previously
    /// stalled the editor's main thread on a single bad-arity row.
    fn rows_to_nodes(result: &NamedRows) -> Vec<Node> {
        let mut nodes = Vec::new();
        for row in &result.rows {
            match row_to_node(row) {
                Ok(node) => nodes.push(node),
                Err(e) => tracing::warn!(error = %e, "KB store: skipping malformed node row"),
            }
        }
        nodes
    }
}
