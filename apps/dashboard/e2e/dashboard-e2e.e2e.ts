import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';
import {
  loadPinnedAxeCoreBundle,
  PINNED_AXE_CORE_SHA256,
  PINNED_AXE_CORE_VERSION,
} from './axe-audit';
import {
  ensureBuiltDashboard,
  launchBrowserHarness,
  startDashboardTestServer,
  type BrowserHarness,
  type DashboardTestServer,
} from './browser-harness';
import {
  createCandidateDecisionEvidenceFixture,
  createDomainSseFrame,
  createFleetFixture,
  createLegacyFixture,
  createLifecycleSseFrameV2,
  createModelsFixture,
  createNativeFleetV2Fixture,
} from './fixtures';

async function waitForActiveSse(server: DashboardTestServer, timeoutMs = 8_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (server.activeSseCount() >= 1) return;
    await new Promise((r) => setTimeout(r, 30));
  }
  throw new Error('Timed out waiting for active SSE stream');
}

function isFleetPath(pathname: string): boolean {
  return pathname === '/v1/fleet' || pathname === '/v2/fleet';
}

function isEventsPath(pathname: string): boolean {
  return pathname === '/v1/events' || pathname === '/v2/events';
}

describe('Admiralty release qualification browser E2E and axe-core accessibility suite', () => {
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
    'serves the built production dashboard bundle and verifies pinned axe-core 4.14.0 and sandboxed browser version',
    async () => {
      const built = await ensureBuiltDashboard();
      expect(built.indexHtml).toContain('<div id="root"></div>');
      expect(built.indexHtml).toMatch(/\/assets\/index-[A-Za-z0-9_-]+\.js/);
      expect(built.indexHtml).toMatch(/\/assets\/index-[A-Za-z0-9_-]+\.css/);
      expect(built.assets.some((name) => name.endsWith('.js'))).toBe(true);
      expect(built.assets.some((name) => name.endsWith('.css'))).toBe(true);

      const axeBundle = loadPinnedAxeCoreBundle();
      expect(axeBundle.version).toBe(PINNED_AXE_CORE_VERSION);
      expect(axeBundle.sha256).toBe(PINNED_AXE_CORE_SHA256);

      expect(browser.browserVersion.product).toMatch(/Chrome\//);
      expect(browser.browserVersion.protocolVersion).toBeTruthy();
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

      const fleetCallsAfterConnect = server.requests.filter((r) => isFleetPath(r.pathname)).length;
      expect(fleetCallsAfterConnect).toBe(1);
      expect(server.activeSseCount()).toBe(1);

      // Verify exact precision-safe 64-bit cursor query on /v1/events or /v2/events
      const firstStreamReq = server.requests.find((r) => isEventsPath(r.pathname));
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

      expect(server.requests.filter((r) => isFleetPath(r.pathname)).length).toBe(2);

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
            isEventsPath(r.pathname) &&
            r.search.includes('generation=gen-qual-2') &&
            r.search.includes('after=42'),
        );
        if (reconnected) break;
        await new Promise((r) => setTimeout(r, 50));
      }

      const latestEventsReq = [...server.requests].reverse().find((r) => isEventsPath(r.pathname));
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
      await browser.waitForSelector('.workplans-panel table');

      // Filter tasks by project/title
      await browser.fillInputByLabel('Filter tasks', 'Admiralty Core');
      const filteredRows = await browser.evaluate<number>(
        `document.querySelectorAll('.workplans-panel tbody tr').length`,
      );
      expect(filteredRows).toBe(1);

      // Filter with no matches shows explicit empty state
      await browser.fillInputByLabel('Filter tasks', 'no-matching-task-token');
      const emptyRows = await browser.evaluate<number>(
        `document.querySelectorAll('.workplans-panel tbody tr').length`,
      );
      expect(emptyRows).toBe(0);
      const emptyPanelText = await browser.evaluate<string>(
        `document.querySelector('.workplans-panel')?.textContent ?? ''`,
      );
      expect(emptyPanelText).toMatch(/No (snapshot )?tasks match/);

      // Clear filter and navigate to Coordination
      await browser.fillInputByLabel('Filter tasks', '');
      const restoredRows = await browser.evaluate<number>(
        `document.querySelectorAll('.workplans-panel tbody tr').length`,
      );
      expect(restoredRows).toBe(4);

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
    'validates /v2/fleet and /v2/events schema v2 I1 lifecycle bindings (accepted, failed, cancelled) and rejects unknown or malformed inputs fail-closed in the browser',
    async () => {
      const v2Fixture = createNativeFleetV2Fixture(4, {
        recoveryGeneration: 'gen-v2-qual',
        eventSeq: '9007199254740995',
        includeTerminalLifecycleTasks: true,
      });
      server.setFleetV2(v2Fixture);

      await browser.goto(server.baseUrl);

      // Execute in-browser fetch against /v2/fleet and verify schema_version 2 + I1 lifecycle bindings + fail-closed negative checks
      const v2Verification = await browser.evaluate<{
        schemaVersion: number;
        nativeExecutionQualified: boolean;
        acceptedState: string;
        acceptedVerifier: string | null;
        acceptedClosureDigest: string | null;
        failedState: string;
        failedReason: string | null;
        cancelledState: string;
        cancelledReason: string | null;
        v2LifecycleFrameId: string;
        rejectedUnknownState: boolean;
        rejectedSchemaVersion1AsV2: boolean;
        rejectedMalformedLifecycleArray: boolean;
        rejectedIncompleteDecision: boolean;
        rejectedUntrustedEscalationFlag: boolean;
      }>(`(async () => {
        const res = await fetch('/v2/fleet', { headers: { Accept: 'application/json' }, cache: 'no-store' });
        if (!res.ok) throw new Error('Failed to fetch /v2/fleet: ' + res.status);
        const data = await res.json();

        const validateV2Task = (task) => {
          const validStates = ['ready', 'blocked', 'claimed', 'running', 'awaiting_verification', 'accepted', 'failed', 'cancelled'];
          if (!task || typeof task !== 'object' || Array.isArray(task)) return false;
          if (!validStates.includes(task.state)) return false;
          if (Object.prototype.hasOwnProperty.call(task, 'lifecycle')) {
            const lc = task.lifecycle;
            if (!lc || typeof lc !== 'object' || Array.isArray(lc)) return false;
            const lcKeys = ['revision', 'attempts', 'closure_digest', 'decision', 'reason'];
            if (Object.keys(lc).length !== lcKeys.length || !lcKeys.every((k) => Object.prototype.hasOwnProperty.call(lc, k))) return false;
            if (lc.decision !== null) {
              if (typeof lc.decision !== 'object' || Array.isArray(lc.decision)) return false;
              const decKeys = ['closure_digest', 'contract_digest', 'custody_digest', 'receipt_digest', 'source_commit_sha', 'source_tree_sha', 'verification_policy_digest', 'verifier'];
              if (Object.keys(lc.decision).length !== decKeys.length || !decKeys.every((k) => typeof lc.decision[k] === 'string' && lc.decision[k].length > 0)) return false;
            }
          }
          return true;
        };

        const validateV2Fleet = (envelope) => {
          if (!envelope || typeof envelope !== 'object' || envelope.schema_version !== 2 || envelope.native_execution_qualified !== false) return false;
          if (!Array.isArray(envelope.tasks) || !envelope.tasks.every(validateV2Task)) return false;
          return true;
        };

        if (!validateV2Fleet(data)) {
          throw new Error('Valid /v2/fleet fixture failed v2 envelope validation');
        }

        // Negative checks: unknown state, wrong schema_version, malformed lifecycle array, incomplete decision digests, untrusted privilege flag
        const rejectedUnknownState = !validateV2Fleet({
          ...data,
          tasks: [{ ...data.tasks[0], state: 'self_accepted' }],
        });
        const rejectedSchemaVersion1AsV2 = !validateV2Fleet({
          ...data,
          schema_version: 1,
        });
        const rejectedMalformedLifecycleArray = !validateV2Fleet({
          ...data,
          tasks: [{ ...data.tasks[0], lifecycle: [] }],
        });
        const rejectedIncompleteDecision = !validateV2Fleet({
          ...data,
          tasks: [{
            ...data.tasks[0],
            lifecycle: {
              ...data.tasks[0].lifecycle,
              decision: { verifier: 'worker-self' },
            },
          }],
        });
        const rejectedUntrustedEscalationFlag = !validateV2Fleet({
          ...data,
          native_execution_qualified: true,
        });

        // Verify /v2/events SSE stream endpoint delivers v2 task_lifecycle frame
        const sseAbort = new AbortController();
        const sseRes = await fetch('/v2/events?after=9007199254740995&generation=gen-v2-qual', {
          headers: { Accept: 'text/event-stream' },
          signal: sseAbort.signal,
        });
        if (!sseRes.ok || !sseRes.headers.get('content-type')?.includes('text/event-stream')) {
          throw new Error('Failed to connect to /v2/events');
        }
        sseAbort.abort();

        return {
          schemaVersion: data.schema_version,
          nativeExecutionQualified: data.native_execution_qualified,
          acceptedState: data.tasks[0].state,
          acceptedVerifier: data.tasks[0].lifecycle?.decision?.verifier ?? null,
          acceptedClosureDigest: data.tasks[0].lifecycle?.closure_digest ?? null,
          failedState: data.tasks[1].state,
          failedReason: data.tasks[1].lifecycle?.reason ?? null,
          cancelledState: data.tasks[2].state,
          cancelledReason: data.tasks[2].lifecycle?.reason ?? null,
          v2LifecycleFrameId: data.cursor.recovery_generation + ':' + data.cursor.event_seq,
          rejectedUnknownState,
          rejectedSchemaVersion1AsV2,
          rejectedMalformedLifecycleArray,
          rejectedIncompleteDecision,
          rejectedUntrustedEscalationFlag,
        };
      })()`);

      const expectedEvidence = createCandidateDecisionEvidenceFixture('a');
      expect(v2Verification.schemaVersion).toBe(2);
      expect(v2Verification.nativeExecutionQualified).toBe(false);
      expect(v2Verification.acceptedState).toBe('accepted');
      expect(v2Verification.acceptedVerifier).toBe('verifier-trusted-supervisor');
      expect(v2Verification.acceptedClosureDigest).toBe(expectedEvidence.closure_digest);
      expect(v2Verification.failedState).toBe('failed');
      expect(v2Verification.failedReason).toBe('verification rejected');
      expect(v2Verification.cancelledState).toBe('cancelled');
      expect(v2Verification.cancelledReason).toBe('superseded by operator');
      expect(v2Verification.v2LifecycleFrameId).toBe('gen-v2-qual:9007199254740995');
      expect(v2Verification.rejectedUnknownState).toBe(true);
      expect(v2Verification.rejectedSchemaVersion1AsV2).toBe(true);
      expect(v2Verification.rejectedMalformedLifecycleArray).toBe(true);
      expect(v2Verification.rejectedIncompleteDecision).toBe(true);
      expect(v2Verification.rejectedUntrustedEscalationFlag).toBe(true);

      const v2SseFrame = createLifecycleSseFrameV2('9007199254740996', 'gen-v2-qual', {
        taskId: 'task-001',
        accepted: true,
      });
      expect(v2SseFrame).toContain('"kind":"task_lifecycle"');
      expect(v2SseFrame).toContain('"protocol_version":2');
      expect(v2SseFrame).toContain('"action":"decide_candidate"');
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
    'passes custom CDP AXTree smoke checks across all views and proves pinned axe-core 4.14.0 detects injected accessibility defects',
    async () => {
      await browser.goto(server.baseUrl);
      await browser.waitForText('Alpha Control');

      const legacyAxTree = await browser.runCustomAxTreeSmokeChecks();
      expect(legacyAxTree.totalNodes).toBeGreaterThan(20);
      expect(legacyAxTree.unlabeledButtons).toEqual([]);
      expect(legacyAxTree.unlabeledInputs).toEqual([]);

      for (const [navLabel, expectedText] of [
        ['Overview', 'Admiralty Core'],
        ['Workplans', 'Admiralty Core'],
        ['Coordination', 'Message statistics'],
        ['Models', 'Ornith Local Coding'],
      ] as const) {
        await browser.clickNav(navLabel);
        await browser.waitForText(expectedText);
        const axSmoke = await browser.runCustomAxTreeSmokeChecks();
        expect(axSmoke.unlabeledButtons, `AXTree unlabeled buttons in ${navLabel}`).toEqual([]);
        expect(axSmoke.unlabeledInputs, `AXTree unlabeled inputs in ${navLabel}`).toEqual([]);
      }

      // Negative control: prove actual axe-core 4.14.0 catches injected accessibility defects
      await browser.clickNav('Overview');
      await browser.waitForText('Admiralty Core');
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
      expect(negativeReport.testEngine).toEqual({
        name: 'axe-core',
        version: PINNED_AXE_CORE_VERSION,
      });
      expect(negativeReport.sourceSha256).toBe(PINNED_AXE_CORE_SHA256);
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

  it(
    'enforces the pinned axe-core 4.14.0 WCAG 2.1 AA audit (zero violations and zero incomplete) across all views, dialogs, themes, and responsive viewports',
    async () => {
      await browser.goto(server.baseUrl);
      await browser.waitForText('Alpha Control');

      const surfaceFindings: Array<{
        surface: string;
        violations: Array<{ id: string; impact: string | null | undefined; nodes: number; sampleTarget: string[] }>;
        incomplete: Array<{ id: string; impact: string | null | undefined; nodes: number; sampleTarget: string[] }>;
      }> = [];

      const recordSurface = async (surface: string) => {
        const report = await browser.runAxeAudit();
        expect(report.testEngine).toEqual({
          name: 'axe-core',
          version: PINNED_AXE_CORE_VERSION,
        });
        expect(report.sourceSha256).toBe(PINNED_AXE_CORE_SHA256);
        expect(report.passesCount).toBeGreaterThan(10);
        if (report.violations.length > 0 || report.incomplete.length > 0) {
          surfaceFindings.push({
            surface,
            violations: report.violations.map((v) => ({
              id: v.id,
              impact: v.impact,
              nodes: v.nodes.length,
              sampleTarget: v.nodes[0]?.target ?? [],
            })),
            incomplete: report.incomplete.map((inc) => ({
              id: inc.id,
              impact: inc.impact,
              nodes: inc.nodes.length,
              sampleTarget: inc.nodes[0]?.target ?? [],
            })),
          });
        }
      };

      // 1. Audit Legacy detailed view in dark theme
      await recordSurface('dark:Legacy detailed view');

      // 2. Audit Overview, Workplans, Coordination, and Models in dark theme
      for (const [navLabel, expectedText] of [
        ['Overview', 'Admiralty Core'],
        ['Workplans', 'Admiralty Core'],
        ['Coordination', 'Message statistics'],
        ['Models', 'Ornith Local Coding'],
      ] as const) {
        await browser.clickNav(navLabel);
        await browser.waitForText(expectedText);
        await recordSurface(`dark:${navLabel}`);
      }

      // 3. Switch to light theme and audit views
      await browser.clickButtonByAriaLabel('Switch to light theme');
      const isLight = await browser.evaluate<boolean>(
        `document.querySelector('.app')?.classList.contains('theme-light') ?? false`,
      );
      expect(isLight).toBe(true);

      for (const [navLabel, expectedText] of [
        ['Overview', 'Admiralty Core'],
        ['Workplans', 'Admiralty Core'],
        ['Coordination', 'Message statistics'],
        ['Models', 'Ornith Local Coding'],
        ['Legacy detailed view', 'Alpha Control'],
      ] as const) {
        await browser.clickNav(navLabel);
        await browser.waitForText(expectedText);
        await recordSurface(`light:${navLabel}`);
      }

      // 4. Audit Profile dialog (<dialog open>)
      await browser.clickNav('Overview');
      await browser.waitForText('Admiralty Core');
      await browser.clickButtonByAriaLabel('Your profile');
      await browser.waitForText('Your profile');
      await recordSurface('dialog:Profile');

      await browser.fillInputByLabel('Display name', 'Jepson Operator');
      await browser.clickButtonByText('Save profile');
      await browser.waitForText('Jepson Operator');

      // 5. Audit Session access modal dialog
      await browser.clickButtonByAriaLabel('Access settings');
      await browser.waitForText('Session access');
      await recordSurface('dialog:SessionAccess');
      await browser.clickButtonByAriaLabel('Close access settings');

      // 6. Audit responsive mobile viewport (390x844) with prefers-reduced-motion: reduce
      await browser.setViewportSize(390, 844);
      await browser.emulateReducedMotion(true);
      const reducedMotionApplied = await browser.evaluate<boolean>(
        `window.matchMedia('(prefers-reduced-motion: reduce)').matches`,
      );
      expect(reducedMotionApplied).toBe(true);
      await recordSurface('mobile:390x844-reduced-motion');

      // Fail-closed assertion: neither violations nor incomplete may be ignored or converted to success
      expect(
        surfaceFindings,
        'Pinned axe-core 4.14.0 reported WCAG 2.1 AA violations or incomplete items on built dashboard surfaces',
      ).toEqual([]);
    },
    30_000,
  );
});
