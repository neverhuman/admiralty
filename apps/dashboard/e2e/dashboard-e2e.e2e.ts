import { existsSync } from 'node:fs';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { parseDomainRecordV2, parseNativeFleet } from '../src/validation';
import {
  loadPinnedAxeCoreBundle,
  PINNED_AXE_CORE_SHA256,
  PINNED_AXE_CORE_VERSION,
} from './axe-audit';
import {
  computeDashboardSourceDigest,
  computeDistBundleDigest,
  createBoundedCdpFrameReader,
  ensureBuiltDashboard,
  fillInputByLabelStage,
  isProcessAlive,
  launchBrowserHarness,
  MAX_STAGE_EVALUATE_BUDGET_MS,
  selectByLabelStage,
  startDashboardTestServer,
  startIsolatedNativeAdmiraltyServer,
  type BrowserHarness,
  type DashboardTestServer,
} from './browser-harness';
import {
  computeCanonicalTaskViewDigest,
  createCandidateDecisionEvidenceFixture,
  createDomainSseFrame,
  createFleetFixture,
  createLegacyFixture,
  createLifecycleSseFrameV2,
  createModelsFixture,
  createNativeFleetV2Fixture,
  createTaskContractFixture,
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

describe('Deterministic E2E harness bounds, shipped v2 parsers, and stateful command fixtures (no browser required)', () => {
  it('enforces CDP frame/buffer byte caps and safe JSON parsing without uncaught exceptions', () => {
    const messages: Array<Record<string, unknown>> = [];
    const errors: Error[] = [];

    const reader = createBoundedCdpFrameReader({
      maxFrameBytes: 128,
      maxBufferBytes: 256,
      onMessage(msg) {
        messages.push(msg as Record<string, unknown>);
      },
      onFatalError(err) {
        errors.push(err);
      },
    });

    reader.push('{"id":1,"result":{"ok":');
    reader.push('true}}\0{"id":2,"result":{"step":2}}\0');
    expect(errors).toEqual([]);
    expect(messages).toHaveLength(2);
    expect(messages[0]).toEqual({ id: 1, result: { ok: true } });

    // Malformed JSON triggers onFatalError cleanly and halts subsequent processing
    const malformedErrors: Error[] = [];
    const malformedReader = createBoundedCdpFrameReader({
      maxFrameBytes: 128,
      maxBufferBytes: 256,
      onMessage() {
        throw new Error('Should not deliver malformed message');
      },
      onFatalError(err) {
        malformedErrors.push(err);
      },
    });
    malformedReader.push('{not-valid-json\0{"id":3}\0');
    expect(malformedErrors).toHaveLength(1);
    expect(malformedErrors[0]?.message).toMatch(/Malformed CDP JSON frame/);

    // Oversized frame triggers onFatalError cleanly
    const overflowErrors: Error[] = [];
    const overflowReader = createBoundedCdpFrameReader({
      maxFrameBytes: 32,
      maxBufferBytes: 64,
      onMessage() {
        throw new Error('Should not deliver oversized message');
      },
      onFatalError(err) {
        overflowErrors.push(err);
      },
    });
    overflowReader.push(`{"id":1,"payload":"${'x'.repeat(40)}"}\0`);
    expect(overflowErrors).toHaveLength(1);
    expect(overflowErrors[0]?.message).toMatch(/exceeded maximum byte bound/);
  });

  it('enforces HTTP fixture server body byte cap, request record cap, and active SSE stream cap explicitly', async () => {
    const boundedServer = await startDashboardTestServer({
      skipBuildCheck: true,
      maxHttpBodyBytes: 128,
      maxRecordedRequests: 3,
      maxRetainedRequestBodyBytes: 96,
      maxActiveSseStreams: 1,
    });
    try {
      // 1. Body exceeding maxHttpBodyBytes is rejected with 413
      const oversizeBodyRes = await fetch(`${boundedServer.baseUrl}/v2/tasks`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ pad: 'a'.repeat(200) }),
      });
      expect(oversizeBodyRes.status).toBe(413);

      // 2. Active SSE stream cap is enforced with 503 and flagged by assertNoOverflow
      const sseAbort1 = new AbortController();
      const sse1 = await fetch(`${boundedServer.baseUrl}/v2/events`, {
        headers: { Accept: 'text/event-stream' },
        signal: sseAbort1.signal,
      });
      expect(sse1.status).toBe(200);
      const sse2 = await fetch(`${boundedServer.baseUrl}/v2/events`, {
        headers: { Accept: 'text/event-stream' },
      });
      expect(sse2.status).toBe(503);
      expect(() => boundedServer.assertNoOverflow()).toThrow(/exceeded maxActiveSseStreams/);
      sseAbort1.abort();
      boundedServer.clearRequests();

      // 3. Request log capacity overflow is rejected with 503 and flagged by assertNoOverflow
      for (let i = 0; i < 3; i++) {
        const okRes = await fetch(`${boundedServer.baseUrl}/v2/fleet`);
        expect(okRes.status).toBe(200);
      }
      const fourthRes = await fetch(`${boundedServer.baseUrl}/v2/fleet`);
      expect(fourthRes.status).toBe(503);
      expect(() => boundedServer.assertNoOverflow()).toThrow(/exceeded maxRecordedRequests/);
      boundedServer.clearRequests();
      expect(() => boundedServer.assertNoOverflow()).not.toThrow();
    } finally {
      await boundedServer.close();
    }
  });

  it('exercises shipped parseNativeFleet and parseDomainRecordV2 against valid and malformed v2 inputs', () => {
    const validFleet = createNativeFleetV2Fixture(4, {
      recoveryGeneration: 'gen-v2-unit',
      eventSeq: '10',
      includeTerminalLifecycleTasks: true,
    });
    const parsed = parseNativeFleet(validFleet);
    expect(parsed.schema_version).toBe(2);
    expect(parsed.native_execution_qualified).toBe(false);
    expect(parsed.tasks[0]?.state).toBe('accepted');

    expect(() =>
      parseNativeFleet({ ...validFleet, schema_version: 1 }),
    ).toThrow(/unqualified fleet envelope/);
    expect(() =>
      parseNativeFleet({ ...validFleet, native_execution_qualified: true }),
    ).toThrow(/unqualified fleet envelope/);
    expect(() =>
      parseNativeFleet({
        ...validFleet,
        tasks: [{ ...validFleet.tasks[0], state: 'self_accepted' }],
      }),
    ).toThrow(/invalid task projection/);
    expect(() =>
      parseNativeFleet({
        ...validFleet,
        tasks: [
          {
            ...validFleet.tasks[0],
            lifecycle: {
              ...validFleet.tasks[0]!.lifecycle!,
              decision: { verifier: 'worker-self' },
            },
          },
        ],
      }),
    ).toThrow(/invalid lifecycle decision/);

    const validLifecycleFrame = createLifecycleSseFrameV2('11', 'gen-v2-unit', {
      taskId: 'task-001',
      accepted: true,
    });
    const dataLine = validLifecycleFrame
      .split('\n')
      .find((line) => line.startsWith('data: '))!
      .slice('data: '.length);
    const parsedRecord = parseDomainRecordV2(JSON.parse(dataLine));
    expect(parsedRecord.seq).toBe('11');
    expect(parsedRecord.event.kind).toBe('task_lifecycle');

    expect(() =>
      parseDomainRecordV2({
        seq: '12',
        observed_utc: '2026-10-18T20:00:00Z',
        event: {
          kind: 'task_lifecycle',
          command: {
            protocol_version: 2,
            task_id: 'task-001',
            expected_task_digest: `sha256:${'a'.repeat(64)}`,
            actor: 'attacker',
            action: { action: 'forged_self_accept' },
          },
        },
      }),
    ).toThrow(/invalid or unknown domain event schema or version/);
  });

  it('enforces stateful contract-faithful POST /v2/tasks and /v2/tasks/lifecycle CAS, auth 401/403, and lost-response idempotent replay after snapshot advancement', async () => {
    const ownerToken = 'adm-test-owner-token-0123456789abcdef';
    const cmdServer = await startDashboardTestServer({
      skipBuildCheck: true,
      nativeOwnerToken: ownerToken,
      nativeAllowedProjects: ['proj-001'],
    });
    try {
      const initialContract = createTaskContractFixture({
        project_id: 'proj-001',
        task_id: 'task-cmd-qual-01',
        revision: 1,
        title: 'Initial pending task v1',
      });
      const createPayload = {
        request_id: 'd85eab6c-6bdd-4d8e-9d39-c34915dbcb10',
        contract: initialContract,
      };

      // 1. Auth 401 on missing or wrong bearer token
      const unauthRes = await fetch(`${cmdServer.baseUrl}/v2/tasks`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(createPayload),
      });
      expect(unauthRes.status).toBe(401);

      // 2. Scope 403 on foreign project outside owner grant
      const forbiddenRes = await fetch(`${cmdServer.baseUrl}/v2/tasks`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          Authorization: `Bearer ${ownerToken}`,
        },
        body: JSON.stringify({
          request_id: 'd85eab6c-6bdd-4d8e-9d39-c34915dbcb11',
          contract: createTaskContractFixture({
            project_id: 'foreign-project',
            task_id: 'task-foreign-01',
          }),
        }),
      });
      expect(forbiddenRes.status).toBe(403);

      // 3. Native create (POST /v2/tasks) never grants execution or acceptance
      const createdRes = await fetch(`${cmdServer.baseUrl}/v2/tasks`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          Authorization: `Bearer ${ownerToken}`,
        },
        body: JSON.stringify(createPayload),
      });
      expect(createdRes.status).toBe(200);
      const createdReceipt = (await createdRes.json()) as Record<string, unknown>;
      expect(createdReceipt.duplicate).toBe(false);
      expect(createdReceipt.execution_granted).toBe(false);
      expect(createdReceipt.acceptance_granted).toBe(false);

      const snapAfterCreate = cmdServer.getFleetV2Snapshot();
      const createdTask = snapAfterCreate.tasks.find(
        (t) => t.contract.task_id === 'task-cmd-qual-01',
      )!;
      expect(createdTask.state).toBe('ready');
      const rev1Digest = computeCanonicalTaskViewDigest(createdTask);

      // 4. Simulate persisted revise_contract command with lost HTTP response
      const revisedContract = createTaskContractFixture({
        project_id: 'proj-001',
        task_id: 'task-cmd-qual-01',
        revision: 2,
        title: 'Revised pending task v2',
      });
      const revisePayload = {
        request_id: '0c272170-8a01-4fbe-a58a-dc695e3b9f21',
        task_id: 'task-cmd-qual-01',
        expected_task_digest: rev1Digest,
        action: {
          action: 'revise_contract',
          contract: revisedContract,
        },
      };
      cmdServer.dropNextCommandResponse(1);
      await expect(
        fetch(`${cmdServer.baseUrl}/v2/tasks/lifecycle`, {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            Authorization: `Bearer ${ownerToken}`,
          },
          body: JSON.stringify(revisePayload),
        }).then((r) => r.json()),
      ).rejects.toThrow();

      // Verify revision=2 was persisted despite lost HTTP response
      const snapAfterLostResponse = cmdServer.getFleetV2Snapshot();
      const revisedTask = snapAfterLostResponse.tasks.find(
        (t) => t.contract.task_id === 'task-cmd-qual-01',
      )!;
      expect(revisedTask.contract.revision).toBe(2);
      expect(revisedTask.lifecycle?.revision).toBe(1);
      const rev2Digest = computeCanonicalTaskViewDigest(revisedTask);

      // 5. Stale CAS (using rev1Digest on rev 2 task with a fresh request_id) fails with 409
      const staleCasRes = await fetch(`${cmdServer.baseUrl}/v2/tasks/lifecycle`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          Authorization: `Bearer ${ownerToken}`,
        },
        body: JSON.stringify({
          request_id: '3944c4e1-679b-4db3-a18f-0e708cf0ec83',
          task_id: 'task-cmd-qual-01',
          expected_task_digest: rev1Digest,
          action: {
            action: 'cancel_pending',
            reason: 'stale CAS attempt',
          },
        }),
      });
      expect(staleCasRes.status).toBe(409);

      // 6. Advance snapshot further by cancelling the pending task with current rev2Digest
      const cancelPayload = {
        request_id: '0c272170-8a01-4fbe-a58a-dc695e3b9f22',
        task_id: 'task-cmd-qual-01',
        expected_task_digest: rev2Digest,
        action: {
          action: 'cancel_pending',
          reason: 'operator cancelled before execution',
        },
      };
      const cancelRes = await fetch(`${cmdServer.baseUrl}/v2/tasks/lifecycle`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          Authorization: `Bearer ${ownerToken}`,
        },
        body: JSON.stringify(cancelPayload),
      });
      expect(cancelRes.status).toBe(200);
      const cancelReceipt = (await cancelRes.json()) as Record<string, unknown>;
      expect(cancelReceipt.duplicate).toBe(false);
      expect(cancelReceipt.execution_granted).toBe(false);
      expect(cancelReceipt.acceptance_granted).toBe(false);

      // 7. Replay the earlier lost-response revisePayload (byte-identical UUID + payload)
      // AFTER the task has already advanced to cancelled: must return 200 duplicate:true without re-applying!
      const replayReviseRes = await fetch(`${cmdServer.baseUrl}/v2/tasks/lifecycle`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          Authorization: `Bearer ${ownerToken}`,
        },
        body: JSON.stringify(revisePayload),
      });
      expect(replayReviseRes.status).toBe(200);
      const replayReviseReceipt = (await replayReviseRes.json()) as Record<string, unknown>;
      expect(replayReviseReceipt.duplicate).toBe(true);
      expect(replayReviseReceipt.execution_granted).toBe(false);
      expect(replayReviseReceipt.acceptance_granted).toBe(false);

      // Verify task remains cancelled and was not reverted to ready by the replay
      const finalTask = cmdServer
        .getFleetV2Snapshot()
        .tasks.find((t) => t.contract.task_id === 'task-cmd-qual-01')!;
      expect(finalTask.state).toBe('cancelled');
      expect(finalTask.lifecycle?.reason).toBe('operator cancelled before execution');

      // 8. Reusing the same request_id with a mutated payload fails with 409
      const collisionRes = await fetch(`${cmdServer.baseUrl}/v2/tasks/lifecycle`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          Authorization: `Bearer ${ownerToken}`,
        },
        body: JSON.stringify({
          ...cancelPayload,
          action: { action: 'cancel_pending', reason: 'mutated reason' },
        }),
      });
      expect(collisionRes.status).toBe(409);
    } finally {
      await cmdServer.close();
    }
  });
});

describe('Admiralty release qualification browser E2E and axe-core accessibility suite', () => {
  let server: DashboardTestServer;
  let browser: BrowserHarness;

  beforeAll(async () => {
    server = await startDashboardTestServer();
    browser = await launchBrowserHarness();
  }, 60_000);

  afterAll(async () => {
    await browser?.close();
    await server?.close();
  }, 90_000);

  beforeEach(async () => {
    server.closeActiveSseStreams();
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
  }, 90_000);

  afterEach(() => {
    server.assertNoOverflow();
  });

  it(
    'serves the fresh built production dashboard bundle matching checked-out source and verifies pinned axe-core 4.14.0 and sandboxed browser version',
    async () => {
      const built = await ensureBuiltDashboard();
      expect(built.mode).toBe('production');
      expect(built.nodeEnv).toBe('production');
      expect(built.sourceDigest).toBe(computeDashboardSourceDigest());
      expect(built.bundleDigest).toBe(computeDistBundleDigest());
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

      // Verify maxStartupAttempts (finite integer in 1..2) and startupTimeoutMs validation
      for (const invalidAttempts of [0, -1, 1.5, 3, Number.POSITIVE_INFINITY, Number.NaN]) {
        await expect(launchBrowserHarness({ maxStartupAttempts: invalidAttempts })).rejects.toThrow(
          /Invalid maxStartupAttempts: expected finite integer in 1\.\.2/,
        );
      }
      for (const invalidTimeout of [0, -100, Number.POSITIVE_INFINITY, Number.NaN]) {
        await expect(launchBrowserHarness({ startupTimeoutMs: invalidTimeout })).rejects.toThrow(
          /Invalid startupTimeoutMs: expected positive finite integer/,
        );
      }

      // Verify bounded browser startup timeout recovery, process-group reaping, and profileDir cleanup
      const spawnedAttempts: Array<{
        attempt: number;
        pid: number | undefined;
        profileDir: string;
        priorCleanedBeforeSpawn?: boolean;
      }> = [];
      const recoveredBrowser = await launchBrowserHarness({
        maxStartupAttempts: 2,
        _testStallAttempts: [1],
        _testStallTimeoutMs: 150,
        _testOnAttemptSpawn(attempt, info) {
          const prior = spawnedAttempts[0];
          const priorCleanedBeforeSpawn =
            attempt === 2 && prior
              ? !existsSync(prior.profileDir) &&
                (typeof prior.pid === 'number' ? !isProcessAlive(prior.pid) : true)
              : undefined;
          spawnedAttempts.push({ attempt, ...info, priorCleanedBeforeSpawn });
        },
      });
      try {
        expect(spawnedAttempts.length).toBe(2);
        expect(spawnedAttempts[0]?.attempt).toBe(1);
        expect(spawnedAttempts[1]?.attempt).toBe(2);
        // Attempt 1's process group and profile directory must be completely cleaned up before attempt 2 spawns
        expect(spawnedAttempts[1]?.priorCleanedBeforeSpawn).toBe(true);
        expect(existsSync(spawnedAttempts[0]!.profileDir)).toBe(false);
        if (typeof spawnedAttempts[0]?.pid === 'number') {
          expect(isProcessAlive(spawnedAttempts[0].pid)).toBe(false);
        }
        expect(recoveredBrowser.browserVersion.product).toMatch(/Chrome\//);
      } finally {
        await recoveredBrowser.close();
      }
      expect(existsSync(spawnedAttempts[1]!.profileDir)).toBe(false);

      // Verify fatal-frame startup cleanup regression: when onFatalError triggers terminateBrowserProcessTree,
      // startup catch awaits the shared memoized cleanup Promise so attempt 1's process group and profileDir
      // are completely cleaned up BEFORE attempt 2 spawns
      const fatalFrameAttempts: Array<{
        attempt: number;
        pid: number | undefined;
        profileDir: string;
        priorCleanedBeforeSpawn?: boolean;
      }> = [];
      const fatalRecoveredBrowser = await launchBrowserHarness({
        maxStartupAttempts: 2,
        _testFatalFrameAttempts: [1],
        _testOnAttemptSpawn(attempt, info) {
          const prior = fatalFrameAttempts[0];
          const priorCleanedBeforeSpawn =
            attempt === 2 && prior
              ? !existsSync(prior.profileDir) &&
                (typeof prior.pid === 'number' ? !isProcessAlive(prior.pid) : true)
              : undefined;
          fatalFrameAttempts.push({ attempt, ...info, priorCleanedBeforeSpawn });
        },
      });
      try {
        expect(fatalFrameAttempts.length).toBe(2);
        expect(fatalFrameAttempts[1]?.priorCleanedBeforeSpawn).toBe(true);
        expect(existsSync(fatalFrameAttempts[0]!.profileDir)).toBe(false);
        if (typeof fatalFrameAttempts[0]?.pid === 'number') {
          expect(isProcessAlive(fatalFrameAttempts[0].pid)).toBe(false);
        }
        expect(fatalRecoveredBrowser.browserVersion.product).toMatch(/Chrome\//);
      } finally {
        await fatalRecoveredBrowser.close();
      }
      expect(existsSync(fatalFrameAttempts[1]!.profileDir)).toBe(false);
      if (typeof fatalFrameAttempts[1]?.pid === 'number') {
        expect(isProcessAlive(fatalFrameAttempts[1].pid)).toBe(false);
      }

      // Verify all-attempts-exhausted failure cleans up every attempt and retains diagnostic evidence
      const failedAttempts: Array<{ attempt: number; pid: number | undefined; profileDir: string }> = [];
      await expect(
        launchBrowserHarness({
          maxStartupAttempts: 2,
          _testStallAttempts: [1, 2],
          _testStallTimeoutMs: 150,
          _testOnAttemptSpawn(attempt, info) {
            failedAttempts.push({ attempt, ...info });
          },
        }),
      ).rejects.toThrow(
        /Browser startup failed after 2 attempts \(attempt 1\/2: [\s\S]* \|\| attempt 2\/2: [\s\S]*\)/,
      );
      expect(failedAttempts.length).toBe(2);
      for (const item of failedAttempts) {
        expect(existsSync(item.profileDir)).toBe(false);
        if (typeof item.pid === 'number') {
          expect(isProcessAlive(item.pid)).toBe(false);
        }
      }

      // Verify waitForCondition/waitForSelector enforces bounded stage timeout and throws stage-specific diagnostics
      // without hanging for CDP_COMMAND_TIMEOUT_MS (20,000ms) into the outer Vitest test deadline
      const stageStartMs = Date.now();
      await expect(browser.waitForSelector('#non-existent-stage-probe', 200)).rejects.toThrow(
        /Timed out waiting for selector "#non-existent-stage-probe" after 200ms/,
      );
      expect(Date.now() - stageStartMs).toBeLessThan(2_500);

      // Verify fillInputByLabel and selectByLabel validate timeoutMs, cap each evaluate call to
      // Math.min(2_500, remainingMs) (never 20,000ms), stop when remaining <= 0, and preserve failure/error context
      for (const invalidStageTimeout of [0, -50, 1.5, Number.POSITIVE_INFINITY, Number.NaN]) {
        await expect(
          fillInputByLabelStage(
            async <T>(): Promise<T> => ({ ok: true } as unknown as T),
            'Title',
            'val',
            invalidStageTimeout,
          ),
        ).rejects.toThrow(/Invalid fillInputByLabel timeoutMs: expected positive finite integer/);
        await expect(
          selectByLabelStage(
            async <T>(): Promise<T> => ({ ok: true } as unknown as T),
            'Select task',
            'task-001',
            invalidStageTimeout,
          ),
        ).rejects.toThrow(/Invalid selectByLabel timeoutMs: expected positive finite integer/);
      }

      const recordedInputBudgets: number[] = [];
      await expect(
        fillInputByLabelStage(
          async <T>(_expr: string, evalTimeoutMs: number): Promise<T> => {
            recordedInputBudgets.push(evalTimeoutMs);
            await new Promise((r) => setTimeout(r, 45));
            if (recordedInputBudgets.length === 1) {
              throw new Error('CDP Runtime.evaluate transient failure on poll 1');
            }
            return {
              ok: false,
              reason: 'label "Revise contract title" found but associated input/textarea is missing (for="revise-title")',
            } as unknown as T;
          },
          'Revise contract title',
          'Updated title',
          140,
        ),
      ).rejects.toThrow(
        /Input for label not found: Revise contract title after 140ms \(reason: label "Revise contract title" found but associated input\/textarea is missing \(for="revise-title"\); lastEvalError: CDP Runtime\.evaluate transient failure on poll 1\)/,
      );
      expect(recordedInputBudgets.length).toBeGreaterThanOrEqual(2);
      for (const budget of recordedInputBudgets) {
        expect(budget).toBeGreaterThan(0);
        expect(budget).toBeLessThanOrEqual(140);
        expect(budget).toBeLessThanOrEqual(MAX_STAGE_EVALUATE_BUDGET_MS);
      }
      // Verify strictly decreasing remaining budget across poll iterations
      expect(recordedInputBudgets[1]!).toBeLessThan(recordedInputBudgets[0]!);

      // Verify 5,000ms default stage timeout caps individual evaluate calls at MAX_STAGE_EVALUATE_BUDGET_MS (2,500ms)
      let firstFiveSecBudget = 0;
      await fillInputByLabelStage(
        async <T>(_expr: string, evalTimeoutMs: number): Promise<T> => {
          firstFiveSecBudget = evalTimeoutMs;
          return { ok: true } as unknown as T;
        },
        'Title',
        'Ready value',
      );
      expect(firstFiveSecBudget).toBeGreaterThan(0);
      expect(firstFiveSecBudget).toBeLessThanOrEqual(MAX_STAGE_EVALUATE_BUDGET_MS);

      const recordedSelectBudgets: number[] = [];
      await expect(
        selectByLabelStage(
          async <T>(_expr: string, evalTimeoutMs: number): Promise<T> => {
            recordedSelectBudgets.push(evalTimeoutMs);
            await new Promise((r) => setTimeout(r, 45));
            if (recordedSelectBudgets.length === 1) {
              throw new Error('Runtime.evaluate timed out after 65ms');
            }
            return {
              ok: false,
              reason: 'option "task-ui-cmd-999" not present in <select> (available options: [task-001, task-002])',
            } as unknown as T;
          },
          'Select task',
          'task-ui-cmd-999',
          140,
        ),
      ).rejects.toThrow(
        /Select for label not found: Select task \(option task-ui-cmd-999\) after 140ms \(reason: option "task-ui-cmd-999" not present in <select> \(available options: \[task-001, task-002\]\); lastEvalError: Runtime\.evaluate timed out after 65ms\)/,
      );
      expect(recordedSelectBudgets.length).toBeGreaterThanOrEqual(2);
      for (const budget of recordedSelectBudgets) {
        expect(budget).toBeGreaterThan(0);
        expect(budget).toBeLessThanOrEqual(140);
        expect(budget).toBeLessThanOrEqual(MAX_STAGE_EVALUATE_BUDGET_MS);
      }
      expect(recordedSelectBudgets[1]!).toBeLessThan(recordedSelectBudgets[0]!);

      // Verify live browser.fillInputByLabel and browser.selectByLabel enforce bounded stage timeouts and preserve DOM failure context
      const liveFillStartMs = Date.now();
      await expect(browser.fillInputByLabel('Non-existent input label probe', 'val', 160)).rejects.toThrow(
        /Input for label not found: Non-existent input label probe after 160ms \(reason: label or aria-label Non-existent input label probe not found/,
      );
      expect(Date.now() - liveFillStartMs).toBeLessThan(1_500);

      const liveSelectStartMs = Date.now();
      await expect(browser.selectByLabel('Non-existent select label probe', 'opt-missing', 160)).rejects.toThrow(
        /Select for label not found: Non-existent select label probe \(option opt-missing\) after 160ms \(reason: label Non-existent select label probe not found/,
      );
      expect(Date.now() - liveSelectStartMs).toBeLessThan(1_500);
    },
    90_000,
  );

  it(
    'reports an actual browser navigation denial before document readiness polling',
    async () => {
      // Chromium refuses this unsafe loopback port before opening a connection.
      // Its errorText is a successful CDP response, not a protocol rejection.
      await expect(browser.goto('http://127.0.0.1:1/')).rejects.toThrow(
        /Navigation to http:\/\/127\.0\.0\.1:1\/ failed after 2 attempts.*Page\.navigate failed: net::ERR_UNSAFE_PORT/,
      );
    },
    90_000,
  );

  it(
    'renders Legacy detailed view, project tabs, temporal plots, hierarchy, and scales across 4, 20, and 100 projects',
    async () => {
      await browser.goto(server.baseUrl);
      await browser.waitForText('Alpha Control');
      await browser.waitForText('Jepson Taylor');
      await browser.waitForText('Token efficiency');

      // Small icons must never compete with live data as megabyte source PNGs.
      const brandAssets = await browser.evaluate<Array<{ path: string; bytes: number }>>(`(async () => {
        const paths = [...new Set([...document.querySelectorAll('img.brand-mark, img.brand-slot-mark, img.legacy-hierarchy-grokbot')].map(image => image.getAttribute('src')))];
        return await Promise.all(paths.map(async path => ({ path, bytes: (await (await fetch(path)).arrayBuffer()).byteLength })));
      })()`);
      expect(brandAssets.length).toBeGreaterThan(0);
      for (const asset of brandAssets) {
        expect(asset.path).toMatch(/^\/assets\/.+-[A-Za-z0-9_-]{8}\.webp$/);
        expect(asset.bytes).toBeLessThan(32 * 1024);
      }
      expect(await browser.evaluate<boolean>(`performance.getEntriesByType('resource').some(entry => { const path = new URL(entry.name).pathname; return path.startsWith('/brand/') && path.endsWith('.png'); })`)).toBe(false);

      // 4 producer projects + 1 rollout project card
      const initialCardCount = await browser.evaluate<number>(
        `document.querySelectorAll('article[data-project-id]').length`,
      );
      expect(initialCardCount).toBe(5);

      // Select a single project tab and verify tab selection + filtered card view
      await browser.clickButtonByText('Alpha Control');
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
      await browser.clickButtonByText('Cost index vs all-Luna (1.0 = all Luna)');
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
    90_000,
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
    90_000,
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
    90_000,
  );

  it(
    'rejects malformed /v2/fleet responses through the shipped UI parser without trust escalation, recovers on valid /v2/fleet, and consumes real /v2/events task_lifecycle frames while rejecting malformed SSE events',
    async () => {
      const validBaseV2 = createNativeFleetV2Fixture(4, {
        recoveryGeneration: 'gen-v2-qual',
        eventSeq: '9007199254740995',
        includeTerminalLifecycleTasks: false,
      });

      // 1. Serve malformed /v2/fleet with unknown state ('self_accepted') and assert shipped UI rejects it fail-closed
      server.setFleetV2({
        ...validBaseV2,
        tasks: [{ ...validBaseV2.tasks[0], state: 'self_accepted' }],
      });
      await browser.goto(server.baseUrl);
      await browser.clickNav('Workplans');
      await browser.waitForText('Task snapshot unavailable');
      await browser.waitForText('Unsupported API response: invalid task projection');

      const footerTextOnError = await browser.evaluate<string>(
        `document.querySelector('.page-footer')?.textContent ?? ''`,
      );
      expect(footerTextOnError).toContain('Native execution not qualified');

      // 2. Serve malformed /v2/fleet with native_execution_qualified: true (untrusted escalation flag) and assert rejection
      server.setFleetV2({
        ...validBaseV2,
        native_execution_qualified: true,
      });
      await browser.clickButtonByText('Retry connection');
      await browser.waitForText('Unsupported API response: unqualified fleet envelope');

      // 3. Serve malformed /v2/fleet with incomplete lifecycle decision digests and assert rejection
      const validTerminalV2 = createNativeFleetV2Fixture(4, {
        recoveryGeneration: 'gen-v2-qual',
        eventSeq: '9007199254740996',
        includeTerminalLifecycleTasks: true,
      });
      server.setFleetV2({
        ...validTerminalV2,
        tasks: [
          {
            ...validTerminalV2.tasks[0],
            lifecycle: {
              ...validTerminalV2.tasks[0]!.lifecycle!,
              decision: { verifier: 'worker-self' },
            },
          },
        ],
      });
      await browser.clickButtonByText('Retry connection');
      await browser.waitForText('Unsupported API response: invalid lifecycle decision');

      // 4. Recover with a valid initial /v2/fleet snapshot (before terminal lifecycle transitions)
      server.setFleetV2(validBaseV2);
      await browser.clickButtonByText('Retry connection');
      await browser.waitForText('Live updates');
      await browser.waitForSelector('.workplans-panel table');

      // 5. Send a real v2 task_lifecycle SSE frame over /v2/events and assert the shipped app consumes it, resnapshots, and renders I1 evidence
      await waitForActiveSse(server);
      server.setFleetV2(validTerminalV2);
      server.sendSseFrame(
        createLifecycleSseFrameV2('9007199254740996', 'gen-v2-qual', {
          taskId: 'task-001',
          accepted: true,
        }),
      );

      const expectedEvidence = createCandidateDecisionEvidenceFixture('a');
      await browser.waitForText(`Closure: ${expectedEvidence.closure_digest}`);
      await browser.waitForText('Lifecycle reason: verification rejected');
      await browser.waitForText('Lifecycle reason: superseded by operator');

      const renderedStates = await browser.evaluate<string[]>(
        `Array.from(document.querySelectorAll('.workplans-panel tbody .state-pill')).map((el) => el.textContent?.trim() ?? '')`,
      );
      expect(renderedStates).toContain('accepted');
      expect(renderedStates).toContain('failed');
      expect(renderedStates).toContain('cancelled');

      // 6. Send a malformed v2 SSE event frame and assert the shipped app rejects it fail-closed while preserving the last valid snapshot
      server.sendSseFrame(
        `event: domain\nid: gen-v2-qual:9007199254740997\ndata: ${JSON.stringify({
          seq: '9007199254740997',
          observed_utc: '2026-10-18T20:28:00Z',
          event: {
            kind: 'task_lifecycle',
            command: {
              protocol_version: 2,
              task_id: 'task-001',
              expected_task_digest: `sha256:${'a'.repeat(64)}`,
              actor: 'attacker',
              action: { action: 'forged_self_accept' },
            },
          },
        })}\n\n`,
      );
      await browser.waitForText('Unsupported API response: invalid or unknown domain event schema or version');

      // 7. Send a cursor/generation mismatch SSE frame and verify automatic full resnapshot + reconnect on the new generation
      await waitForActiveSse(server);
      const nextGenFleet = createNativeFleetV2Fixture(4, {
        recoveryGeneration: 'gen-v2-recovered',
        eventSeq: '77',
        includeTerminalLifecycleTasks: true,
      });
      server.setFleetV2(nextGenFleet);
      server.sendSseFrame(createDomainSseFrame('9007199254740998', 'gen-mismatch-trigger'));

      const deadline = Date.now() + 8_000;
      while (Date.now() < deadline) {
        const reconnected = server.requests.some(
          (r) =>
            r.pathname === '/v2/events' &&
            r.search.includes('generation=gen-v2-recovered') &&
            r.search.includes('after=77'),
        );
        if (reconnected) break;
        await new Promise((r) => setTimeout(r, 50));
      }
      const latestV2EventsReq = [...server.requests]
        .reverse()
        .find((r) => r.pathname === '/v2/events');
      expect(latestV2EventsReq?.search).toContain('generation=gen-v2-recovered');
      expect(latestV2EventsReq?.search).toContain('after=77');
    },
    90_000,
  );

  it(
    'exercises the mounted Commands (CommandJourneys) UI and native POST /v2/tasks and /v2/tasks/lifecycle (create, revision+1 with CAS, stale CAS 409, pending cancel, auth 401/403, and byte-identical replay after lost response and advanced snapshot) against both the harness and the mandatory isolated Rust admiraltyd + SQLite Store',
    async () => {
      const ownerToken = 'adm-v2-browser-owner-token-0123456789abcdef';
      server.setRequireBearerToken(ownerToken);
      server.configureNativeCommands({ ownerToken, allowedProjects: ['proj-001'] });
      server.setFleetV2(
        createNativeFleetV2Fixture(0, {
          recoveryGeneration: 'gen-v2-cmd-browser',
          eventSeq: '100',
        }),
      );

      await browser.goto(server.baseUrl);
      await browser.clickNav('Workplans');
      await browser.waitForText('Fleet request failed (401). Update session access, then retry.');

      // Apply owner bearer token via the real Session access modal
      await browser.clickButtonByAriaLabel('Access settings');
      await browser.waitForText('Session access');
      await browser.fillInputByLabel('Bearer token', ownerToken);
      await browser.clickButtonByText('Apply token');
      await browser.waitForText('Live updates');
      await browser.waitForText('The current fleet snapshot contains no task records.');
      await waitForActiveSse(server);

      const initialContract = createTaskContractFixture({
        project_id: 'proj-001',
        task_id: 'task-browser-v2-001',
        revision: 1,
        title: 'Browser v2 intake task rev 1',
      });
      const revisedContract = createTaskContractFixture({
        project_id: 'proj-001',
        task_id: 'task-browser-v2-001',
        revision: 2,
        title: 'Browser v2 revised task rev 2',
      });
      const foreignContract = createTaskContractFixture({
        project_id: 'foreign-project',
        task_id: 'task-foreign-001',
        revision: 1,
      });

      // Step 1: Verify 401 (no token / wrong token), 403 (foreign project), and 200 (owner-scoped create) from the browser
      const step1 = await browser.evaluate<{
        unauthStatus: number;
        wrongTokenStatus: number;
        forbiddenStatus: number;
        createStatus: number;
        createReceipt: Record<string, unknown>;
      }>(`(async () => {
        const token = ${JSON.stringify(ownerToken)};
        const createBody = JSON.stringify({
          request_id: 'd85eab6c-6bdd-4d8e-9d39-c34915dbcb10',
          contract: ${JSON.stringify(initialContract)},
        });
        const unauthRes = await fetch('/v2/tasks', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: createBody,
        });
        const wrongTokenRes = await fetch('/v2/tasks', {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            Authorization: 'Bearer wrong-owner-token-01234567890123456789',
          },
          body: createBody,
        });
        const forbiddenRes = await fetch('/v2/tasks', {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            Authorization: 'Bearer ' + token,
          },
          body: JSON.stringify({
            request_id: 'd85eab6c-6bdd-4d8e-9d39-c34915dbcb11',
            contract: ${JSON.stringify(foreignContract)},
          }),
        });
        const createRes = await fetch('/v2/tasks', {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            Authorization: 'Bearer ' + token,
          },
          body: createBody,
        });
        return {
          unauthStatus: unauthRes.status,
          wrongTokenStatus: wrongTokenRes.status,
          forbiddenStatus: forbiddenRes.status,
          createStatus: createRes.status,
          createReceipt: await createRes.json(),
        };
      })()`);

      expect(step1.unauthStatus).toBe(401);
      expect(step1.wrongTokenStatus).toBe(401);
      expect(step1.forbiddenStatus).toBe(403);
      expect(step1.createStatus).toBe(200);
      expect(step1.createReceipt.duplicate).toBe(false);
      expect(step1.createReceipt.execution_granted).toBe(false);
      expect(step1.createReceipt.acceptance_granted).toBe(false);

      // Verify live SSE stream triggers UI update showing rev 1 task
      await browser.waitForText('Browser v2 intake task rev 1');
      await browser.waitForText('task-browser-v2-001 · rev 1');

      const rev1Digest = computeCanonicalTaskViewDigest({
        contract: initialContract,
        state: 'ready',
        attempt_id: null,
        owner: null,
        candidate_ref: null,
      });
      const rev2Digest = computeCanonicalTaskViewDigest({
        contract: revisedContract,
        state: 'ready',
        attempt_id: null,
        owner: null,
        candidate_ref: null,
        lifecycle: {
          revision: 1,
          attempts: {},
          closure_digest: null,
          reason: null,
          decision: null,
        },
      });

      // Step 2: Simulate lost HTTP response on revise_contract (rev 1 -> rev 2):
      // server persists rev 2 and drops the socket before replying.
      server.dropNextCommandResponse(1);
      const lostRevise = await browser.evaluate<{ dropped: boolean; status?: number; body?: string }>(`(async () => {
        try {
          const r = await fetch('/v2/tasks/lifecycle', {
            method: 'POST',
            headers: {
              'Content-Type': 'application/json',
              Authorization: 'Bearer ' + ${JSON.stringify(ownerToken)},
            },
            body: JSON.stringify({
              request_id: '0c272170-8a01-4fbe-a58a-dc695e3b9f21',
              task_id: 'task-browser-v2-001',
              expected_task_digest: ${JSON.stringify(rev1Digest)},
              action: {
                action: 'revise_contract',
                contract: ${JSON.stringify(revisedContract)},
              },
            }),
          });
          const text = await r.text();
          JSON.parse(text);
          return { dropped: false, status: r.status, body: text };
        } catch {
          return { dropped: true };
        }
      })()`);
      expect(lostRevise).toEqual({ dropped: true });

      // Verify snapshot already advanced to rev 2 via live SSE
      await browser.waitForText('Browser v2 revised task rev 2');
      await browser.waitForText('task-browser-v2-001 · rev 2');

      // Step 3: Test stale CAS 409 (new request_id with old rev1Digest), advance snapshot further via cancel_pending (using rev2Digest),
      // and then retry the byte-identical revise_contract (from Step 2) AFTER both lost response and subsequent snapshot advancement!
      const step3 = await browser.evaluate<{
        staleCasStatus: number;
        cancelStatus: number;
        cancelReceipt: Record<string, unknown>;
        replayReviseAfterCancelStatus: number;
        replayReviseAfterCancelReceipt: Record<string, unknown>;
        replayCancelStatus: number;
        replayCancelReceipt: Record<string, unknown>;
        mutatedReplayStatus: number;
      }>(`(async () => {
        const token = ${JSON.stringify(ownerToken)};
        const reviseBody = JSON.stringify({
          request_id: '0c272170-8a01-4fbe-a58a-dc695e3b9f21',
          task_id: 'task-browser-v2-001',
          expected_task_digest: ${JSON.stringify(rev1Digest)},
          action: {
            action: 'revise_contract',
            contract: ${JSON.stringify(revisedContract)},
          },
        });

        // Stale CAS using rev1Digest with a new request_id must fail 409
        const staleCasRes = await fetch('/v2/tasks/lifecycle', {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            Authorization: 'Bearer ' + token,
          },
          body: JSON.stringify({
            request_id: '3944c4e1-679b-4db3-a18f-0e708cf0ec83',
            task_id: 'task-browser-v2-001',
            expected_task_digest: ${JSON.stringify(rev1Digest)},
            action: {
              action: 'cancel_pending',
              reason: 'stale CAS must be rejected',
            },
          }),
        });

        // Advance snapshot further: cancel pending using current rev2Digest
        const cancelBody = JSON.stringify({
          request_id: '0c272170-8a01-4fbe-a58a-dc695e3b9f22',
          task_id: 'task-browser-v2-001',
          expected_task_digest: ${JSON.stringify(rev2Digest)},
          action: {
            action: 'cancel_pending',
            reason: 'operator cancelled pending task',
          },
        });
        const cancelRes = await fetch('/v2/tasks/lifecycle', {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            Authorization: 'Bearer ' + token,
          },
          body: cancelBody,
        });
        const cancelReceipt = await cancelRes.json();

        // Retry byte-identical reviseBody AFTER lost response AND AFTER snapshot advanced to cancelled!
        const replayReviseRes = await fetch('/v2/tasks/lifecycle', {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            Authorization: 'Bearer ' + token,
          },
          body: reviseBody,
        });
        const replayReviseReceipt = await replayReviseRes.json();

        // Replay byte-identical cancelBody
        const replayCancelRes = await fetch('/v2/tasks/lifecycle', {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            Authorization: 'Bearer ' + token,
          },
          body: cancelBody,
        });
        const replayCancelReceipt = await replayCancelRes.json();

        // Mutated payload with same request_id must fail 409
        const mutatedReplayRes = await fetch('/v2/tasks/lifecycle', {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            Authorization: 'Bearer ' + token,
          },
          body: JSON.stringify({
            request_id: '0c272170-8a01-4fbe-a58a-dc695e3b9f22',
            task_id: 'task-browser-v2-001',
            expected_task_digest: ${JSON.stringify(rev2Digest)},
            action: {
              action: 'cancel_pending',
              reason: 'mutated reason after commit',
            },
          }),
        });

        return {
          staleCasStatus: staleCasRes.status,
          cancelStatus: cancelRes.status,
          cancelReceipt,
          replayReviseAfterCancelStatus: replayReviseRes.status,
          replayReviseAfterCancelReceipt: replayReviseReceipt,
          replayCancelStatus: replayCancelRes.status,
          replayCancelReceipt,
          mutatedReplayStatus: mutatedReplayRes.status,
        };
      })()`);

      expect(step3.staleCasStatus).toBe(409);
      expect(step3.cancelStatus).toBe(200);
      expect(step3.cancelReceipt.duplicate).toBe(false);
      expect(step3.cancelReceipt.execution_granted).toBe(false);
      expect(step3.cancelReceipt.acceptance_granted).toBe(false);

      expect(step3.replayReviseAfterCancelStatus).toBe(200);
      expect(step3.replayReviseAfterCancelReceipt.duplicate).toBe(true);
      expect(step3.replayReviseAfterCancelReceipt.execution_granted).toBe(false);
      expect(step3.replayReviseAfterCancelReceipt.acceptance_granted).toBe(false);

      expect(step3.replayCancelStatus).toBe(200);
      expect(step3.replayCancelReceipt.duplicate).toBe(true);
      expect(step3.replayCancelReceipt.event_seq).toBe(step3.cancelReceipt.event_seq);

      expect(step3.mutatedReplayStatus).toBe(409);

      // Verify browser UI renders the final rev 2 cancelled projection in Workplans
      await browser.waitForText('Lifecycle reason: operator cancelled pending task');
      const stateBadge = await browser.evaluate<string>(
        `document.querySelector('.workplans-panel tbody .state-pill')?.textContent?.trim() ?? ''`,
      );
      expect(stateBadge).toBe('cancelled');

      // Step 4: Exercise the newly mounted Commands (CommandJourneys) UI tab in App.tsx
      await browser.clickNav('Commands');
      await browser.waitForText('Live work intake & CAS contract revision · non-dispatching admission boundary');
      await browser.clickButtonByText('Switch to JSON import');

      // 4a. Foreign project via Commands UI must surface 403
      await browser.fillInputByLabel('TaskContract JSON', JSON.stringify(foreignContract, null, 2));
      await browser.clickButtonByText('Submit pending task (POST /v2/tasks)');
      await browser.waitForText('Command rejected (draft & request_id preserved)');
      await browser.waitForText('Task submission failed (403)');

      // 4b. Create a new pending task via Commands UI Work intake
      const uiTaskContract = createTaskContractFixture({
        project_id: 'proj-001',
        task_id: 'task-ui-cmd-002',
        revision: 1,
        title: 'UI intake created task rev 1',
      });
      await browser.fillInputByLabel('TaskContract JSON', JSON.stringify(uiTaskContract, null, 2));
      await browser.clickButtonByText('Submit pending task (POST /v2/tasks)');
      await browser.waitForText('Durable operator command persisted:');

      // 4c. Switch to Task detail & evidence tab inside Commands UI and select task-ui-cmd-002
      await browser.clickButtonByText('Task detail & evidence');
      await browser.waitForText('CAS-guarded pending cancellation & +1 contract revision');
      await browser.selectByLabel('Select task', 'task-ui-cmd-002');
      await browser.waitForText('UI intake created task rev 1');

      // 4d. Exercise persisted +1 contract revision with lost HTTP response + UI retained request_id retry!
      await browser.fillInputByLabel('Revise contract title', 'UI detail revised task rev 2');
      server.dropNextCommandResponse(1);
      await browser.clickButtonByText('Submit rev 2 contract revision');
      await browser.waitForText('Retained idempotency request_id:');
      await browser.clickButtonByText('Retry retained contract revision');
      await browser.waitForText('Idempotent lifecycle replay acknowledged (duplicate: true):');

      // 4e. Attempting cancel_pending before rebinding pinned CAS digest surfaces stale CAS 409,
      // then clicking 'Rebind CAS to latest snapshot' and cancelling succeeds!
      await browser.fillInputByLabel('Pending cancellation reason', 'UI operator cancelled pending task');
      await browser.clickButtonByText('Cancel pending task (POST /v2/tasks/lifecycle)');
      await browser.waitForText('Lifecycle command rejected (draft & CAS preserved)');
      await browser.waitForText('Lifecycle command failed (409)');
      await browser.clickButtonByText('Rebind CAS to latest snapshot');
      await browser.clickButtonByText('Cancel pending task (POST /v2/tasks/lifecycle)');
      await browser.waitForText('Lifecycle transition persisted:');

      // Reset bearer token requirement for subsequent tests
      server.setRequireBearerToken(null);
      await browser.evaluate(`window.sessionStorage.removeItem('admiralty.session.bearer_token')`);

      // Step 5: Mandatory qualification against the real isolated Rust admiraltyd binary + SQLite Store
      const realServer = await startIsolatedNativeAdmiraltyServer({
        ownerToken: 'adm-real-native-owner-token-0123456789abcdef',
        allowedProjects: ['proj-001'],
      });
      try {
        await browser.goto(realServer.baseUrl);
        await browser.clickNav('Workplans');
        await browser.waitForText('Fleet request failed (401). Update session access, then retry.');

        await browser.clickButtonByAriaLabel('Access settings');
        await browser.waitForText('Session access');
        await browser.fillInputByLabel('Bearer token', realServer.ownerToken);
        await browser.clickButtonByText('Apply token');
        await browser.waitForText('Live updates');

        // Exercise mounted Commands UI against the real Rust admiraltyd + SQLite Store
        await browser.clickNav('Commands');
        await browser.waitForText('Live work intake & CAS contract revision · non-dispatching admission boundary');
        await browser.clickButtonByText('Switch to JSON import');
        await browser.fillInputByLabel('TaskContract JSON', JSON.stringify(initialContract, null, 2));
        await browser.clickButtonByText('Submit pending task (POST /v2/tasks)');
        await browser.waitForText('Durable operator command persisted:');

        // Verify revise_contract, stale CAS 409, cancel_pending, and byte-identical replay after cancel against real Rust admiraltyd
        const realCheck = await browser.evaluate<{
          reviseStatus: number;
          staleCasStatus: number;
          cancelStatus: number;
          replayReviseStatus: number;
          replayReviseDuplicate: boolean;
        }>(`(async () => {
          const token = ${JSON.stringify(realServer.ownerToken)};
          const reviseBody = JSON.stringify({
            request_id: '0c272170-8a01-4fbe-a58a-dc695e3b9f21',
            task_id: 'task-browser-v2-001',
            expected_task_digest: ${JSON.stringify(rev1Digest)},
            action: { action: 'revise_contract', contract: ${JSON.stringify(revisedContract)} },
          });
          const reviseRes = await fetch('/v2/tasks/lifecycle', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', Authorization: 'Bearer ' + token },
            body: reviseBody,
          });
          const staleCasRes = await fetch('/v2/tasks/lifecycle', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', Authorization: 'Bearer ' + token },
            body: JSON.stringify({
              request_id: '3944c4e1-679b-4db3-a18f-0e708cf0ec83',
              task_id: 'task-browser-v2-001',
              expected_task_digest: ${JSON.stringify(rev1Digest)},
              action: { action: 'cancel_pending', reason: 'stale' },
            }),
          });
          const cancelRes = await fetch('/v2/tasks/lifecycle', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', Authorization: 'Bearer ' + token },
            body: JSON.stringify({
              request_id: '0c272170-8a01-4fbe-a58a-dc695e3b9f22',
              task_id: 'task-browser-v2-001',
              expected_task_digest: ${JSON.stringify(rev2Digest)},
              action: { action: 'cancel_pending', reason: 'real store cancel' },
            }),
          });
          const replayReviseRes = await fetch('/v2/tasks/lifecycle', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', Authorization: 'Bearer ' + token },
            body: reviseBody,
          });
          const replayReviseJson = await replayReviseRes.json();
          return {
            reviseStatus: reviseRes.status,
            staleCasStatus: staleCasRes.status,
            cancelStatus: cancelRes.status,
            replayReviseStatus: replayReviseRes.status,
            replayReviseDuplicate: Boolean(replayReviseJson.duplicate),
          };
        })()`);
        expect(realCheck).toEqual({
          reviseStatus: 200,
          staleCasStatus: 409,
          cancelStatus: 200,
          replayReviseStatus: 200,
          replayReviseDuplicate: true,
        });
        await browser.clickNav('Workplans');
        await browser.waitForText('Lifecycle reason: real store cancel');
        await browser.evaluate(`window.sessionStorage.removeItem('admiralty.session.bearer_token')`);
      } finally {
        await realServer.close();
      }
    },
    90_000,
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
    90_000,
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
        ['Commands', 'Live work intake & CAS contract revision'],
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
        badContrast.style.color = '#222b36';
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
    90_000,
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

      // 2. Audit Overview, Workplans, Coordination, Commands, and Models in dark theme
      for (const [navLabel, expectedText] of [
        ['Overview', 'Admiralty Core'],
        ['Workplans', 'Admiralty Core'],
        ['Coordination', 'Message statistics'],
        ['Commands', 'Live work intake & CAS contract revision'],
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
        ['Commands', 'Live work intake & CAS contract revision'],
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
    90_000,
  );
});
