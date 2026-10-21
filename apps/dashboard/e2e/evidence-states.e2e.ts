import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { ensureBuiltDashboard, launchBrowserHarness, startDashboardTestServer, type BrowserHarness, type DashboardTestServer } from './browser-harness';
import { createFleetFixture } from './fixtures';

describe('P0 Commands evidence availability browser journey', () => {
  let browser: BrowserHarness;
  let server: DashboardTestServer;

  beforeAll(async () => {
    if (!process.env.ADMIRALTY_P0_EVIDENCE_DIR) {
      throw new Error('Set ADMIRALTY_P0_EVIDENCE_DIR to the private P0 evidence directory.');
    }
    await ensureBuiltDashboard();
    server = await startDashboardTestServer({ skipBuildCheck: true });
    browser = await launchBrowserHarness();
  }, 60_000);

  afterAll(async () => {
    await browser?.close();
    await server?.close();
  }, 90_000);

  it('shows 401 as unavailable, loaded empty as observed zero, and failed refresh as retained cursor without posting', async () => {
    const evidenceDir = process.env.ADMIRALTY_P0_EVIDENCE_DIR!;
    server.setFleetStatus(401, 'Synthetic access required fixture');
    await browser.resetPage();
    await browser.goto(`${server.baseUrl}/#/commands`);
    await browser.waitForText('Fleet access required');
    await browser.waitForText('Local contract fields valid; duplicate coverage unavailable');
    await browser.waitForText('Duplicate and overlap check unavailable until a fleet snapshot is observed.');
    expect(await browser.evaluate<string>(`document.body.innerText`)).not.toContain('Ready for pending task submission');
    expect(await browser.evaluate<string>(`document.body.innerText`)).not.toContain('No duplicate or overlapping candidates detected.');
    await browser.captureScreenshot(`${evidenceDir}/401-unavailable.png`);
    expect(server.requests.filter(request => request.method === 'POST')).toEqual([]);

    server.setFleet(createFleetFixture(0, { eventSeq: '0' }));
    server.clearRequests();
    await browser.resetPage();
    await browser.goto(`${server.baseUrl}/#/commands`);
    await browser.waitForText('Ready for pending task submission');
    await browser.waitForText('0 of 0 snapshot task(s)');
    expect(await browser.evaluate<string>(`document.body.innerText`)).toContain('No duplicate task IDs');
    await browser.captureScreenshot(`${evidenceDir}/loaded-empty.png`);
    expect(server.requests.filter(request => request.method === 'POST')).toEqual([]);

    server.setFleet(createFleetFixture(1, { eventSeq: '8' }));
    server.clearRequests();
    await browser.resetPage();
    await browser.goto(`${server.baseUrl}/#/commands`);
    await browser.waitForText('Ready for pending task submission');
    server.setFleetStatus(401, 'Synthetic refresh failure fixture');
    await browser.clickButtonByText('Operator actions');
    await browser.clickButtonByText('Refresh snapshot & replay');
    await browser.waitForText('Retained fleet snapshot');
    await browser.waitForText('Last observed source cursor: recovery generation gen-qual-1, event seq #8');
    await browser.clickButtonByText('Work intake');
    await browser.waitForText('snapshot stale');
    await browser.waitForText('submission is paused until refresh succeeds');
    await browser.waitForText('Refresh snapshot before submitting');
    expect(await browser.evaluate<boolean>(`[...document.querySelectorAll('.models-form-actions button')].some(button => button.textContent?.includes('Refresh snapshot before submitting') && button.disabled)`)).toBe(true);
    await browser.captureScreenshot(`${evidenceDir}/retained-stale-cursor.png`);
    await browser.evaluate(`document.querySelector('#intake-preview-heading')?.scrollIntoView({ block: 'start' })`);
    await browser.captureScreenshot(`${evidenceDir}/retained-stale-preview.png`);
    expect(server.requests.filter(request => request.method === 'POST')).toEqual([]);
    server.assertNoOverflow();
  }, 60_000);
});
