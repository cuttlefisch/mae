//! What a [`crate::query::KbQueryLayer`] implementation can actually answer.
//!
//! @ai-caution: [capability-model] MAE had **three half-built capability
//! signals and no single one**, which is why the conformance suite could not be
//! written and why a caller could not tell a real answer from a shrug:
//!
//! 1. `KbStore`'s ~29 methods defaulting to
//!    `NotSupported("… requires CozoDB backend")` — a genuine fail-loud design,
//!    but `grep NotSupported crates/` returns **zero call-site handlers**, so it
//!    surfaces as a raw string to the user or the model.
//! 2. `daemon/src/kb_query.rs`'s `kb/query.capabilities` endpoint — the right
//!    *place*, the wrong *content*: it reports only `encryption` and
//!    `searchable`, never which methods are answerable.
//! 3. `RemoteHubQueryLayer`'s empty returns — and these are **two different
//!    things wearing the same clothes**: `links_from`/`links_to`/`health_report`/
//!    `id_title_pairs` return empty because ADR-053's surface has no such
//!    endpoint (a real capability gap), while `search`/`list_ids` return empty
//!    after `set_outcome(LastOutcome::MalformedResponse(..))` (a **swallowed
//!    error**). A caller cannot distinguish "no links" from "cannot answer" from
//!    "the hub replied with garbage".
//!
//! This module supplies the missing declaration. The contract it enables:
//!
//! > For every method an implementation **declares** it supports, its results
//! > must match the reference implementation. A method it does **not** declare
//! > is a *declared gap* — reported, not a conformance failure.
//!
//! Without that distinction a conformance suite is unwritable: `RemoteHub`
//! would fail forever and the suite would be disabled, which is the usual fate
//! of a suite that cannot express legitimate difference.

use std::collections::BTreeSet;

/// A method on [`crate::query::KbQueryLayer`] whose support can vary by backing.
///
/// Deliberately an enum rather than strings: adding a trait method and
/// forgetting to classify it should be a compile error in [`QueryMethod::ALL`],
/// not a silently-missing capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QueryMethod {
    Get,
    Contains,
    Search,
    LinksFrom,
    LinksTo,
    ListIds,
    IdTitlePairs,
    IdTitleBodyTriples,
    HealthReport,
    Neighborhood,
    Related,
    LinkedInDegree,
    NodeCrdtState,
    TodoNodes,
    Agenda,
    History,
    NamespacePrefixes,
}

impl QueryMethod {
    /// Every method. Exhaustive by construction — see the `match` in [`Self::name`],
    /// which the compiler forces you to extend when a variant is added.
    pub const ALL: &'static [QueryMethod] = &[
        QueryMethod::Get,
        QueryMethod::Contains,
        QueryMethod::Search,
        QueryMethod::LinksFrom,
        QueryMethod::LinksTo,
        QueryMethod::ListIds,
        QueryMethod::IdTitlePairs,
        QueryMethod::IdTitleBodyTriples,
        QueryMethod::HealthReport,
        QueryMethod::Neighborhood,
        QueryMethod::Related,
        QueryMethod::LinkedInDegree,
        QueryMethod::NodeCrdtState,
        QueryMethod::TodoNodes,
        QueryMethod::Agenda,
        QueryMethod::History,
        QueryMethod::NamespacePrefixes,
    ];

    /// Wire name, as reported by `kb/query.capabilities`.
    pub fn name(self) -> &'static str {
        match self {
            QueryMethod::Get => "get",
            QueryMethod::Contains => "contains",
            QueryMethod::Search => "search",
            QueryMethod::LinksFrom => "links_from",
            QueryMethod::LinksTo => "links_to",
            QueryMethod::ListIds => "list_ids",
            QueryMethod::IdTitlePairs => "id_title_pairs",
            QueryMethod::IdTitleBodyTriples => "id_title_body_triples",
            QueryMethod::HealthReport => "health_report",
            QueryMethod::Neighborhood => "neighborhood",
            QueryMethod::Related => "related",
            QueryMethod::LinkedInDegree => "linked_in_degree",
            QueryMethod::NodeCrdtState => "node_crdt_state",
            QueryMethod::TodoNodes => "todo_nodes",
            QueryMethod::Agenda => "agenda",
            QueryMethod::History => "history",
            QueryMethod::NamespacePrefixes => "namespace_prefixes",
        }
    }
}

/// The set of methods an implementation declares it can answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryCapabilities(BTreeSet<QueryMethod>);

impl QueryCapabilities {
    /// A fully-capable backing — the local Cozo store, and the default for any
    /// implementation that does not say otherwise.
    ///
    /// Defaulting to "everything" is deliberate: a new implementation that
    /// forgets to declare is treated as claiming full support, so the
    /// conformance suite tests it against the reference and **fails loudly**
    /// rather than quietly excusing it. The opposite default would let a
    /// backing opt out of testing by omission.
    pub fn all() -> Self {
        Self(QueryMethod::ALL.iter().copied().collect())
    }

    /// Full support minus the named gaps.
    pub fn all_except(gaps: &[QueryMethod]) -> Self {
        let mut s = Self::all();
        for g in gaps {
            s.0.remove(g);
        }
        s
    }

    pub fn supports(&self, m: QueryMethod) -> bool {
        self.0.contains(&m)
    }

    /// Methods this backing cannot answer — what `kb/query.capabilities` should
    /// report so a caller can degrade deliberately instead of guessing.
    pub fn gaps(&self) -> Vec<QueryMethod> {
        QueryMethod::ALL
            .iter()
            .copied()
            .filter(|m| !self.supports(*m))
            .collect()
    }

    /// Wire form: the sorted names of supported methods.
    pub fn names(&self) -> Vec<&'static str> {
        self.0.iter().map(|m| m.name()).collect()
    }
}

impl Default for QueryCapabilities {
    fn default() -> Self {
        Self::all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant must have a distinct wire name — a duplicate would make
    /// two capabilities indistinguishable to a remote caller.
    #[test]
    fn wire_names_are_unique_and_cover_every_variant() {
        let names: BTreeSet<&str> = QueryMethod::ALL.iter().map(|m| m.name()).collect();
        assert_eq!(
            names.len(),
            QueryMethod::ALL.len(),
            "duplicate wire name among {:?}",
            QueryMethod::ALL
                .iter()
                .map(|m| m.name())
                .collect::<Vec<_>>()
        );
    }

    /// The default must be "claims everything", so a new backing that forgets to
    /// declare gets TESTED rather than excused.
    #[test]
    fn the_default_claims_full_support_so_omission_fails_loudly() {
        let d = QueryCapabilities::default();
        assert!(
            d.gaps().is_empty(),
            "default must claim every method: {:?}",
            d.gaps()
        );
        for m in QueryMethod::ALL {
            assert!(d.supports(*m), "{} not claimed by default", m.name());
        }
    }

    #[test]
    fn declared_gaps_are_reported_and_not_supported() {
        let c = QueryCapabilities::all_except(&[QueryMethod::LinksFrom, QueryMethod::HealthReport]);
        assert!(!c.supports(QueryMethod::LinksFrom));
        assert!(c.supports(QueryMethod::Get));
        let gaps: Vec<&str> = c.gaps().iter().map(|m| m.name()).collect();
        assert_eq!(gaps, vec!["links_from", "health_report"]);
    }
}
