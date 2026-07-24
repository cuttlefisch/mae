/**
 * "MAE for VS Code" — registers a dynamic MCP server definition provider
 * (ADR-050 D1 full / Phase I / #384) that auto-spawns a headless MAE
 * instance (never a GUI window) for the current workspace when none is
 * running, and points `mae-mcp-shim` at its stable socket. Never touches
 * `.vscode/mcp.json` — see `headlessDiscovery.ts`'s module doc.
 */

import * as vscode from 'vscode';

import {
  DEFAULT_HEADLESS_TIMEOUT_MS,
  ensureGuidanceConfigured,
  ensureHeadlessRunning,
} from './headlessDiscovery';
import { InvalidExecutableError, resolveShimCommand } from './shimCommand';

const PROVIDER_ID = 'mae-editor-provider';
const SERVER_LABEL = 'MAE';

function firstWorkspaceFolder(): vscode.WorkspaceFolder | undefined {
  // Deliberate: only ever the first folder. MAE's `Editor` has no internal
  // multi-project model (ADR-055's own documented trade-off) — a
  // multi-root workspace pairs with whichever project the first folder is.
  return vscode.workspace.workspaceFolders?.[0];
}

class MaeMcpServerDefinitionProvider implements vscode.McpServerDefinitionProvider {
  constructor(private readonly context: vscode.ExtensionContext) {}

  provideMcpServerDefinitions(): vscode.McpServerDefinition[] {
    const folder = firstWorkspaceFolder();
    if (!folder) {
      // No workspace open: a safe, documented no-op. Mitigates a real,
      // confirmed VS Code platform quirk (microsoft/vscode#266221) where an
      // extension contributing `mcpServerDefinitionProviders` can be
      // activated even in an empty window with no folder open.
      return [];
    }
    const config = vscode.workspace.getConfiguration('mae', folder.uri);
    const shimPath = config.get<string>('shimPath', 'mae-mcp-shim');
    // Env/cwd are resolved lazily in resolveMcpServerDefinition (the
    // documented place for async "ensure it's actually running" work) —
    // this is an optimistic placeholder VS Code may show before resolution.
    return [new vscode.McpStdioServerDefinition(SERVER_LABEL, shimPath, [], {})];
  }

  async resolveMcpServerDefinition(
    _definition: vscode.McpServerDefinition
  ): Promise<vscode.McpServerDefinition | undefined> {
    const folder = firstWorkspaceFolder();
    if (!folder) {
      return undefined;
    }

    const config = vscode.workspace.getConfiguration('mae', folder.uri);
    const shimPath = config.get<string>('shimPath', 'mae-mcp-shim');
    const headlessBinary = config.get<string>('headlessBinaryPath', 'mae');
    const permissionCeiling = config.get<string>('permissionCeiling', '').trim();
    const timeoutMs = config.get<number>('headlessTimeoutMs', DEFAULT_HEADLESS_TIMEOUT_MS);
    const workspaceRoot = folder.uri.fsPath;

    let ensured;
    try {
      ensured = await ensureHeadlessRunning(headlessBinary, workspaceRoot, undefined, timeoutMs);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      void vscode.window.showErrorMessage(`MAE: failed to start a headless instance — ${message}`);
      return undefined;
    }

    // K3 (post-ship quality pass): first-activation-per-workspace guidance
    // auto-configure. Deliberately AFTER the headless instance is confirmed
    // running (not before) -- this step is best-effort and must never be
    // the reason MCP pairing itself fails. globalState (not workspaceState)
    // is used so re-opening the same folder as a different workspace root
    // still tracks it correctly by the folder's own path, matching how
    // ensureHeadlessRunning's own socket keying works per-project, not
    // per-VS-Code-workspace-session.
    if (config.get<boolean>('autoConfigureGuidance', true)) {
      const stateKey = `mae.guidanceConfigured:${workspaceRoot}`;
      if (!this.context.globalState.get<boolean>(stateKey, false)) {
        try {
          const result = await ensureGuidanceConfigured(headlessBinary, workspaceRoot, undefined, timeoutMs);
          if (result.code !== 0) {
            console.warn(`MAE: --ensure-guidance-config exited ${result.code}: ${result.stderr}`);
          }
        } catch (err) {
          const message = err instanceof Error ? err.message : String(err);
          console.warn(`MAE: --ensure-guidance-config failed: ${message}`);
        }
        // Mark attempted regardless of outcome: this is a best-effort,
        // set-if-unset server-side operation (K3's CLI flag never
        // overwrites an existing explicit value) -- retrying on every
        // single activation would add no value once we've tried once, and
        // a transient failure shouldn't spam this on every workspace open.
        void this.context.globalState.update(stateKey, true);
      }
    }

    let plan;
    try {
      plan = resolveShimCommand(shimPath);
    } catch (err) {
      const message = err instanceof InvalidExecutableError ? err.message : String(err);
      void vscode.window.showErrorMessage(`MAE: invalid "mae.shimPath" setting — ${message}`);
      return undefined;
    }

    const env: Record<string, string> = { MAE_MCP_SOCKET: ensured.socketPath };
    if (permissionCeiling) {
      env.MAE_MCP_PERMISSION_CEILING = permissionCeiling;
    }

    const resolved = new vscode.McpStdioServerDefinition(SERVER_LABEL, plan.command, plan.args, env);
    resolved.cwd = folder.uri;
    return resolved;
  }
}

export function activate(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.lm.registerMcpServerDefinitionProvider(
      PROVIDER_ID,
      new MaeMcpServerDefinitionProvider(context)
    )
  );
}

export function deactivate(): void {
  // Nothing to tear down: the headless MAE instance is intentionally
  // long-lived and outlives this extension host (detached spawn) — VS Code
  // closing is not a reason to kill a project's shared headless instance.
}
