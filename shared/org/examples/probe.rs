//! Every mae-org output channel for one file, for reviewing leaks by hand
//! (the leak oracle's channels: `tests/leak_oracle/channels.rs`).
//!
//! ```sh
//! cargo run -p mae-org --example probe -- show FILE
//! cargo run -p mae-org --example probe -- grep FILE NEEDLE
//! cargo run -p mae-org --example probe -- splice FILE ID BODYFILE
//! cargo run -p mae-org --example probe -- rtime FILE [N]
//! cargo run -p mae-org --example probe -- sem FILE
//! cargo run -p mae-org --example probe -- loop FILE PHASE N
//! ```
//!
//! `show` prints every non-empty channel (body HTML and hast, `text()`,
//! `node_text` per node, every `nodes[]` field and the result JSON, in every
//! render mode) and the diagnostics. `grep` names the channels holding
//! NEEDLE (case-insensitively), with context. `splice` runs `replace_body`
//! on node ID with BODYFILE's text and prints every channel that changed.
//! `sem` prints the semantics (its `Debug`).
//! `rtime` times parse, semantics and render (min and median of N runs).
//! `loop` runs one PHASE (parse, semantics, render, highlights or sections)
//! N times and
//! nothing else, so a sampling profiler sees that phase alone.
//! `conformance/cmp.sh` puts `grep` beside Emacs's export.

#[path = "../tests/leak_oracle/channels.rs"]
mod channels;

use mae_org::{parse, replace_body, semantics, ParseOptions, SemanticsOptions};
use std::collections::BTreeMap;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const USAGE: &str =
    "usage: probe show FILE | grep FILE NEEDLE | splice FILE ID BODYFILE | rtime FILE [N] | loop FILE PHASE N";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let read = |i: usize| {
        args.get(i)
            .map(|p| std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}")))
    };
    let result = match (args.first().map(String::as_str), read(1)) {
        (Some("show"), Some(Ok(t))) => Ok(show(&t)),
        (Some("grep"), Some(Ok(t))) if args.len() == 3 => Ok(grep(&t, &args[2])),
        (Some("splice"), Some(Ok(t))) if args.len() == 4 => {
            read(3).expect("arg").map(|b| splice(&t, &args[2], &b))
        }
        (Some("rtime"), Some(Ok(t))) => Ok(rtime(
            &t,
            args.get(2).and_then(|n| n.parse().ok()).unwrap_or(10),
        )),
        (Some("loop"), Some(Ok(t))) if args.len() == 4 => args[3]
            .parse()
            .map_err(|_| USAGE.to_string())
            .and_then(|n| phase_loop(&t, &args[2], n)),
        (Some("sem"), Some(Ok(t))) => Ok(format!(
            "{:#?}\n",
            semantics(&parse(&t, &ParseOptions::default()))
        )),
        (_, Some(Err(e))) => Err(e),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}

fn header(v: &channels::MaeView) -> String {
    format!(
        "diagnostics={:?} empty={} macro_failed={}\n",
        v.diags, v.empty, v.failed
    )
}

fn show(text: &str) -> String {
    let v = channels::mae_view(text);
    let mut out = header(&v);
    for c in v.channels.iter().filter(|c| !c.text.is_empty()) {
        out.push_str(&format!("== {} [{:?}]\n{}\n", c.name, c.kind, c.text));
    }
    out
}

fn grep(text: &str, needle: &str) -> String {
    let v = channels::mae_view(text);
    let n = needle.to_lowercase();
    let mut out = header(&v);
    let mut hits = 0;
    for c in &v.channels {
        let lower = c.text.to_lowercase();
        let Some(i) = lower.find(&n) else {
            continue;
        };
        hits += 1;
        let lo = c.text.floor_char_boundary(i.saturating_sub(60));
        let hi = c
            .text
            .ceil_char_boundary((i + n.len() + 60).min(c.text.len()));
        out.push_str(&format!(
            "HIT {} [{:?}]: …{}…\n",
            c.name,
            c.kind,
            &c.text[lo..hi]
        ));
    }
    out.push_str(&format!(
        "{hits} channel(s) of {} hold {needle:?}\n",
        v.channels.len()
    ));
    out
}

fn splice(text: &str, id: &str, body: &str) -> String {
    let opts = ParseOptions::default();
    let doc = parse(text, &opts);
    let sem = semantics(&doc);
    let Some(node) = sem.nodes.iter().find(|n| n.id == id) else {
        return format!("no node {id:?}\n");
    };
    let new = match replace_body(text, &doc, node, body, &opts, &SemanticsOptions::default()) {
        Ok(s) => s,
        Err(e) => return format!("REFUSED: {e}\n"),
    };
    let before: BTreeMap<String, String> = channels::mae_view(text)
        .channels
        .into_iter()
        .map(|c| (c.name, c.text))
        .collect();
    let after: BTreeMap<String, String> = channels::mae_view(&new)
        .channels
        .into_iter()
        .map(|c| (c.name, c.text))
        .collect();
    let mut out = format!("ACCEPTED\n-- new text:\n{new}\n");
    for k in before
        .keys()
        .chain(after.keys().filter(|k| !before.contains_key(*k)))
    {
        let (b, a) = (before.get(k), after.get(k));
        if b != a {
            out.push_str(&format!(
                "CHANGED {k}\n  before: {}\n  after:  {}\n",
                b.map_or("(none)", |s| s),
                a.map_or("(none)", |s| s)
            ));
        }
    }
    out
}

fn rtime(text: &str, n: usize) -> String {
    let mut times: BTreeMap<&str, Vec<Duration>> = BTreeMap::new();
    for _ in 0..n.max(1) {
        let t = Instant::now();
        let doc = parse(text, &ParseOptions::default());
        times.entry("parse").or_default().push(t.elapsed());
        let t = Instant::now();
        let sem = semantics(&doc);
        times.entry("semantics").or_default().push(t.elapsed());
        let t = Instant::now();
        let r = mae_org::render(
            &doc,
            &sem,
            &mae_org::RenderOptions::default(),
            &mae_org::Resolver::default(),
        );
        times.entry("render").or_default().push(t.elapsed());
        std::hint::black_box(r);
    }
    let mut out = format!("{} bytes, {n} runs\n", text.len());
    for (k, mut v) in times {
        v.sort();
        out.push_str(&format!(
            "{k:<10} min {:?} median {:?}\n",
            v[0],
            v[v.len() / 2]
        ));
    }
    out
}

/// Runs `phase` `n` times over `text`; its inputs are built once, outside the loop.
fn phase_loop(text: &str, phase: &str, n: usize) -> Result<String, String> {
    let doc = parse(text, &ParseOptions::default());
    let sem = semantics(&doc);
    let (ro, rs) = (
        mae_org::RenderOptions::default(),
        mae_org::Resolver::default(),
    );
    for _ in 0..n {
        match phase {
            "parse" => drop(std::hint::black_box(parse(text, &ParseOptions::default()))),
            "semantics" => drop(std::hint::black_box(semantics(&doc))),
            "render" => drop(std::hint::black_box(mae_org::render(&doc, &sem, &ro, &rs))),
            "highlights" => drop(std::hint::black_box(mae_org::highlights(&doc))),
            "sections" => drop(std::hint::black_box(mae_org::sections(&doc))),
            _ => return Err(format!("unknown phase {phase}\n{USAGE}")),
        }
    }
    Ok(format!("{phase} x{n}\n"))
}
