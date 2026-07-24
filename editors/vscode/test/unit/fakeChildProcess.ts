import { EventEmitter } from 'events';
import type * as cp from 'child_process';

/**
 * A minimal fake `ChildProcess` for injecting into `SpawnFn`-accepting
 * functions under test, avoiding any real process spawn. Exposes `.stdout`/
 * `.stderr` as their own EventEmitters (matching the real shape) and lets a
 * test drive completion via `emitClose`/`emitError` on its own schedule.
 */
export class FakeChildProcess extends EventEmitter {
  stdout = new EventEmitter();
  stderr = new EventEmitter();
  killed = false;

  kill(): boolean {
    this.killed = true;
    return true;
  }

  unref(): void {
    // no-op — nothing real to unref
  }

  emitStdout(chunk: string): void {
    this.stdout.emit('data', Buffer.from(chunk));
  }

  emitClose(code: number | null): void {
    this.emit('close', code);
  }

  emitError(err: Error): void {
    this.emit('error', err);
  }
}

/** A `SpawnFn`-compatible spy: records every call and returns a fresh
 * `FakeChildProcess` each time (captured in `.calls` for assertions). */
export interface RecordedCall {
  command: string;
  args: string[];
  options: cp.SpawnOptions;
  child: FakeChildProcess;
}

export function createSpawnSpy(): {
  spawnFn: (command: string, args: string[], options: cp.SpawnOptions) => cp.ChildProcess;
  calls: RecordedCall[];
} {
  const calls: RecordedCall[] = [];
  const spawnFn = (command: string, args: string[], options: cp.SpawnOptions): cp.ChildProcess => {
    const child = new FakeChildProcess();
    calls.push({ command, args, options, child });
    return child as unknown as cp.ChildProcess;
  };
  return { spawnFn, calls };
}
