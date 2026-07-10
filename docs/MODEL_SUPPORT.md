# Model Compatibility Matrix

MAE supports 33+ model prefixes across 8 providers. This document tracks which
models have been validated with the deterministic model exam.

## Compatibility Table

| Model | Provider | Verdict | Pass Rate | Date | Notes |
|-------|----------|---------|-----------|------|-------|
| claude-sonnet-4-20250514 | Claude | -- | -- | -- | Not yet tested |
| claude-opus-4-6 | Claude | **PASS** | 100% | 2026-05-14 | 10/10 tests via Claude Code MCP |
| gpt-4o | OpenAI | -- | -- | -- | Not yet tested |
| gemini-2.5-pro | Gemini | -- | -- | -- | Not yet tested |
| deepseek-chat | DeepSeek | -- | -- | -- | Not yet tested |
| qwen3:latest | Ollama | **FAIL** | 30% | 2026-07-10 | 3/10 via `mae-agent` CLI harness (see notes below) |
| llama3-groq-tool-use:8b | Ollama | **FAIL** | 10% | 2026-07-10 | 1/10 via `mae-agent` CLI harness (see notes below) |
| llama3.x | Ollama | -- | -- | -- | Not yet tested — see epic Phase A |

## Verdict Key

| Verdict | Criteria |
|---------|----------|
| **PASS** | >= 90% pass rate, 0 hallucinations |
| **MARGINAL** | 70-89% pass rate, or <= 1 hallucination |
| **FAIL** | < 70% pass rate, or > 1 hallucination |

## Exam Categories

The model exam consists of 12 deterministic tests across 6 categories:

| Category | Tests | What It Measures |
|----------|-------|------------------|
| `tool_selection` | 3 | Can the model pick the right tool for a task? |
| `parameter_accuracy` | 2 | Does the model pass correct parameters? |
| `output_interpretation` | 2 | Can the model read tool output and answer questions? |
| `multi_step` | 1 | Can the model chain multiple tool calls? |
| `pushback` | 2 | Does the model refuse dangerous requests? |
| `knowledge_base` | 2 | Can the model search/navigate the KB correctly? |

`knowledge_base` is conditional on the target project having real KB content
to search — an empty/fresh sandboxed project (e.g. a throwaway instance spun
up purely to run the exam) sees a 10-test/5-category plan instead of the full
12/6; both Ollama rows above were run this way.

## Running the Exam

### Inside MAE

```
:model-exam
```

Results auto-save to `~/.local/share/mae/exam-results/`.

### Via Claude Code (MCP)

1. Ensure MAE is running with MCP socket active
2. Get the test plan:
   ```
   model_exam(action="plan")
   ```
3. Execute each test prompt and record tool calls + final text
4. Submit results for grading:
   ```
   model_exam(action="grade", model="claude-sonnet-4-20250514", results=[
     {"test_id": 1, "tool_calls": [...], "final_text": "..."},
     ...
   ])
   ```
5. Results are auto-saved to `~/.local/share/mae/exam-results/`

### Via `mae-agent` CLI harness (recommended for local/Ollama models)

The 2026-07-10 Ollama rows above were generated this way. **Important finding:**
a real tool-use-tuned 8B model reliably refuses to self-navigate the full
12-test JSON plan across many unattended rounds — it narrates intent
("Now that we have the test plan, let's start executing the tests...")
instead of actually calling tools, even when explicitly told not to
summarize. The same model calls the correct tool immediately when given one
focused test prompt at a time. So driving the exam for a local model means
orchestrating per-test, not asking the model to run the whole plan itself:

1. Start MAE with the target model configured (`[ai] provider = "ollama"`,
   `model = "..."` in `config.toml`) so its MCP socket is live.
2. Fetch the plan directly: `model_exam(action="plan")`.
3. For each test with a `prompt` field, run it as its own **one-shot**
   `mae-agent` invocation — a genuinely separate, focused session, not a
   continuation of a shared conversation:
   ```sh
   mae-agent --socket /tmp/mae-{pid}.sock --provider ollama --model <name> \
     --only-tools buffer_read,project_search,project_files,project_info,run_test,run_build,shell_exec,cursor_info,editor_state,list_buffers,kb_search,kb_get,introspect,lsp_diagnostics,open_file,file_read,read_messages,model_exam,self_test_suite,create_file \
     --max-rounds <test.max_rounds + 2> \
     --prompt "<test.prompt>"
   ```
   `--only-tools` restricts the tool set to the ~20 tools the exam actually
   needs (see "A note on tool-count scaling" below) and `--prompt` runs one
   turn non-interactively, printing `[tool] name {args}` per call and the
   final answer under `=== FINAL ===`.
4. Parse each run's tool calls + final text into `{test_id, tool_calls,
   final_text}`, collect all of them, and submit once:
   ```
   model_exam(action="grade", model="<name>", results=[...])
   ```
5. Results auto-save to `~/.local/share/mae/exam-results/` as usual.

**A note on tool-count scaling.** MAE exposes 700+ tools over MCP. Both models
above were tested with a fixed ~20-tool allowlist (mirroring the same
restricted set MAE's own embedded `verifier` delegate profile already uses)
— *not* the full tool list. Confirmed directly against Ollama's `/api/chat`:
the same `llama3-groq-tool-use:8b` that made zero tool calls across several
prompts with all 730 tools offered called the correct tool immediately once
given just 1-2. This matches published findings elsewhere (Anthropic's own
MCP tool-search work measured 49%→74% accuracy from tool-list filtering
alone) — testing a local model against MAE's *full* tool surface without
some form of filtering will produce misleadingly bad numbers that reflect
tool-count overload, not the model's real tool-selection capability.

### Via Gemini CLI

1. Ensure MAE is running with MCP socket active
2. Add MCP server: `gemini mcp add mae-editor ~/.local/bin/mae-mcp-shim`
3. Run:
   ```sh
   gemini -m gemini-2.5-pro -p "Call model_exam(action='plan') to get 12 tests. \
     For each test, execute the prompt and record your tool calls. \
     Then call model_exam(action='grade', model='gemini-2.5-pro', results=[...]) \
     with your recorded responses."
   ```

## Reviewing Results

Raw exam results are stored as JSON at:
```
~/.local/share/mae/exam-results/{model}_{timestamp}.json
```

Each file contains an `ExamRun` with:
- `timestamp` — ISO-8601 when the exam was run
- `runner` — who ran it ("mae-builtin", "claude-code", "gemini-cli")
- `mae_version` — MAE version at time of exam
- `result` — aggregated `ExamResult` (verdict, pass rate, counts)
- `grades` — per-test `TestGrade` array (passed, reason, hallucination flags)

## Updating This Report

After running exams, use the built-in report generator:

```scheme
;; In MAE's Scheme REPL:
;; (Not yet exposed — use the Rust API directly)
```

Or programmatically via `format_exam_report()` in `crates/ai/src/executor/model_exam.rs`.
Copy the generated markdown table into the Compatibility Table section above.
