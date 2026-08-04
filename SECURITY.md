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
- **readonly** — AI can read buffers and navigate, but cannot modify files (**default**)
- **write** — AI can edit buffers and create files
- **shell** — AI can execute shell commands
- **privileged** — Full access including configuration changes

The tier is an **auto-approval ceiling**, not a wall. Since ADR-090 a permission check answers one
of three things:

| Answer | When | What happens |
|---|---|---|
| **allow** | at or below `auto_approve_tier` | runs, no prompt |
| **ask** | above `auto_approve_tier` | an interactive surface prompts a human; a non-interactive one **denies** |
| **deny** | a session-declared ceiling (ADR-051), a tool-category restriction (ADR-085), or a configuration that would not parse | refused outright; no prompt can raise it |

**The default changed in v0.15 and this is a breaking change.** MAE used to ship
`auto_approve_tier = "trusted"` (= shell), which auto-approved essentially everything. It now ships
**readonly**: reads run, writes and shell are *asked*. Nothing is silently denied, so the stricter
default does not break `run_build`/`run_test` — it asks about them. If you want the old behaviour,
set `auto_approve_tier = "shell"` explicitly and understand what you are granting.

Which surfaces can ask:

| Surface | `ask` |
|---|---|
| `mae-agent` TUI (the default AI surface, ADR-049) | prompts inline (`y` / `a` / `n`) |
| Embedded editor session (`:ai`, `delegate()`) | prompts in the conversation buffer; answer with `:ai-accept` / `:ai-reject`. `ai-mode = auto-accept` pre-answers **ask** only — it can never turn a **deny** into an allow |
| `mae-agent --prompt` | **denies** — no human attached, and it says so |
| External MCP dispatch (VS Code/Copilot, Claude Code via the shim) | **denies** — MAE implements no MCP elicitation, and the requesting client is not the local human |
| `mae --self-test` | **denies** — headless by definition |

If you drive MAE from an external editor's agent, raise `auto_approve_tier` deliberately for that
deployment; that path cannot prompt, so `ask` and `deny` look the same from the client's side.

> [!WARNING]
> **Tiers bound; they do not prevent.** A prompt is a usability mechanism that makes a restrictive
> default affordable — it is **not** a security control. Anthropic reports users approve ~93% of
> permission prompts. Symlink escapes, path-canonicalisation bypasses, and exfiltration through an
> allowlisted binary all stay *within* a granted tier, and composition is unbounded: a write-tier
> agent can edit a `Makefile` and then ask you to build. For genuinely untrusted input, run MAE in a
> container. See ADR-084's "What this does *not* fix" for the full, deliberately unflattering list.

**Setting the tier.** Only two surfaces reach the enforced policy:
- `MAE_AI_PERMISSIONS=readonly|write|shell|privileged` (environment variable), or
- `auto_approve_tier` in `config.toml`'s `[ai]` section. Parsing is case-insensitive and accepts the
  documented aliases (`standard`, `trusted`, `full`, `read-only`); an unrecognised value is a startup
  error naming the bad value and the valid ones, rather than failing open.

The `ai_tier` editor option (`(set-option! "ai_tier" …)` / `:set ai-tier …`) currently changes only the
status-bar badge and does **not** alter the enforced policy (ADR-084 D7, still open). Earlier revisions
of this document referred to an option named `permission_tier`; no such option exists.

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
- **Permission tier:** The shipped default is now `readonly` (reads auto-approved, writes and shell *asked*). Raise it deliberately via `MAE_AI_PERMISSIONS` (e.g. `MAE_AI_PERMISSIONS=write`) or `auto_approve_tier` under `[ai]` in `config.toml` — and remember that a non-interactive surface (external MCP, `--prompt`, `--self-test`) denies rather than asks, so those deployments need an explicit tier. The `ai_tier` editor option does not affect enforcement — see the warning above. Do not rely on any tier as a boundary against an adversarial or prompt-injected model; run MAE in a container for genuinely untrusted input.
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
