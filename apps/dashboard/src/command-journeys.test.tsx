import { describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { TaskContract } from '../../../packages/contracts/index';
import type { Fleet } from './api';
import CommandJourneys, {
  AttentionCoordinationJourney,
  IntakeJourney,
  OperatorActionsJourney,
  TaskDetailEvidenceJourney,
  buildIntakePreview,
  deriveAttentionItems,
  deriveCoordinationLedger,
  deriveTaskDetail,
  emptyIntakeDraft,
  findDuplicateCandidates,
  getCommandFleet,
  getHealthStatus,
  parseAnyFleet,
  parseDomainRecordV2,
  parseHealthStatus,
  parseModelSelectionOutcome,
  replayEventsToCursor,
  selectModelDryRun,
  type AnyDomainRecord,
  type AnyFleet,
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
        expected_task_digest: 'sha256:task-cas-digest',
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

describe('Gate 8 live work intake preview and duplicate detection', () => {
  it('validates a structured intake draft and preserves the non-dispatch guardrail', () => {
    const draft = emptyIntakeDraft('admiralty');
    draft.task_id = 'task-new-journey';
    draft.owned_paths = 'apps/dashboard/src/ui/NewPanel.tsx';
    const preview = buildIntakePreview(draft, sampleFleet().tasks);
    expect(preview.valid).toBe(true);
    expect(preview.errors).toEqual([]);
    expect(preview.cliCommand).toBe('admiralty work submit --contract workplans/task-new-journey.rev1.json');
    expect(preview.nonDispatchNotice).toContain('never dispatches workers');
  });

  it('rejects unsafe paths, overlapping owned/forbidden scopes, self-dependencies, and shell metacharacters', () => {
    const draft = emptyIntakeDraft('admiralty');
    draft.owned_paths = '../outside.ts, apps/dashboard/src/ui/CommandJourneys.tsx';
    draft.forbidden_paths = 'apps/dashboard/src/ui/CommandJourneys.tsx';
    draft.dependencies = draft.task_id;
    draft.check_program = 'sh -c';
    const preview = buildIntakePreview(draft, []);
    expect(preview.valid).toBe(false);
    expect(preview.errors.some(err => err.includes('Invalid owned path'))).toBe(true);
    expect(preview.errors.some(err => err.includes('Write scope conflict'))).toBe(true);
    expect(preview.errors.some(err => err.includes('cannot depend on itself'))).toBe(true);
    expect(preview.errors.some(err => err.includes('argv executable name'))).toBe(true);
  });

  it('detects duplicate task IDs, overlapping active owned paths, and revision supersession', () => {
    const fleet = sampleFleet();
    const staleDup = emptyIntakeDraft('admiralty');
    staleDup.task_id = 'task-alpha';
    staleDup.revision = '1';
    const stalePreview = buildIntakePreview(staleDup, fleet.tasks);
    expect(stalePreview.valid).toBe(false);
    expect(stalePreview.errors.some(err => err.includes('already exists at revision 1'))).toBe(true);

    const nextRev = emptyIntakeDraft('admiralty');
    nextRev.task_id = 'task-alpha';
    nextRev.revision = '2';
    const nextRevPreview = buildIntakePreview(nextRev, fleet.tasks);
    expect(nextRevPreview.valid).toBe(true);
    expect(nextRevPreview.warnings.some(warn => warn.includes('Supersedes existing task'))).toBe(true);

    const pathOverlap = findDuplicateCandidates(
      sampleContract({
        task_id: 'task-distinct',
        title: 'Unrelated title',
        owned_paths: ['apps/dashboard/src/ui/CommandJourneys.tsx'],
      }),
      fleet.tasks,
    );
    expect(pathOverlap).toHaveLength(1);
    expect(pathOverlap[0]).toMatchObject({
      task_id: 'task-alpha',
      reason: 'owned_path_overlap',
      overlapping_paths: ['apps/dashboard/src/ui/CommandJourneys.tsx'],
    });
  });

  it('renders the intake journey with validation status and duplicate candidates', () => {
    const html = renderToStaticMarkup(<IntakeJourney fleet={sampleFleet()} />);
    expect(html).toContain('Author or import TaskContract');
    expect(html).toContain('owned path overlap');
    expect(html).toContain('admiralty work submit --contract');
  });
});

describe('Gate 8 task detail and I1 v2 durable evidence timeline', () => {
  it('joins task contract, dependency status, verification checks, and v2 lifecycle decision evidence', () => {
    const fleet = sampleFleet();
    const detail = deriveTaskDetail(fleet.tasks[0], fleet, sampleRecords());
    expect(detail.projectName).toBe('Admiralty Control Plane');
    expect(detail.dependencies).toEqual([
      { task_id: 'task-dep', found: false, state: 'missing', satisfied: false },
    ]);
    expect(detail.criteriaWithChecks).toHaveLength(1);
    expect(detail.criteriaWithChecks[0].checks[0].program).toBe('pnpm');
    expect(detail.timeline.map(item => item.kind)).toEqual([
      'task_submitted',
      'task_claimed',
      'candidate_submitted',
      'message_queued',
      'message_queued',
      'task_lifecycle',
    ]);
    expect(detail.latestDecision).toMatchObject({
      verifier: 'verifier-service',
      receipt_digest: 'sha256:receipt-01',
      source_commit_sha: 'f7ae3c1',
    });
    expect(detail.relatedMessages).toHaveLength(2);
    expect(detail.relatedMessages[0].acknowledged).toBe(false);
    expect(detail.relatedMessages[1].acknowledged).toBe(true);
  });

  it('parses v2 fleet snapshots and replays SSE domain events up to the snapshot cursor', async () => {
    const v2Snapshot: AnyFleet = {
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
            decision: {
              verifier: 'verifier-service',
              receipt_digest: 'sha256:receipt-01',
              custody_digest: 'sha256:custody-01',
              closure_digest: 'sha256:closure-01',
              source_commit_sha: 'f7ae3c1',
              source_tree_sha: 'tree-f7ae3c1',
              verification_policy_digest: 'sha256:policy-01',
              contract_digest: 'sha256:contract-01',
            },
            reason: null,
          },
        },
      ],
    };
    expect(parseAnyFleet(v2Snapshot).schema_version).toBe(2);
    expect(parseDomainRecordV2(sampleRecords()[6]).event.kind).toBe('task_lifecycle');

    const records = [sampleRecords()[0], sampleRecords()[6]];
    const ssePayload = records
      .map(record => `event: domain\nid: gen-42:${record.seq}\ndata: ${JSON.stringify(record)}\n\n`)
      .join('');
    const fetcher = vi.fn<typeof fetch>(async (url) => {
      if (url === '/v2/fleet') {
        return Response.json(v2Snapshot);
      }
      const stream = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(new TextEncoder().encode(ssePayload));
          controller.close();
        },
      });
      return new Response(stream, { headers: { 'Content-Type': 'text/event-stream' } });
    });

    const fetched = await getCommandFleet('secret-token', undefined, fetcher);
    expect(fetched.schema_version).toBe(2);

    const replayed = await replayEventsToCursor(
      { recovery_generation: 'gen-42', event_seq: '7' },
      'secret-token',
      undefined,
      fetcher,
      200,
      '/v2/events',
    );
    expect(replayed).toHaveLength(2);
    expect(replayed[1].event.kind).toBe('task_lifecycle');
    expect(String(fetcher.mock.calls[1][0])).toBe('/v2/events?after=0&generation=gen-42');
    expect(new Headers(fetcher.mock.calls[1][1]?.headers).get('Authorization')).toBe('Bearer secret-token');
  });

  it('renders task detail, verifier decision evidence, and timeline markup', () => {
    const html = renderToStaticMarkup(
      <TaskDetailEvidenceJourney
        fleet={sampleFleet()}
        records={sampleRecords()}
        selectedTaskId="task-alpha"
      />,
    );
    expect(html).toContain('Implement command UI journeys');
    expect(html).toContain('cand-sha256-abc');
    expect(html).toContain('sha256:receipt-01');
    expect(html).toContain('Candidate accepted by verifier verifier-service');
  });
});

describe('Gate 8 attention and coordination triage', () => {
  it('derives prioritized attention items including v2 failed tasks and clears answered questions', () => {
    const fleet: AnyFleet = {
      ...sampleFleet(),
      schema_version: 2,
      tasks: [
        ...sampleFleet().tasks,
        {
          contract: sampleContract({ task_id: 'task-failed', title: 'Brokenrunner check' }),
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

  it('renders attention queue and scoped coordination ledger', () => {
    const html = renderToStaticMarkup(
      <AttentionCoordinationJourney fleet={sampleFleet()} records={sampleRecords()} />,
    );
    expect(html).toContain('Prioritized blockers, verification gates &amp; pending questions');
    expect(html).toContain('Should CommandJourneys remain standalone until captain merge?');
    expect(html).toContain('Handoff reported failed check &quot;chk-lint&quot;');
    expect(html).toContain('ACK #6');
    expect(html).toContain('Pending ACK');
  });
});

describe('Gate 8 operator actions, health check, and dry-run model selection', () => {
  it('validates /healthz and POST /v1/models/select responses', async () => {
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
    expect(
      parseModelSelectionOutcome({
        status: 'waiting',
        model_id: 'ornith',
        reason: 'setup_required',
      }),
    ).toEqual({
      status: 'waiting',
      model_id: 'ornith',
      reason: 'setup_required',
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
  });

  it('renders OperatorActionsJourney and full CommandJourneys tabs cleanly', () => {
    const actionsHtml = renderToStaticMarkup(
      <OperatorActionsJourney
        fleet={sampleFleet()}
        health={{ status: 'ok', version: '0.1.0', native_execution_qualified: false }}
      />,
    );
    expect(actionsHtml).toContain('Evaluate model selection &amp; fallback policy');
    expect(actionsHtml).toContain('ok (v0.1.0)');
    expect(actionsHtml).toContain('gen-42');

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
