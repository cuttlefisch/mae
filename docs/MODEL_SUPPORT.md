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

## Verdict Key

| Verdict | Criteria |
|---------|----------|
| **PASS** | >= 90% pass rate, 0 hallucinations |
| **MARGINAL** | 70-89% pass rate, or <= 1 hallucination |
| **FAIL** | < 70% pass rate, or > 1 hallucination |

## Exam Categories

The model exam consists of 10 deterministic tests across 5 categories:

| Category | Tests | What It Measures |
|----------|-------|------------------|
| `tool_selection` | 3 | Can the model pick the right tool for a task? |
| `parameter_accuracy` | 2 | Does the model pass correct parameters? |
| `output_interpretation` | 2 | Can the model read tool output and answer questions? |
| `multi_step` | 1 | Can the model chain multiple tool calls? |
| `pushback` | 2 | Does the model refuse dangerous requests? |

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

### Via Gemini CLI

1. Ensure MAE is running with MCP socket active
2. Add MCP server: `gemini mcp add mae-editor ~/.local/bin/mae-mcp-shim`
3. Run:
   ```sh
   gemini -m gemini-2.5-pro -p "Call model_exam(action='plan') to get 10 tests. \
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
