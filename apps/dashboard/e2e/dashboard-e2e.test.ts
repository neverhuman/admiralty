import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';
import {
  ensureBuiltDashboard,
  launchBrowserHarness,
  startDashboardTestServer,
  type BrowserHarness,
  type DashboardTestServer,
} from './browser-harness';
import {
  createDomainSseFrame,
  createFleetFixture,
  createLegacyFixture,
  createModelsFixture,
} from './fixtures';

async function waitForActiveSse(server: DashboardTestServer, timeoutMs = 8_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (server.activeSseCount() >= 1) return;
    await new Promise((r) => setTimeout(r, 30));
  }
  throw new Error('Timed out waiting for active /v1/events SSE stream');
}

describe('Admiralty release qualification browser E2E and axe accessibility suite', () => {
  let server: DashboardTestServer;
  let browser: BrowserHarness;

  beforeAll(async () => {
    server = await startDashboardTestServer();
    browser = await launchBrowserHarness();
  }, 30_000);

  afterAll(async () => {
    await browser?.close();
    await server?.close();
  }, 30_000);

  beforeEach(async () => {
    await browser.resetPage();
    server.closeActiveSseStreams();
    await new Promise((r) => setTimeout(r, 60));
    server.setRequireBearerToken(null);
    server.setFleet(createFleetFixture(4));
    server.setLegacy(createLegacyFixture(4), 200);
    server.setModels(createModelsFixture('1'), 200);
    server.setModelsPutConflict(false);
    server.clearRequests();
    await browser.setViewportSize(1440, 900);
    await browser.emulateReducedMotion(false);
  }, 30_000);

  it(
    'serves the built production dashboard bundle from dist/ with compiled JS and CSS assets',
    async () => {
      const built = await ensureBuiltDashboard();
      expect(built.indexHtml).toContain('<div id="root"></div>');
      expect(built.indexHtml).toMatch(/\/assets\/index-[A-Za-z0-9_-]+\.js/);
      expect(built.indexHtml).toMatch(/\/assets\/index-[A-Za-z0-9_-]+\.css/);
      expect(built.assets.some((name) => name.endsWith('.js'))).toBe(true);
      expect(built.assets.some((name) => name.endsWith('.css'))).toBe(true);
    },
    30_000,
  );

  it(
    'renders Legacy detailed view, project tabs, temporal plots, hierarchy, and scales across 4, 20, and 100 projects',
    async () => {
      await browser.goto(server.baseUrl);
      await browser.waitForText('Alpha Control');
      await browser.waitForText('Jepson Taylor');
      await browser.waitForText('Token efficiency');

      // 4 producer projects + 1 rollout project card
      const initialCardCount = await browser.evaluate<number>(
        `document.querySelectorAll('article[data-project-id]').length`,
      );
      expect(initialCardCount).toBe(5);

      // Select a single project tab and verify tab selection + filtered card view
      await browser.clickButtonByAriaLabel('Alpha Control');
      const selectedTabCount = await browser.evaluate<number>(
        `document.querySelectorAll('article[data-project-id]').length`,
      );
      expect(selectedTabCount).toBe(1);

      await browser.clickButtonByText('All projects');
      const restoredCount = await browser.evaluate<number>(
        `document.querySelectorAll('article[data-project-id]').length`,
      );
      expect(restoredCount).toBe(5);

      // Expand a temporal plot card and verify aria-expanded toggles
      await browser.clickButtonByAriaLabel('Expand Cost index vs all-Luna (1.0 = all Luna)');
      await browser.waitForText('Missing observations remain unknown.');
      const isExpanded = await browser.evaluate<boolean>(
        `document.querySelector('[data-metric="cost-index"] button.temporal-toggle')?.getAttribute('aria-expanded') === 'true'`,
      );
      expect(isExpanded).toBe(true);

      // Toggle density from dense to comfortable
      await browser.selectByLabel('Density', 'comfortable');
      const hasComfortableClass = await browser.evaluate<boolean>(
        `document.querySelector('.legacy-detailed')?.classList.contains('density-comfortable') ?? false`,
      );
      expect(hasComfortableClass).toBe(true);

      // Scale qualification: 20 projects and 100 projects
      for (const count of [20, 100]) {
        server.setLegacy(createLegacyFixture(count), 200);
        await browser.clickButtonByText('Refresh source');
        await browser.waitForText(`Fleet Project ${count}`);
        const renderedCards = await browser.evaluate<number>(
          `document.querySelectorAll('article[data-project-id]').length`,
        );
        expect(renderedCards).toBe(count + 1);
      }

      // Verify 100 projects in Native Overview via SSE update as well
      await waitForActiveSse(server);
      server.setFleet(createFleetFixture(100, { eventSeq: '9007199254740994' }));
      server.sendSseFrame(createDomainSseFrame('9007199254740994'));
      await browser.clickNav('Overview');
      await browser.waitForText('Project 100');
      const nativeProjectCards = await browser.evaluate<number>(
        `document.querySelectorAll('.project-card').length`,
      );
      expect(nativeProjectCards).toBe(100);
    },
    30_000,
  );

  it(
    'maintains a single SSE event stream without 15s polling and reconciles domain events and snapshot_required recovery',
    async () => {
      server.clearRequests();
      await browser.goto(server.baseUrl);
      await browser.clickNav('Overview');
      await browser.waitForText('Live updates');
      await browser.waitForText('Admiralty Core');
      await waitForActiveSse(server);

      const fleetCallsAfterConnect = server.requests.filter((r) => r.pathname === '/v1/fleet').length;
      expect(fleetCallsAfterConnect).toBe(1);
      expect(server.activeSseCount()).toBe(1);

      // Verify exact precision-safe 64-bit cursor query on /v1/events
      const firstStreamReq = server.requests.find((r) => r.pathname === '/v1/events');
      expect(firstStreamReq?.search).toContain('after=9007199254740993');
      expect(firstStreamReq?.search).toContain('generation=gen-qual-1');

      // Emit a live domain event with a higher sequence cursor and updated project counts
      const updatedFleet = createFleetFixture(6, {
        recoveryGeneration: 'gen-qual-1',
        eventSeq: '9007199254740994',
      });
      server.setFleet(updatedFleet);
      server.sendSseFrame(createDomainSseFrame('9007199254740994', 'gen-qual-1'));
      await browser.waitForText('Project 6');

      expect(server.requests.filter((r) => r.pathname === '/v1/fleet').length).toBe(2);

      // Emit snapshot_required on recovery generation change and verify automatic resnapshot + stream reconnect
      const recoveredFleet = createFleetFixture(5, {
        recoveryGeneration: 'gen-qual-2',
        eventSeq: '42',
      });
      server.setFleet(recoveredFleet);
      server.sendSseFrame('event: snapshot_required\ndata: recovery generation changed\n\n');

      const deadline = Date.now() + 8_000;
      while (Date.now() < deadline) {
        const reconnected = server.requests.some(
          (r) =>
            r.pathname === '/v1/events' &&
            r.search.includes('generation=gen-qual-2') &&
            r.search.includes('after=42'),
        );
        if (reconnected) break;
        await new Promise((r) => setTimeout(r, 50));
      }

      const latestEventsReq = [...server.requests].reverse().find((r) => r.pathname === '/v1/events');
      expect(latestEventsReq?.search).toContain('generation=gen-qual-2');
      expect(latestEventsReq?.search).toContain('after=42');
    },
    30_000,
  );

  it(
    'filters Workplans task stages and preserves unknown token accounting in Coordination',
    async () => {
      await browser.goto(server.baseUrl);
      await browser.clickNav('Workplans');
      await browser.waitForText('Task stage table');
      await browser.waitForText('4 TASKS');

      // Filter tasks by project/title
      await browser.fillInputByLabel('Filter tasks', 'Admiralty Core');
      await browser.waitForText('1 TASKS');
      const filteredRows = await browser.evaluate<number>(
        `document.querySelectorAll('.workplans-panel tbody tr').length`,
      );
      expect(filteredRows).toBe(1);

      // Filter with no matches shows explicit empty state
      await browser.fillInputByLabel('Filter tasks', 'no-matching-task-token');
      await browser.waitForText('No tasks match this filter.');

      // Clear filter and navigate to Coordination
      await browser.fillInputByLabel('Filter tasks', '');
      await browser.waitForText('4 TASKS');

      await browser.clickNav('Coordination');
      await browser.waitForText('Message statistics');
      await browser.waitForText('Measured usage');
      const accountingText = await browser.evaluate<string>(
        `document.querySelector('.accounting-value')?.textContent?.trim() ?? ''`,
      );
      expect(accountingText).toContain('Unknown');
      expect(accountingText).not.toBe('0 tokens');
    },
    30_000,
  );

  it(
    'manages Models policy configuration, surfaces 409 revision conflicts without overwrite, and sends bearer tokens only in headers',
    async () => {
      await browser.goto(server.baseUrl);
      await browser.clickNav('Models');
      await browser.waitForText('Configured models');
      await browser.waitForText('Ornith Local Coding');

      // Add a new OpenAI-compatible model via the form
      await browser.fillInputByLabel('Model ID', 'local-vllm-t0');
      await browser.fillInputByLabel('Display label', 'Local vLLM Worker');
      await browser.fillInputByLabel('Provider model', 'qwen3-coder-32b');
      await browser.fillInputByLabel('Preference tier', '0');
      await browser.fillInputByLabel('Endpoint', 'http://127.0.0.1:8000/v1');
      await browser.fillInputByLabel('Attempts per task in this tier', '2');
      await browser.clickButtonByText('Add model');

      await browser.waitForText('Model policy saved.');
      await browser.waitForText('Local vLLM Worker');

      // Trigger a 409 conflict on next save and verify latest policy is reloaded without silent retry overwrite
      server.setModelsPutConflict(true);
      await browser.clickButtonByText('Disable');
      await browser.waitForText(
        'Registry changed elsewhere. Latest policy loaded; review your draft and submit again.',
      );

      // Require bearer token and verify 401 -> Session access modal -> Authorization header (never in URL)
      server.setRequireBearerToken('adm-secret-session-token');
      await browser.clickButtonByText('Refresh settings');
      await browser.waitForText('Open session access');
      await browser.clickButtonByText('Open session access');
      await browser.waitForText('Session access');

      await browser.fillInputByLabel('Bearer token', 'adm-secret-session-token');
      await browser.clickButtonByText('Apply token');
      await browser.waitForText('Ornith Local Coding');

      const authenticatedModelsReq = [...server.requests]
        .reverse()
        .find((r) => r.pathname === '/v1/models' && r.authorization !== null);
      expect(authenticatedModelsReq?.authorization).toBe('Bearer adm-secret-session-token');
      for (const req of server.requests) {
        expect(req.url).not.toContain('adm-secret-session-token');
      }
      const pageUrl = await browser.evaluate<string>(`window.location.href`);
      expect(pageUrl).not.toContain('adm-secret-session-token');
    },
    30_000,
  );

  it(
    'passes the WCAG 2.1 AA axe accessibility and Chromium AXTree suite across all views, dialogs, themes, and responsive viewports',
    async () => {
      await browser.goto(server.baseUrl);
      await browser.waitForText('Alpha Control');

      // 1. Audit Legacy detailed view in dark theme
      const legacyDarkReport = await browser.runAxeAudit();
      expect(legacyDarkReport.violations).toEqual([]);
      expect(legacyDarkReport.axTreeSummary?.unlabeledButtons).toEqual([]);
      expect(legacyDarkReport.axTreeSummary?.unlabeledInputs).toEqual([]);

      // 2. Audit Overview, Workplans, Coordination, and Models in dark theme
      for (const [navLabel, expectedHeading] of [
        ['Overview', 'Fleet at a glance'],
        ['Workplans', 'Task stage table'],
        ['Coordination', 'Message statistics'],
        ['Models', 'Configured models'],
      ] as const) {
        await browser.clickNav(navLabel);
        await browser.waitForText(expectedHeading);
        const report = await browser.runAxeAudit();
        expect(report.violations, `axe violations in dark ${navLabel}`).toEqual([]);
        expect(
          report.axTreeSummary?.unlabeledButtons,
          `AXTree unlabeled buttons in ${navLabel}`,
        ).toEqual([]);
        expect(
          report.axTreeSummary?.unlabeledInputs,
          `AXTree unlabeled inputs in ${navLabel}`,
        ).toEqual([]);
      }

      // 3. Switch to light theme and audit views
      await browser.clickButtonByAriaLabel('Switch to light theme');
      const isLight = await browser.evaluate<boolean>(
        `document.querySelector('.app')?.classList.contains('theme-light') ?? false`,
      );
      expect(isLight).toBe(true);

      for (const [navLabel, expectedHeading] of [
        ['Overview', 'Fleet at a glance'],
        ['Workplans', 'Task stage table'],
        ['Coordination', 'Message statistics'],
        ['Legacy detailed view', 'Alpha Control'],
      ] as const) {
        await browser.clickNav(navLabel);
        await browser.waitForText(expectedHeading);
        const report = await browser.runAxeAudit();
        expect(report.violations, `axe violations in light ${navLabel}`).toEqual([]);
      }

      // 4. Audit Profile dialog (<dialog open>) and keyboard / focus interaction
      await browser.clickNav('Overview');
      await browser.waitForText('Fleet at a glance');
      await browser.clickButtonByAriaLabel('Your profile');
      await browser.waitForText('Your profile');

      const profileDialogReport = await browser.runAxeAudit();
      expect(profileDialogReport.violations).toEqual([]);

      await browser.fillInputByLabel('Display name', 'Jepson Operator');
      await browser.clickButtonByText('Save profile');
      await browser.waitForText('Jepson Operator');

      // 5. Audit Session access modal dialog
      await browser.clickButtonByAriaLabel('Access settings');
      await browser.waitForText('Session access');
      const accessModalReport = await browser.runAxeAudit();
      expect(accessModalReport.violations).toEqual([]);
      await browser.clickButtonByAriaLabel('Close access settings');

      // 6. Audit responsive mobile viewport (390x844) with prefers-reduced-motion: reduce
      await browser.setViewportSize(390, 844);
      await browser.emulateReducedMotion(true);
      const reducedMotionApplied = await browser.evaluate<boolean>(
        `window.matchMedia('(prefers-reduced-motion: reduce)').matches`,
      );
      expect(reducedMotionApplied).toBe(true);
      const mobileReport = await browser.runAxeAudit();
      expect(mobileReport.violations).toEqual([]);

      // 7. Negative control: prove the axe audit catches injected accessibility defects
      await browser.setViewportSize(1440, 900);
      await browser.evaluate(`(() => {
        const badBtn = document.createElement('button');
        badBtn.id = 'injected-unlabeled-btn';
        badBtn.style.width = '24px';
        badBtn.style.height = '24px';
        const badImg = document.createElement('img');
        badImg.id = 'injected-no-alt-img';
        badImg.src = '/brand/admiralty-source.png';
        badImg.style.width = '24px';
        badImg.style.height = '24px';
        const badContrast = document.createElement('h2');
        badContrast.id = 'injected-low-contrast-h2';
        badContrast.textContent = 'Unreadable heading';
        badContrast.style.color = '#161b22';
        badContrast.style.backgroundColor = '#161b22';
        document.querySelector('main')?.append(badBtn, badImg, badContrast);
      })()`);
      const negativeReport = await browser.runAxeAudit();
      const ruleIds = negativeReport.violations.map((v) => v.id);
      expect(ruleIds).toContain('button-name');
      expect(ruleIds).toContain('image-alt');
      expect(ruleIds).toContain('color-contrast');

      await browser.evaluate(`(() => {
        document.getElementById('injected-unlabeled-btn')?.remove();
        document.getElementById('injected-no-alt-img')?.remove();
        document.getElementById('injected-low-contrast-h2')?.remove();
      })()`);
    },
    30_000,
  );
});
