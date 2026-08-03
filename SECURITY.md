# Security Policy

## Supported Versions

MAE is in **early alpha**. Only the latest release on `main` receives security fixes.

## Reporting a Vulnerability

Report security issues via [GitHub Issues](https://github.com/cuttlefisch/mae/issues) with the `security` label, or email the maintainer directly if the issue is sensitive.

For sensitive reports, include:
- Description of the vulnerability
- Steps to reproduce
- Impact assessment

## Security Model

MAE has several security-relevant subsystems. This section documents the current posture honestly — what's strong, what's moderate, and what's known-limited.

### Strong Protections

**Permission tiers** — The AI agent operates under a configurable permission tier:
- **readonly** — AI can read buffers and navigate, but cannot modify files
- **write** — AI can edit buffers and create files
- **shell** — AI can execute shell commands (default)
- **privileged** — Full access including configuration changes

> [!WARNING]
> **The tier is not yet enforced on every path.** A pre-v0.15 audit found that the embedded AI
> session does not consult the permission policy at all, and that the `write` tier reaches shell
> effects through the Scheme-eval queue. An earlier revision of this document claimed tiers were
> "enforced before every tool execution with no bypass vectors" — that claim was wrong and has been
> removed.
>
> **Fixed since that audit:** an unrecognised tier value is now rejected at startup rather than
> silently falling back to `shell`; a project-local `.mae/init.scm` now requires explicit workspace
> trust; the `knowledge` tool-category allowlist no longer reaches code execution; and every Scheme
> primitive now carries a declared tier checked at the interpreter's single dispatch point.
>
> **Still open:** that last check has no effect yet, because nothing lowers the ambient tier — the
> embedded and MCP entry points do not yet carry a policy. Until they do, treat the tier as a
> guard-rail against accident, **not** as a boundary against a prompt-injected or adversarial model.

**Setting the tier.** Only two surfaces reach the enforced policy:
- `MAE_AI_PERMISSIONS=readonly|write|shell|privileged` (environment variable), or
- `auto_approve_tier` in `config.toml`'s `[ai]` section — **lowercase values only**. An unrecognised
  value is now a startup error naming the bad value and the valid ones, rather than failing open.

The `ai_tier` editor option (`(set-option! "ai_tier" …)` / `:set ai-tier …`) currently changes only the
status-bar badge and does **not** alter the enforced policy. Earlier revisions of this document referred
to an option named `permission_tier`; no such option exists.

**Workspace trust** — A project-local `.mae/init.scm` is arbitrary Scheme, and Scheme can spawn processes, so evaluating one is equivalent to running the project's code. MAE evaluates it **only** from a directory listed in `~/.config/mae/trusted-projects` (one absolute path per line; `#` comments allowed). Untrusted directories are skipped with a message naming the file and the line to add. Trust is exact-match: it is deliberately **not** inherited by subdirectories, so trusting a project does not trust a vendored dependency cloned inside it. A missing, unreadable, or malformed trust list trusts nothing.

Trust can only be granted by editing that file. There is no command, Scheme primitive, or MCP tool that grants it — an agent able to grant trust could then write `.mae/init.scm` and escalate across a restart. For the same reason, AI-originated writes to MAE's own configuration (`~/.config/mae/**` and any `.mae/**`) are refused across `create_file`, `rename_file`, and buffer saves; a human editing their own config, including `:set-save`, is unaffected.

The legacy v0.6 fallbacks that loaded a bare `init.scm` or `scheme/init.scm` from the working directory have been removed — those filenames are too ordinary to be safe. Move such a file to `~/.config/mae/init.scm`.

**Watchdog thread** — A background thread monitors AI operations for stalls. If an AI operation exceeds 10 seconds without progress, the watchdog captures a backtrace and triggers auto-recovery. The user can also cancel via Esc or Ctrl-C (input lock).

**Stagnation scoring** — Semantic progress checkpoints are evaluated every 10 rounds. If the AI makes no meaningful progress (repeating the same actions), it receives escalating warnings and is eventually aborted.

**Oscillation detection** — Detects A-B-A-B action patterns (the AI undoing and redoing the same change) and issues a warning, then aborts if the pattern continues.

**Budget guards** — Per-session cost limits with configurable warn and hard-cap thresholds. Prevents runaway API spending.

**Input lock** — During AI operations, keyboard input is locked to prevent interference. Esc or Ctrl-C cancels the operation cleanly.

**CI advisory enforcement** — `cargo-deny` runs in CI to check for known security advisories in dependencies.

### Moderate Protections

**Shell blocklist** — 6 hardcoded catastrophic patterns are blocked before shell execution:
- `rm -rf /`, `rm -fr /`, `mkfs.`, `dd if=`, `:(){ :`, `>(){ :`
- This is substring matching — a defense-in-depth measure, not a sandbox.

**Context trimming** — Token-aware context management prevents unbounded memory growth. However, there is no secret filtering — API keys or sensitive data in buffer content may be sent to the AI provider.

**Babel code execution** — Org-babel code blocks have configurable trust policies:
- `Never` — never execute automatically
- `NoExport` — skip during export
- `Yes` — execute (requires explicit user confirmation)
- `Query` — prompt the user each time

### Known Limitations

**No filesystem sandboxing** — The AI agent can read and write any file the user's process can access. There is no seccomp, landlock, or container-based isolation. If you run MAE with untrusted AI prompts or org files, run it inside a container (see below).

**MCP socket** — The Unix socket at `/tmp/mae-{PID}.sock` is protected by filesystem permissions only. Any process running as the same user can connect. There is no per-client authentication or token-based auth.

**Shell blocklist is bypassable** — The blocklist uses simple substring matching. Commands chained with `&&`, `||`, or `;` after the blocked pattern, or commands using variable expansion, can bypass it. This is by design — the blocklist catches accidental catastrophic commands, not adversarial input.

**Transcripts contain raw output** — Conversation transcripts saved to `~/.local/share/mae/transcripts/` include raw tool call results. If a buffer contains secrets (API keys, passwords), those may appear in transcripts. Review transcripts before sharing.

**Babel has no process isolation** — Code block execution runs in a subprocess with the same permissions as MAE. There is no resource limiting (CPU, memory, network) beyond what the OS provides.

### Recommendations

- **Secrets are never plaintext in config.toml.** `config.toml` is a legacy bootstrap; do not store API keys or the collab PSK in it directly.
- **API keys:** Use `api_key_command` with a password manager (e.g., `api_key_command = "pass show anthropic/api-key"`), not plaintext `api_key`.
- **Collab secrets:** Never put `collab_psk` plaintext in config.toml — use `collab_psk_command` (shell to pass/keychain), or preferably `collab_auth_mode = "key"` (Ed25519 trusted-peer mTLS) with the keystore at `$XDG_DATA_HOME/mae/collab/trusted_keys`.
- **Permission tier:** Set it via the `MAE_AI_PERMISSIONS` environment variable (e.g. `MAE_AI_PERMISSIONS=write`), or lowercase `auto_approve_tier` under `[ai]` in `config.toml`. The `ai_tier` editor option does not affect enforcement — see the warning above. Until the tracked advisory is resolved, do not rely on any tier as a boundary against an adversarial or prompt-injected model; run MAE in a container for genuinely untrusted input.
- **Untrusted files:** Run MAE in a container when opening untrusted org files or working with untrusted AI prompts (see below).
- **Transcripts:** Review files in `~/.local/share/mae/transcripts/` before sharing or committing them.
- **MCP access:** The MCP socket is ephemeral (per-process PID). Only grant `mae-mcp-shim` access to tools appropriate for your trust level.

### Running in a Container

MAE includes a Dockerfile for isolated execution:

```sh
# Quick: run mae in a container with a project directory mounted read-only
docker compose build runtime
docker run --rm -it -v /path/to/project:/work:ro mae mae /work/file.org

# Persistent config across runs
docker run --rm -it \
  -v ~/.config/mae:/home/mae/.config/mae \
  -v /path/to/project:/work \
  mae mae /work/file.org

# Maximum isolation (no network)
docker run --rm -it --network=none \
  -v /path/to/project:/work:ro \
  mae mae /work/untrusted.org
```

The container runs as a non-root `mae` user with pre-created XDG directories.
Terminal mode only — GUI requires display forwarding (X11/Wayland).
