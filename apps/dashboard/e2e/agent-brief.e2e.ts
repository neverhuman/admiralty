import { mkdirSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
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
    const evidence = resolve('../../target/cutover-evidence/agent-brief');
    mkdirSync(evidence, { recursive: true });
    for (const theme of ['dark', 'light']) {
      if (theme === 'light') await browser.clickButtonByAriaLabel('Switch to light theme');
      for (const width of [390, 768, 1024, 1280]) {
        await browser.setViewportSize(width, 844);
        const geometry = await browser.evaluate(`(()=>({viewport:{width:innerWidth,height:innerHeight},active:document.activeElement?.tagName,
          regions:[...document.querySelectorAll('.main-area,.content,.brief-health-report,.brief-health-projects,.brief-health-table-scroll,.brief-health-cards,table,th,td')].map(e=>({tag:e.tagName,class:e.className,rect:e.getBoundingClientRect().toJSON(),scrollWidth:e.scrollWidth,clientWidth:e.clientWidth,scrollLeft:e.scrollLeft,overflow:getComputedStyle(e).overflow,display:getComputedStyle(e).display}))}))()`);
        writeFileSync(join(evidence, `geometry-${width}-${theme}.json`), JSON.stringify(geometry, null, 2));
        const audit = await browser.runAxeAudit();
        writeFileSync(join(evidence, `audit-${width}-${theme}.json`), JSON.stringify(audit, null, 2));
        await browser.captureScreenshot(join(evidence, `brief-${width}-${theme}.png`));
        expect(audit.violations, `${width}px ${theme}`).toEqual([]);
        expect(audit.incomplete, `${width}px ${theme}`).toEqual([]);
      }
    }
  } finally { await browser?.close(); await server.close(); }
});
