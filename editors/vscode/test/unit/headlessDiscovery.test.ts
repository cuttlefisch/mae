import * as assert from 'assert';
import * as cp from 'child_process';
import * as fs from 'fs';
import * as net from 'net';
import * as os from 'os';
import * as path from 'path';

import { InvalidExecutableError } from '../../src/shimCommand';
import {
  HeadlessEnsureError,
  probeSocket,
  resolveStableSocketPath,
  spawnHeadlessInstance,
} from '../../src/headlessDiscovery';
import { createSpawnSpy } from './fakeChildProcess';

/**
 * Binds a real Unix socket at `socketPath` in a child process, then SIGKILLs
 * it — leaving a genuinely orphaned socket file on disk, since a killed
 * process never runs its own close/unlink handler. Mirrors the Rust-side
 * precedent this composes with (`headless_loop.rs`'s own
 * `claim_stable_socket_clears_a_stale_file_with_no_live_listener` test,
 * which relies on the identical "a Unix listener does not unlink on drop"
 * property) — this is the ADR-055 orphan-cleanup scenario, exercised here
 * specifically through the extension's own client-side detection primitive
 * rather than assumed to compose correctly with the server side untested.
 */
async function createOrphanedSocketFile(socketPath: string): Promise<void> {
  const script = "require('net').createServer().listen(process.argv[1]);";
  const child = cp.spawn(process.execPath, ['-e', script, socketPath], { stdio: 'ignore' });
  await new Promise((r) => setTimeout(r, 200)); // let the bind land
  child.kill('SIGKILL');
  await new Promise((r) => setTimeout(r, 100)); // let the kill land
}

function makeTempExecutable(dir: string, name: string): string {
  const filePath = path.join(dir, name);
  fs.writeFileSync(filePath, '#!/bin/sh\nexit 0\n');
  fs.chmodSync(filePath, 0o755);
  return filePath;
}

describe('headlessDiscovery', () => {
  let tmpDir: string;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mae-vscode-test-'));
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  describe('spawnHeadlessInstance', () => {
    it('spawns with shell:false, an argv array, detached+ignored stdio, and cwd set to the workspace root', () => {
      const exe = makeTempExecutable(tmpDir, 'mae');
      const { spawnFn, calls } = createSpawnSpy();

      spawnHeadlessInstance(exe, tmpDir, spawnFn);

      assert.strictEqual(calls.length, 1);
      const call = calls[0];
      assert.strictEqual(call.command, exe);
      assert.deepStrictEqual(call.args, ['--headless']);
      assert.strictEqual(call.options.shell, false, 'must never spawn through a shell');
      assert.strictEqual(call.options.cwd, tmpDir);
      assert.strictEqual(call.options.detached, true);
      assert.strictEqual(call.options.stdio, 'ignore');
    });

    // --- Adversarial: capability declaration abuse (#384 DoD) ---
    //
    // The realistic hostile-workspace vector: mae.headlessBinaryPath set to
    // a shell-injection-shaped string in .vscode/settings.json. It must
    // never reach the spawn call at all.
    it('never invokes spawnFn when the configured binary path does not resolve to a real executable', () => {
      const originalPath = process.env.PATH;
      try {
        process.env.PATH = tmpDir; // empty -- nothing can accidentally resolve
        const { spawnFn, calls } = createSpawnSpy();

        assert.throws(
          () => spawnHeadlessInstance('; rm -rf ~ #', tmpDir, spawnFn),
          InvalidExecutableError
        );
        assert.strictEqual(calls.length, 0, 'a rejected path must never reach spawnFn');
      } finally {
        process.env.PATH = originalPath;
      }
    });

    it('spawns a real file whose name contains shell metacharacters as a single literal argv command, never shell-parsed', () => {
      const maliciousName = '; rm -rf ~ #';
      const exe = makeTempExecutable(tmpDir, maliciousName);
      const { spawnFn, calls } = createSpawnSpy();

      spawnHeadlessInstance(exe, tmpDir, spawnFn);

      assert.strictEqual(calls.length, 1);
      assert.strictEqual(calls[0].command, exe, 'the whole funny-named path is one literal argv element');
      assert.strictEqual(calls[0].options.shell, false);
    });

    // Bug fix (QA-pass finding): an async spawn failure (EACCES, a
    // post-validation-race ENOENT, etc.) fires an 'error' event on the
    // ChildProcess. With no listener attached, Node treats this as an
    // uncaught exception -- a real extension-host crash risk, since it
    // happens outside any try/catch in extension.ts (the spawn call itself
    // returns synchronously before the async error would ever fire).
    it('never crashes the process when the spawned child later emits an error event', () => {
      const exe = makeTempExecutable(tmpDir, 'mae');
      const { spawnFn } = createSpawnSpy();
      let captured: Error | undefined;

      const child = spawnHeadlessInstance(exe, tmpDir, spawnFn, (err) => {
        captured = err;
      });

      // Before the fix, this would throw synchronously (Node's EventEmitter
      // treats an unlistened 'error' event as fatal) -- proving it doesn't
      // throw here IS proving the listener is attached.
      assert.doesNotThrow(() => {
        child.emit('error', new Error('spawn EACCES'));
      });
      assert.strictEqual(captured?.message, 'spawn EACCES');
    });
  });

  describe('resolveStableSocketPath', () => {
    it('returns the trimmed stdout of a successful `mae --headless --print-socket-path` run', async () => {
      const exe = makeTempExecutable(tmpDir, 'mae');
      const { spawnFn, calls } = createSpawnSpy();

      const resultPromise = resolveStableSocketPath(exe, tmpDir, spawnFn);
      // Let the promise's executor register listeners before we drive it.
      await Promise.resolve();
      assert.strictEqual(calls.length, 1);
      assert.deepStrictEqual(calls[0].args, ['--headless', '--print-socket-path']);
      calls[0].child.emitStdout('/home/user/.local/share/mae/headless/abc123.sock\n');
      calls[0].child.emitClose(0);

      assert.strictEqual(await resultPromise, '/home/user/.local/share/mae/headless/abc123.sock');
    });

    it('throws HeadlessEnsureError on a nonzero exit code', async () => {
      const exe = makeTempExecutable(tmpDir, 'mae');
      const { spawnFn, calls } = createSpawnSpy();

      const resultPromise = resolveStableSocketPath(exe, tmpDir, spawnFn);
      await Promise.resolve();
      calls[0].child.emitStdout('');
      calls[0].child.emitClose(1);

      await assert.rejects(resultPromise, HeadlessEnsureError);
    });
  });

  describe('probeSocket', () => {
    it('resolves true when something is listening at the path', async () => {
      const socketPath = path.join(tmpDir, 'live.sock');
      const server = net.createServer();
      await new Promise<void>((resolve) => server.listen(socketPath, resolve));
      try {
        assert.strictEqual(await probeSocket(socketPath), true);
      } finally {
        server.close();
      }
    });

    it('resolves false when nothing is listening at the path', async () => {
      const socketPath = path.join(tmpDir, 'nothing-here.sock');
      assert.strictEqual(await probeSocket(socketPath), false);
    });

    // --- Adversarial: orphan-cleanup through the extension's own lifecycle
    // path (#384 DoD) ---
    it('never mistakes a genuinely orphaned (kill -9\'d) socket file for a live instance', async function () {
      this.timeout(5000);
      const socketPath = path.join(tmpDir, 'orphaned.sock');
      await createOrphanedSocketFile(socketPath);

      assert.ok(
        fs.existsSync(socketPath),
        'the orphaned file must genuinely still be present on disk (the whole point of the scenario)'
      );
      assert.strictEqual(
        await probeSocket(socketPath),
        false,
        'a kill -9\'d socket file must never be mistaken for a live instance -- ' +
          'ensureHeadlessRunning depends on this to correctly spawn a fresh instance ' +
          'instead of getting stuck believing a dead one is still running'
      );
    });
  });
});
