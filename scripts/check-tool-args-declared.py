#!/usr/bin/env python3
"""check-tool-args-declared.py -- every argument a tool reads is in its schema.

Tool calls refuse arguments their schema does not declare
(`validate_tool_args`, crates/ai/src/executor/tool_dispatch.rs). So an
implementation that reads a name its schema leaves out is silently
unreachable: the call is refused before the implementation runs. That
happened to lsp_diagnostics `scope`, terminal_read `lines` and two
promised aliases when refusal was introduced.

This walks the executor's dispatch arms (`"tool" => fn(..args..)`),
follows the functions they pass the arguments to, collects every
`args.get("name")`-style read, and compares it with the tool's
`ToolDefBuilder` props plus `LEGACY_ARG_ALIASES`. It is a source-level
heuristic; a confirmed false positive goes in NOT_AN_ARGUMENT with why.

Usage: scripts/check-tool-args-declared.py [repo-root]   (exit 1 on findings)
"""
import collections
import os
import re
import sys

ROOT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "..")
SRC = os.path.join(ROOT, "crates/ai/src")

# (tool, name) pairs the scan reports but that are not tool arguments.
NOT_AN_ARGUMENT = {
    # reads `body` from a node's JSON (kb.rs, execute_kb_add_link), not from the call
    ("kb_add_link", "body"),
}

files = []
for d, _, fs in os.walk(SRC):
    for f in fs:
        if f.endswith(".rs") and "test" not in f:
            files.append(os.path.join(d, f))
texts = {f: open(f, encoding="utf-8").read() for f in files}

declared = collections.defaultdict(set)
for t in texts.values():
    for m in re.finditer(r'ToolDefBuilder::new\(\s*"([a-z_0-9]+)"', t):
        end = t.find(".build()", m.end())
        for p in re.finditer(r'\.prop[a-z_]*\(\s*"([a-zA-Z_0-9]+)"', t[m.end():end]):
            declared[m.group(1)].add(p.group(1))
        declared[m.group(1)]

aliases = set()
dispatch = texts.get(os.path.join(SRC, "executor/tool_dispatch.rs"), "")
table = re.search(r"LEGACY_ARG_ALIASES[^=]*=\s*&\[(.*?)\];", dispatch, re.S)
if table:
    aliases = set(re.findall(r'\(\s*"([a-z_0-9]+)"\s*,\s*"([a-zA-Z_0-9]+)"\s*\)', table.group(1)))

fns = {}
for t in texts.values():
    for m in re.finditer(r"\bfn ([a-z_0-9]+)\s*(<[^>]*>)?\(", t):
        b, semi = t.find("{", m.end()), t.find(";", m.end())
        if b < 0 or (0 <= semi < b):
            continue
        depth, i = 0, b
        while i < len(t):
            if t[i] == "{":
                depth += 1
            elif t[i] == "}":
                depth -= 1
                if depth == 0:
                    break
            i += 1
        fns[m.group(1)] = fns.get(m.group(1), "") + t[b:i + 1]

KEY = re.compile(
    r'(?:args|arguments|a|input|params|call\.arguments|v|obj)\s*\.\s*get\(\s*"([a-zA-Z_0-9]+)"\s*\)'
    r'|(?:args|arguments|call\.arguments)\s*\[\s*"([a-zA-Z_0-9]+)"\s*\]'
    r'|\(\s*&?(?:args|arguments|call\.arguments)\s*,\s*"([a-zA-Z_0-9]+)"'
)
CALL = re.compile(r"\b([a-z_0-9]+)\s*\(")


def direct(body):
    return {next(g for g in m.groups() if g) for m in KEY.finditer(body)}


def passes_args(caller_body, callee):
    return re.search(r"\b" + callee + r"\s*\([^)]*\b(args|arguments|call)\b", caller_body)


memo = {}


def reads(fn, seen=frozenset()):
    if fn in memo:
        return memo[fn]
    if fn in seen or fn not in fns:
        return set()
    body = fns[fn]
    ks = direct(body)
    for c in set(CALL.findall(body)):
        if c in fns and c != fn and passes_args(body, c):
            ks |= reads(c, seen | {fn})
    memo[fn] = ks
    return ks


arms = collections.defaultdict(set)
for f, t in texts.items():
    if "/executor/" not in f:
        continue
    for m in re.finditer(r'"([a-z_0-9]+)"(?:\s*\|\s*"[a-z_0-9]+")*\s*=>\s*', t):
        names = re.findall(r'"([a-z_0-9]+)"', t[m.start():m.end()])
        nxt = re.search(r'\n\s*"[a-z_0-9]+"(?:\s*\|\s*"[a-z_0-9]+")*\s*=>|\n\s*_\s*=>', t[m.end():])
        body = t[m.end(): m.end() + (nxt.start() if nxt else 600)]
        ks = direct(body)
        for c in set(CALL.findall(body)):
            if c in fns and passes_args(body, c):
                ks |= reads(c)
        for n in names:
            arms[n] |= ks

bad = []
for tool in sorted(arms):
    if tool not in declared:
        continue
    extra = {k for k in arms[tool] - declared[tool]
             if (tool, k) not in aliases and (tool, k) not in NOT_AN_ARGUMENT}
    if extra:
        bad.append(f"  {tool}: reads {sorted(extra)}, schema has {sorted(declared[tool])}")

if len(declared) < 100 or len(arms) < 100:
    print(f"check-tool-args-declared: scan found only {len(declared)} schemas / {len(arms)} "
          "dispatch arms -- the source layout changed; fix the scanner", file=sys.stderr)
    sys.exit(2)
if bad:
    print("Tools read arguments their schema does not declare; such calls are refused before")
    print("the tool runs. Declare them (or list a kept alias in LEGACY_ARG_ALIASES):")
    print("\n".join(bad))
    sys.exit(1)
print(f"tool args: every argument read is declared ({len(arms)} dispatch arms checked).")
