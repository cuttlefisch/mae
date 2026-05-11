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

**Permission tiers** — The AI agent operates under a configurable permission tier (`config.toml` or `MAE_AI_PERMISSIONS` env var). Tiers are enforced before every tool execution with no bypass vectors:
- **readonly** — AI can read buffers and navigate, but cannot modify files
- **write** — AI can edit buffers and create files
- **shell** — AI can execute shell commands (default)
- **privileged** — Full access including configuration changes

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

- **API keys:** Use `api_key_command` with a password manager (e.g., `api_key_command = "pass show anthropic/api-key"`), not plaintext `api_key` in config.toml.
- **Permission tier:** Set `permission_tier = "write"` unless your workflow requires shell access. Use `"readonly"` for review-only sessions.
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
