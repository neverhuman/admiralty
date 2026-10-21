import { expect, it } from 'vitest';
import { launchBrowserHarness, startIsolatedNativeAdmiraltyServer, type BrowserHarness } from './browser-harness';

it('renders the bounded brief from the real Rust service with stale and unknown evidence', async () => {
  const source = { generated_utc: '2026-10-20T00:00:00Z', order: ['fleet-a'], projects: {
    'fleet-a': { total: 100, done: 8, blocked: 2, captain: { tail: 'PRIVATE SOURCE TAIL' } },
  } };
  const server = await startIsolatedNativeAdmiraltyServer({ legacySnapshot: source, legacyPublicRead: true });
  let browser: BrowserHarness | undefined;
  try {
    const response = await fetch(`${server.baseUrl}/v1/legacy/brief`);
    const bytes = await response.text(); expect(response.status).toBe(200);
    expect(new TextEncoder().encode(bytes).length).toBeLessThanOrEqual(16384);
    expect(bytes).not.toContain('PRIVATE SOURCE TAIL');
    browser = await launchBrowserHarness();
    await browser.goto(`${server.baseUrl}/#/brief`);
    await browser.waitForText('Producer observations are stale');
    await browser.waitForText('fleet-a');
    const text = await browser.evaluate<string>('document.body.textContent');
    expect(text).toContain('missing'); expect(text).toContain('unknown');
    expect(text).not.toContain('PRIVATE SOURCE TAIL');
    const axe = await browser.runAxeAudit();
    expect(axe.violations).toEqual([]); expect(axe.incomplete).toEqual([]);
    await browser.setViewportSize(390, 844);
    const mobile = await browser.runAxeAudit();
    expect(mobile.violations).toEqual([]); expect(mobile.incomplete).toEqual([]);
  } finally { await browser?.close(); await server.close(); }
});
