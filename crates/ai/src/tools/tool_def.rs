//! Fluent builder for [`ToolDefinition`].
//!
//! Every `*_tools.rs` file in this module used to hand-write a ~15-line
//! `ToolDefinition { parameters: ToolParameters { properties: HashMap::from([...]) ... } }`
//! struct literal per tool — ~200 near-identical copies across the module. `ToolDefBuilder`
//! replaces that boilerplate with one call per property, so a tool definition reads as its
//! actual content (name, description, params) instead of the `ToolParameters`/`ToolProperty`
//! nesting required to construct one.

use std::collections::HashMap;

use crate::types::{PermissionTier, ToolDefinition, ToolParameters, ToolProperty};

pub(super) struct ToolDefBuilder {
    name: String,
    description: String,
    properties: HashMap<String, ToolProperty>,
    required: Vec<String>,
    permission: Option<PermissionTier>,
}

impl ToolDefBuilder {
    pub(super) fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            properties: HashMap::new(),
            required: Vec::new(),
            permission: None,
        }
    }

    /// Add a plain (non-enum-constrained) property.
    pub(super) fn prop(
        mut self,
        name: impl Into<String>,
        prop_type: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        self.properties.insert(
            name.into(),
            ToolProperty {
                prop_type: prop_type.into(),
                description: description.into(),
                enum_values: None,
                items: None,
                properties: None,
                object_required: None,
            },
        );
        self
    }

    /// Add an array-of-objects property (JSON Schema `items: {type: "object",
    /// properties: {...}, required: [...]}`) — for a param that's genuinely
    /// structured, not a flat list of scalars. `item_fields` is
    /// `(field_name, field_type, field_description)` triples describing the
    /// shape each array element must have; `item_required` names which of
    /// those fields are mandatory per element.
    pub(super) fn prop_array_of_objects(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        item_fields: impl IntoIterator<Item = (&'static str, &'static str, &'static str)>,
        item_required: impl IntoIterator<Item = &'static str>,
    ) -> Self {
        let item_properties: HashMap<String, ToolProperty> = item_fields
            .into_iter()
            .map(|(field_name, field_type, field_description)| {
                (
                    field_name.to_string(),
                    ToolProperty {
                        prop_type: field_type.to_string(),
                        description: field_description.to_string(),
                        enum_values: None,
                        items: None,
                        properties: None,
                        object_required: None,
                    },
                )
            })
            .collect();
        self.properties.insert(
            name.into(),
            ToolProperty {
                prop_type: "array".to_string(),
                description: description.into(),
                enum_values: None,
                items: Some(Box::new(ToolProperty {
                    prop_type: "object".to_string(),
                    description: String::new(),
                    enum_values: None,
                    items: None,
                    properties: Some(item_properties),
                    object_required: Some(
                        item_required.into_iter().map(|s| s.to_string()).collect(),
                    ),
                })),
                properties: None,
                object_required: None,
            },
        );
        self
    }

    /// Add a property constrained to a fixed set of values.
    pub(super) fn prop_enum(
        mut self,
        name: impl Into<String>,
        prop_type: impl Into<String>,
        description: impl Into<String>,
        enum_values: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.properties.insert(
            name.into(),
            ToolProperty {
                prop_type: prop_type.into(),
                description: description.into(),
                enum_values: Some(enum_values.into_iter().map(Into::into).collect()),
                items: None,
                properties: None,
                object_required: None,
            },
        );
        self
    }

    /// Mark the given property names as required. Whether each name was actually
    /// registered via `prop`/`prop_enum` is checked globally by the
    /// `tool_definitions_have_valid_required_params` test in `mod.rs`, not here.
    pub(super) fn required(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.required = names.into_iter().map(Into::into).collect();
        self
    }

    pub(super) fn permission(mut self, tier: PermissionTier) -> Self {
        self.permission = Some(tier);
        self
    }

    pub(super) fn build(self) -> ToolDefinition {
        ToolDefinition {
            name: self.name,
            description: self.description,
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: self.properties,
                required: self.required,
            },
            permission: self.permission,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_with_no_properties_matches_hand_written_shape() {
        let built = ToolDefBuilder::new("t", "desc")
            .permission(PermissionTier::ReadOnly)
            .build();
        assert_eq!(built.name, "t");
        assert_eq!(built.description, "desc");
        assert_eq!(built.parameters.schema_type, "object");
        assert!(built.parameters.properties.is_empty());
        assert!(built.parameters.required.is_empty());
        assert_eq!(built.permission, Some(PermissionTier::ReadOnly));
    }

    #[test]
    fn builder_prop_and_required_round_trip() {
        let built = ToolDefBuilder::new("t", "desc")
            .prop("id", "string", "the id")
            .required(["id"])
            .permission(PermissionTier::Write)
            .build();
        let prop = built.parameters.properties.get("id").expect("id property");
        assert_eq!(prop.prop_type, "string");
        assert_eq!(prop.description, "the id");
        assert_eq!(prop.enum_values, None);
        assert_eq!(built.parameters.required, vec!["id".to_string()]);
    }

    #[test]
    fn builder_prop_enum_collects_values() {
        let built = ToolDefBuilder::new("t", "desc")
            .prop_enum("mode", "string", "which mode", ["a", "b", "c"])
            .build();
        let prop = built
            .parameters
            .properties
            .get("mode")
            .expect("mode property");
        assert_eq!(
            prop.enum_values,
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
    }

    #[test]
    fn builder_without_permission_defaults_none() {
        let built = ToolDefBuilder::new("t", "desc").build();
        assert_eq!(built.permission, None);
    }
}
