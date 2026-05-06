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

MAE has several security-relevant subsystems:

### AI Permission Tiers
The AI agent operates under a configurable permission tier (`config.toml` or `MAE_AI_PERMISSIONS` env var):
- **readonly** — AI can read buffers and navigate, but cannot modify files
- **write** — AI can edit buffers and create files
- **shell** — AI can execute shell commands (default)
- **privileged** — Full access including configuration changes

### Babel Code Execution
Org-babel code block execution (`SPC e e`) runs user code in a subprocess. Safety policies:
- Execution requires explicit user confirmation
- Shell blocks are gated by the AI permission tier
- No network access by default in babel sessions

### Shell Access
The embedded terminal (`SPC o t`) spawns a real shell process. The AI can observe output and send input only when the permission tier is `shell` or `privileged`.

### MCP Bridge
The MCP socket (`$XDG_RUNTIME_DIR/mae-mcp.sock`) is user-local (Unix permissions). It exposes the same tool surface as the built-in AI agent.
