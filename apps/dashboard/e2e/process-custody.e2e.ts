import { describe, expect, it } from 'vitest';
import type { ChildProcess } from 'node:child_process';
import { isProcessAlive, waitForOwnedProcessExit } from './browser-harness';

describe('owned process custody', () => {
  it('treats permission, IO and unknown probe errors as retained custody', () => {
    for (const code of ['EPERM', 'EACCES', 'EIO', undefined]) {
      expect(isProcessAlive(123, () => { throw Object.assign(new Error('probe unavailable'), { code }); })).toBe(true);
    }
    expect(isProcessAlive(123, () => {})).toBe(true);
    expect(isProcessAlive(123, () => { throw Object.assign(new Error('absent'), { code: 'ESRCH' }); })).toBe(false);
  });

  it('requires a direct child exit observation even when a numeric PID probe says absent', async () => {
    const child = { pid: 2147483647, exitCode: null, signalCode: null } as unknown as ChildProcess;
    await expect(waitForOwnedProcessExit(child, 10)).rejects.toThrow('retain its private files');
    await expect(waitForOwnedProcessExit({ pid: 2147483647, exitCode: 0, signalCode: null } as unknown as ChildProcess, 10)).resolves.toBeUndefined();
    await expect(waitForOwnedProcessExit({ pid: 2147483647, exitCode: null, signalCode: 'SIGKILL' } as unknown as ChildProcess, 10)).resolves.toBeUndefined();
  });
});
