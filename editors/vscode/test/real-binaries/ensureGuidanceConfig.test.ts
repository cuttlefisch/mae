/**
 * Real-binary proof for K3 (post-ship quality pass): `ensureGuidanceConfigured`
 * actually invokes the real `mae --ensure-guidance-config` and produces the
 * real init.scm change server-side (`crates/mae/src/cli.rs::
 * handle_ensure_guidance_config`) — not just a correctly-shaped fake spawn
 * call (that's what `test/unit/headlessDiscovery.test.ts` covers).
 *
 * Deliberately isolates `XDG_CONFIG_HOME`/`HOME` via a per-test tmpdir passed
 * through `ensureGuidanceConfigured`'s `env` parameter — this test WRITES a
 * real config file, so unlike the read-only `headlessRoundTrip.test.ts`, it
 * must never touch the real developer machine's actual `~/.config/mae/init.scm`.
 *
 * Skips cleanly (not a failure) when `MAE_BIN` doesn't resolve to a real
 * executable, mirroring `headlessRoundTrip.test.ts`'s own precedent.
 */

import * as assert from 'assert';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';

import { resolveExecutable } from '../../src/shimCommand';
import { ensureGuidanceConfigured } from '../../src/headlessDiscovery';

function tryResolve(configured: string): string | undefined {
  try {
    return resolveExecutable(configured);
  } catch {
    return undefined;
  }
}

describe('real-binary ensure-guidance-config', () => {
  let tmpDir: string;
  let workspaceRoot: string;
  let xdgConfigHome: string;
  let maeBinary: string | undefined;

  before(function () {
    maeBinary = tryResolve(process.env.MAE_BIN ?? 'mae');
    if (!maeBinary) {
      console.log(
        'SKIP real-binary ensure-guidance-config: mae not resolvable ' +
          '(set MAE_BIN, or build the Rust workspace and add it to PATH)'
      );
      this.skip();
    }
  });

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mae-vscode-guidance-'));
    workspaceRoot = path.join(tmpDir, 'workspace');
    xdgConfigHome = path.join(tmpDir, 'config');
    fs.mkdirSync(workspaceRoot, { recursive: true });
    fs.mkdirSync(xdgConfigHome, { recursive: true });
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it('writes a real init.scm enabling ai_guidance_export_live_sync for a fresh project', async function () {
    this.timeout(15000);

    const isolatedEnv: NodeJS.ProcessEnv = {
      ...process.env,
      XDG_CONFIG_HOME: xdgConfigHome,
      XDG_DATA_HOME: path.join(tmpDir, 'data'),
      HOME: tmpDir,
      MAE_SKIP_WIZARD: '1',
    };

    const result = await ensureGuidanceConfigured(
      maeBinary!,
      workspaceRoot,
      undefined,
      15000,
      isolatedEnv
    );
    assert.strictEqual(
      result.code,
      0,
      `expected a clean exit, got code=${result.code} stderr=${result.stderr}`
    );

    const initScmPath = path.join(xdgConfigHome, 'mae', 'init.scm');
    assert.ok(fs.existsSync(initScmPath), `expected ${initScmPath} to be written`);
    const content = fs.readFileSync(initScmPath, 'utf8');
    assert.ok(
      content.includes('(set-option! "ai_guidance_export_live_sync" "true")'),
      `expected ai_guidance_export_live_sync to be enabled, got: ${content}`
    );
  });

  it('never overwrites an already-explicit ai_guidance_kb', async function () {
    this.timeout(15000);

    const maeConfigDir = path.join(xdgConfigHome, 'mae');
    fs.mkdirSync(maeConfigDir, { recursive: true });
    fs.writeFileSync(
      path.join(maeConfigDir, 'init.scm'),
      '(set-option! "ai_guidance_kb" "MaePractices")\n'
    );

    const isolatedEnv: NodeJS.ProcessEnv = {
      ...process.env,
      XDG_CONFIG_HOME: xdgConfigHome,
      XDG_DATA_HOME: path.join(tmpDir, 'data'),
      HOME: tmpDir,
      MAE_SKIP_WIZARD: '1',
    };

    const result = await ensureGuidanceConfigured(
      maeBinary!,
      workspaceRoot,
      undefined,
      15000,
      isolatedEnv
    );
    assert.strictEqual(result.code, 0);

    const content = fs.readFileSync(path.join(maeConfigDir, 'init.scm'), 'utf8');
    assert.ok(
      content.includes('(set-option! "ai_guidance_kb" "MaePractices")'),
      `an already-explicit ai_guidance_kb must survive unchanged, got: ${content}`
    );
  });
});
