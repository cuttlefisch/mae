# Tool Addition Checklist

When adding a new AI tool to MAE, follow these steps:

## 1. Define
- Add `ToolDefinition` to the correct category file in `crates/ai/src/tools/`
- Set name, description, parameters (with types + required flags), and permission tier

## 2. Implement
- Add handler in `crates/ai/src/tool_impls/` (matching category file)
- Wire dispatch in `crates/ai/src/tool_impls/mod.rs`

## 3. Classify tier
- Add to `classify_tool_tier()` in `crates/ai/src/tools/categories.rs`
- Core = safe read-only ops; Extended = write/shell/privileged

## 4. Classify category
- Verify `classify_tool_category()` handles the name prefix
- Categories: editing, navigation, project, debug, lsp, kb, shell, meta

## 5. Test
- Unit tests: happy path, missing args, precondition failure
- Self-test coverage: add entry in `crates/mae/src/executor/mod.rs`

## 6. Prompt guidance
- Add to `crates/mae/src/prompts/pair-programmer.xml` if the tool has non-obvious behavior
- Especially: blocking behavior, side effects, ordering constraints

## 7. Verify
- `make check` — no warnings
- `make test` — all pass
- Tool appears in `command_list` output
- Tool is callable by the AI agent at the correct permission tier
