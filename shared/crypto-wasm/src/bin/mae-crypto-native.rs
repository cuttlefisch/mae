//! The native side of the Node interop test (`js/test.mjs`): mae-sync's own
//! `content_crypto`, not the WASM crate's `#[path]` copy of it. Reads
//! `OP<TAB>HEX` lines on stdin; writes `HEX` or `ERR <code>` per line.

use std::io::{BufRead, Write};

mod native {
    mae_crypto_wasm::define_call!(mae_sync::content_crypto);
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            std::process::exit(2)
        };
        let (op, hex) = line.split_once('\t').unwrap_or((&line, ""));
        let op = op.parse::<u32>().unwrap_or(0);
        let reply = match hex_decode(hex) {
            None => "ERR bad-hex".to_string(),
            Some(req) => match native::call(op, &req) {
                Ok(bytes) => bytes.iter().map(|b| format!("{b:02x}")).collect(),
                Err(code) => format!("ERR {code}"),
            },
        };
        if writeln!(out, "{reply}").is_err() {
            std::process::exit(1);
        }
    }
}
