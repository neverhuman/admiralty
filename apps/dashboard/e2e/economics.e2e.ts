import { expect, it } from 'vitest';
import { launchBrowserHarness, startDashboardTestServer, type BrowserHarness } from './browser-harness';

it('imports exact economics, filters units, compares cohorts, exports and rejects invalid data in the built dashboard', async () => {
  const server = await startDashboardTestServer();
  let browser: BrowserHarness | undefined;
  const row = { id: 'base-task', project: 'harbor', task: 'failed-task', role: 'worker', purpose: 'task', provider: 'codex',
    cache: 'uncached', cohort: 'baseline', maturity: 'mature', outcome: 'failed', rotation: 'unavailable', unit: 'token:input',
    amount: '9007199254740993', coverage: 'complete', harness_floor: null, source: 'fixture-ledger', version: 'v1' };
  const report = { schema_version: 1, observed_utc: '2026-10-19T14:20:00Z', source_revision: 'a'.repeat(40), rows: [row,
    { ...row, id: 'base-coord', purpose: 'coordination', amount: '10' },
    { ...row, id: 'candidate-task', cohort: 'candidate' },
    { ...row, id: 'candidate-coord', cohort: 'candidate', purpose: 'coordination', amount: '5' },
    { ...row, id: 'usd', unit: 'USD:micro', amount: null, coverage: 'unknown' },
  ] };
  async function importReport(value: unknown) {
    await browser!.evaluate(`(()=>{const input=document.querySelector('.economics input[type=file]');const transfer=new DataTransfer();transfer.items.add(new File([${JSON.stringify(JSON.stringify(value))}],'report.json',{type:'application/json'}));input.files=transfer.files;input.dispatchEvent(new Event('change',{bubbles:true}));})()`);
  }
  try {
    browser = await launchBrowserHarness();
    await browser.goto(`${server.baseUrl}/#/projects`);
    await browser.waitForText('Economics explorer');
    await importReport(report);
    await browser.waitForText('9007199254741003');
    expect(await browser.evaluate("document.querySelector('.economics')?.textContent")).toContain('unknown · 0/1');
    await browser.selectByLabel('Break down by', 'purpose');
    await browser.selectByLabel('Selected group', 'coordination');
    expect(await browser.evaluate("document.querySelector('[aria-label=\"Economics breakdown\"]')?.textContent")).not.toContain('9007199254740993');
    await browser.selectByLabel('Baseline cohort', 'baseline');
    await browser.selectByLabel('Candidate cohort', 'candidate');
    await browser.waitForText('Inconclusive: complete measured coverage is required.');
    await importReport({ ...report, rows: report.rows.slice(0, 4) });
    await browser.waitForText('4 observations');
    await browser.selectByLabel('Baseline cohort', 'baseline');
    await browser.selectByLabel('Candidate cohort', 'candidate');
    await browser.waitForText('coordination-only reduction 50.00%');
    await browser.evaluate("window.__economicsExport=null;URL.createObjectURL=blob=>{window.__economicsExport=blob;return 'blob:fixture'};HTMLAnchorElement.prototype.click=function(){}");
    await browser.clickButtonByText('Export selection (JSON)');
    const exported = await browser.evaluate<string>('window.__economicsExport.text()');
    expect(JSON.parse(exported).rows[0].amount).toBe('9007199254740993');
    expect(JSON.parse(exported).rows).toHaveLength(4);
    const axe = await browser.runAxeAudit();
    expect(axe.violations).toEqual([]); expect(axe.incomplete).toEqual([]);
    await importReport({ ...report, rows: [{ ...row, amount: 9007199254740992 }] });
    await browser.waitForText('Invalid economics row');
    expect(await browser.evaluate("document.querySelector('.economics')?.textContent")).toContain('Normalized usage unavailable');
    server.assertNoOverflow();
  } finally { await browser?.close(); await server.close(); }
}, 45_000);
