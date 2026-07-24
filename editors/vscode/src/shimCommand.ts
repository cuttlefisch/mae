/**
 * Resolves user-configured binary paths (`mae.shimPath`, `mae.headlessBinaryPath`)
 * to real, existing, executable files — never through a shell.
 *
 * This is the extension's actual trust boundary against a hostile workspace
 * (CLAUDE.md's "capability declaration abuse" concern): a cloned, untrusted
 * repository can ship a `.vscode/settings.json` that sets these values to
 * anything it wants. The primary defense is structural, not this file's
 * validation: every spawn call this extension makes (`headlessDiscovery.ts`)
 * always passes `shell: false` with an argv array, so shell metacharacters
 * in a configured value are inert regardless of content — the OS never
 * interprets them, it just tries to execve() a file with that literal name.
 * This module's job is the second, complementary guard: reject a configured
 * value that doesn't resolve to a real, existing, executable file, so a
 * bogus/malicious value fails loudly instead of silently doing nothing (or
 * doing something surprising).
 */

import * as fs from 'fs';
import * as path from 'path';

export class InvalidExecutableError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'InvalidExecutableError';
  }
}

/** Everything needed to hand off to `child_process.spawn` (or an injected
 * equivalent) — always an argv array, never a shell string. */
export interface SpawnPlan {
  command: string;
  args: string[];
}

/**
 * Resolve `configuredPath` to an existing, executable file's absolute path.
 * If it contains no path separator, it is treated as a bare command name and
 * searched across `PATH` (manually, byte-for-byte — never by invoking a
 * shell or shell builtin to do the search). Throws `InvalidExecutableError`
 * if nothing resolves; never falls back to a guess or a default.
 */
export function resolveExecutable(configuredPath: string): string {
  if (!configuredPath || typeof configuredPath !== 'string') {
    throw new InvalidExecutableError('empty or invalid executable path configured');
  }

  const hasSeparator = configuredPath.includes('/') || configuredPath.includes(path.sep);
  const candidates = hasSeparator ? [configuredPath] : searchPath(configuredPath);

  for (const candidate of candidates) {
    if (isExecutableFile(candidate)) {
      return candidate;
    }
  }
  throw new InvalidExecutableError(
    `'${configuredPath}' does not resolve to an existing executable file` +
      (hasSeparator ? '' : ' anywhere on PATH')
  );
}

function searchPath(command: string): string[] {
  const pathEnv = process.env.PATH ?? '';
  const dirs = pathEnv.split(path.delimiter).filter(Boolean);
  const exts = process.platform === 'win32' ? (process.env.PATHEXT ?? '.EXE;.CMD;.BAT').split(';') : [''];
  const results: string[] = [];
  for (const dir of dirs) {
    for (const ext of exts) {
      results.push(path.join(dir, command + ext));
    }
  }
  return results;
}

function isExecutableFile(candidate: string): boolean {
  try {
    const stat = fs.statSync(candidate);
    if (!stat.isFile()) {
      return false;
    }
    if (process.platform === 'win32') {
      // No X_OK bit on Windows; existence + regular file is the real check.
      return true;
    }
    fs.accessSync(candidate, fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

/**
 * Resolve `configuredShimPath` (`mae.shimPath`) into a `SpawnPlan` for the
 * `McpStdioServerDefinition` VS Code will construct/spawn. VS Code's own
 * `McpStdioServerDefinition` is documented to run outside a shell by default
 * — this function's contribution is solely the existence/executable-bit
 * validation above, never a shell-safety mechanism of its own.
 */
export function resolveShimCommand(configuredShimPath: string): SpawnPlan {
  return { command: resolveExecutable(configuredShimPath), args: [] };
}
