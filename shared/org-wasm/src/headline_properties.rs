//! `headlineProperties` (`mae_org::headline_properties`): every headline's
//! property drawer, node or not. Offsets are UTF-8 byte offsets into `text`,
//! like every span the facade returns.

use crate::custom_id::span;
use mae_org::{json_string, ParseOptions, Span};

/// `{"headlines": [{heading, level, properties: [{key, keyAsWritten, value}]}]}`.
pub fn headline_properties(text: &str, parse: &ParseOptions, ranges: Option<&[Span]>) -> String {
    let headlines: Vec<String> = mae_org::headline_properties(text, parse, ranges)
        .iter()
        .map(|h| {
            let props: Vec<String> = h
                .properties
                .iter()
                .map(|p| {
                    format!(
                        "{{\"key\":{},\"keyAsWritten\":{},\"value\":{}}}",
                        json_string(&p.key),
                        json_string(&p.key_as_written),
                        json_string(&p.value)
                    )
                })
                .collect();
            format!(
                "{{\"heading\":{},\"level\":{},\"properties\":[{}]}}",
                span(h.heading),
                h.level,
                props.join(",")
            )
        })
        .collect();
    format!("{{\"headlines\":[{}]}}", headlines.join(","))
}
