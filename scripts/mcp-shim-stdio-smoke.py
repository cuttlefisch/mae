#!/usr/bin/env python3
"""Generic-MCP-client stdio smoke test for mae-mcp-shim.

Invoked via scripts/mcp-shim-stdio-smoke.sh -- see that wrapper for usage.
Not meant to be run directly (it takes the shim binary path as argv[1]).
"""
import json
import subprocess
import sys


def main(shim_path):
    proc = subprocess.Popen(
        [shim_path],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        bufsize=1,
        text=True,
    )

    def send(obj):
        proc.stdin.write(json.dumps(obj) + "\n")
        proc.stdin.flush()

    def recv():
        line = proc.stdout.readline()
        if not line:
            return None
        return json.loads(line)

    try:
        # 1. initialize
        send(
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "clientInfo": {"name": "mcp-shim-stdio-smoke", "version": "1.0"},
                    "capabilities": {},
                },
            }
        )
        resp = recv()
        assert resp is not None, "no response to initialize (is a mae instance running?)"
        assert "error" not in resp, f"initialize failed: {resp}"
        result = resp["result"]
        print("initialize OK — serverInfo:", result.get("serverInfo"))
        instructions = result.get("instructions")
        print(
            "initialize.instructions present:",
            instructions is not None,
            f"({len(instructions) if instructions else 0} chars)"
            + (f": {instructions!r}" if instructions else ""),
        )

        # 2. notifications/initialized (no id, no response expected)
        send({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})

        # 3. tools/list
        send({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
        resp = recv()
        assert resp is not None and "error" not in resp, f"tools/list failed: {resp}"
        tools = resp["result"]["tools"]
        print(f"tools/list OK — {len(tools)} tools")
        kb_search = next((t for t in tools if t["name"] == "kb_search"), None)
        assert kb_search is not None, "kb_search tool missing from tools/list"
        ann = kb_search.get("annotations")
        print("kb_search annotations:", ann)
        assert ann and ann.get("readOnlyHint") is True, (
            "kb_search must be annotated readOnlyHint=true (ADR-050 D2) — a stale/pre-Phase-A "
            "mae build would fail this exact check"
        )

        # 4. tools/call: introspect (a safe, always-available read tool)
        send(
            {
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "introspect", "arguments": {}},
            }
        )
        resp = recv()
        assert resp is not None and "error" not in resp, f"tools/call introspect failed: {resp}"
        print("tools/call(introspect) OK")

        # 5. tools/call: kb_search — a real read-only KB round trip, the
        #    actual capability this whole pairing story is for.
        send(
            {
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {"name": "kb_search", "arguments": {"query": "window", "limit": 3}},
            }
        )
        resp = recv()
        assert resp is not None and "error" not in resp, f"tools/call kb_search failed: {resp}"
        print("tools/call(kb_search) OK")

        # 6. tools/call: kb_get — the other half of the "kb_search/kb_get
        #    round trip" this pairing exists for. A nonexistent id is
        #    deliberate: this asserts the *protocol* round trip (a valid
        #    JSON-RPC response, not an error) rather than depending on any
        #    particular KB content being registered.
        send(
            {
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {"name": "kb_get", "arguments": {"id": "concept:does-not-exist"}},
            }
        )
        resp = recv()
        assert resp is not None and "error" not in resp, f"tools/call kb_get failed: {resp}"
        print("tools/call(kb_get) OK")

        print("\nALL CHECKS PASSED")
    finally:
        proc.stdin.close()
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <path-to-mae-mcp-shim>", file=sys.stderr)
        sys.exit(2)
    try:
        main(sys.argv[1])
    except AssertionError as e:
        print(f"SMOKE TEST FAILED: {e}", file=sys.stderr)
        sys.exit(1)
