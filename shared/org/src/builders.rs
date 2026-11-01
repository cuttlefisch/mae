//! Building the option and data types from outside the crate.
//!
//! Every option and result type is `#[non_exhaustive]`, so a field can be
//! added in a minor version without breaking a consumer pinned to
//! `mae-org-v0.1`: a consumer starts from `Default` (or a constructor here)
//! and either sets the public fields of that value or chains the `with_*`
//! setters below. `tests/api_contract.rs` builds every one of them through
//! this surface only, as an external crate must.

use crate::{
    ArchivedTrees, HeadlineIds, LinkTypes, ParseOptions, RenderOptions, Resolved, Resolver,
    SemanticsOptions, TodoSequence,
};

fn strings<S: Into<String>>(v: impl IntoIterator<Item = S>) -> Vec<String> {
    v.into_iter().map(Into::into).collect()
}

impl TodoSequence {
    /// A sequence: `todo` keywords, then (after Org's `|`) `done` ones.
    pub fn new<S: Into<String>>(
        todo: impl IntoIterator<Item = S>,
        done: impl IntoIterator<Item = S>,
    ) -> TodoSequence {
        TodoSequence {
            todo: strings(todo),
            done: strings(done),
        }
    }
}

impl ParseOptions {
    /// [`ParseOptions::todo_sequences`].
    pub fn with_todo_sequences(mut self, sequences: Vec<TodoSequence>) -> Self {
        self.todo_sequences = sequences;
        self
    }

    /// [`ParseOptions::max_depth`].
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// [`ParseOptions::link_types`].
    pub fn with_link_types<S: Into<String>>(mut self, types: impl IntoIterator<Item = S>) -> Self {
        self.link_types = strings(types);
        self
    }

    /// [`ParseOptions::radio_regexp_limit`].
    pub fn with_radio_regexp_limit(mut self, limit: usize) -> Self {
        self.radio_regexp_limit = limit;
        self
    }
}

impl LinkTypes {
    /// [`LinkTypes::web`].
    pub fn with_web<S: Into<String>>(mut self, schemes: impl IntoIterator<Item = S>) -> Self {
        self.web = strings(schemes);
        self
    }

    /// [`LinkTypes::mail`].
    pub fn with_mail<S: Into<String>>(mut self, schemes: impl IntoIterator<Item = S>) -> Self {
        self.mail = strings(schemes);
        self
    }

    /// [`LinkTypes::other`].
    pub fn with_other<S: Into<String>>(mut self, schemes: impl IntoIterator<Item = S>) -> Self {
        self.other = strings(schemes);
        self
    }
}

impl SemanticsOptions {
    /// [`SemanticsOptions::exclude_tags`].
    pub fn with_exclude_tags<S: Into<String>>(mut self, tags: impl IntoIterator<Item = S>) -> Self {
        self.exclude_tags = strings(tags);
        self
    }

    /// [`SemanticsOptions::select_tags`].
    pub fn with_select_tags<S: Into<String>>(mut self, tags: impl IntoIterator<Item = S>) -> Self {
        self.select_tags = strings(tags);
        self
    }

    /// [`SemanticsOptions::archive_tag`].
    pub fn with_archive_tag(mut self, tag: impl Into<String>) -> Self {
        self.archive_tag = tag.into();
        self
    }

    /// [`SemanticsOptions::archived_trees`].
    pub fn with_archived_trees(mut self, archived: ArchivedTrees) -> Self {
        self.archived_trees = archived;
        self
    }

    /// [`SemanticsOptions::link_types`].
    pub fn with_link_types(mut self, types: LinkTypes) -> Self {
        self.link_types = types;
        self
    }

    /// [`SemanticsOptions::link_exclude_keywords`].
    pub fn with_link_exclude_keywords<S: Into<String>>(
        mut self,
        keywords: impl IntoIterator<Item = S>,
    ) -> Self {
        self.link_exclude_keywords = strings(keywords);
        self
    }

    /// [`SemanticsOptions::link_exclude_properties`].
    pub fn with_link_exclude_properties<S: Into<String>>(
        mut self,
        properties: impl IntoIterator<Item = S>,
    ) -> Self {
        self.link_exclude_properties = strings(properties);
        self
    }
}

impl RenderOptions {
    /// [`RenderOptions::headline_ids`].
    pub fn with_headline_ids(mut self, ids: HeadlineIds) -> Self {
        self.headline_ids = ids;
        self
    }

    /// [`RenderOptions::images`].
    pub fn with_images(mut self, images: bool) -> Self {
        self.images = images;
        self
    }

    /// [`RenderOptions::property_drawers`].
    pub fn with_property_drawers(mut self, drawers: bool) -> Self {
        self.property_drawers = drawers;
        self
    }
}

impl Resolved {
    /// A target at `href` (validated when used: see [`Resolved::href`]),
    /// with no title.
    pub fn new(href: impl Into<String>) -> Resolved {
        Resolved {
            href: href.into(),
            title: None,
        }
    }

    /// [`Resolved::title`].
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

impl Resolver {
    /// Adds (or replaces) KB node `id`'s target.
    pub fn with_target(mut self, id: impl Into<String>, target: Resolved) -> Self {
        self.targets.insert(id.into(), target);
        self
    }

    /// [`Resolver::placeholder`].
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }
}
