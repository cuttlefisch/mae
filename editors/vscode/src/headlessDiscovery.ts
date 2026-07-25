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
import * as fs from 'fs';
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
const SPAWN_POLL_INTERVAL_MS = 250;

/**
 * Default budget (ms) for `mae --headless --print-socket-path` to complete,
 * and (scaled up) for a freshly-spawned instance to start accepting
 * connections. 3s was the original default -- too tight in practice: on a
 * remote/WSL session where the workspace folder is a cross-boundary mount
 * (e.g. a Windows drive mounted into WSL2 via 9p, especially under active
 * antivirus real-time scanning), even a handful of directory stat calls can
 * take multiple seconds, well past what's typical on a native filesystem.
 * Both `ensureHeadlessRunning` call sites accept an override (wired to the
 * `mae.headlessTimeoutMs` setting in `extension.ts`) rather than forcing
 * every environment to accept one hardcoded value.
 */
export const DEFAULT_HEADLESS_TIMEOUT_MS = 15000;

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
  spawnFn: SpawnFn,
  env?: NodeJS.ProcessEnv
): Promise<{ code: number | null; stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    const child = spawnFn(command, args, { cwd, shell: false, ...(env ? { env } : {}) });
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
 * K3 (post-ship quality pass): deterministic, non-AI-dependent guidance-KB
 * setup for a fresh workspace — runs the real `mae --ensure-guidance-config`
 * once (reuses the proven `set_option`/`save_option_to_init` persistence
 * path server-side, `crates/mae/src/cli.rs::handle_ensure_guidance_config`,
 * rather than an LLM having to correctly guess which of many MCP tools to
 * call for a one-shot setup step). Best-effort by design, mirroring the CLI
 * flag's own "nothing here is a hard error" contract — never throws on a
 * non-zero exit; the caller should log a failure, not block MCP pairing on
 * it. Set-if-unset on the server side, so calling this on every activation
 * (guarded by the caller's own per-workspace `globalState` check) is safe
 * either way — this function itself has no idempotency logic of its own.
 */
export async function ensureGuidanceConfigured(
  maeBinary: string,
  workspaceRoot: string,
  spawnFn: SpawnFn = cp.spawn,
  timeoutMs: number = DEFAULT_HEADLESS_TIMEOUT_MS,
  env?: NodeJS.ProcessEnv
): Promise<{ code: number | null; stdout: string; stderr: string }> {
  const resolved = resolveExecutable(maeBinary);
  return runCapture(resolved, ['--ensure-guidance-config'], workspaceRoot, timeoutMs, spawnFn, env);
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
  spawnFn: SpawnFn = cp.spawn,
  timeoutMs: number = DEFAULT_HEADLESS_TIMEOUT_MS
): Promise<string> {
  const resolved = resolveExecutable(maeBinary);
  const { code, stdout, stderr } = await runCapture(
    resolved,
    ['--headless', '--print-socket-path'],
    workspaceRoot,
    timeoutMs,
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
 *
 * `onSpawnError`, if given, is called if the child emits an `'error'` event
 * (EACCES, a post-validation-race ENOENT if the binary vanishes between
 * `resolveExecutable`'s check and the actual spawn, etc.). Attaching a
 * listener here is the load-bearing part, independent of what the callback
 * does: an unhandled `'error'` event on a `ChildProcess` is fatal (Node
 * throws it as an uncaught exception outside any caller's try/catch) —
 * QA-pass finding, this was previously unguarded.
 */
export function spawnHeadlessInstance(
  maeBinary: string,
  workspaceRoot: string,
  spawnFn: SpawnFn = cp.spawn,
  onSpawnError?: (err: Error) => void
): cp.ChildProcess {
  const resolvedBinary = resolveExecutable(maeBinary);
  const child = spawnFn(resolvedBinary, ['--headless'], {
    cwd: workspaceRoot,
    detached: true,
    stdio: 'ignore',
    shell: false,
  });
  child.on('error', (err: Error) => {
    onSpawnError?.(err);
  });
  child.unref?.();
  return child;
}

export interface EnsureHeadlessResult {
  socketPath: string;
  spawnedNewInstance: boolean;
}

function isProcessAlive(pid: number): boolean {
  try {
    // Signal 0 sends nothing -- it only tests whether the process exists
    // and is signalable by us. Throws ESRCH (no such process) or EPERM
    // (exists but owned by someone else, which still means "alive").
    process.kill(pid, 0);
    return true;
  } catch (err) {
    return (err as NodeJS.ErrnoException).code === 'EPERM';
  }
}

/**
 * Cross-process spawn coordination (A1 hardening finding): the in-process
 * lock in `ensureHeadlessRunning` below only dedupes concurrent callers
 * within ONE extension host process. Two separate VS Code windows on the
 * same project each run their own extension host, so a real TOCTOU race
 * remains between processes -- exactly the bug class documented in a real
 * Codex Desktop + VS Code extension issue (openai/codex#25742): three
 * duplicate stdio server trees spawned within five minutes from
 * uncoordinated hosts. `fs`'s `wx` open flag gives an OS-level atomic
 * exclusive-create, so this is a genuine cross-process mutex, not a
 * best-effort heuristic. Returns `true` if THIS call acquired the lock (and
 * is therefore responsible for both spawning and eventually releasing it).
 */
function tryAcquireSpawnLock(lockPath: string): boolean {
  try {
    fs.writeFileSync(lockPath, String(process.pid), { flag: 'wx' });
    return true;
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code !== 'EEXIST') {
      // An unexpected I/O error (e.g. permissions) -- fail OPEN rather than
      // silently refusing to ever spawn again. The in-process lock already
      // covers the common same-process race; a same-machine attacker
      // pre-creating this file is the same pre-existing filesystem-trust
      // boundary every MAE listener already documents (SECURITY.md).
      return true;
    }
  }

  let holderPid: number | undefined;
  try {
    holderPid = parseInt(fs.readFileSync(lockPath, 'utf8').trim(), 10);
  } catch {
    holderPid = undefined;
  }
  if (holderPid !== undefined && !Number.isNaN(holderPid) && isProcessAlive(holderPid)) {
    return false; // a live process holds the lock -- let it do the spawning
  }

  // A stale lock: its holder process is gone (crashed/killed before it
  // could clean up), or the file was unreadable/corrupt. Reclaim it.
  try {
    fs.unlinkSync(lockPath);
  } catch {
    // Lost a cleanup race with another reclaimer -- fine, retry below.
  }
  try {
    fs.writeFileSync(lockPath, String(process.pid), { flag: 'wx' });
    return true;
  } catch {
    return false; // someone else won the reclaim race -- let them spawn
  }
}

function releaseSpawnLock(lockPath: string): void {
  try {
    fs.unlinkSync(lockPath);
  } catch {
    // Already gone -- fine.
  }
}

async function ensureHeadlessRunningUncached(
  maeBinary: string,
  workspaceRoot: string,
  spawnFn: SpawnFn,
  timeoutMs: number
): Promise<EnsureHeadlessResult> {
  const socketPath = await resolveStableSocketPath(maeBinary, workspaceRoot, spawnFn, timeoutMs);

  if (await probeSocket(socketPath)) {
    return { socketPath, spawnedNewInstance: false };
  }

  const lockPath = `${socketPath}.spawn-lock`;
  const acquiredLock = tryAcquireSpawnLock(lockPath);

  let spawnError: Error | undefined;
  if (acquiredLock) {
    spawnHeadlessInstance(maeBinary, workspaceRoot, spawnFn, (err) => {
      spawnError = err;
    });
  }

  try {
    const started = await pollUntilListening(socketPath, timeoutMs);
    if (!started) {
      const detail = spawnError ? ` (spawn error: ${spawnError.message})` : '';
      const who = acquiredLock ? 'spawned' : 'waited for another process to spawn';
      throw new HeadlessEnsureError(
        `${who} 'mae --headless' for ${workspaceRoot} but it never accepted connections on ` +
          `${socketPath} within ${timeoutMs}ms${detail}`
      );
    }
    return { socketPath, spawnedNewInstance: acquiredLock };
  } finally {
    if (acquiredLock) {
      releaseSpawnLock(lockPath);
    }
  }
}

/**
 * In-process dedup: two `resolveMcpServerDefinition` calls that race within
 * the SAME extension host (e.g. VS Code re-resolving while a first ensure is
 * still in flight) must share one ensure operation, not each independently
 * probe-then-spawn -- the other half of the A1 TOCTOU fix, complementing the
 * cross-process lock file above. Keyed by (binary, workspaceRoot): the pair
 * that actually determines the target socket path. Entries are removed once
 * the in-flight ensure settles (success or failure) so a later, genuinely
 * separate call (e.g. after the instance later crashes) starts fresh rather
 * than replaying a stale cached outcome forever.
 */
const inFlightEnsures = new Map<string, Promise<EnsureHeadlessResult>>();

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
  spawnFn: SpawnFn = cp.spawn,
  timeoutMs: number = DEFAULT_HEADLESS_TIMEOUT_MS
): Promise<EnsureHeadlessResult> {
  const lockKey = `${maeBinary} ${workspaceRoot}`;
  const existing = inFlightEnsures.get(lockKey);
  if (existing) {
    return existing;
  }
  const promise = ensureHeadlessRunningUncached(maeBinary, workspaceRoot, spawnFn, timeoutMs);
  inFlightEnsures.set(lockKey, promise);
  // The caller of `ensureHeadlessRunning` (via the `promise` we return
  // below) is what actually observes/handles a rejection -- this `.finally`
  // is purely a bookkeeping side effect. `.catch` on ITS OWN derived promise
  // here so a rejection doesn't also surface as a second, spurious
  // "unhandled rejection" from this chain alone.
  promise.finally(() => inFlightEnsures.delete(lockKey)).catch(() => {});
  return promise;
}
