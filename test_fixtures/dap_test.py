"""DAP self-test fixture — a simple program with inspectable state.

This file is used by MAE's `:self-test dap` category. The AI agent
launches debugpy, sets a breakpoint on the marked line, and inspects
variables after the breakpoint hits.

DO NOT MODIFY the line numbers without updating the self-test suite
in crates/ai/src/executor/mod.rs.
"""

def greet(name: str) -> str:
    """Build a greeting string."""
    greeting = f"Hello, {name}!"  # line 13 — breakpoint target
    return greeting


def main():
    names = ["MAE", "Emacs", "Vim"]
    results = []
    for name in names:
        msg = greet(name)  # line 21 — step-into target
        results.append(msg)
    total = len(results)  # line 23 — variable inspection target
    print(f"Greeted {total} editors")


if __name__ == "__main__":
    main()
