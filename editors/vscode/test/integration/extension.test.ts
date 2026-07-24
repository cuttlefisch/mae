/**
 * Real-extension-host smoke test. Proves only that the extension loads and
 * `activate()` succeeds in a genuine VS Code instance without throwing — the
 * unit suite (`test/unit/`) is what proves the underlying logic is correct;
 * re-deriving those guarantees here would be redundant, not additional
 * coverage.
 */

import * as assert from 'assert';

import * as vscode from 'vscode';

describe('MAE extension host smoke test', () => {
  it('is discoverable and activates cleanly', async () => {
    const ext = vscode.extensions.getExtension('mae-editor.mae-vscode');
    assert.ok(ext, 'extension should be discoverable by the test host');

    await ext!.activate();

    assert.strictEqual(ext!.isActive, true, 'activate() must complete without throwing');
  });
});
