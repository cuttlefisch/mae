// Browser-side half of the Phase 0 interop spike (ADR-097 follow-on).
//
// This file IS the "browser" in "can a browser Yjs client converge with a real
// Rust KbNodeDoc". It deliberately uses the stock `yjs` package with no MAE
// code, no shim and no custom decoding — if it needed any of those, the answer
// to the spike's question would be "no", and that is exactly what we want the
// test to be able to discover.
//
// Invoked by `shared/sync/tests/browser_interop.rs`, which owns every
// convergence assertion. This script only *reads* what a browser can see and
// *produces* the update a browser edit would generate; it asserts nothing about
// convergence itself, so it cannot accidentally launder a Rust-side bug into a
// pass.
//
// Usage: node driver.mjs <command> <fixture-dir>
//   read   — decode base.bin, write observed.json (what the browser can see)
//   edit   — decode base.bin, apply a real edit, write browser-update.bin
//
// Every offset used here is a UTF-16 code-unit offset, matching the
// `OffsetKind::Utf16` the Rust docs are created with (`shared/sync/src/text.rs`).
// The fixtures deliberately contain non-BMP characters so that a byte- or
// char-offset mismatch cannot pass silently.

import * as Y from "yjs";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [, , command, dir] = process.argv;
if (!command || !dir) {
  console.error("usage: node driver.mjs <read|edit> <fixture-dir>");
  process.exit(2);
}

/** Load a v1 update produced by Rust into a stock Yjs doc. */
function loadBase() {
  const doc = new Y.Doc();
  const update = new Uint8Array(readFileSync(join(dir, "base.bin")));
  Y.applyUpdate(doc, update);
  return doc;
}

/**
 * Read the node the way a browser client would: the root map is named "node"
 * (yrs `get_or_insert_map("node")`), `title`/`body` are Y.Text, `tags`/`links`/
 * `aliases` are Y.Array, `meta`/`props` are Y.Map.
 */
function observe(doc) {
  const node = doc.getMap("node");
  const text = (key) => {
    const v = node.get(key);
    return v instanceof Y.Text ? v.toString() : null;
  };
  const array = (key) => {
    const v = node.get(key);
    return v instanceof Y.Array ? v.toArray() : null;
  };
  const map = (key) => {
    const v = node.get(key);
    return v instanceof Y.Map ? Object.fromEntries(v.entries()) : null;
  };
  return {
    // `id` and the v2 scalars are plain values, not shared types.
    id: node.get("id") ?? null,
    schema_v: node.get("schema_v") ?? null,
    kind: node.get("kind") ?? null,
    todo: node.get("todo") ?? null,
    prio: node.get("prio") ?? null,
    title: text("title"),
    body: text("body"),
    tags: array("tags"),
    links: array("links"),
    aliases: array("aliases"),
    props: map("props"),
    // Reported so the Rust side can assert the browser saw real shared types,
    // not a degraded/opaque decode that happens to stringify correctly.
    types: {
      title: node.get("title")?.constructor?.name ?? null,
      body: node.get("body")?.constructor?.name ?? null,
      tags: node.get("tags")?.constructor?.name ?? null,
      props: node.get("props")?.constructor?.name ?? null,
    },
  };
}

if (command === "read") {
  const doc = loadBase();
  writeFileSync(join(dir, "observed.json"), JSON.stringify(observe(doc), null, 2));
} else if (command === "edit") {
  const doc = loadBase();
  const node = doc.getMap("node");
  const body = node.get("body");
  if (!(body instanceof Y.Text)) {
    console.error("body is not a Y.Text — a browser cannot edit this node");
    process.exit(1);
  }

  // The edit spec is chosen by the Rust side so the test, not this script,
  // decides what the browser does. `at` is a UTF-16 offset.
  const spec = JSON.parse(readFileSync(join(dir, "edit-spec.json"), "utf8"));
  if (spec.at > body.length) {
    console.error(`edit offset ${spec.at} past body length ${body.length}`);
    process.exit(1);
  }

  // Capture the pre-edit state vector so we emit ONLY the browser's own edit as
  // a diff — the same thing a real client sends upstream, rather than a full
  // state dump that would mask a broken incremental path.
  const before = Y.encodeStateVector(doc);
  doc.transact(() => {
    body.insert(spec.at, spec.insert);
  });
  const update = Y.encodeStateAsUpdate(doc, before);

  writeFileSync(join(dir, "browser-update.bin"), Buffer.from(update));
  writeFileSync(
    join(dir, "browser-observed.json"),
    JSON.stringify(observe(doc), null, 2),
  );
} else {
  console.error(`unknown command: ${command}`);
  process.exit(2);
}
