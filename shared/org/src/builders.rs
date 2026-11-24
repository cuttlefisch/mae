//! Building the option and data types from outside the crate.
//!
//! Every option and result type is `#[non_exhaustive]`, so a field can be
//! added in a minor version without breaking a consumer pinned to
//! `mae-org-v0.1`: a consumer starts from `Default` (or a constructor here)
//! and either sets the public fields of that value or chains the `with_*`
//! setters below. `tests/api_contract.rs` builds every one of them through
//! this surface only, as an external crate must.

use crate::{
    ArchivedTrees, AttachmentState, HeadlineIds, LinkTypes, ParseOptions, RenderOptions, Resolved,
    ResolvedAttachment, Resolver, SemanticsOptions, TodoSequence,
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

    /// [`RenderOptions::toplevel_hlevel`].
    pub fn with_toplevel_hlevel(mut self, level: crate::ToplevelHlevel) -> Self {
        self.toplevel_hlevel = level;
        self
    }

    /// [`RenderOptions::headline_levels`].
    pub fn with_headline_levels(mut self, levels: Option<crate::HeadlineLevels>) -> Self {
        self.headline_levels = levels;
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
            custom_ids: Default::default(),
        }
    }

    /// Adds one of the target page's `CUSTOM_ID`s, at the anchor it renders
    /// at ([`Resolved::custom_ids`]).
    pub fn with_custom_id(
        mut self,
        custom_id: impl Into<String>,
        anchor: impl Into<String>,
    ) -> Self {
        self.custom_ids.insert(custom_id.into(), anchor.into());
        self
    }

    /// [`Resolved::title`].
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

impl ResolvedAttachment {
    /// A file at `url` (validated when used: see [`ResolvedAttachment::url`])
    /// of type `media_type`, size unknown.
    pub fn new(url: impl Into<String>, media_type: impl Into<String>) -> ResolvedAttachment {
        ResolvedAttachment {
            url: url.into(),
            media_type: media_type.into(),
            width: None,
            height: None,
        }
    }

    /// [`ResolvedAttachment::width`].
    pub fn with_width(mut self, width: u32) -> Self {
        self.width = Some(width);
        self
    }

    /// [`ResolvedAttachment::height`].
    pub fn with_height(mut self, height: u32) -> Self {
        self.height = Some(height);
        self
    }
}

impl Resolver {
    /// Adds (or replaces) what the host found for attachment or file link
    /// `target` ([`Resolver::attachments`]).
    pub fn with_attachment(mut self, target: impl Into<String>, state: AttachmentState) -> Self {
        self.attachments.insert(target.into(), state);
        self
    }

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
