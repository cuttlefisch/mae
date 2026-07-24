# MAE for VS Code

Pairs VS Code (GitHub Copilot Agent mode, or any other MCP-capable extension) with a
[MAE](https://github.com/cuttlefisch/mae) instance for the current workspace — KB
search/CRUD, dev-guidance-KB-driven behavior — with zero manual `.vscode/mcp.json` setup.

See `docs/EXTERNAL_EDITOR_MCP_PAIRING.md` (repo root) for the full pairing story, including
the hand-edited-`.vscode/mcp.json` path this extension supersedes for VS Code specifically
(that path still works, and is still the right choice for other MCP hosts).

## What it does

On activation, this extension registers a dynamic MCP server definition provider
(`vscode.lm.registerMcpServerDefinitionProvider`). When VS Code (or Copilot) needs to talk
to MAE:

1. It resolves the stable, project-keyed headless socket path for the current workspace by
   asking the real `mae` binary (`mae --headless --print-socket-path`) — never by
   reimplementing that path-hashing scheme itself.
2. It probes that socket. If nothing is listening, it spawns `mae --headless` (never a GUI
   window) detached, so it outlives the extension host, and waits for it to come up.
3. It hands VS Code an `mae-mcp-shim` command pointed at that socket
   (`MAE_MCP_SOCKET=<path>`).

It never reads or writes `.vscode/mcp.json` — discovery and lifecycle are entirely
in-memory, via the dynamic provider API.

## Requirements

- `mae` and `mae-mcp-shim` on `PATH` (`make install` from the MAE repo root, or your package
  manager), or configure `mae.headlessBinaryPath` / `mae.shimPath` to explicit paths.
- VS Code `^1.104.0`+ (the minimum version this extension has verified the
  `McpServerDefinitionProvider` API is present in as a stable, non-proposed API — see
  "Verifying the VS Code API floor" below before bumping `engines.vscode`).

## Settings

| Setting | Default | Purpose |
|---|---|---|
| `mae.shimPath` | `"mae-mcp-shim"` | Path to the shim binary (resolved via `PATH` if bare). |
| `mae.headlessBinaryPath` | `"mae"` | Path to the `mae` binary used to auto-spawn a headless instance. |
| `mae.permissionCeiling` | `""` (unset) | Optional self-declared permission ceiling (ADR-051) — `ReadOnly`/`Write`/`Shell`/`Privileged`. Can only *tighten* MAE's own server-side policy, never loosen it. |

**Note on `mae.*` settings and workspace trust:** both path settings are validated to
resolve to a real, existing, executable file before any process is spawned — and every
spawn this extension makes uses `shell: false` with an argv array, never a shell string, so
a value from an untrusted workspace's `.vscode/settings.json` can't be used for shell
injection either way. See `src/shimCommand.ts`'s module doc and
`test/unit/*.test.ts`'s adversarial tests for the exact threat model and proof.

## Development

```bash
cd editors/vscode
npm install
npm run compile        # tsc typecheck + build to out/
npm run test:unit       # fast unit suite (no real VS Code host)
npm run test:integration  # real VS Code extension host smoke test (needs a display;
                           # use `xvfb-run -a npm run test:integration` headless on Linux)
npm run package          # produces a .vsix via vsce (not published to the Marketplace
                          # by this repo — that's a separate, human-driven step)
```

Open this directory in VS Code and press F5 to launch an Extension Development Host for
interactive testing.

### Verifying the VS Code API floor

`McpServerDefinitionProvider`/`McpStdioServerDefinition` are stable (non-proposed) APIs as
of the `@types/vscode` version pinned in `package.json`'s `devDependencies`. If you bump
`engines.vscode`, re-pin `@types/vscode` to the *same* version (not a caret range — the
`.d.ts` must match the declared minimum exactly) and confirm the symbols are still present:

```bash
grep -c "McpServerDefinitionProvider\|McpStdioServerDefinition" node_modules/@types/vscode/index.d.ts
```

A result of `0` means that version doesn't have the stable API yet — bump higher and
re-check rather than assuming.

## Marketplace publishing

Not part of this repo's automation. `npm run package` produces a `.vsix` a maintainer can
publish manually with `vsce publish` once a real Microsoft publisher account/token exists —
an intentionally separate, human-driven step.
