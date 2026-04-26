use std::collections::HashMap;

use crate::types::*;

pub(super) fn web_tool_definitions() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: "web_fetch".into(),
        description: "Fetch a URL and return its text content. HTML is converted to readable text. Response truncated to 32KB.".into(),
        parameters: ToolParameters {
            schema_type: "object".into(),
            properties: HashMap::from([(
                "url".into(),
                ToolProperty {
                    prop_type: "string".into(),
                    description: "The URL to fetch (http or https)".into(),
                    enum_values: None,
                },
            )]),
            required: vec!["url".into()],
        },
        permission: Some(PermissionTier::Shell),
    }]
}
