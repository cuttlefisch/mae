/**
 * Real-binary integration test: proves the extension's own spawn logic
 * (`resolveStableSocketPath`, `spawnHeadlessInstance`, `resolveShimCommand`)
 * works against the REAL `mae` and `mae-mcp-shim` binaries, not just an
 * injected fake `spawnFn` (that's what `test/unit/` covers — real logic,
 * fake process). This is the gap a QA/CI-audit pass on this epic flagged
 * explicitly: the `@vscode/test-electron` smoke test only proves
 * `activate()` succeeds, never that a real headless instance actually comes
 * up and a real MCP tool call round-trips through the real shim.
 *
 * Deliberately NOT part of `test/integration/`'s `@vscode/test-electron`
 * suite — this doesn't need a real VS Code extension host, only Node's real
 * `child_process.spawn`, so it runs via plain mocha (like `test/unit/`) but
 * in its own directory so it's never accidentally swept into either the
 * fast unit suite or the extension-host smoke test.
 *
 * Skips cleanly (not a failure) when `MAE_BIN`/`MAE_SHIM_BIN` don't resolve
 * to real executables — mirrors the Rust side's own
 * `daemon_supervisor.rs::find_daemon_binary` skip-when-not-built precedent,
 * so a plain local `npm test` never requires the Rust workspace to be
 * built. CI builds (or downloads) the real binaries and sets these env
 * vars specifically to exercise this test for real.
 */

import * as assert from 'assert';
import * as cp from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import * as readline from 'readline';

import { resolveExecutable, resolveShimCommand } from '../../src/shimCommand';
import { probeSocket, resolveStableSocketPath, spawnHeadlessInstance } from '../../src/headlessDiscovery';

function tryResolve(configured: string): string | undefined {
  try {
    return resolveExecutable(configured);
  } catch {
    return undefined;
  }
}

function sendSigterm(child: cp.ChildProcess): void {
  child.kill('SIGTERM');
}

async function waitForExit(child: cp.ChildProcess, timeoutMs: number): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  await new Promise<void>((resolve) => {
    const timer = setTimeout(resolve, timeoutMs);
    child.once('exit', () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

/** One newline-delimited JSON-RPC round trip over the shim's stdio, mirroring
 * exactly what a real MCP host (VS Code) does. */
function shimCall(
  rl: readline.Interface,
  stdin: NodeJS.WritableStream,
  request: unknown,
  timeoutMs: number
): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('timed out waiting for shim response')), timeoutMs);
    const onLine = (line: string) => {
      if (!line.trim()) return;
      rl.off('line', onLine);
      clearTimeout(timer);
      try {
        resolve(JSON.parse(line));
      } catch (err) {
        reject(err);
      }
    };
    rl.on('line', onLine);
    stdin.write(JSON.stringify(request) + '\n');
  });
}

describe('real-binary headless + shim round trip', () => {
  let tmpDir: string;
  let workspaceRoot: string;
  let maeBinary: string | undefined;
  let shimBinary: string | undefined;

  before(function () {
    maeBinary = tryResolve(process.env.MAE_BIN ?? 'mae');
    shimBinary = tryResolve(process.env.MAE_SHIM_BIN ?? 'mae-mcp-shim');
    if (!maeBinary || !shimBinary) {
      console.log(
        'SKIP real-binary round trip: mae/mae-mcp-shim not resolvable ' +
          '(set MAE_BIN/MAE_SHIM_BIN, or build the Rust workspace and add it to PATH)'
      );
      this.skip();
    }
  });

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mae-vscode-real-'));
    workspaceRoot = path.join(tmpDir, 'workspace');
    fs.mkdirSync(path.join(workspaceRoot, '.git'), { recursive: true });
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it('spawns a real headless instance and completes a real MCP round trip through the real shim', async function () {
    this.timeout(30000);

    const socketPath = await resolveStableSocketPath(maeBinary!, workspaceRoot);
    const headless = spawnHeadlessInstance(maeBinary!, workspaceRoot);

    try {
      const deadline = Date.now() + 20000;
      let live = false;
      while (Date.now() < deadline) {
        if (await probeSocket(socketPath)) {
          live = true;
          break;
        }
        await new Promise((r) => setTimeout(r, 200));
      }
      assert.ok(live, `real headless instance never bound its socket at ${socketPath}`);

      const plan = resolveShimCommand(shimBinary!);
      const shim = cp.spawn(plan.command, plan.args, {
        env: { ...process.env, MAE_MCP_SOCKET: socketPath },
        stdio: ['pipe', 'pipe', 'pipe'],
      });
      const rl = readline.createInterface({ input: shim.stdout! });

      try {
        const initResp = await shimCall(
          rl,
          shim.stdin!,
          {
            jsonrpc: '2.0',
            id: 1,
            method: 'initialize',
            params: { clientInfo: { name: 'real-binaries-test', version: '1.0' }, protocolVersion: '2025-11-25' },
          },
          15000
        );
        assert.ok(initResp.result, `initialize failed: ${JSON.stringify(initResp)}`);

        shim.stdin!.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');

        const toolsResp = await shimCall(rl, shim.stdin!, { jsonrpc: '2.0', id: 2, method: 'tools/list' }, 15000);
        const result = toolsResp.result as { tools?: unknown[] } | undefined;
        assert.ok(result?.tools && result.tools.length > 100, `expected the real tool set, got: ${JSON.stringify(toolsResp).slice(0, 300)}`);
      } finally {
        rl.close();
        shim.kill();
      }
    } finally {
      sendSigterm(headless);
      await waitForExit(headless, 5000);
      if (headless.exitCode === null && headless.signalCode === null) {
        headless.kill('SIGKILL');
      }
    }
  });
});
