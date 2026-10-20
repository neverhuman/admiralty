import { describe, expect, it } from 'vitest';
import type { ChildProcess } from 'node:child_process';
import { chmodSync, existsSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { isProcessAlive, launchBrowserHarness, OwnedProcessCustodyError, waitForOwnedProcessExit, type BrowserHarness } from './browser-harness';

describe('owned process custody', () => {
  it('treats permission, IO and unknown probe errors as retained custody', () => {
    for (const code of ['EPERM', 'EACCES', 'EIO', undefined]) {
      expect(isProcessAlive(123, () => { throw Object.assign(new Error('probe unavailable'), { code }); })).toBe(true);
    }
    expect(isProcessAlive(123, () => {})).toBe(true);
    expect(isProcessAlive(123, () => { throw null; })).toBe(true);
    expect(isProcessAlive(123, () => { throw 'unknown'; })).toBe(true);
    expect(isProcessAlive(123, () => { throw Object.assign(new Error('absent'), { code: 'ESRCH' }); })).toBe(false);
  });

  it('requires a direct child exit observation even when a numeric PID probe says absent', async () => {
    const child = { pid: 2147483647, exitCode: null, signalCode: null } as unknown as ChildProcess;
    await expect(waitForOwnedProcessExit(child, 10)).rejects.toThrow('retain its private files');
    await expect(waitForOwnedProcessExit({ pid: 2147483647, exitCode: 0, signalCode: null } as unknown as ChildProcess, 10)).resolves.toBeUndefined();
    await expect(waitForOwnedProcessExit({ pid: 2147483647, exitCode: null, signalCode: 'SIGKILL' } as unknown as ChildProcess, 10)).resolves.toBeUndefined();
  });

  it('does not launch another attempt after unreaped-child or unresolved-group cleanup', async () => {
    // No OS process is spawned: exercise the actual retry coordinator with
    // controlled attempt failures, independently of the direct-child guard.
    for (const outcome of ['unreaped child', 'unresolved group']) {
      const root = mkdtempSync(join(realpathSync(tmpdir()), 'adm-custody-test-'));
      chmodSync(root, 0o700);
      const retained = join(root, 'private-profile-custody');
      let attempts = 0;
      try {
        await expect(launchBrowserHarness({ maxStartupAttempts: 2, _testLaunchAttempt: async () => {
          attempts++;
          writeFileSync(retained, 'controlled synthetic custody', { mode: 0o600 });
          throw new OwnedProcessCustodyError(outcome);
        } })).rejects.toThrow(outcome);
        expect(attempts).toBe(1);
        expect(existsSync(retained)).toBe(true);
      } finally {
        // Only synthetic files from this test; no process ever used them.
        rmSync(root, { recursive: true });
      }
    }
  });

  it('can retry after an ordinary startup failure with completed cleanup', async () => {
    let attempts = 0;
    const expected = {} as BrowserHarness;
    await expect(launchBrowserHarness({ maxStartupAttempts: 2, _testLaunchAttempt: async () => {
      attempts++;
      if (attempts === 1) throw new Error('startup failed after confirmed cleanup');
      return expected;
    } })).resolves.toBe(expected);
    expect(attempts).toBe(2);
  });
});
