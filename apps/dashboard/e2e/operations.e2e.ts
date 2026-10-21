import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { launchBrowserHarness, startIsolatedNativeAdmiraltyServer, REPO_ROOT,
  type BrowserHarness, type IsolatedNativeAdmiraltyServer } from './browser-harness';

function fixture() {
  const stamp = new Date().toISOString();
  const report = { schema_version: 1, instance_id: 'operations-fixture', observed_utc: stamp,
    integration_ref: 'refs/heads/main', integration_oid: 'a'.repeat(40), state: 'healthy', actionable: false,
    observed_commits: 5, observed_refs: 2, observed_checkouts: 1, missing_parent_oids: [], shallow: false,
    truncated: false, observation_error: false, fingerprint: 'fixture-health', issues: [], branches: [] };
  return { generated_utc: stamp, interval_s: 45,
    projects: { harbor: { name: 'Harbor' }, ...Object.fromEntries(Array.from({ length: 75 }, (_, i) => [`p${i}`, { name: `Project ${i}` }])) },
    git_health: { harbor: { ...report, state: 'attention', actionable: true }, p0: report },
    collector: { implementation: 'rust', duration_ms: 12.34, stages_ms: { git: 3.21 } },
    global_resources: { hosts: [{ host: 'fixture-node', reachable: true, nproc: 8, load: [1, 2, 3], mem: { avail_gb: 16 }, warnings: [] }] },
  };
}

describe('fleet operations on the built Rust service', () => {
  let browser: BrowserHarness, server: IsolatedNativeAdmiraltyServer;
  const evidence = join(REPO_ROOT, 'target', 'operations-evidence');
  beforeAll(async () => {
    mkdirSync(evidence, { recursive: true });
    server = await startIsolatedNativeAdmiraltyServer({ legacySnapshot: fixture() });
    browser = await launchBrowserHarness();
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });
  beforeEach(async () => { await browser.resetPage(); await browser.setViewportSize(1440, 1000); });
  async function connect() {
    await browser.goto(`${server.baseUrl}/#/operations`);
    await browser.waitForText('Session access required.');
    await browser.clickButtonByAriaLabel('Access settings');
    await browser.fillInputByLabel('Bearer token', server.ownerToken);
    await browser.clickButtonByText('Apply token');
    await browser.waitForSelector('.ops-health tbody tr');
  }
  it('filters and pages evidence, exposes the agent packet and avoids unused native streams', async () => {
    await connect();
    expect(await browser.evaluate(`document.querySelectorAll('.ops-health tbody tr').length`)).toBe(50);
    await browser.clickButtonByText('Next');
    expect(await browser.evaluate(`document.querySelectorAll('.ops-health tbody tr').length`)).toBe(26);
    await browser.selectByLabel('Show', 'attention');
    expect(await browser.evaluate(`document.querySelectorAll('.ops-health tbody tr').length`)).toBe(1);
    await browser.waitForText('Harbor');
    await browser.selectByLabel('Show', 'all');
    await browser.fillInputByLabel('Search projects', 'Harbor');
    expect(await browser.evaluate(`document.querySelectorAll('.ops-health tbody tr').length`)).toBe(1);
    await browser.clickButtonByText('Inspect agent JSON');
    const packet = await browser.evaluate<string>(`document.querySelector('.ops-brief pre').textContent`);
    expect(JSON.parse(packet)).toMatchObject({ projects: 76, attention: 1, omitted_exceptions: 55, acceptance_granted: false });
    const resources = await browser.evaluate<string[]>(`performance.getEntriesByType('resource').map(entry => new URL(entry.name).pathname)`);
    expect(resources).toContain('/v1/legacy/detail/initial');
    expect(resources.filter(path => path.includes('/fleet') || path.includes('/events'))).toEqual([]);
    await browser.captureScreenshot(join(evidence, 'operations-desktop.png'));
  });
  it('clears private diagnostics when session access is removed', async () => {
    await connect();
    await browser.clickButtonByAriaLabel('Access settings');
    await browser.clickButtonByText('Clear session');
    await browser.waitForText('Session access required.');
    expect(await browser.evaluate(`document.querySelector('.ops-health')`)).toBeNull();
    expect(await browser.evaluate(`document.querySelector('.ops-brief')`)).toBeNull();
  });
  it('supports both themes and narrow screens with accessible text and controls', async () => {
    await connect();
    for (const width of [1440, 390]) {
      await browser.setViewportSize(width, 1000);
      for (const theme of ['dark', 'light']) {
        const current = await browser.evaluate<string>(`document.querySelector('.app').className`);
        if (!current.includes(`theme-${theme}`)) await browser.clickButtonByAriaLabel(`Switch to ${theme} theme`);
        const report = await browser.runAxeAudit();
        expect(report.violations, `${width}/${theme}`).toEqual([]);
        expect(report.incomplete, `${width}/${theme} incomplete`).toEqual([]);
      }
    }
    await browser.captureScreenshot(join(evidence, 'operations-mobile.png'));
  });
});
