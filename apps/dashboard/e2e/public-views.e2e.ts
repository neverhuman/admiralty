import { expect, it } from 'vitest';
import { createHash } from 'node:crypto';
import * as nodeFs from 'node:fs';
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { join, realpathSync, resolve } from 'node:path';
import { launchBrowserHarness, startIsolatedNativeAdmiraltyServer, REPO_ROOT,
  type BrowserHarness, type IsolatedNativeAdmiraltyServer } from './browser-harness';

interface NativeFileStat {
  dev: number; ino: number; size: number; uid: number; mode: number;
  nlink: number; mtimeMs: number; ctimeMs: number;
  isFile(): boolean;
}
const nativeFdFs = nodeFs as unknown as {
  constants: { O_RDONLY: number; O_NOFOLLOW: number; O_NONBLOCK: number };
  openSync(path: string, flags: number): number;
  closeSync(fd: number): void;
  fstatSync(fd: number): NativeFileStat;
  lstatSync(path: string): NativeFileStat;
  readSync(fd: number, buffer: Uint8Array, offset: number, length: number, position: number | null): number;
};
const nativeProcess = process as typeof process & { getuid(): number };
const MAX_NATIVE_BINARY_BYTES = 512 * 1024 * 1024;
const NATIVE_HASH_BUDGET_MS = 15_000;

function nativeFileIdentity(stat: NativeFileStat): string {
  return [stat.dev, stat.ino, stat.size, stat.uid, stat.mode, stat.nlink, stat.mtimeMs, stat.ctimeMs].join(':');
}

function verifyNativeBinaryPin(supplied: string | undefined, pin: string | undefined): void {
  if (!supplied || !supplied.startsWith('/') || !pin || !/^[0-9a-f]{64}$/.test(pin)) {
    throw new Error('Exact native binary and SHA256 required; no build/fallback/skip is permitted');
  }
  const path = realpathSync(supplied);
  if (path !== resolve(supplied)) throw new Error('Use the canonical native binary path');
  const fd = nativeFdFs.openSync(path, nativeFdFs.constants.O_RDONLY | nativeFdFs.constants.O_NOFOLLOW | nativeFdFs.constants.O_NONBLOCK);
  try {
    const before = nativeFdFs.fstatSync(fd);
    if (!before.isFile() || before.uid !== nativeProcess.getuid() || before.size <= 0 || before.size > MAX_NATIVE_BINARY_BYTES) {
      throw new Error('Native fixture binary must be an owned bounded regular file');
    }
    const hash = createHash('sha256'), buffer = new Uint8Array(64 * 1024);
    const deadline = Date.now() + NATIVE_HASH_BUDGET_MS;
    let bytes = 0;
    for (;;) {
      if (Date.now() >= deadline) throw new Error('Native fixture binary hash exceeded its time bound');
      const n = nativeFdFs.readSync(fd, buffer, 0, buffer.length, null);
      if (!n) break;
      bytes += n;
      if (bytes > before.size || bytes > MAX_NATIVE_BINARY_BYTES) throw new Error('Native fixture binary changed while hashing');
      hash.update(buffer.subarray(0, n) as Buffer);
    }
    if (bytes !== before.size || hash.digest('hex') !== pin
      || nativeFileIdentity(nativeFdFs.fstatSync(fd)) !== nativeFileIdentity(before)
      || nativeFileIdentity(nativeFdFs.lstatSync(path)) !== nativeFileIdentity(before)) {
      throw new Error('Native fixture binary pin/identity mismatch');
    }
  } finally { nativeFdFs.closeSync(fd); }
}

it('renders public Git/Atlas observations after private401 and reconnects with owner access', async () => {
  const expectedHash = process.env.ADMIRALTY_E2E_ADMIRALTYD_SHA256;
  const binary = process.env.ADMIRALTY_E2E_ADMIRALTYD_BIN;
  verifyNativeBinaryPin(binary, expectedHash);
  const stamp = '2026-10-18T12:00:00Z', oid = 'a'.repeat(40), node = 'fixture-node';
  const source = {
    generated_utc: stamp, host: node, simulated: false, order: ['harbor'],
    projects: { harbor: { name: 'Harbor', total: 3 } },
    hierarchy: { super_captain: { id: '11.admiral', host: node },
      captains: [{ id: '12.manager', screen: '12.manager', host: node, key: 'harbor' }] },
    agents: [
      { id: '11.admiral', screen: 'admiral', host: node, model: 'Gemini 4 Argon', state: 'active' },
      { id: '12.manager', screen: 'manager', host: node, project: 'harbor', model: 'gpt-6.1-sol', state: 'active' },
      { id: '13.luna', screen: 'luna', host: node, project: 'harbor', model: 'gpt-6-luna', state: 'active' },
      { id: '14.review', screen: 'review', host: node, project: 'harbor', model: 'Haiku 4.5', state: 'active' },
    ],
    git: { harbor: { instance_id: 'public-fixture', node, observed_utc: stamp, shallow: false, truncated: false,
      commits: [{ oid, parents: [], subject: 'Fixture public commit', committed_utc: stamp }],
      refs: [{ name: 'refs/heads/main', oid, commit_oid: oid, kind: 'branch' }], checkouts: [] } },
  };
  mkdirSync(join(REPO_ROOT, 'target'), { recursive: true });
  const evidence = mkdtempSync(join(REPO_ROOT, 'target', 'public-views-evidence-'));
  let server: IsolatedNativeAdmiraltyServer | undefined, browser: BrowserHarness | undefined;
  const axes: unknown[] = [];
  let baseUrl = '';
  try {
    server = await startIsolatedNativeAdmiraltyServer({ legacySnapshot: source, legacyPublicRead: true });
    baseUrl = server.baseUrl;
    expect(server.binaryPath).toBe(binary);
    expect((await fetch(`${baseUrl}/v2/views`)).status).toBe(401);
    expect((await fetch(`${baseUrl}/v1/legacy`)).status).toBe(200);
    browser = await launchBrowserHarness();
    await browser.setViewportSize(1440, 1000);
    await browser.goto(`${baseUrl}/#/git?project=harbor`);
    await browser.waitForText('Showing the explicitly public producer snapshot');
    await browser.waitForSelector('.git-canvas');
    expect(await browser.evaluate('document.body.textContent')).not.toContain('Visual feed failed (401)');
    await browser.evaluate("document.querySelector('.git-canvas').dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}))");
    await browser.waitForSelector('.lv-inspector');
    expect(await browser.evaluate("document.querySelector('.lv-inspector')?.textContent")).toContain(stamp);
    await browser.captureScreenshot(join(evidence, 'git-public.png'));
    let axe = await browser.runAxeAudit();
    axes.push({ mode: 'git-public', axe });
    writeFileSync(join(evidence, 'axe.json'), JSON.stringify(axes, null, 2));
    expect(axe.violations).toEqual([]); expect(axe.incomplete).toEqual([]);
    await browser.evaluate("location.hash='#/atlas'");
    await browser.waitForSelector('.atlas-agent.admiral');
    await browser.waitForSelector('.atlas-agent.commodore');
    const colors = await browser.evaluate<Record<string, string>>(`Object.fromEntries(['gemini','sol','luna','claude'].map(f=>{const e=document.querySelector('.model-badge--'+f);if(!e)throw Error('Missing '+f);return[f,getComputedStyle(e).backgroundColor]}))`);
    expect(new Set(Object.values(colors)).size).toBe(4);
    expect(Object.values(colors)).not.toContain('rgba(0, 0, 0, 0)');
    expect(Object.values(colors)).not.toContain('rgb(139, 148, 158)');
    await browser.captureScreenshot(join(evidence, 'atlas-public.png'));
    axe = await browser.runAxeAudit(); axes.push({ mode: 'atlas-public', axe });
    writeFileSync(join(evidence, 'axe.json'), JSON.stringify(axes, null, 2));
    expect(axe.violations).toEqual([]); expect(axe.incomplete).toEqual([]);
    await browser.clickButtonByText('Connect session access');
    await browser.fillInputByLabel('Bearer token', server.ownerToken);
    await browser.clickButtonByText('Apply token');
    await browser.waitForSelector('.lv-feed.is-connected');
    expect(await browser.evaluate('document.body.textContent')).not.toContain('Showing the explicitly public producer snapshot');
    expect((await fetch(`${baseUrl}/v2/views`)).status).toBe(401);
    writeFileSync(join(evidence, 'proof.json'), JSON.stringify({
      recorded_utc: new Date().toISOString(), native_binary: binary, native_sha256: expectedHash,
      source_timestamp: stamp, private_without_bearer: 401, explicit_public_legacy: 200,
      modes: ['git-public', 'atlas-public', 'authenticated-native'], colors, axes,
      source_time_seen_in_real_git_inspector: true, provider_turns: 0,
      scope: 'Real isolated Rust service and production React bundle; synthetic producer observations, no live fleet/provider qualification',
    }, null, 2));
    console.log(`Public visual regression proof: ${evidence}`);
  } finally {
    await browser?.close(); await server?.close();
  }
  await expect(fetch(`${baseUrl}/healthz`)).rejects.toThrow();
}, 30_000);
