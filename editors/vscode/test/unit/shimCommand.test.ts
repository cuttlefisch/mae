import * as assert from 'assert';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';

import { InvalidExecutableError, resolveExecutable, resolveShimCommand } from '../../src/shimCommand';

function makeTempExecutable(dir: string, name: string): string {
  const filePath = path.join(dir, name);
  fs.writeFileSync(filePath, '#!/bin/sh\nexit 0\n');
  fs.chmodSync(filePath, 0o755);
  return filePath;
}

describe('shimCommand', () => {
  let tmpDir: string;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mae-vscode-test-'));
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  describe('resolveExecutable', () => {
    it('resolves a real executable given by absolute path', () => {
      const exe = makeTempExecutable(tmpDir, 'real-mae-shim');
      assert.strictEqual(resolveExecutable(exe), exe);
    });

    it('resolves a bare command name via a PATH search', () => {
      const exe = makeTempExecutable(tmpDir, 'mae-mcp-shim-fixture');
      const originalPath = process.env.PATH;
      try {
        process.env.PATH = `${tmpDir}${path.delimiter}${originalPath ?? ''}`;
        assert.strictEqual(resolveExecutable('mae-mcp-shim-fixture'), exe);
      } finally {
        process.env.PATH = originalPath;
      }
    });

    it('throws for a nonexistent absolute path', () => {
      const missing = path.join(tmpDir, 'does-not-exist');
      assert.throws(() => resolveExecutable(missing), InvalidExecutableError);
    });

    it('throws for a non-executable regular file', () => {
      const filePath = path.join(tmpDir, 'not-executable');
      fs.writeFileSync(filePath, 'just text');
      fs.chmodSync(filePath, 0o644);
      if (process.platform === 'win32') {
        return; // no X_OK bit on Windows -- this case is Linux/macOS-only
      }
      assert.throws(() => resolveExecutable(filePath), InvalidExecutableError);
    });

    it('throws for an empty configured path', () => {
      assert.throws(() => resolveExecutable(''), InvalidExecutableError);
    });

    // --- Adversarial: capability declaration abuse (#384 DoD) ---
    //
    // A hostile, cloned repository's .vscode/settings.json can set
    // mae.shimPath to anything it wants. The two realistic failure modes:
    // (1) a shell-injection-shaped string that doesn't correspond to any
    //     real executable -- must be rejected before anything is spawned;
    // (2) a real, resolvable file whose *name itself* happens to contain
    //     shell metacharacters (a legal Unix filename) -- must resolve to
    //     its literal path, never be shell-interpreted anywhere downstream.

    it('rejects a shell-injection-shaped configured value that resolves to nothing real', () => {
      const originalPath = process.env.PATH;
      try {
        // Constrain PATH to an empty temp dir so this can never accidentally
        // match a real binary on the test runner's actual PATH.
        process.env.PATH = tmpDir;
        assert.throws(() => resolveExecutable('; rm -rf ~ #'), InvalidExecutableError);
      } finally {
        process.env.PATH = originalPath;
      }
    });

    it('resolves a real file whose name contains shell metacharacters to its literal path, unmodified', () => {
      // A legal (if unusual) Unix filename -- proves resolution never
      // shell-parses the value, it only ever checks file existence/mode.
      const maliciousName = '; rm -rf ~ #';
      const exe = makeTempExecutable(tmpDir, maliciousName);
      assert.strictEqual(resolveExecutable(exe), exe);
    });
  });

  describe('resolveShimCommand', () => {
    it('returns the resolved command with an empty argv array', () => {
      const exe = makeTempExecutable(tmpDir, 'shim');
      const plan = resolveShimCommand(exe);
      assert.strictEqual(plan.command, exe);
      assert.deepStrictEqual(plan.args, []);
    });

    it('propagates InvalidExecutableError for a bogus configured path, never returning a fallback', () => {
      assert.throws(() => resolveShimCommand(path.join(tmpDir, 'nope')), InvalidExecutableError);
    });
  });
});
