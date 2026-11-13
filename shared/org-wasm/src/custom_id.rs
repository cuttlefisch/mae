//! The `CUSTOM_ID` operations (`mae_org::custom_id_edits` and friends):
//! request fields in, JSON out. Offsets are UTF-8 byte offsets into `text`,
//! like every span the facade returns.

use crate::json::Json;
use mae_org::{json_string, CustomIdOccurrence, ParseOptions, Span, TextEdit};

/// The `CUSTOM_ID` fields of a request.
#[derive(Default)]
pub struct CustomIdRequest {
    /// `[[start, end], …]`: a headline is in scope when its headline line
    /// starts inside one. Absent (or null): the whole text.
    pub ranges: Option<Vec<Span>>,
    pub uuids: Option<Vec<String>>,
    pub position: Option<u32>,
    pub uuid: Option<String>,
}

/// `{"headings": [[start, end], …]}`: the headline lines needing one.
pub fn missing(text: &str, parse: &ParseOptions, r: &CustomIdRequest) -> String {
    let spans = mae_org::headings_missing_custom_id(text, parse, r.ranges.as_deref());
    let spans: Vec<String> = spans.iter().map(|s| span(*s)).collect();
    format!("{{\"headings\":[{}]}}", spans.join(","))
}

/// `{"edits": [{start, end, insert}, …]}`, descending.
pub fn edits(text: &str, parse: &ParseOptions, r: &CustomIdRequest) -> Result<String, String> {
    let uuids = r.uuids.as_deref().ok_or("customIdEdits needs \"uuids\"")?;
    let edits = mae_org::custom_id_edits_with(text, parse, r.ranges.as_deref(), uuids)
        .map_err(|e| e.to_string())?;
    let edits: Vec<String> = edits.iter().map(edit).collect();
    Ok(format!("{{\"edits\":[{}]}}", edits.join(",")))
}

/// `{"groups": [[{value, heading}, …], …]}`.
pub fn duplicates(text: &str, parse: &ParseOptions) -> String {
    let groups: Vec<String> = mae_org::duplicate_custom_ids(text, parse)
        .iter()
        .map(|g| {
            let g: Vec<String> = g.iter().map(occurrence).collect();
            format!("[{}]", g.join(","))
        })
        .collect();
    format!("{{\"groups\":[{}]}}", groups.join(","))
}

/// `{"edit": {start, end, insert}}`.
pub fn replace(text: &str, parse: &ParseOptions, r: &CustomIdRequest) -> Result<String, String> {
    let at = r.position.ok_or("replaceCustomId needs \"position\"")?;
    let uuid = r.uuid.as_deref().ok_or("replaceCustomId needs \"uuid\"")?;
    let e = mae_org::replace_custom_id(text, parse, at, uuid).map_err(|e| e.to_string())?;
    Ok(format!("{{\"edit\":{}}}", edit(&e)))
}

fn span(s: Span) -> String {
    format!("[{},{}]", s.start, s.end)
}

fn edit(e: &TextEdit) -> String {
    format!(
        "{{\"start\":{},\"end\":{},\"insert\":{}}}",
        e.start,
        e.end,
        json_string(&e.insert)
    )
}

fn occurrence(o: &CustomIdOccurrence) -> String {
    format!(
        "{{\"value\":{},\"heading\":{}}}",
        span(o.value),
        span(o.heading)
    )
}

/// A byte offset: a whole number from 0 to `u32::MAX`.
pub fn offset(j: Option<&Json>, what: &str) -> Result<Option<u32>, String> {
    match j {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Num(n)) if n.fract() == 0.0 && *n >= 0.0 && *n <= f64::from(u32::MAX) => {
            Ok(Some(*n as u32))
        }
        Some(_) => Err(format!("{what} must be a byte offset (a whole number)")),
    }
}

/// `[[start, end], …]`, each `start <= end`.
pub fn ranges(j: Option<&Json>) -> Result<Option<Vec<Span>>, String> {
    let items = match j {
        None | Some(Json::Null) => return Ok(None),
        Some(Json::Arr(items)) => items,
        Some(_) => return Err("ranges must be an array of [start, end]".into()),
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Json::Arr(pair) = item else {
            return Err("each range must be [start, end]".into());
        };
        let [s, e] = pair.as_slice() else {
            return Err("each range must be [start, end]".into());
        };
        let start = offset(Some(s), "range start")?.unwrap_or(0);
        let end = offset(Some(e), "range end")?.unwrap_or(0);
        if start > end {
            return Err(format!("range [{start}, {end}] ends before it starts"));
        }
        out.push(Span { start, end });
    }
    Ok(Some(out))
}
