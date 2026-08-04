# ADR-089: Project-local init files require explicit workspace trust

**Status:** Proposed. Security-blocking for v0.15.
**Relates to:** ADR-084 (permission enforcement — the tier does not and cannot bound this path, since
init files are evaluated before any policy exists), ADR-088 (carried authority — the same provenance
question asked of tool arguments rather than of files).
**Tracking:** issue #592 (pre-v0.15 audit epic). Fixed in v0.15; see ADR-084 on why this was
never disclosed as an advisory. Note this defect needed no AI agent, no MCP client and no prompt
injection — it is grouped with the AI-permission work only because that audit is where it surfaced.

## Context

`load_init_files` (`crates/mae/src/bootstrap.rs:1040-1043`) unconditionally appends the current working
directory's `.mae/init.scm` to the layered init set and evaluates it if present:

```rust
    // Layer 2: project-local (.mae/init.scm in cwd)
    if let Ok(cwd) = std::env::current_dir() {
        let project_init = cwd.join(".mae").join("init.scm");
        layers.push(project_init);
    }
```

There is no trust prompt, no trusted-directory list, and no restriction on what the file may evaluate.
`crates/scheme/src/runtime/misc_primitives.rs:108-109` spawns `sh -c`, and init files are evaluated by
the same VM.

**Verified empirically.** A directory containing only `.mae/init.scm` with a `(shell-command …)`
payload, opened with an isolated `HOME`/`XDG_*` and `MAE_AI_PERMISSIONS=readonly`, executed the command
at startup. The configured permission tier is irrelevant — init files run during bootstrap, before any
`PermissionPolicy` is constructed.

Two compounding exposures in the same function:

- On a **fresh install** (no user `init.scm`), the legacy v0.6 fallbacks additionally load bare
  `init.scm` and `scheme/init.scm` from the working directory (`bootstrap.rs:1048-1052`), widening the
  set of attacker-controlled filenames to ones far more likely to appear in an ordinary repository.
- An AI agent at `write` tier can *create* `.mae/init.scm`: `create_file` is `PermissionTier::Write`
  (`crates/ai/src/tools/core_tools.rs:166-170`) with no path guard. That converts a single injected
  tool call into escalation that survives restart — the shape of CVE-2025-53773, where GitHub Copilot
  was induced to write `chat.tools.autoApprove` into `.vscode/settings.json`.

The attack chain needs no AI and no injection: clone a repository, open MAE in it, arbitrary code
execution as the user.

This is precisely the exposure VS Code's Workspace Trust exists to prevent — *"Merely opening a folder
doesn't imply you trust its contents"* — and the reason its Restricted Mode disables AI agents
specifically. Every trust primitive surveyed across comparable tools (VS Code Workspace Trust, Devin
CLI's `--respect-workspace-trust`, Windsurf Restricted Mode) binds trust to **location**, and MAE has no
equivalent concept. The npm ecosystem reached the same conclusion about a far more deliberate user
action: `npm install` is now blocked from running install scripts by default, because invoking a tool on
content is not consent to execute that content.

## Decision (proposed)

1. **A project-local init file is evaluated only from a directory the user has explicitly trusted.**
   Trust is per-directory, persisted in user config (not in the project), and asked once. Untrusted
   directories load the user's own `~/.config/mae/init.scm` and nothing project-local.
2. **The default is untrusted.** A directory not on the list is untrusted; an unreadable or corrupt
   trust list means untrusted. Fail-safe defaults, consistent with ADR-084 D4.
3. **Retire the bare-`init.scm` / `scheme/init.scm` cwd fallbacks.** They are v0.6 compatibility for a
   layout MAE has not shipped in many releases, and they collide with ordinary repository filenames.
   Removing them is a smaller compatibility cost than gating them.
4. **Config and init paths become privileged write targets.** `create_file`, `buffer_write`, and any
   other write-tier tool refuse to create or modify `~/.config/mae/*`, `.mae/*`, and `config.toml`
   without the privileged tier. An agent must not be able to edit the file that governs the agent —
   Claude Code's `requiresUserInteraction` ratchet is the precedent: hints may tighten gating, never
   loosen it.
5. **The trust prompt states what is at stake** — that a project init file runs arbitrary code with the
   user's privileges — rather than asking a generic "trust this folder?" question.

## Consequences

**Positive.** Closes an unauthenticated local RCE reachable by cloning a repository. Brings MAE to
parity with the trust model every comparable editor already ships. Decision 4 independently blocks the
persistence half of several AI-permission findings.

**Negative / Risks.** Project-local init is a real feature that some users rely on; it will require a
one-time confirmation per project. Decision 3 is a breaking change for anyone still on the v0.6 layout.
A trust prompt is only as good as the user's attention to it — Anthropic reports 93% approval rates on
permission prompts — so this is a necessary boundary, not a sufficient one, and it should not be cited
as making project init files safe.

## Verification

Per principle #14, the primary tests are the attacker's:

- A directory with a hostile `.mae/init.scm` opened without prior trust: the payload does **not**
  execute, and the editor still starts.
- The same directory after explicit trust: it does execute (the feature still works).
- Trust is not inherited by a subdirectory or a sibling, and is not readable or writable from the
  project itself.
- A write-tier agent attempting `create_file` on `.mae/init.scm`, `~/.config/mae/init.scm`, and
  `config.toml`: refused in all three cases, including via path traversal (`./foo/../.mae/init.scm`)
  and via a symlink pointing at a config path.
- A corrupt/unparseable trust list yields untrusted, not trusted.
