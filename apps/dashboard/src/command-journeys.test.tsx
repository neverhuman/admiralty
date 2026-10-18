import { describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { CandidateDecisionEvidence, TaskContract } from '../../../packages/contracts/index';
import { ApiError, type Fleet } from './api';
import CommandJourneys, {
  AttentionCoordinationJourney,
  EventReplayError,
  IntakeJourney,
  OperatorActionsJourney,
  TaskDetailEvidenceJourney,
  buildIntakePreview,
  canCancelPendingTask,
  canReviseTaskContract,
  computeTaskViewDigest,
  contractFromTaskForRevision,
  deriveAttentionItems,
  deriveCoordinationLedger,
  deriveTaskDetail,
  draftFromTaskForRevision,
  draftToContractCandidate,
  emptyIntakeDraft,
  executeIntakeCommand,
  executeTaskDetailLifecycleCommand,
  findDuplicateCandidates,
  generateCanonicalRequestId,
  getCommandFleet,
  getHealthStatus,
  isCanonicalRequestId,
  isValidRustId,
  parseAnyFleet,
  parseCommandReceipt,
  parseDomainRecordV2,
  parseHealthStatus,
  parseModelSelectionOutcome,
  parseNativeFleet,
  replayEventsToCursor,
  selectModelDryRun,
  serializeAttemptsBTreeMap,
  serializeTaskViewForDigest,
  splitArgv,
  submitNativeLifecycle,
  submitNativeTask,
  type AnyDomainRecord,
  type AnyFleet,
  type AnyFleetTask,
  type RetainedCommandEnvelope,
} from './ui/CommandJourneys';

const sampleContract = (patch: Partial<TaskContract> = {}): TaskContract => ({
  schema_version: 1,
  project_id: 'admiralty',
  root_id: 'root-main',
  task_id: 'task-alpha',
  revision: 1,
  title: 'Implement command UI journeys',
  objective: 'Render intake, task detail, attention, and actions views from existing APIs.',
  non_goals: ['Do not enable native execution'],
  owned_paths: ['apps/dashboard/src/ui/CommandJourneys.tsx'],
  forbidden_paths: ['crates/admiralty-core/src/events.rs'],
  read_first: ['docs/plans/admiralty-v2/REMAINING_AUDIT_2026-10-18.md'],
  dependencies: ['task-dep'],
  criteria: [
    {
      id: 'crit-ui',
      text: 'Dashboard tests and build pass',
      check_ids: ['chk-test'],
    },
  ],
  checks: [
    {
      id: 'chk-test',
      program: 'pnpm',
      args: ['--dir', 'apps/dashboard', 'test', '--', '--run'],
      cwd: '.',
      timeout_secs: 120,
    },
  ],
  authority_ref: 'operator:gate-8',
  max_attempts: 2,
  simulated: false,
  ...patch,
});

const sampleFleet = (): Fleet => ({
  schema_version: 1,
  simulated: false,
  cursor: { recovery_generation: 'gen-42', event_seq: '4' },
  projects: [
    {
      id: 'admiralty',
      name: 'Admiralty Control Plane',
      total: 3,
      running: 1,
      awaiting_verification: 1,
      blocked: 1,
      accepted: 0,
    },
  ],
  tasks: [
    {
      contract: sampleContract({
        task_id: 'task-alpha',
        title: 'Implement command UI journeys',
        dependencies: ['task-dep'],
      }),
      state: 'awaiting_verification',
      attempt_id: 'att-1',
      owner: 'worker-ui',
      candidate_ref: 'cand-sha256-abc',
    },
    {
      contract: sampleContract({
        task_id: 'task-blocked',
        title: 'Wire durable scheduler recovery',
        owned_paths: ['crates/admiralty-core/src/scheduler.rs'],
        dependencies: ['task-alpha'],
      }),
      state: 'blocked',
      attempt_id: null,
      owner: null,
      candidate_ref: null,
    },
    {
      contract: sampleContract({
        task_id: 'task-ready',
        title: 'Audit offline CLI diagnostics',
        owned_paths: ['crates/admiralty-cli/src/diagnostics.rs'],
        dependencies: [],
      }),
      state: 'ready',
      attempt_id: null,
      owner: null,
      candidate_ref: null,
    },
  ],
  coordination: {
    queued_messages: 3,
    acknowledged_messages: 1,
    routine_model_turns: 0,
    measured_tokens: null,
  },
  sqlite_version: '3.51.3',
  native_execution_qualified: false,
});

const makeEvidence = (
  verifier: string,
  receiptDigest: string,
  commitSha = 'f7ae3c10123456789abcdef0123456789abcdef01',
): CandidateDecisionEvidence => ({
  verifier,
  receipt_digest: receiptDigest,
  custody_digest: `sha256:${'c'.repeat(64)}`,
  closure_digest: `sha256:${'d'.repeat(64)}`,
  source_commit_sha: commitSha,
  source_tree_sha: '0123456789abcdef0123456789abcdef01234567',
  verification_policy_digest: `sha256:${'e'.repeat(64)}`,
  contract_digest: `sha256:${'f'.repeat(64)}`,
});

const sampleRecords = (): AnyDomainRecord[] => [
  {
    seq: '1',
    observed_utc: '2026-10-18T20:00:00Z',
    event: {
      kind: 'task_submitted',
      contract: sampleContract({ task_id: 'task-alpha' }),
    },
  },
  {
    seq: '2',
    observed_utc: '2026-10-18T20:01:00Z',
    event: {
      kind: 'task_claimed',
      task_id: 'task-alpha',
      revision: 1,
      attempt_id: 'att-1',
      owner: 'worker-ui',
    },
  },
  {
    seq: '3',
    observed_utc: '2026-10-18T20:02:00Z',
    event: {
      kind: 'candidate_submitted',
      task_id: 'task-alpha',
      attempt_id: 'att-1',
      candidate_ref: 'cand-sha256-abc',
    },
  },
  {
    seq: '4',
    observed_utc: '2026-10-18T20:03:00Z',
    event: {
      kind: 'message_queued',
      message: {
        schema_version: 1,
        message_id: 'msg-q1',
        project_id: 'admiralty',
        sender: 'worker-ui',
        recipient: 'captain-admiralty',
        context_epoch: 'epoch-1',
        authority_ref: 'auth-1',
        causation_id: null,
        payload: {
          kind: 'question',
          task_id: 'task-alpha',
          requirement: 'Confirm disjoint UI file path',
          question: 'Should CommandJourneys remain standalone until captain merge?',
          options: ['Yes, standalone file only', 'Wire immediately'],
        },
      },
    },
  },
  {
    seq: '5',
    observed_utc: '2026-10-18T20:04:00Z',
    event: {
      kind: 'message_queued',
      message: {
        schema_version: 1,
        message_id: 'msg-h1',
        project_id: 'admiralty',
        sender: 'verifier-1',
        recipient: 'worker-ui',
        context_epoch: 'epoch-1',
        authority_ref: 'auth-2',
        causation_id: 'msg-q1',
        payload: {
          kind: 'handoff',
          task_id: 'task-alpha',
          candidate_ref: 'cand-sha256-abc',
          failed_check: 'chk-lint',
          next_action: 'Fix unused import in standalone component',
        },
      },
    },
  },
  {
    seq: '6',
    observed_utc: '2026-10-18T20:05:00Z',
    event: {
      kind: 'message_acknowledged',
      message_id: 'msg-h1',
      recipient: 'worker-ui',
      context_epoch: 'epoch-1',
    },
  },
  {
    seq: '7',
    observed_utc: '2026-10-18T20:06:00Z',
    event: {
      kind: 'task_lifecycle',
      command: {
        protocol_version: 2,
        task_id: 'task-alpha',
        actor: 'verifier-service',
        expected_task_digest: `sha256:${'a'.repeat(64)}`,
        action: {
          action: 'decide_candidate',
          attempt_id: 'att-1',
          candidate_ref: 'cand-sha256-abc',
          accepted: true,
          evidence: {
            verifier: 'verifier-service',
            receipt_digest: 'sha256:receipt-01',
            custody_digest: 'sha256:custody-01',
            closure_digest: 'sha256:closure-01',
            source_commit_sha: 'f7ae3c1',
            source_tree_sha: 'tree-f7ae3c1',
            verification_policy_digest: 'sha256:policy-01',
            contract_digest: 'sha256:contract-01',
          },
        },
      },
    },
  },
];

describe('Gate 8 live work intake preview, Rust admission equivalence, and duplicate coverage', () => {
  it('validates a structured intake draft, preserves ordered/duplicate check_args, and formats --database/--file CLI', () => {
    const draft = emptyIntakeDraft('admiralty');
    draft.task_id = 'task-new-journey';
    draft.owned_paths = 'apps/dashboard/src/ui/NewPanel.tsx';
    draft.check_args = '--include, a, --include, b';
    const preview = buildIntakePreview(draft, sampleFleet().tasks);
    expect(preview.valid).toBe(true);
    expect(preview.operation).toBe('create');
    expect(preview.errors).toEqual([]);
    expect(preview.contract?.checks[0].args).toEqual(['--include', 'a', '--include', 'b']);
    expect(preview.cliCommand).toBe(
      'admiralty work submit --database <admiralty.sqlite> --file workplans/task-new-journey.rev1.json',
    );
    expect(preview.cliCommand).not.toContain('--contract');
    expect(preview.nonDispatchNotice).toContain('never dispatches workers');

    // Also verify JSON string-array argv mode with repeated flags and empty string argument
    draft.check_args = '["--include", "a", "--include", "b", ""]';
    const jsonArgsCandidate = draftToContractCandidate(draft) as TaskContract;
    expect(jsonArgsCandidate.checks[0].args).toEqual(['--include', 'a', '--include', 'b', '']);
    expect(splitArgv(['--include', 'a', '--include', 'b'])).toEqual(['--include', 'a', '--include', 'b']);
  });

  it('rejects Rust-invalid IDs (:, /, ., ..), ../cwd traversal, NUL argv, duplicate criterion IDs, running-task revision, and revision+2', () => {
    for (const badId of ['', '.', '..', 'proj:1', 'a/b', '../x']) {
      expect(isValidRustId(badId)).toBe(false);
    }
    expect(isValidRustId('P00.a-1_ok')).toBe(true);

    // 1. Invalid IDs and ../cwd
    const badDraft = emptyIntakeDraft('admiralty');
    badDraft.project_id = 'bad:project';
    badDraft.root_id = '..';
    badDraft.task_id = 'bad/task';
    badDraft.owned_paths = '../outside.ts, apps/dashboard/src/ui/CommandJourneys.tsx';
    badDraft.forbidden_paths = 'apps/dashboard/src/ui/CommandJourneys.tsx';
    badDraft.check_cwd = '../cwd';
    badDraft.check_program = 'sh -c';
    const badPreview = buildIntakePreview(badDraft, []);
    expect(badPreview.valid).toBe(false);
    expect(badPreview.cliCommand).toBeNull();
    expect(badPreview.errors.some(err => err.includes('Invalid project_id'))).toBe(true);
    expect(badPreview.errors.some(err => err.includes('Invalid root_id'))).toBe(true);
    expect(badPreview.errors.some(err => err.includes('Invalid task_id'))).toBe(true);
    expect(badPreview.errors.some(err => err.includes('Invalid owned path'))).toBe(true);
    expect(badPreview.errors.some(err => err.includes('Write scope conflict'))).toBe(true);
    expect(badPreview.errors.some(err => err.includes('Invalid check cwd "../cwd"'))).toBe(true);
    expect(badPreview.errors.some(err => err.includes('argv executable name'))).toBe(true);

    // 2. NUL argument and duplicate criterion IDs in imported JSON contract
    const nulAndDupCandidate = {
      ...sampleContract({ task_id: 'task-nul-check' }),
      criteria: [
        { id: 'crit-dup', text: 'First criterion', check_ids: ['chk-test'] },
        { id: 'crit-dup', text: 'Second criterion with duplicate ID', check_ids: ['chk-test'] },
      ],
      checks: [
        {
          id: 'chk-test',
          program: 'cargo',
          args: ['test', 'bad\0arg'],
          cwd: '.',
          timeout_secs: 60,
        },
      ],
    };
    const nulPreview = buildIntakePreview(nulAndDupCandidate, []);
    expect(nulPreview.valid).toBe(false);
    expect(nulPreview.errors.some(err => err.includes('Duplicate criterion ID "crit-dup"'))).toBe(true);
    expect(nulPreview.errors.some(err => err.includes('args must not contain NUL bytes'))).toBe(true);

    // 3. Running / active-attempt task revision is rejected even at rev 2
    const fleet = sampleFleet();
    const activeRevDraft = emptyIntakeDraft('admiralty');
    activeRevDraft.task_id = 'task-alpha'; // awaiting_verification with attempt_id: 'att-1'
    activeRevDraft.revision = '2';
    activeRevDraft.authority_ref = 'operator:gate-8';
    const activeRevPreview = buildIntakePreview(activeRevDraft, fleet.tasks);
    expect(activeRevPreview.valid).toBe(false);
    expect(activeRevPreview.errors.some(err => err.includes('not eligible for contract revision'))).toBe(true);

    // 4. Revision + 2 on an unstarted ready task is rejected (must be strictly +1)
    const revPlusTwoDraft = emptyIntakeDraft('admiralty');
    revPlusTwoDraft.task_id = 'task-ready';
    revPlusTwoDraft.revision = '3';
    revPlusTwoDraft.authority_ref = 'operator:gate-8';
    const revPlusTwoPreview = buildIntakePreview(revPlusTwoDraft, fleet.tasks);
    expect(revPlusTwoPreview.valid).toBe(false);
    expect(revPlusTwoPreview.errors.some(err => err.includes('increment by exactly +1 (expected revision 2, got 3)'))).toBe(
      true,
    );

    // 5. Revision + 1 on an unstarted, unreferenced ready task with preserved identity/authority succeeds as revise_contract
    const validRevDraft = emptyIntakeDraft('admiralty');
    validRevDraft.task_id = 'task-ready';
    validRevDraft.revision = '2';
    validRevDraft.authority_ref = 'operator:gate-8';
    validRevDraft.owned_paths = 'crates/admiralty-cli/src/diagnostics.rs';
    const validRevPreview = buildIntakePreview(validRevDraft, fleet.tasks);
    expect(validRevPreview.valid).toBe(true);
    expect(validRevPreview.operation).toBe('revise_contract');
    expect(validRevPreview.expectedTaskDigest).toBe(computeTaskViewDigest(fleet.tasks[2]));
    expect(validRevPreview.cliCommand).toBeNull();
    expect(validRevPreview.warnings.some(warn => warn.includes('Supersedes existing unstarted task "task-ready"'))).toBe(
      true,
    );
  });

  it('reports partial duplicate coverage when snapshot tasks exceed 500 and still catches same_task_id beyond 500', () => {
    const manyTasks: AnyFleetTask[] = Array.from({ length: 505 }, (_, index) => ({
      contract: sampleContract({
        task_id: `task-bulk-${index + 1}`,
        title: `Unique bulk work item ${index + 1}`,
        owned_paths: [`src/module_${index + 1}.rs`],
        dependencies: [],
      }),
      state: 'ready',
      attempt_id: null,
      owner: null,
      candidate_ref: null,
    }));

    // Target task_id at index 504 (505th task, beyond the 500 heuristic cap)
    const dupBeyond500 = findDuplicateCandidates(
      sampleContract({
        task_id: 'task-bulk-505',
        revision: 1,
        title: 'Completely distinct title',
        owned_paths: ['src/brand_new.rs'],
      }),
      manyTasks,
    );
    expect(dupBeyond500.inspected).toBe(500);
    expect(dupBeyond500.total).toBe(505);
    expect(dupBeyond500.partial).toBe(true);
    expect(dupBeyond500).toHaveLength(1);
    expect(dupBeyond500[0].task_id).toBe('task-bulk-505');
    expect(dupBeyond500[0].reason).toBe('same_task_id');

    // Preview with no duplicates on >500 tasks surfaces partial duplicate scan coverage in UI
    const freshDraft = emptyIntakeDraft('admiralty');
    freshDraft.task_id = 'task-fresh-600';
    freshDraft.title = 'Distinctive xylophone telemetry';
    freshDraft.owned_paths = 'src/xylophone.rs';
    const html = renderToStaticMarkup(
      <IntakeJourney fleet={{ ...sampleFleet(), tasks: manyTasks as Fleet['tasks'] }} initialDraft={freshDraft} />,
    );
    expect(html).toContain('Partial duplicate scan');
    expect(html).toContain('500');
    expect(html).toContain('505');
    expect(html).toContain('full duplicate coverage not declared');
  });
});

describe('Gate 8 live operator HTTP commands (POST /v2/tasks and POST /v2/tasks/lifecycle)', () => {
  it('submits POST /v2/tasks with Bearer header, retains canonical UUID across lost-reply retries, and handles 401/403/409/503', async () => {
    const requestId = generateCanonicalRequestId();
    expect(isCanonicalRequestId(requestId)).toBe(true);
    const contract = sampleContract({ task_id: 'task-live-1', dependencies: [] });

    let attempt = 0;
    const seenBodies: string[] = [];
    const fetcher = vi.fn<typeof fetch>(async (url, init) => {
      expect(url).toBe('/v2/tasks');
      expect(init?.method).toBe('POST');
      expect(new Headers(init?.headers).get('Authorization')).toBe('Bearer owner-secret-token');
      expect(new Headers(init?.headers).get('Content-Type')).toBe('application/json');
      const bodyText = String(init?.body ?? '');
      seenBodies.push(bodyText);
      const parsedBody = JSON.parse(bodyText) as Record<string, unknown>;
      // Must never send actor, execution_granted, or acceptance_granted in request body
      expect(Object.keys(parsedBody).sort()).toEqual(['contract', 'request_id']);

      attempt++;
      if (attempt === 1) {
        return Response.json(
          {
            event_seq: '12',
            source_contiguous_seq: '1',
            duplicate: false,
            execution_granted: false,
            acceptance_granted: false,
          },
          { status: 201 },
        );
      }
      // Second call with the exact same request_id + contract simulates lost-reply retry -> idempotent duplicate: true
      return Response.json(
        {
          event_seq: '12',
          source_contiguous_seq: '1',
          duplicate: true,
          execution_granted: false,
          acceptance_granted: false,
        },
        { status: 200 },
      );
    });

    const firstReceipt = await submitNativeTask(
      { request_id: requestId, contract },
      'owner-secret-token',
      undefined,
      fetcher,
    );
    expect(firstReceipt).toEqual({
      event_seq: '12',
      source_contiguous_seq: '1',
      duplicate: false,
      execution_granted: false,
      acceptance_granted: false,
    });

    // Retry with identical request_id and payload
    const retryReceipt = await submitNativeTask(
      { request_id: requestId, contract },
      'owner-secret-token',
      undefined,
      fetcher,
    );
    expect(retryReceipt.duplicate).toBe(true);
    expect(seenBodies[0]).toBe(seenBodies[1]);

    // Rejects receipt that claims execution_granted: true
    expect(() =>
      parseCommandReceipt({
        event_seq: '12',
        source_contiguous_seq: '1',
        duplicate: false,
        execution_granted: true,
        acceptance_granted: false,
      }),
    ).toThrow('invalid command receipt envelope');
  });

  it('submits POST /v2/tasks/lifecycle for cancel_pending and revise_contract with TaskView CAS digest and surfaces 403/409 conflicts', async () => {
    const fleet = sampleFleet();
    const readyTask = fleet.tasks[2]; // task-ready, state: 'ready', attempt_id: null
    expect(canCancelPendingTask(readyTask)).toBe(true);
    expect(canReviseTaskContract(readyTask, fleet.tasks)).toBe(true);
    // Running/awaiting_verification task is ineligible for pending cancel or contract revision
    expect(canCancelPendingTask(fleet.tasks[0])).toBe(false);
    expect(canReviseTaskContract(fleet.tasks[0], fleet.tasks)).toBe(false);
    // task-alpha is referenced by task-blocked, so even if it were ready it could not be revised
    expect(
      canReviseTaskContract(
        { ...fleet.tasks[0], state: 'ready', attempt_id: null },
        fleet.tasks,
      ),
    ).toBe(false);

    const expectedDigest = computeTaskViewDigest(readyTask);
    expect(expectedDigest).toMatch(/^sha256:[0-9a-f]{64}$/);

    const cancelReqId = generateCanonicalRequestId();
    const reviseReqId = generateCanonicalRequestId();
    const revisedContract: TaskContract = {
      ...readyTask.contract,
      revision: 2,
      title: 'Audit offline CLI diagnostics (rev 2)',
    };

    const fetcher = vi.fn<typeof fetch>(async (url, init) => {
      expect(url).toBe('/v2/tasks/lifecycle');
      expect(init?.method).toBe('POST');
      expect(new Headers(init?.headers).get('Authorization')).toBe('Bearer owner-secret-token');
      const body = JSON.parse(String(init?.body ?? '')) as {
        request_id: string;
        task_id: string;
        expected_task_digest: string;
        action: { action: string };
      };
      expect(Object.keys(body).sort()).toEqual(['action', 'expected_task_digest', 'request_id', 'task_id']);
      if (body.expected_task_digest !== expectedDigest) {
        return Response.json({ error: 'lifecycle command was not applied' }, { status: 409 });
      }
      return Response.json({
        event_seq: '15',
        source_contiguous_seq: '1',
        duplicate: false,
        execution_granted: false,
        acceptance_granted: false,
      });
    });

    const cancelReceipt = await submitNativeLifecycle(
      {
        request_id: cancelReqId,
        task_id: readyTask.contract.task_id,
        expected_task_digest: expectedDigest,
        action: { action: 'cancel_pending', reason: 'Superseded by new architecture plan' },
      },
      'owner-secret-token',
      undefined,
      fetcher,
    );
    expect(cancelReceipt.event_seq).toBe('15');
    expect(cancelReceipt.execution_granted).toBe(false);

    const reviseReceipt = await submitNativeLifecycle(
      {
        request_id: reviseReqId,
        task_id: readyTask.contract.task_id,
        expected_task_digest: expectedDigest,
        action: { action: 'revise_contract', contract: revisedContract },
      },
      'owner-secret-token',
      undefined,
      fetcher,
    );
    expect(reviseReceipt.event_seq).toBe('15');

    // Stale CAS digest triggers 409 ApiError
    const staleDigest = `sha256:${'0'.repeat(64)}`;
    await expect(
      submitNativeLifecycle(
        {
          request_id: generateCanonicalRequestId(),
          task_id: readyTask.contract.task_id,
          expected_task_digest: staleDigest,
          action: { action: 'cancel_pending', reason: 'Stale CAS test' },
        },
        'owner-secret-token',
        undefined,
        fetcher,
      ),
    ).rejects.toThrowError(new ApiError('Lifecycle command failed (409): lifecycle command was not applied', 409));

    // Wrong bearer (403) surfaces actionable error
    const forbiddenFetcher = vi.fn<typeof fetch>(async () =>
      Response.json({ error: 'contract is outside the owner grant' }, { status: 403 }),
    );
    await expect(
      submitNativeTask(
        { request_id: generateCanonicalRequestId(), contract: revisedContract },
        'wrong-token',
        undefined,
        forbiddenFetcher,
      ),
    ).rejects.toThrowError(new ApiError('Task submission failed (403): contract is outside the owner grant', 403));
  });
});

describe('Gate 8 task detail, snapshot decision precedence, and bounded event replay', () => {
  it('prefers authoritative snapshot lifecycle.decision over older replayed historical decisions (decision B over replay decision A)', () => {
    const decisionA = makeEvidence('verifier-older-A', `sha256:${'1'.repeat(64)}`);
    const decisionB = makeEvidence('verifier-current-B', `sha256:${'2'.repeat(64)}`);

    const v2TaskWithDecisionB: AnyFleetTask = {
      ...sampleFleet().tasks[0],
      state: 'accepted',
      attempt_id: 'att-2',
      candidate_ref: 'cand-sha256-current-b',
      lifecycle: {
        revision: 2,
        attempts: { 'att-1': 'worker-ui', 'att-2': 'worker-ui' },
        closure_digest: decisionB.closure_digest,
        decision: decisionB,
        reason: null,
      },
    };
    const v2Fleet: AnyFleet = {
      ...sampleFleet(),
      schema_version: 2,
      tasks: [v2TaskWithDecisionB],
    };

    const olderReplayWithDecisionA: AnyDomainRecord[] = [
      {
        seq: '2',
        observed_utc: '2026-10-18T19:00:00Z',
        event: {
          kind: 'task_lifecycle',
          command: {
            protocol_version: 2,
            task_id: 'task-alpha',
            actor: 'verifier-older-A',
            expected_task_digest: `sha256:${'a'.repeat(64)}`,
            action: {
              action: 'decide_candidate',
              attempt_id: 'att-1',
              candidate_ref: 'cand-sha256-old-a',
              accepted: false,
              evidence: decisionA,
            },
          },
        },
      },
    ];

    const detail = deriveTaskDetail(v2TaskWithDecisionB, v2Fleet, olderReplayWithDecisionA);
    expect(detail.latestDecision).toEqual(decisionB);
    expect(detail.latestDecision?.verifier).toBe('verifier-current-B');
    expect(detail.latestDecisionSource).toBe('snapshot');

    const html = renderToStaticMarkup(
      <TaskDetailEvidenceJourney
        fleet={v2Fleet}
        records={olderReplayWithDecisionA}
        selectedTaskId="task-alpha"
      />,
    );
    expect(html).toContain('Trusted verifier decision evidence (authoritative snapshot):');
    expect(html).toContain('verifier-current-B');
    expect(html).toContain(decisionB.receipt_digest);
  });

  it('enforces maxEvents cap and targetSeq inside SSE frame callback, detects early EOF, timeout, denied replay, and already-aborted signal', async () => {
    const makeFrame = (seq: number) => {
      const record: AnyDomainRecord = {
        seq: String(seq),
        observed_utc: '2026-10-18T20:00:00Z',
        event: {
          kind: 'task_Started' in {} ? 'task_started' : 'task_started',
          task_id: 'task-alpha',
          attempt_id: 'att-1',
        },
      };
      return `event: domain\nid: gen-42:${seq}\ndata: ${JSON.stringify(record)}\n\n`;
    };

    // 1. >200 frames in a single chunk with targetSeq=250 stops at 200 inside callback and marks partial_cap
    const chunk250 = Array.from({ length: 250 }, (_, idx) => makeFrame(idx + 1)).join('');
    const fetcher250 = vi.fn<typeof fetch>(async () => {
      const stream = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(new TextEncoder().encode(chunk250));
          controller.close();
        },
      });
      return new Response(stream, { headers: { 'Content-Type': 'text/event-stream' } });
    });

    const capped = await replayEventsToCursor(
      { recovery_generation: 'gen-42', event_seq: '250' },
      'token',
      undefined,
      fetcher250,
      200,
      '/v2/events',
    );
    expect(capped).toHaveLength(200);
    expect(capped.partial).toBe(true);
    expect(capped.status).toBe('partial_cap');
    expect(capped.coveredThroughSeq).toBe('200');
    expect(capped.targetSeq).toBe('250');

    // 2. Never includes records after targetSeq even when bundled in the same chunk
    const chunk10 = Array.from({ length: 10 }, (_, idx) => makeFrame(idx + 1)).join('');
    const fetcherTarget5 = vi.fn<typeof fetch>(async () => {
      const stream = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(new TextEncoder().encode(chunk10));
          controller.close();
        },
      });
      return new Response(stream, { headers: { 'Content-Type': 'text/event-stream' } });
    });
    const exact5 = await replayEventsToCursor(
      { recovery_generation: 'gen-42', event_seq: '5' },
      'token',
      undefined,
      fetcherTarget5,
      200,
      '/v2/events',
    );
    expect(exact5).toHaveLength(5);
    expect(exact5[4].seq).toBe('5');
    expect(exact5.partial).toBe(false);
    expect(exact5.status).toBe('complete');

    // 3. Early EOF before targetSeq throws EventReplayError with partial_eof and partial records
    const chunk3 = Array.from({ length: 3 }, (_, idx) => makeFrame(idx + 1)).join('');
    const fetcherEof = vi.fn<typeof fetch>(async () => {
      const stream = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(new TextEncoder().encode(chunk3));
          controller.close();
        },
      });
      return new Response(stream, { headers: { 'Content-Type': 'text/event-stream' } });
    });
    await expect(
      replayEventsToCursor(
        { recovery_generation: 'gen-42', event_seq: '10' },
        'token',
        undefined,
        fetcherEof,
        200,
        '/v2/events',
      ),
    ).rejects.toMatchObject({
      name: 'EventReplayError',
      status: 'partial_eof',
      coveredThroughSeq: '3',
      targetSeq: '10',
    });

    // 4. Stalled stream exceeding timeoutMs rejects with EventReplayError('unavailable')
    const fetcherStalled = vi.fn<typeof fetch>(async (_url, init) => {
      const stream = new ReadableStream<Uint8Array>({
        start() {
          /* never enqueues or closes */
        },
      });
      if (init?.signal?.aborted) throw new Error('aborted');
      return new Response(stream, { headers: { 'Content-Type': 'text/event-stream' } });
    });
    await expect(
      replayEventsToCursor(
        { recovery_generation: 'gen-42', event_seq: '5' },
        'token',
        undefined,
        fetcherStalled,
        200,
        '/v2/events',
        30,
      ),
    ).rejects.toMatchObject({
      name: 'EventReplayError',
      status: 'unavailable',
    });

    // 5. Already-aborted signal throws immediately without calling fetcher
    const preAborted = new AbortController();
    preAborted.abort();
    const neverCalledFetcher = vi.fn<typeof fetch>();
    await expect(
      replayEventsToCursor(
        { recovery_generation: 'gen-42', event_seq: '5' },
        'token',
        preAborted.signal,
        neverCalledFetcher,
      ),
    ).rejects.toThrow('Event replay aborted');
    expect(neverCalledFetcher).not.toHaveBeenCalled();

    // 6. Denied replay (403) surfaces error banner in UI and does not conflate with empty history
    const unavailableHtml = renderToStaticMarkup(
      <TaskDetailEvidenceJourney
        fleet={sampleFleet()}
        records={[]}
        replayCoverage={{
          status: 'unavailable',
          coveredCount: 0,
          coveredThroughSeq: '0',
          targetSeq: '4',
          maxEvents: 200,
          detail: 'Event replay failed (403)',
        }}
        selectedTaskId="task-alpha"
      />,
    );
    expect(unavailableHtml).toContain('Event history unavailable');
    expect(unavailableHtml).toContain('Event replay failed (403)');
  });

  it('strictly validates NativeFleet and AnyFleet envelopes, rejecting null coordination, bad projects, bad lifecycle, and unsafe integers', () => {
    const validV2: AnyFleet = {
      ...sampleFleet(),
      schema_version: 2,
      tasks: [
        {
          ...sampleFleet().tasks[0],
          state: 'accepted',
          lifecycle: {
            revision: 1,
            attempts: { 'att-1': 'worker-ui' },
            closure_digest: 'sha256:closure-01',
            decision: makeEvidence('verifier-service', 'sha256:receipt-01'),
            reason: null,
          },
        },
      ],
    };
    expect(parseNativeFleet(validV2).schema_version).toBe(2);
    expect(parseAnyFleet(validV2).schema_version).toBe(2);

    // Reject coordination: null
    expect(() => parseAnyFleet({ ...validV2, coordination: null })).toThrow('expected object');
    // Reject unsafe integer in project counts
    expect(() =>
      parseNativeFleet({
        ...validV2,
        projects: [{ ...validV2.projects[0], total: Number.MAX_SAFE_INTEGER + 1 }],
      }),
    ).toThrow('invalid project total');
    // Reject malformed lifecycle (extra or missing keys, or unsafe revision)
    expect(() =>
      parseNativeFleet({
        ...validV2,
        tasks: [
          {
            ...validV2.tasks[0],
            lifecycle: { revision: 0x1_0000_0000, attempts: {}, closure_digest: null, decision: null, reason: null },
          },
        ],
      }),
    ).toThrow('invalid lifecycle revision');
    // Reject v1 fleet with lifecycle metadata attached
    expect(() =>
      parseAnyFleet({
        ...sampleFleet(),
        tasks: [{ ...sampleFleet().tasks[0], lifecycle: validV2.tasks[0].lifecycle }],
      }),
    ).toThrow('v1 task projection cannot contain lifecycle metadata');
  });
});

describe('Gate 8 attention, coordination triage, and operator actions', () => {
  it('derives prioritized attention items including v2 failed tasks and clears answered questions', () => {
    const fleet: AnyFleet = {
      ...sampleFleet(),
      schema_version: 2,
      tasks: [
        ...sampleFleet().tasks,
        {
          contract: sampleContract({ task_id: 'task-failed', title: 'Broken runner check' }),
          state: 'failed',
          attempt_id: 'att-fail',
          owner: 'worker-2',
          candidate_ref: null,
          lifecycle: {
            revision: 1,
            attempts: { 'att-fail': 'worker-2' },
            closure_digest: 'sha256:close',
            decision: null,
            reason: 'Check chk-test exited 101',
          },
        },
      ],
    };
    const records = sampleRecords();
    const items = deriveAttentionItems(fleet, records);
    expect(items.map(item => item.category)).toEqual([
      'blocked_task',
      'failed_task',
      'handoff_failed_check',
      'pending_question',
      'awaiting_verification',
      'unassigned_ready',
    ]);
    expect(items[1].detail).toContain('Check chk-test exited 101');

    const answeredRecords: AnyDomainRecord[] = [
      ...records,
      {
        seq: '8',
        observed_utc: '2026-10-18T20:07:00Z',
        event: {
          kind: 'message_queued',
          message: {
            schema_version: 1,
            message_id: 'msg-a1',
            project_id: 'admiralty',
            sender: 'captain-admiralty',
            recipient: 'worker-ui',
            context_epoch: 'epoch-1',
            authority_ref: 'auth-3',
            causation_id: 'msg-q1',
            payload: {
              kind: 'answer',
              question_id: 'msg-q1',
              answer: 'Yes, standalone file only',
              decision_ref: 'SC-DEC-095',
            },
          },
        },
      },
    ];
    const afterAnswer = deriveAttentionItems(fleet, answeredRecords);
    expect(afterAnswer.some(item => item.category === 'pending_question')).toBe(false);
    expect(deriveCoordinationLedger(answeredRecords)).toHaveLength(3);
  });

  it('validates /healthz, POST /v1/models/select, and renders all CommandJourneys views cleanly', async () => {
    expect(
      parseHealthStatus({ status: 'ok', version: '0.1.0', native_execution_qualified: false }),
    ).toEqual({
      status: 'ok',
      version: '0.1.0',
      native_execution_qualified: false,
    });
    expect(() =>
      parseHealthStatus({ status: 'ok', version: '0.1.0', native_execution_qualified: true }),
    ).toThrow('Unsupported health envelope');

    expect(
      parseModelSelectionOutcome({
        status: 'selected',
        model_id: 'ornith',
        usage_cost: { status: 'unknown' },
      }),
    ).toEqual({
      status: 'selected',
      model_id: 'ornith',
      usage_cost: { status: 'unknown' },
    });

    const fetcher = vi.fn<typeof fetch>(async (url, init) => {
      if (url === '/healthz') {
        return Response.json({ status: 'ok', version: '0.1.0', native_execution_qualified: false });
      }
      expect(url).toBe('/v1/models/select');
      expect(init?.method).toBe('POST');
      expect(new Headers(init?.headers).get('Authorization')).toBe('Bearer operator-token');
      return Response.json({
        status: 'waiting',
        model_id: 'ornith',
        reason: 'setup_required',
      });
    });

    const health = await getHealthStatus(undefined, fetcher);
    expect(health.version).toBe('0.1.0');

    const outcome = await selectModelDryRun(
      {
        required_capabilities: ['coding'],
        pinned_model: null,
        current_model: 'ornith',
        attempts_by_tier: {},
        last_failure: 'setup_error',
        allow_escalation: true,
        allow_capacity_fallback: true,
      },
      { in_use: {}, unavailable: [] },
      'operator-token',
      undefined,
      fetcher,
    );
    expect(outcome).toEqual({
      status: 'waiting',
      model_id: 'ornith',
      reason: 'setup_required',
    });

    expect(parseDomainRecordV2(sampleRecords()[6]).event.kind).toBe('task_lifecycle');
    const replayErr = new EventReplayError('EOF', 'partial_eof', [], '0', '4');
    expect(replayErr.status).toBe('partial_eof');

    const fleetFetcher = vi.fn<typeof fetch>(async () => Response.json({ ...sampleFleet(), schema_version: 2 }));
    const fetchedFleet = await getCommandFleet('operator-token', undefined, fleetFetcher);
    expect(fetchedFleet.schema_version).toBe(2);

    const attentionHtml = renderToStaticMarkup(
      <AttentionCoordinationJourney fleet={sampleFleet()} records={sampleRecords()} />,
    );
    expect(attentionHtml).toContain('Prioritized blockers, verification gates &amp; pending questions');

    const actionsHtml = renderToStaticMarkup(
      <OperatorActionsJourney
        fleet={sampleFleet()}
        health={{ status: 'ok', version: '0.1.0', native_execution_qualified: false }}
      />,
    );
    expect(actionsHtml).toContain('Evaluate model selection &amp; fallback policy');

    // Verify TaskDetailEvidenceJourney renders pending cancel + revise controls for unstarted ready task
    const readyDetailHtml = renderToStaticMarkup(
      <TaskDetailEvidenceJourney
        fleet={sampleFleet()}
        records={sampleRecords()}
        selectedTaskId="task-ready"
      />,
    );
    expect(readyDetailHtml).toContain('Cancel pending task (POST /v2/tasks/lifecycle)');
    expect(readyDetailHtml).toContain('Submit rev 2 contract revision');

    // Verify TaskDetailEvidenceJourney disables pending cancel/revise on awaiting_verification task and never shows fake accept/retry buttons
    const activeDetailHtml = renderToStaticMarkup(
      <TaskDetailEvidenceJourney
        fleet={sampleFleet()}
        records={sampleRecords()}
        selectedTaskId="task-alpha"
      />,
    );
    expect(activeDetailHtml).toContain('Pending cancellation and contract revision are disabled');
    expect(activeDetailHtml).toContain('never expose fake accept, retry, or worker launch controls');

    const rootHtml = renderToStaticMarkup(
      <CommandJourneys
        fleet={sampleFleet()}
        records={sampleRecords()}
        health={{ status: 'ok', version: '0.1.0', native_execution_qualified: false }}
        initialTab="attention"
      />,
    );
    expect(rootHtml).toContain('Work intake');
    expect(rootHtml).toContain('Task detail &amp; evidence (3)');
    expect(rootHtml).toContain('Attention &amp; coordination (5)');
    expect(rootHtml).toContain('Operator actions');
  });
});

describe('PR #71 review repairs: Rust BTreeMap TaskView golden digests, full-contract revision, and lost-reply retained retries', () => {
  it('matches all 7 Rust admiralty_core::content_digest(&task_view) golden fixtures including numeric ("10", "2") and "__proto__" attempt keys', () => {
    const goldenContract: TaskContract = {
      schema_version: 1,
      project_id: 'golden-project',
      root_id: 'golden-root',
      task_id: 'numeric-map-cas',
      revision: 1,
      title: 'TaskView CAS golden: "quoted" \\ slash / <>& café 😀',
      objective:
        'Preserve literal Unicode\u2028line separator\u2029paragraph separator; newline\n tab\t; map order and explicit null.',
      non_goals: ['Serialization fixture only; never a runtime acceptance grant'],
      owned_paths: ['src'],
      forbidden_paths: ['secrets'],
      read_first: ['README.md'],
      dependencies: [],
      criteria: [
        {
          id: 'criterion',
          text: 'Rust and browser serialize the same TaskView bytes',
          check_ids: ['check'],
        },
      ],
      checks: [
        {
          id: 'check',
          program: 'fixture-no-execution',
          args: ['--quoted="value"', '\\literal\\'],
          cwd: '.',
          timeout_secs: 1,
        },
      ],
      authority_ref: 'fixture-only:serialization',
      max_attempts: 3,
      simulated: false,
    };

    const twoAttempts = JSON.parse('{"2":"worker-two","10":"worker-ten"}') as Record<string, string>;
    expect(serializeAttemptsBTreeMap(twoAttempts)).toBe('{"10":"worker-ten","2":"worker-two"}');

    const threeAttemptsWithProto = JSON.parse(
      '{"2":"worker-two","10":"worker-ten","__proto__":"worker-prototype-key"}',
    ) as Record<string, string>;
    expect(serializeAttemptsBTreeMap(threeAttemptsWithProto)).toBe(
      '{"10":"worker-ten","2":"worker-two","__proto__":"worker-prototype-key"}',
    );

    const cases: Array<{ name: string; task: AnyFleetTask; digest: string }> = [
      {
        name: 'v1-ready-absent-lifecycle',
        task: {
          contract: goldenContract,
          state: 'ready',
          attempt_id: null,
          owner: null,
          candidate_ref: null,
        },
        digest: 'sha256:a43b9bec5de4116b9147103909597eccdbfb3b57d837a77f01ac60107b433a7c',
      },
      {
        name: 'v1-running-absent-lifecycle',
        task: {
          contract: goldenContract,
          state: 'running',
          attempt_id: '10',
          owner: 'worker-ten',
          candidate_ref: null,
        },
        digest: 'sha256:59717d5422005f3a44675c876af45b968ad6e1edc7a935f2c571d97a5e852614',
      },
      {
        name: 'native-ready-two-attempts-null-fields',
        task: {
          contract: goldenContract,
          state: 'ready',
          attempt_id: null,
          owner: null,
          candidate_ref: null,
          lifecycle: {
            revision: 5,
            attempts: twoAttempts,
            closure_digest: null,
            decision: null,
            reason: null,
          },
        },
        digest: 'sha256:431ce5ecfb4e02ef95f5a877fc974ec7eb3305a81200ab00eabf3b0c2c6cf4bf',
      },
      {
        name: 'native-failed-two-attempts',
        task: {
          contract: goldenContract,
          state: 'failed',
          attempt_id: '2',
          owner: 'worker-two',
          candidate_ref: null,
          lifecycle: {
            revision: 4,
            attempts: twoAttempts,
            closure_digest: 'sha256:2cf0d9a99ffaeb43b5d291ded1f164df04ab851ee50ff552cf111a4ba79efe0c',
            decision: null,
            reason: 'Fixture 2 closed; reason has "quotes" and café 😀',
          },
        },
        digest: 'sha256:4727b42f03ed5d3c21301f8c80229a1872d41a7b50a7b989c85b07b04bf2cc11',
      },
      {
        name: 'native-running-three-attempts-prototype-key',
        task: {
          contract: goldenContract,
          state: 'running',
          attempt_id: '__proto__',
          owner: 'worker-prototype-key',
          candidate_ref: null,
          lifecycle: {
            revision: 7,
            attempts: threeAttemptsWithProto,
            closure_digest: null,
            decision: null,
            reason: null,
          },
        },
        digest: 'sha256:310bc1a1e0de1ac8ecfb8b03eb31e3a21e669c6dfda490868b3b1d4725ac58a6',
      },
      {
        name: 'native-awaiting-three-attempts',
        task: {
          contract: goldenContract,
          state: 'awaiting_verification',
          attempt_id: '__proto__',
          owner: 'worker-prototype-key',
          candidate_ref: 'sha256:295d2c45a691be33eb201fb057dbe881b3a64282acbdfca732d56ad1b5959461',
          lifecycle: {
            revision: 8,
            attempts: threeAttemptsWithProto,
            closure_digest: null,
            decision: null,
            reason: null,
          },
        },
        digest: 'sha256:49a072d243f457579c397da929094838bba8f57cc6f7e84d13ed5e740953a390',
      },
      {
        name: 'native-accepted-complete-decision',
        task: {
          contract: goldenContract,
          state: 'accepted',
          attempt_id: '__proto__',
          owner: 'worker-prototype-key',
          candidate_ref: 'sha256:295d2c45a691be33eb201fb057dbe881b3a64282acbdfca732d56ad1b5959461',
          lifecycle: {
            revision: 9,
            attempts: threeAttemptsWithProto,
            closure_digest: 'sha256:74e957884aeea317b5cea69d553f1cc845daec2b9425c6fbd728ea5a8d88b7ff',
            decision: {
              contract_digest: 'sha256:5b75ffa79792afd78c74396be48914a01a5d4facd7bcf58be0a0a7db1732418c',
              verification_policy_digest:
                'sha256:44da8685dbd9b56ab183dad041d47a17f9d76afeab7b2a015953659ef3e06b76',
              receipt_digest: 'sha256:845af683f75482b2dcf42e62c275304937be43395d9b9b643abda8f158c0d73e',
              custody_digest: 'sha256:ea66ad93d9e3738b6c7f460df06fb154c757f773eb62f769575f8d1e32b05c6e',
              closure_digest: 'sha256:74e957884aeea317b5cea69d553f1cc845daec2b9425c6fbd728ea5a8d88b7ff',
              source_commit_sha: 'eb62f5f635bea46e98c76e91499b56b4c7673849',
              source_tree_sha: 'e1bbf5df822c9e03915654edef7f1093af88ad51',
              verifier: 'fixture-independent-verifier',
            },
            reason: null,
          },
        },
        digest: 'sha256:9875eb2c879bd993740d6745fd471738e8088518f42086b4ffbbd388baa317a8',
      },
    ];

    for (const item of cases) {
      expect(computeTaskViewDigest(item.task), item.name).toBe(item.digest);
      if (
        item.task.lifecycle &&
        Object.prototype.hasOwnProperty.call(item.task.lifecycle.attempts, '__proto__')
      ) {
        expect(serializeTaskViewForDigest(item.task)).toContain(
          '"attempts":{"10":"worker-ten","2":"worker-two","__proto__":"worker-prototype-key"}',
        );
      }
    }
  });

  it('preserves all criteria, all checks, reverse/mixed check_ids, and comma-containing values on revision roundtrip and first-check rename', () => {
    const multiTask: AnyFleetTask = {
      contract: sampleContract({
        task_id: 'task-multi-rev',
        revision: 1,
        non_goals: ['Do not split commas, such as a,b,c', 'Keep second non-goal'],
        owned_paths: ['apps/dashboard/src/ui/CommandJourneys.tsx', 'apps/dashboard/src/command-journeys.test.tsx'],
        forbidden_paths: ['crates/admiralty-core/src/events.rs', 'crates/admiralty-server/src/operator_commands.rs'],
        read_first: ['docs/plan,part1.md', 'docs/plan,part2.md'],
        dependencies: [],
        criteria: [
          {
            id: 'crit-1',
            text: 'First criterion binds ONLY second check chk-b, with commas, preserved',
            check_ids: ['chk-b'],
          },
          {
            id: 'crit-2',
            text: 'Second criterion binds mixed/duplicate order [chk-b, chk-a, chk-b]',
            check_ids: ['chk-b', 'chk-a', 'chk-b'],
          },
        ],
        checks: [
          {
            id: 'chk-a',
            program: 'pnpm',
            args: ['--filter', 'a,b', 'test'],
            cwd: 'apps/dashboard',
            timeout_secs: 90,
          },
          {
            id: 'chk-b',
            program: 'cargo',
            args: ['test', '--', '--exact', 'case_a,case_b'],
            cwd: '.',
            timeout_secs: 180,
          },
        ],
      }),
      state: 'ready',
      attempt_id: null,
      owner: null,
      candidate_ref: null,
    };

    // 1. No-edit roundtrip (plus title edit) preserves criteria[0].check_ids = ['chk-b'] without rewriting to ['chk-a']
    const revDraft = draftFromTaskForRevision(multiTask);
    expect(revDraft.revision).toBe('2');
    const noEditCandidate = draftToContractCandidate(revDraft) as TaskContract;
    expect(noEditCandidate.criteria[0].check_ids).toEqual(['chk-b']);
    expect(noEditCandidate.criteria[1].check_ids).toEqual(['chk-b', 'chk-a', 'chk-b']);

    revDraft.title = 'Revised multi-check contract title';
    const candidate = draftToContractCandidate(revDraft) as TaskContract;
    expect(candidate.revision).toBe(2);
    expect(candidate.title).toBe('Revised multi-check contract title');
    expect(candidate.non_goals).toEqual(['Do not split commas, such as a,b,c', 'Keep second non-goal']);
    expect(candidate.read_first).toEqual(['docs/plan,part1.md', 'docs/plan,part2.md']);
    expect(candidate.criteria).toEqual(multiTask.contract.criteria);
    expect(candidate.checks).toEqual(multiTask.contract.checks);
    expect(contractFromTaskForRevision(multiTask, { title: 'Revised multi-check contract title' })).toEqual(candidate);

    const preview = buildIntakePreview(revDraft, [multiTask]);
    expect(preview.valid).toBe(true);
    expect(preview.operation).toBe('revise_contract');
    expect(preview.contract?.criteria).toEqual(multiTask.contract.criteria);
    expect(preview.contract?.checks).toEqual(multiTask.contract.checks);
    expect(preview.contract?.non_goals).toEqual(multiTask.contract.non_goals);

    // 2. Renaming the first check ID ('chk-a' -> 'chk-a-renamed') replaces only exact 'chk-a' occurrences across all criteria
    const renamedCheckDraft = draftFromTaskForRevision(multiTask);
    renamedCheckDraft.check_id = 'chk-a-renamed';
    const renamedPreview = buildIntakePreview(renamedCheckDraft, [multiTask]);
    expect(renamedPreview.valid).toBe(true);
    expect(renamedPreview.contract?.checks.map(c => c.id)).toEqual(['chk-a-renamed', 'chk-b']);
    expect(renamedPreview.contract?.criteria[0].check_ids).toEqual(['chk-b']);
    expect(renamedPreview.contract?.criteria[1].check_ids).toEqual(['chk-b', 'chk-a-renamed', 'chk-b']);
  });

  it('retries retained create, cancel_pending, and revise_contract envelopes with unchanged UUID and body after server commit + lost reply + snapshot advancement', async () => {
    // 1. CREATE: attempt 1 persists on server then throws network error; snapshot advances to include created task; retry sends exact same body & UUID
    const createDraft = emptyIntakeDraft('admiralty');
    createDraft.task_id = 'task-lost-create';
    const initialCreatePreview = buildIntakePreview(createDraft, []);
    expect(initialCreatePreview.valid).toBe(true);

    const createRetainedRef: { current: RetainedCommandEnvelope | null } = { current: null };
    const createBodies: string[] = [];
    let createAttempt = 0;
    const createFetcher = vi.fn<typeof fetch>(async (_url, init) => {
      createBodies.push(String(init?.body ?? ''));
      createAttempt++;
      if (createAttempt === 1) {
        throw new Error('Network lost after server persisted task-lost-create');
      }
      return Response.json({
        event_seq: '21',
        source_contiguous_seq: '1',
        duplicate: true,
        execution_granted: false,
        acceptance_granted: false,
      });
    });

    await expect(
      executeIntakeCommand({
        retainedRef: createRetainedRef,
        preview: initialCreatePreview,
        effectiveCasDigest: null,
        token: 'owner-token',
        fetcher: createFetcher,
      }),
    ).rejects.toThrow('Network lost after server persisted task-lost-create');
    expect(createRetainedRef.current?.kind).toBe('create');
    const firstCreateRequestId = createRetainedRef.current!.request_id;

    // Snapshot advances: task-lost-create now exists at revision 1, making new-command preview invalid!
    const advancedCreateFleetTasks: AnyFleetTask[] = [
      {
        contract: initialCreatePreview.contract!,
        state: 'ready',
        attempt_id: null,
        owner: null,
        candidate_ref: null,
      },
    ];
    const advancedCreatePreview = buildIntakePreview(createDraft, advancedCreateFleetTasks);
    expect(advancedCreatePreview.valid).toBe(false);

    // IntakeJourney with retained envelope keeps retry button enabled despite !advancedCreatePreview.valid
    const createRetryHtml = renderToStaticMarkup(
      <IntakeJourney
        fleet={{ ...sampleFleet(), tasks: advancedCreateFleetTasks as Fleet['tasks'] }}
        initialDraft={createDraft}
        initialRetainedEnvelope={createRetainedRef.current}
      />,
    );
    expect(createRetryHtml).toContain('Retry retained task submission');
    expect(createRetryHtml).not.toContain('disabled=""');

    const createRetryReceipt = await executeIntakeCommand({
      retainedRef: createRetainedRef,
      preview: advancedCreatePreview,
      effectiveCasDigest: null,
      token: 'owner-token',
      fetcher: createFetcher,
    });
    expect(createRetryReceipt.duplicate).toBe(true);
    expect(createBodies).toHaveLength(2);
    expect(createBodies[1]).toBe(createBodies[0]);
    expect(JSON.parse(createBodies[1]).request_id).toBe(firstCreateRequestId);
    expect(createRetainedRef.current).toBeNull();

    // 2. CANCEL_PENDING: attempt 1 persists cancellation then loses reply; snapshot advances to state='cancelled'; retry sends exact retained body
    const baseFleet = sampleFleet();
    const readyTask = baseFleet.tasks[2]; // task-ready
    const initialDetail = deriveTaskDetail(readyTask, baseFleet, []);
    expect(initialDetail.canCancelPending).toBe(true);

    const cancelRetainedRef: { current: RetainedCommandEnvelope | null } = { current: null };
    const cancelBodies: string[] = [];
    let cancelAttempt = 0;
    const cancelFetcher = vi.fn<typeof fetch>(async (_url, init) => {
      cancelBodies.push(String(init?.body ?? ''));
      cancelAttempt++;
      if (cancelAttempt === 1) {
        throw new Error('Network lost after server persisted cancel_pending');
      }
      return Response.json({
        event_seq: '22',
        source_contiguous_seq: '1',
        duplicate: true,
        execution_granted: false,
        acceptance_granted: false,
      });
    });

    await expect(
      executeTaskDetailLifecycleCommand({
        kind: 'cancel_pending',
        retainedRef: cancelRetainedRef,
        detail: initialDetail,
        boundDigest: initialDetail.taskDigest,
        cancelReason: 'Cancel before worker claim',
        token: 'owner-token',
        fetcher: cancelFetcher,
      }),
    ).rejects.toThrow('Network lost after server persisted cancel_pending');
    expect(cancelRetainedRef.current?.kind).toBe('cancel_pending');

    // Snapshot advances: task-ready is now state='cancelled' (canCancelPending === false, new taskDigest)
    const cancelledTask: AnyFleetTask = { ...readyTask, state: 'cancelled' };
    const cancelledFleet: AnyFleet = { ...baseFleet, tasks: [baseFleet.tasks[0], baseFleet.tasks[1], cancelledTask] };
    const advancedCancelDetail = deriveTaskDetail(cancelledTask, cancelledFleet, []);
    expect(advancedCancelDetail.canCancelPending).toBe(false);
    expect(advancedCancelDetail.taskDigest).not.toBe(initialDetail.taskDigest);

    const cancelRetryHtml = renderToStaticMarkup(
      <TaskDetailEvidenceJourney
        fleet={cancelledFleet}
        records={[]}
        selectedTaskId="task-ready"
        initialRetainedEnvelope={cancelRetainedRef.current}
      />,
    );
    expect(cancelRetryHtml).toContain('Retry retained pending cancellation');

    const cancelRetryReceipt = await executeTaskDetailLifecycleCommand({
      kind: 'cancel_pending',
      retainedRef: cancelRetainedRef,
      detail: advancedCancelDetail,
      boundDigest: advancedCancelDetail.taskDigest,
      cancelReason: 'Cancel before worker claim',
      token: 'owner-token',
      fetcher: cancelFetcher,
    });
    expect(cancelRetryReceipt.duplicate).toBe(true);
    expect(cancelBodies).toHaveLength(2);
    expect(cancelBodies[1]).toBe(cancelBodies[0]);
    expect(cancelRetainedRef.current).toBeNull();

    // 3. REVISE_CONTRACT: attempt 1 persists rev 2 then loses reply; snapshot advances to rev 2 (and running); retry sends exact retained rev 2 envelope
    const reviseRetainedRef: { current: RetainedCommandEnvelope | null } = { current: null };
    const reviseBodies: string[] = [];
    let reviseAttempt = 0;
    const reviseFetcher = vi.fn<typeof fetch>(async (_url, init) => {
      reviseBodies.push(String(init?.body ?? ''));
      reviseAttempt++;
      if (reviseAttempt === 1) {
        throw new Error('Network lost after server persisted revise_contract');
      }
      return Response.json({
        event_seq: '23',
        source_contiguous_seq: '1',
        duplicate: true,
        execution_granted: false,
        acceptance_granted: false,
      });
    });

    await expect(
      executeTaskDetailLifecycleCommand({
        kind: 'revise_contract',
        retainedRef: reviseRetainedRef,
        detail: initialDetail,
        boundDigest: initialDetail.taskDigest,
        reviseTitle: 'Revised title rev 2',
        reviseObjective: 'Revised objective rev 2',
        token: 'owner-token',
        fetcher: reviseFetcher,
      }),
    ).rejects.toThrow('Network lost after server persisted revise_contract');
    expect(reviseRetainedRef.current?.kind).toBe('revise_contract');

    // Snapshot advances: task-ready is now at revision 2 and claimed/running (canReviseContract === false, new digest)
    const rev2RunningTask: AnyFleetTask = {
      ...readyTask,
      contract: {
        ...readyTask.contract,
        revision: 2,
        title: 'Revised title rev 2',
        objective: 'Revised objective rev 2',
      },
      state: 'running',
      attempt_id: 'att-rev2',
      owner: 'worker-1',
    };
    const rev2Fleet: AnyFleet = { ...baseFleet, tasks: [baseFleet.tasks[0], baseFleet.tasks[1], rev2RunningTask] };
    const advancedReviseDetail = deriveTaskDetail(rev2RunningTask, rev2Fleet, []);
    expect(advancedReviseDetail.canReviseContract).toBe(false);

    const reviseRetryReceipt = await executeTaskDetailLifecycleCommand({
      kind: 'revise_contract',
      retainedRef: reviseRetainedRef,
      detail: advancedReviseDetail,
      boundDigest: advancedReviseDetail.taskDigest,
      reviseTitle: 'Revised title rev 2',
      reviseObjective: 'Revised objective rev 2',
      token: 'owner-token',
      fetcher: reviseFetcher,
      onRefreshSnapshot: () => {
        throw new Error('Transient post-commit snapshot refresh failure');
      },
    });
    expect(reviseRetryReceipt.duplicate).toBe(true);
    expect(reviseBodies).toHaveLength(2);
    expect(reviseBodies[1]).toBe(reviseBodies[0]);
    expect(JSON.parse(reviseBodies[1]).action.contract.revision).toBe(2);
    expect(JSON.parse(reviseBodies[1]).expected_task_digest).toBe(initialDetail.taskDigest);
    expect(reviseRetainedRef.current).toBeNull();

    // 4. Post-receipt onRefreshSnapshot throwing in executeIntakeCommand also resolves receipt cleanly
    const freshRetainedRef: { current: RetainedCommandEnvelope | null } = { current: null };
    const refreshThrowReceipt = await executeIntakeCommand({
      retainedRef: freshRetainedRef,
      preview: initialCreatePreview,
      effectiveCasDigest: null,
      token: 'owner-token',
      fetcher: createFetcher,
      onRefreshSnapshot: async () => {
        throw new Error('Post-commit fleet refresh error');
      },
    });
    expect(refreshThrowReceipt.event_seq).toBe('21');
    expect(freshRetainedRef.current).toBeNull();
  });
});


