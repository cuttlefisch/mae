//! The `syntax` and `syntaxSection` operations: highlighting spans for an
//! editor's decorations (`mae_org::highlights` / `sections` /
//! `section_highlights`).
//!
//! `syntax` answers for a whole text:
//!
//! ```json
//! {"kinds": ["HEADLINE", "STARS", …],
//!  "spans": [k, start, end, k, start, end, …],
//!  "file": {"line_ending": "lf", "radio_targets": ["…"],
//!           "todo": [{"todo": ["TODO"], "done": ["DONE"]}]},
//!  "sections": [{"start": 0, "end": 12, "level": 0, "ancestors": 0,
//!                "declarations": 2166136261, "followed": true}, …]}
//! ```
//!
//! `spans` is flat (three numbers a span, `k` an index into `kinds`) to keep
//! the response small. A span's end includes its post-blank (the spaces
//! after an object, the blank lines after an element), as `org-element`
//! reports it; a request with `"post_blank": false` gets each span ending
//! before it instead, for an editor that styles a node or tests whether the
//! cursor is in it. `syntaxSection` takes one region's NEW text as `text`
//! with its `section` and the `file` object as `syntax` gave them, and
//! answers `{"kinds", "spans"}` with offsets in the whole text, or
//! `{"needs_full": "headline" | "declarations" | "boundary"}` when the whole
//! text must be sent to `syntax` again. Offsets are UTF-8 bytes.

use crate::json::Json;
use mae_org::{
    json_string, FragmentContext, Highlight, LineEnding, NeedsFull, ParseOptions, Section,
    SectionHighlights, SyntaxKind, TodoSequence,
};

/// `{"kinds": […], "spans": […]}` without the braces. Each span ends at its
/// `end`, or at its `own_end` (before its post-blank) when `post_blank` is
/// false.
fn spans_json(spans: &[Highlight], post_blank: bool) -> String {
    let mut kinds: Vec<SyntaxKind> = Vec::new();
    let mut flat = Vec::with_capacity(spans.len() * 3);
    for h in spans {
        let k = kinds.iter().position(|k| *k == h.kind).unwrap_or_else(|| {
            kinds.push(h.kind);
            kinds.len() - 1
        });
        let end = if post_blank { h.end } else { h.own_end };
        flat.push(format!("{k},{},{end}", h.start));
    }
    let names: Vec<String> = kinds
        .iter()
        .map(|k| json_string(&format!("{k:?}")))
        .collect();
    format!(
        "\"kinds\":[{}],\"spans\":[{}]",
        names.join(","),
        flat.join(",")
    )
}

fn strings_json(v: &[String]) -> String {
    let s: Vec<String> = v.iter().map(|x| json_string(x)).collect();
    format!("[{}]", s.join(","))
}

fn eol_name(e: LineEnding) -> &'static str {
    match e {
        LineEnding::Lf => "lf",
        LineEnding::CrLf => "crlf",
        LineEnding::Cr => "cr",
    }
}

fn file_json(ctx: &FragmentContext) -> String {
    let radio = ctx.radio_targets.clone().unwrap_or_default();
    let todo: Vec<String> = ctx
        .todo_sequences
        .clone()
        .unwrap_or_default()
        .iter()
        .map(|t| {
            format!(
                "{{\"todo\":{},\"done\":{}}}",
                strings_json(&t.todo),
                strings_json(&t.done)
            )
        })
        .collect();
    format!(
        "{{\"line_ending\":\"{}\",\"radio_targets\":{},\"todo\":[{}]}}",
        eol_name(ctx.line_ending),
        strings_json(&radio),
        todo.join(",")
    )
}

fn section_json(s: &Section) -> String {
    format!(
        "{{\"start\":{},\"end\":{},\"level\":{},\"ancestors\":{},\"declarations\":{},\"followed\":{}}}",
        s.start, s.end, s.context.level, s.context.ancestors, s.declarations, s.followed_by_headline
    )
}

/// The `syntax` operation.
pub fn syntax(text: &str, options: &ParseOptions, post_blank: bool) -> String {
    let doc = mae_org::parse(text, options);
    let sections = mae_org::sections(&doc);
    let file = sections
        .first()
        .map_or_else(|| "null".to_string(), |s| file_json(&s.context));
    let secs: Vec<String> = sections.iter().map(section_json).collect();
    format!(
        "{{{},\"file\":{file},\"sections\":[{}]}}",
        spans_json(&mae_org::highlights(&doc), post_blank),
        secs.join(",")
    )
}

fn num(j: Option<&Json>, what: &str) -> Result<u64, String> {
    match j {
        Some(Json::Num(n)) if *n >= 0.0 && n.fract() == 0.0 && *n <= f64::from(u32::MAX) => {
            Ok(*n as u64)
        }
        _ => Err(format!("{what} must be a non-negative integer")),
    }
}

fn strs(j: Option<&Json>, what: &str) -> Result<Vec<String>, String> {
    match j {
        None | Some(Json::Null) => Ok(Vec::new()),
        Some(Json::Arr(v)) => v
            .iter()
            .map(|x| match x {
                Json::Str(s) => Ok(s.clone()),
                _ => Err(format!("{what} must hold strings")),
            })
            .collect(),
        Some(_) => Err(format!("{what} must be an array")),
    }
}

fn file_context(file: &Json, level: usize, ancestors: usize) -> Result<FragmentContext, String> {
    let eol = match file.get("line_ending") {
        Some(Json::Str(s)) if s == "lf" => LineEnding::Lf,
        Some(Json::Str(s)) if s == "crlf" => LineEnding::CrLf,
        Some(Json::Str(s)) if s == "cr" => LineEnding::Cr,
        _ => return Err("file.line_ending must be \"lf\", \"crlf\" or \"cr\"".into()),
    };
    let todo = match file.get("todo") {
        None | Some(Json::Null) => Vec::new(),
        Some(Json::Arr(v)) => v
            .iter()
            .map(|t| {
                Ok(TodoSequence::new(
                    strs(t.get("todo"), "file.todo[].todo")?,
                    strs(t.get("done"), "file.todo[].done")?,
                ))
            })
            .collect::<Result<_, String>>()?,
        Some(_) => return Err("file.todo must be an array".into()),
    };
    Ok(FragmentContext::new(level, eol)
        .with_ancestors(ancestors)
        .with_radio_targets(strs(file.get("radio_targets"), "file.radio_targets")?)
        .with_todo_sequences(todo))
}

fn read_section(section: &Json, file: &Json) -> Result<Section, String> {
    let n = |k: &str| num(section.get(k), &format!("section.{k}"));
    let followed = match section.get("followed") {
        Some(Json::Bool(b)) => *b,
        _ => return Err("section.followed must be a boolean".into()),
    };
    let context = file_context(file, n("level")? as usize, n("ancestors")? as usize)?;
    Ok(Section::new(
        n("start")? as u32,
        n("end")? as u32,
        context,
        n("declarations")? as u32,
        followed,
    ))
}

/// The `syntaxSection` operation.
pub fn syntax_section(
    text: &str,
    options: &ParseOptions,
    section: Option<&Json>,
    file: Option<&Json>,
    post_blank: bool,
) -> Result<String, String> {
    let section = section.ok_or("syntaxSection needs \"section\"")?;
    let file = file.ok_or("syntaxSection needs \"file\"")?;
    let s = read_section(section, file)?;
    Ok(match mae_org::section_highlights(text, options, &s) {
        SectionHighlights::Spans(spans) => format!("{{{}}}", spans_json(&spans, post_blank)),
        SectionHighlights::NeedsFull(why) => {
            let w = match why {
                NeedsFull::Headline => "headline",
                NeedsFull::Declarations => "declarations",
                NeedsFull::Boundary => "boundary",
                _ => "unknown",
            };
            format!("{{\"needs_full\":\"{w}\"}}")
        }
    })
}
