/**
 * Ensures a headless MAE instance (ADR-055) is running for the current
 * workspace, spawning one if necessary — the "auto-spawn... when none is
 * running" half of ADR-050 D1/Phase I's design. Never touches
 * `.vscode/mcp.json`: discovery and lifecycle here are entirely in-memory,
 * via VS Code's dynamic `McpServerDefinitionProvider` API, which structurally
 * sidesteps the JSONC-mutation-safety concerns a file-editing approach would
 * carry.
 */

import * as cp from 'child_process';
import * as net from 'net';

import { resolveExecutable } from './shimCommand';

/** Injectable so tests can assert on exact spawn arguments without spawning
 * a real process — defaults to the real `child_process.spawn`. */
export type SpawnFn = (
  command: string,
  args: string[],
  options: cp.SpawnOptions
) => cp.ChildProcess;

const PROBE_TIMEOUT_MS = 500;
const SPAWN_CONFIRM_TIMEOUT_MS = 5000;
const SPAWN_POLL_INTERVAL_MS = 250;
const PRINT_SOCKET_PATH_TIMEOUT_MS = 3000;

export class HeadlessEnsureError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'HeadlessEnsureError';
  }
}

function runCapture(
  command: string,
  args: string[],
  cwd: string,
  timeoutMs: number,
  spawnFn: SpawnFn
): Promise<{ code: number | null; stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    const child = spawnFn(command, args, { cwd, shell: false });
    let stdout = '';
    let stderr = '';
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      child.kill?.();
      reject(new HeadlessEnsureError(`'${command} ${args.join(' ')}' timed out after ${timeoutMs}ms`));
    }, timeoutMs);
    child.stdout?.on('data', (d: Buffer) => (stdout += d.toString()));
    child.stderr?.on('data', (d: Buffer) => (stderr += d.toString()));
    child.on('error', (err: Error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      reject(err);
    });
    child.on('close', (code: number | null) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve({ code, stdout, stderr });
    });
  });
}

/**
 * Resolve the stable, project-keyed headless socket path by asking the real
 * `mae` binary (`mae --headless --print-socket-path`) rather than
 * reimplementing its hashing scheme in TypeScript — the single source of
 * truth `crates/mae/src/cli.rs::resolve_print_socket_path` guarantees this
 * always matches exactly what `mae --headless` itself would claim.
 */
export async function resolveStableSocketPath(
  maeBinary: string,
  workspaceRoot: string,
  spawnFn: SpawnFn = cp.spawn
): Promise<string> {
  const resolved = resolveExecutable(maeBinary);
  const { code, stdout, stderr } = await runCapture(
    resolved,
    ['--headless', '--print-socket-path'],
    workspaceRoot,
    PRINT_SOCKET_PATH_TIMEOUT_MS,
    spawnFn
  );
  const socketPath = stdout.trim();
  if (code !== 0 || !socketPath) {
    throw new HeadlessEnsureError(
      `mae --headless --print-socket-path failed (exit ${code}): ${stderr.trim() || 'no output'}`
    );
  }
  return socketPath;
}

/**
 * Whether something is currently listening on `socketPath`. Deliberately
 * does no peer-identity verification beyond "did a connection succeed" —
 * that's `mae-mcp-shim`'s job (its own `initialize` -> `notifications/
 * initialized` -> `$/ping` handshake, already proven in Phase B), not
 * something worth duplicating here. A same-machine attacker pre-binding this
 * path is the same pre-existing Unix-socket trust boundary every MAE
 * listener already has (SECURITY.md: filesystem-permissions-only) — not a
 * new gap this extension introduces.
 */
export function probeSocket(socketPath: string, timeoutMs = PROBE_TIMEOUT_MS): Promise<boolean> {
  return new Promise((resolve) => {
    let settled = false;
    const socket = net.createConnection({ path: socketPath });
    const finish = (result: boolean) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeAllListeners();
      socket.destroy();
      resolve(result);
    };
    const timer = setTimeout(() => finish(false), timeoutMs);
    socket.once('connect', () => finish(true));
    socket.once('error', () => finish(false));
  });
}

async function pollUntilListening(socketPath: string, totalTimeoutMs: number): Promise<boolean> {
  const deadline = Date.now() + totalTimeoutMs;
  do {
    if (await probeSocket(socketPath)) {
      return true;
    }
    await new Promise((r) => setTimeout(r, SPAWN_POLL_INTERVAL_MS));
  } while (Date.now() < deadline);
  return false;
}

/**
 * Spawn `mae --headless` for `workspaceRoot`, detached so it outlives this
 * extension host process (survives VS Code window reload). Always
 * `shell: false` with an argv array — the adversarial "capability
 * declaration abuse" test (a hostile workspace's `mae.headlessBinaryPath`)
 * targets exactly this call.
 */
export function spawnHeadlessInstance(
  maeBinary: string,
  workspaceRoot: string,
  spawnFn: SpawnFn = cp.spawn
): cp.ChildProcess {
  const resolvedBinary = resolveExecutable(maeBinary);
  const child = spawnFn(resolvedBinary, ['--headless'], {
    cwd: workspaceRoot,
    detached: true,
    stdio: 'ignore',
    shell: false,
  });
  child.unref?.();
  return child;
}

export interface EnsureHeadlessResult {
  socketPath: string;
  spawnedNewInstance: boolean;
}

/**
 * Ensure a headless MAE instance is running for `workspaceRoot`: probe the
 * stable socket path; if nothing answers, spawn one and poll-confirm it came
 * up. Never silently pretends success — throws `HeadlessEnsureError` if a
 * freshly spawned instance never starts accepting connections, so the caller
 * can surface a visible error rather than handing VS Code a definition that
 * silently never works (gate G1).
 */
export async function ensureHeadlessRunning(
  maeBinary: string,
  workspaceRoot: string,
  spawnFn: SpawnFn = cp.spawn
): Promise<EnsureHeadlessResult> {
  const socketPath = await resolveStableSocketPath(maeBinary, workspaceRoot, spawnFn);

  if (await probeSocket(socketPath)) {
    return { socketPath, spawnedNewInstance: false };
  }

  spawnHeadlessInstance(maeBinary, workspaceRoot, spawnFn);

  const started = await pollUntilListening(socketPath, SPAWN_CONFIRM_TIMEOUT_MS);
  if (!started) {
    throw new HeadlessEnsureError(
      `spawned 'mae --headless' for ${workspaceRoot} but it never accepted connections on ` +
        `${socketPath} within ${SPAWN_CONFIRM_TIMEOUT_MS}ms`
    );
  }
  return { socketPath, spawnedNewInstance: true };
}
