/**
 * Mocha suite loader called by the real VS Code extension host that
 * `@vscode/test-electron`'s `runTests` launches (`runTest.ts`). Must export
 * a `run(): Promise<void>` — see `@vscode/test-electron`'s documented
 * `extensionTestsPath` contract.
 */

import * as fs from 'fs';
import * as path from 'path';

import Mocha from 'mocha';

export function run(): Promise<void> {
  const mocha = new Mocha({ ui: 'bdd', color: true, timeout: 20000 });
  const testsRoot = __dirname;

  const files = fs.readdirSync(testsRoot).filter((f) => f.endsWith('.test.js'));
  for (const f of files) {
    mocha.addFile(path.resolve(testsRoot, f));
  }

  return new Promise((resolve, reject) => {
    try {
      mocha.run((failures) => {
        if (failures > 0) {
          reject(new Error(`${failures} integration test(s) failed.`));
        } else {
          resolve();
        }
      });
    } catch (err) {
      reject(err);
    }
  });
}
