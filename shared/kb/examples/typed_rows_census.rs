//! Counts a KB store's typed `links` rows and whether its stored bodies
//! declare them (#854). Opens the cozo database directly, WITHOUT
//! `CozoKbStore::open` (which would run the version-gated repair), so run it
//! on a COPY. Prints counts only: never ids, titles or text.
//!
//! Usage: `cargo run --release --example typed_rows_census -p mae-kb -- <store.sqlite>...`

use cozo::{DataValue, DbInstance, ScriptMutability};
use mae_kb::{CozoKbStore, Node, NodeKind};
use std::collections::{BTreeMap, HashMap, HashSet};

fn rows(db: &DbInstance, q: &str) -> Vec<Vec<DataValue>> {
    db.run_script(q, BTreeMap::new(), ScriptMutability::Immutable)
        .map(|r| r.rows)
        .unwrap_or_default()
}

fn s(v: &DataValue) -> String {
    v.get_str().unwrap_or("").to_string()
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let open = args.first().is_some_and(|a| a == "--open");
    if open {
        args.remove(0);
    }
    for path in args {
        if open {
            let t = std::time::Instant::now();
            let store = CozoKbStore::open(&path).expect("open");
            let ms = t.elapsed().as_millis();
            println!("open+pass {ms} ms");
            drop(store);
        }
        let db = match DbInstance::new("sqlite", &path, "") {
            Ok(db) => db,
            Err(e) => {
                eprintln!("{path}: open failed: {e}");
                continue;
            }
        };
        let mut bodies: HashMap<String, (String, String, bool)> = HashMap::new();
        for r in rows(
            &db,
            "?[id, body, source, has_crdt] := *nodes{id, body, source, has_crdt}",
        ) {
            let crdt = r[3].get_bool().unwrap_or(false);
            bodies.insert(s(&r[0]), (s(&r[1]), s(&r[2]), crdt));
        }
        let links = rows(&db, "?[src, dst, rel] := *links{src, dst, rel_type: rel}");
        let typed: Vec<_> = links.iter().filter(|r| s(&r[2]) != "references").collect();
        let mut declared_cache: HashMap<String, HashSet<(String, String)>> = HashMap::new();
        let (mut declared, mut src_missing, mut src_seed, mut src_crdt) = (0, 0, 0, 0);
        let mut undeclared_nodes = HashSet::new();
        for r in &typed {
            let (src, dst, rel) = (s(&r[0]), s(&r[1]), s(&r[2]));
            let Some((body, source, crdt)) = bodies.get(&src) else {
                src_missing += 1;
                continue;
            };
            let set = declared_cache.entry(src.clone()).or_insert_with(|| {
                let node = Node::new(src.clone(), "", NodeKind::Note, body.clone());
                CozoKbStore::derived_links(&node)
                    .into_iter()
                    .map(|(d, r, _, _)| (d, r))
                    .collect()
            });
            if set.contains(&(dst, rel)) {
                declared += 1;
                continue;
            }
            if source.eq_ignore_ascii_case("seed") {
                src_seed += 1;
            }
            if *crdt {
                src_crdt += 1;
            }
            undeclared_nodes.insert(src);
        }
        let crdt_nodes = bodies.values().filter(|b| b.2).count();
        let repaired = rows(
            &db,
            "?[count(id)] := *node_versions{id, change_summary}, \
             change_summary = 'typed links written into the body (#854)'",
        )
        .first()
        .and_then(|r| r[0].get_int())
        .unwrap_or(0);
        println!(
            "{}: nodes={} crdt_nodes={} links={} typed={} declared={} undeclared={} \
             (src_missing={} src_seed={} src_crdt={} nodes_affected={}) repair_versions={}",
            std::path::Path::new(&path)
                .file_stem()
                .and_then(|f| f.to_str())
                .unwrap_or("?"),
            bodies.len(),
            crdt_nodes,
            links.len(),
            typed.len(),
            declared,
            typed.len() - declared - src_missing,
            src_missing,
            src_seed,
            src_crdt,
            undeclared_nodes.len(),
            repaired,
        );
    }
}
