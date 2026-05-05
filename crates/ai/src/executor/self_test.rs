//! Self-test plan builder.

/// Build a structured JSON test plan for the self-test suite.
///
/// Returns a JSON object that any MCP-connected agent can parse and execute
/// mechanically -- no prose interpretation required.
pub(crate) fn build_self_test_plan(filter: &str) -> String {
    let filters: Vec<&str> = if filter.is_empty() {
        vec![]
    } else {
        filter.split(',').map(|s| s.trim()).collect()
    };
    let include = |name: &str| filters.is_empty() || filters.contains(&name);

    let mut categories = Vec::new();

    if include("introspection") {
        categories.push(serde_json::json!({
            "name": "introspection",
            "conditional": false,
            "tests": [
                {
                    "tool": "cursor_info",
                    "args": {},
                    "assert": "Returns JSON with cursor_row, mode, line_count fields"
                },
                {
                    "tool": "editor_state",
                    "args": {},
                    "assert": "Returns JSON with mode, theme, buffer_count >= 1"
                },
                {
                    "tool": "list_buffers",
                    "args": {},
                    "assert": "Returns at least 1 buffer"
                },
                {
                    "tool": "window_layout",
                    "args": {},
                    "assert": "Returns JSON with at least 1 window"
                },
                {
                    "tool": "command_list",
                    "args": {"format": "names"},
                    "assert": "Returns >= 30 commands; must include: save, quit, help, terminal, agent-list, agent-setup, self-test, lsp-goto-definition, debug-start, ai-prompt"
                },
                {
                    "tool": "ai_permissions",
                    "args": {},
                    "assert": "Returns text with current auto-approve tier"
                },
                {
                    "tool": "audit_configuration",
                    "args": {},
                    "assert": "Returns JSON with ai_agent, ai_chat, lsp_servers, dap_adapters, init_files, options_modified, issues fields"
                }
            ]
        }));
    }

    if include("editing") {
        categories.push(serde_json::json!({
            "name": "editing",
            "conditional": false,
            "setup": [
                "Before starting: clean up any leftovers from a previous run.",
                "Call close_buffer with name='mae-self-test-editing.txt' and force=true (ignore errors if buffer doesn't exist).",
                "Call shell_exec with command='rm -f /tmp/mae-self-test-editing.txt' to remove any stale test file."
            ],
            "tests": [
                {
                    "tool": "create_file",
                    "args": {"path": "/tmp/mae-self-test-editing.txt", "content": "hello world"},
                    "assert": "Success, file created"
                },
                {
                    "tool": "open_file",
                    "args": {"path": "/tmp/mae-self-test-editing.txt"},
                    "assert": "Buffer opened"
                },
                {
                    "tool": "buffer_read",
                    "args": {"start_line": 1, "end_line": 1},
                    "assert": "Contains 'hello world'"
                },
                {
                    "tool": "buffer_write",
                    "args": {"start_line": 1, "end_line": 1, "content": "hello MAE"},
                    "assert": "Success"
                },
                {
                    "tool": "buffer_read",
                    "args": {"start_line": 1, "end_line": 1},
                    "assert": "Contains 'hello MAE' (verifies write)"
                },
                {
                    "tool": "list_buffers",
                    "args": {},
                    "assert": "Test file buffer appears in list"
                },
                {
                    "tool": "switch_buffer",
                    "args": {"name": "*AI*"},
                    "assert": "Success"
                },
                {
                    "tool": "close_buffer",
                    "args": {"name": "mae-self-test-editing.txt", "force": true},
                    "assert": "Success (force=true closes even if modified)"
                }
            ],
            "cleanup": [
                "Call shell_exec with command='rm -f /tmp/mae-self-test-editing.txt'",
                "Verify you are on the *AI* buffer (switch_buffer if needed)"
            ]
        }));
    }

    if include("help") {
        categories.push(serde_json::json!({
            "name": "help",
            "conditional": false,
            "tests": [
                {
                    "tool": "kb_search",
                    "args": {"query": "buffer"},
                    "assert": "Returns at least 1 result"
                },
                {
                    "tool": "kb_list",
                    "args": {"prefix": "concept:"},
                    "assert": "Returns at least 5 concept nodes"
                },
                {
                    "tool": "kb_get",
                    "args": {"id": "concept:buffer"},
                    "assert": "Returns node with title, body, links"
                },
                {
                    "tool": "kb_links_from",
                    "args": {"id": "concept:buffer"},
                    "assert": "Returns at least 1 outgoing link"
                },
                {
                    "tool": "kb_links_to",
                    "args": {"id": "concept:buffer"},
                    "assert": "Returns at least 1 incoming link (from index)"
                },
                {
                    "tool": "kb_graph",
                    "args": {"id": "concept:buffer", "depth": 1},
                    "assert": "Returns nodes and edges arrays"
                },
                {
                    "tool": "help_open",
                    "args": {"id": "index"},
                    "assert": "Opens *Help* buffer for the user"
                },
                {
                    "tool": "switch_buffer",
                    "args": {"name": "*AI*"},
                    "assert": "Switch back to *AI* after help tests (important: subsequent tests need a non-Help buffer active)"
                }
            ],
            "cleanup": [
                "Close the *Help* buffer with close_buffer (name: '*Help*', force: true)",
                "Switch back to *AI* buffer if not already there"
            ]
        }));
    }

    if include("project") {
        categories.push(serde_json::json!({
            "name": "project",
            "conditional": true,
            "precondition": "Call project_info first. If it fails or returns no root, SKIP this entire category.",
            "tests": [
                {
                    "tool": "project_info",
                    "args": {},
                    "assert": "Returns JSON with root field"
                },
                {
                    "tool": "project_files",
                    "args": {"pattern": "*.rs"},
                    "assert": "Returns at least 1 file"
                },
                {
                    "tool": "project_search",
                    "args": {"pattern": "fn main", "max_results": 5},
                    "assert": "Returns at least 1 match"
                }
            ]
        }));
    }

    if include("lsp") {
        categories.push(serde_json::json!({
            "name": "lsp",
            "conditional": true,
            "precondition_steps": [
                "1. Call project_info — if no root, SKIP entire category.",
                "2. Call open_file('test_fixtures/lsp_test.rs').",
                "3. Call shell_exec('sleep 3') — rust-analyzer needs startup time.",
                "4. Call lsp_diagnostics() — if error, call shell_exec('sleep 5') then lsp_diagnostics() once more. Still fails → SKIP.",
                "IMPORTANT: Do NOT retry any individual LSP test more than once — if it returns empty, mark FAIL and move on."
            ],
            "tests": [
                {
                    "tool": "open_file",
                    "args": {"path": "test_fixtures/lsp_test.rs"},
                    "assert": "Buffer opened"
                },
                {
                    "tool": "lsp_diagnostics",
                    "args": {},
                    "assert": "Returns JSON (0 errors means LSP parsed OK)"
                },
                {
                    "tool": "lsp_document_symbols",
                    "args": {},
                    "assert": "Returns symbols including Counter, new, increment, get, count_to"
                },
                {
                    "tool": "lsp_hover",
                    "args": {"line": 15, "character": 12},
                    "assert": "Returns hover info for Counter struct (line 15 col 12). If empty, FAIL."
                },
                {
                    "tool": "lsp_definition",
                    "args": {"line": 35, "character": 28},
                    "assert": "Resolves Counter::new call (line 35 col 28) to constructor at line 20. If empty, FAIL."
                },
                {
                    "tool": "lsp_references",
                    "args": {"line": 15, "character": 12},
                    "assert": "Returns >= 3 references to Counter. If empty, FAIL."
                }
            ],
            "cleanup": [
                "close_buffer(name: 'lsp_test.rs', force: true), then switch_buffer(name: '*AI*')"
            ]
        }));
    }

    if include("performance") {
        categories.push(serde_json::json!({
            "name": "performance",
            "conditional": false,
            "tests": [
                {
                    "tool": "perf_stats",
                    "args": {},
                    "assert": "Returns JSON with rss_bytes field"
                },
                {
                    "tool": "perf_benchmark",
                    "args": {"benchmark": "buffer_insert", "size": 10000},
                    "assert": "duration_us < 2000000"
                },
                {
                    "tool": "perf_benchmark",
                    "args": {"benchmark": "syntax_parse", "size": 1000},
                    "assert": "duration_us < 100000"
                },
                {
                    "tool": "perf_benchmark",
                    "args": {"benchmark": "scroll_stress", "size": 50},
                    "assert": "p95_us < 50000 (no frame > 50ms during scroll)"
                },
                {
                    "tool": "introspect",
                    "args": {"section": "frame"},
                    "assert": "Returns JSON with render_phase_us containing syntax, layout, draw fields"
                },
                {
                    "tool": "perf_profile",
                    "args": {"action": "start"},
                    "assert": "Returns JSON with status 'recording_started'"
                },
                {
                    "tool": "perf_profile",
                    "args": {"action": "stop"},
                    "assert": "Returns JSON with status 'recording_stopped'"
                },
                {
                    "tool": "perf_profile",
                    "args": {"action": "report"},
                    "assert": "Returns JSON with total_frames, frame_stats, redraw_level_distribution, cache_stats, diagnosis fields"
                }
            ]
        }));
    }

    if include("dap") {
        categories.push(serde_json::json!({
            "name": "dap",
            "conditional": true,
            "precondition": "Call shell_exec('python3 -c \"import debugpy\"'). If it fails, SKIP entire category.",
            "tests": [
                {
                    "name": "start_session",
                    "tool": "dap_start",
                    "args": {"adapter": "debugpy", "program": "test_fixtures/dap_test.py", "stop_on_entry": true},
                    "assert": "Blocks until session starts AND debuggee stops at entry. Returns JSON with status 'stopped', reason 'entry', thread and frame info. If error, SKIP remaining DAP tests."
                },
                {
                    "name": "set_breakpoint",
                    "tool": "dap_set_breakpoint",
                    "args": {"source": "test_fixtures/dap_test.py", "line": 13},
                    "assert": "Breakpoint set on line 13"
                },
                {
                    "name": "continue_to_breakpoint",
                    "tool": "dap_continue",
                    "args": {},
                    "assert": "Blocks until debuggee stops at breakpoint. Returns JSON with status 'stopped', reason 'breakpoint', frame info with source and line. If timeout, FAIL."
                },
                {
                    "name": "check_output",
                    "tool": "dap_output",
                    "args": {"lines": 10},
                    "assert": "Returns output JSON"
                },
                {
                    "name": "disconnect",
                    "tool": "dap_disconnect",
                    "args": {"terminate_debuggee": true},
                    "assert": "Session ends cleanly"
                }
            ],
            "cleanup": [
                "dap_disconnect(terminate_debuggee: true) — ignore errors"
            ],
            "CRITICAL": "dap_start, dap_continue, dap_step BLOCK until the operation completes — do NOT call debug_state or sleep after them. If ANY DAP tool fails or times out, IMMEDIATELY call read_messages(level: 'warn', last_n: 20) to check the *Messages* log for adapter errors — the root cause is almost always logged there. Maximum 10 tool calls for this entire category."
        }));
    }

    if include("git") {
        categories.push(serde_json::json!({
            "name": "git",
            "conditional": true,
            "precondition": "Call git_status first. If it fails or returns error, SKIP this entire category.",
            "tests": [
                {
                    "tool": "git_status",
                    "args": {},
                    "assert": "Returns JSON with branch, staged, unstaged, untracked arrays"
                },
                {
                    "tool": "git_log",
                    "args": {"limit": 3},
                    "assert": "Returns at least 1 log entry (if in a valid repo)"
                },
                {
                    "tool": "git_diff",
                    "args": {},
                    "assert": "Returns a diff string (may be empty)"
                },
                {
                    "tool": "github_pr_status",
                    "args": {},
                    "assert": "Executes successfully (even if no PR exists, it should return a structured response from the gh cli)"
                }
            ]
        }));
    }

    if include("scrolling") {
        categories.push(serde_json::json!({
            "name": "scrolling",
            "conditional": false,
            "setup": [
                "Open assets/markup-demo.md (contains images and links)"
            ],
            "tests": [
                {
                    "tool": "open_file",
                    "args": {"path": "assets/markup-demo.md"},
                    "assert": "Buffer opened"
                },
                {
                    "tool": "execute_command",
                    "args": {"command": "move-to-last-line"},
                    "assert": "Success"
                },
                {
                    "tool": "cursor_info",
                    "args": {},
                    "assert": "cursor_row should be > 10 (file has many lines)"
                },
                {
                    "tool": "execute_command",
                    "args": {"command": "move-to-first-line"},
                    "assert": "Success"
                },
                {
                    "tool": "cursor_info",
                    "args": {},
                    "assert": "cursor_row == 0, scroll_offset == 0"
                },
                {
                    "tool": "execute_command",
                    "args": {"command": "scroll-down-line"},
                    "assert": "Success (repeat 5 times)"
                },
                {
                    "tool": "cursor_info",
                    "args": {},
                    "assert": "scroll_offset > 0"
                },
                {
                    "tool": "execute_command",
                    "args": {"command": "scroll-up-line"},
                    "assert": "Success (repeat 5 times)"
                },
                {
                    "tool": "cursor_info",
                    "args": {},
                    "assert": "scroll_offset back to 0 or close"
                },
                {
                    "tool": "perf_benchmark",
                    "args": {"benchmark": "scroll_stress", "size": 50},
                    "assert": "p95_us < 50000 (no frame > 50ms during scroll)"
                }
            ],
            "cleanup": [
                "close_buffer(name: 'markup-demo.md', force: true), then switch_buffer(name: '*AI*')"
            ]
        }));
    }

    if include("guidance") {
        categories.push(serde_json::json!({
            "name": "guidance",
            "conditional": false,
            "tests": [
                {
                    "tool": "command_list",
                    "args": {"format": "names"},
                    "assert": "Returns list containing define-key, describe-key (keybinding discovery)"
                },
                {
                    "tool": "kb_search",
                    "args": {"query": "keybinding"},
                    "assert": "Returns at least 1 result about keybindings"
                },
                {
                    "tool": "command_list",
                    "args": {"format": "names"},
                    "assert": "Returns list containing split-vertical, split-horizontal, focus-left, close-window, window-grow, window-maximize (window management discovery)"
                },
                {
                    "tool": "editor_state",
                    "args": {},
                    "assert": "Returns JSON with theme field (validates AI can inspect configuration)"
                },
                {
                    "tool": "kb_search",
                    "args": {"query": "configuration options"},
                    "assert": "Returns at least 1 result about configuration"
                }
            ]
        }));
    }

    let plan = serde_json::json!({
        "version": 2,
        "description": "MAE self-test plan. Call each tool with the given args, check the assertion, report [PASS]/[FAIL]/[SKIP] per test.",
        "output_format": "=== MAE Self-Test Report ===\nCategory: <name>\n  [PASS] <tool> -- <what was verified>\n  [FAIL] <tool> -- expected <X>, got <Y>\n  [SKIP] <tool> -- <reason>\n\nSummary: N passed, N failed, N skipped",
        "instructions": [
            "IMPORTANT: Do NOT call self_test_suite again once you have the plan. You already have everything you need.",
            "State is automatically saved before tests and restored after the session completes. Do NOT call editor_save_state or editor_restore_state.",
            "Step 1: For each category in order:",
            "  1a. Execute the category's 'setup' array (if any). Ignore errors — they clean up stale state.",
            "  1b. Run each test in sequence. Record PASS/FAIL/SKIP. If a tool fails or times out, call read_messages(level: 'warn') to see logged errors before retrying or skipping.",
            "  1c. Execute the category's 'cleanup' array (if any).",
            "Step 2: Final cleanup — delete test files via shell_exec: rm -f /tmp/mae-self-test-editing.txt",
            "Step 3: Output the report. Do NOT quit the editor."
        ],
        "cleanup": [
            "Delete test files via shell_exec: rm -f /tmp/mae-self-test-editing.txt",
            "Do NOT quit the editor"
        ],
        "categories": categories
    });

    serde_json::to_string_pretty(&plan).unwrap_or_else(|_| "{}".to_string())
}
