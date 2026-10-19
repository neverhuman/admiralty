import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { launchBrowserHarness, startDashboardTestServer, type BrowserHarness, type DashboardTestServer } from './browser-harness';
import { createNativeFleetV2Fixture } from './fixtures';

describe('P29 integration and quality browser journey', () => {
  let browser: BrowserHarness;
  let server: DashboardTestServer;

  beforeAll(async () => {
    server = await startDashboardTestServer();
    server.setFleet(createNativeFleetV2Fixture(2));
    browser = await launchBrowserHarness();
  }, 60_000);

  afterAll(async () => {
    await browser?.close();
    await server?.close();
  });

  it('filters candidates, opens evidence with a keyboard, and passes axe at narrow width with reduced motion', async () => {
    await browser.goto(`${server.baseUrl}/#/commands`);
    await browser.waitForText('Integration & quality');
    await browser.clickButtonByText('Integration & quality');
    await browser.waitForText('Candidates, blockers & evidence');
    await browser.fillInputByLabel('Search candidates or blockers', 'no-matching-candidate');
    await browser.waitForText('No tasks match the current filters.');
    await browser.fillInputByLabel('Search candidates or blockers', '');
    await browser.waitForSelector('.integration-table tbody button');
    const targetTitle = await browser.evaluate<string>(`document.querySelector('.integration-table tbody button').textContent`);
    await browser.evaluate(`document.querySelector('.integration-table tbody button').focus()`);
    await browser.pressKey('Enter');
    await browser.waitForText('CAS-guarded pending cancellation');
    expect(await browser.evaluate<string>(`document.body.innerText`)).toContain(targetTitle);
    await browser.clickButtonByText('Integration & quality');
    await browser.setViewportSize(390, 844);
    await browser.emulateReducedMotion(true);
    for (const theme of ['theme-dark', 'theme-light']) {
      await browser.evaluate(`document.querySelector('.app').classList.remove('theme-dark', 'theme-light'); document.querySelector('.app').classList.add('${theme}')`);
      const audit = await browser.runAxeAudit();
      expect(audit.violations).toEqual([]);
    }
    expect(await browser.evaluate<boolean>(`document.documentElement.scrollWidth <= window.innerWidth`)).toBe(true);
    await browser.captureScreenshot('../../target/cutover-evidence/p29-integration-mobile.png');
    expect(server.requests.filter(request => request.method !== 'GET')).toEqual([]);
    server.assertNoOverflow();
  }, 60_000);
});
