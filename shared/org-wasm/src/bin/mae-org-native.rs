//! The facade, natively: the other side of the Node differential
//! (`js/test.mjs`). Reads a file of `OP<TAB>REQUEST` lines (OP the ABI
//! number, REQUEST JSON on one line) and writes each response on its own
//! line, through the same `api::call` the WASM module runs.

use std::io::Write;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: mae-org-native REQUESTS-FILE");
        std::process::exit(2);
    };
    let input = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(2);
    });
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    for line in input.lines() {
        let (op, request) = line.split_once('\t').unwrap_or((line, ""));
        let op = op.parse::<u32>().unwrap_or(0);
        let response = mae_org_wasm::api::call(op, request);
        if writeln!(out, "{response}").is_err() {
            std::process::exit(1);
        }
    }
}
