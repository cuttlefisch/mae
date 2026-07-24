/**
 * Entry point for the `@vscode/test-electron` smoke test (run via
 * `npm run test:integration`, `node ./out/test/integration/runTest.js`).
 * Launches a real VS Code extension host (headless-capable via `xvfb-run`
 * in CI) with this extension loaded, and runs `./index.js`'s mocha suite
 * inside it — proving `activate()` succeeds in a real host, not just that
 * the unit-level logic is individually correct (that's `test/unit/`'s job).
 */

import * as path from 'path';

import { runTests } from '@vscode/test-electron';

async function main(): Promise<void> {
  try {
    // out/test/integration -> out/test -> out -> editors/vscode (the
    // package root, which must contain package.json).
    const extensionDevelopmentPath = path.resolve(__dirname, '../../../');
    const extensionTestsPath = path.resolve(__dirname, './index');

    await runTests({ extensionDevelopmentPath, extensionTestsPath });
  } catch (err) {
    console.error('MAE extension integration test failed to run:', err);
    process.exit(1);
  }
}

void main();
