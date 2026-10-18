export type TaskStage = 'ready' | 'blocked' | 'claimed' | 'running' | 'awaiting_verification';

export interface FleetFixture {
  schema_version: 1;
  simulated: boolean;
  cursor: { recovery_generation: string; event_seq: string };
  projects: Array<{
    id: string;
    name: string;
    total: number;
    running: number;
    awaiting_verification: number;
    blocked: number;
    accepted: number;
  }>;
  tasks: Array<{
    contract: {
      schema_version: number;
      project_id: string;
      root_id: string;
      task_id: string;
      revision: number;
      title: string;
      objective: string;
      non_goals: string[];
      owned_paths: string[];
      forbidden_paths: string[];
      read_first: string[];
      dependencies: string[];
      criteria: Array<{ id: string; text: string; check_ids: string[] }>;
      checks: Array<{ id: string; program: string; args: string[]; cwd: string; timeout_secs: number }>;
      authority_ref: string;
      max_attempts: number;
      simulated: boolean;
    };
    state: TaskStage;
    attempt_id: string | null;
    owner: string | null;
    candidate_ref: string | null;
  }>;
  coordination: {
    queued_messages: number;
    acknowledged_messages: number;
    routine_model_turns: 0;
    measured_tokens: string | null;
  };
  sqlite_version: string;
  native_execution_qualified: false;
}

export interface ModelsFixture {
  schema_version: 1;
  revision: string;
  native_execution_qualified: false;
  models: Array<{
    id: string;
    label: string;
    provider: 'openai_compatible' | 'opencode_ssh' | 'codex_cli';
    provider_model: string;
    tier: number;
    enabled: boolean;
    node_id: string | null;
    max_concurrency: number;
    capabilities: string[];
    task_max_attempts: number;
    escalate_to: string | null;
    usage_cost: { status: 'unknown' };
  }>;
}

const STATES: TaskStage[] = [
  'ready',
  'running',
  'claimed',
  'awaiting_verification',
  'blocked',
];

export function createFleetFixture(
  projectCount = 4,
  options: {
    simulated?: boolean;
    recoveryGeneration?: string;
    eventSeq?: string;
    queuedMessages?: number;
    acknowledgedMessages?: number;
    measuredTokens?: string | null;
  } = {},
): FleetFixture {
  const projects: FleetFixture['projects'] = [];
  const tasks: FleetFixture['tasks'] = [];

  for (let i = 0; i < projectCount; i++) {
    const id = `proj-${String(i + 1).padStart(3, '0')}`;
    const name =
      i === 0
        ? 'Admiralty Core'
        : i === 1
          ? 'Store & Replay'
          : i === 2
            ? 'Runtime Supervisor'
            : `Project ${i + 1}`;
    const accepted = (i % 4) + 1;
    const running = (i % 3) + 1;
    const awaitingVerification = i % 2;
    const blocked = i % 3 === 0 ? 1 : 0;
    const total = accepted + running + awaitingVerification + blocked + 1;
    projects.push({
      id,
      name,
      total,
      running,
      awaiting_verification: awaitingVerification,
      blocked,
      accepted,
    });

    const state = STATES[i % STATES.length];
    const taskId = `task-${String(i + 1).padStart(3, '0')}`;
    tasks.push({
      contract: {
        schema_version: 1,
        project_id: id,
        root_id: `root-${id}`,
        task_id: taskId,
        revision: 1,
        title: `Qualify ${name} slice ${i + 1}`,
        objective: `Execute release qualification checks for ${name}`,
        non_goals: ['No unsafe host mutation'],
        owned_paths: [`crates/${id}/src/lib.rs`],
        forbidden_paths: ['schemas/*'],
        read_first: ['docs/plans/admiralty-v2/MASTER_PLAN.md'],
        dependencies: [],
        criteria: [
          {
            id: `crit-${i + 1}`,
            text: 'All deterministic qualification checks pass',
            check_ids: [`chk-${i + 1}`],
          },
        ],
        checks: [
          {
            id: `chk-${i + 1}`,
            program: 'cargo',
            args: ['test', '--locked'],
            cwd: '.',
            timeout_secs: 120,
          },
        ],
        authority_ref: 'authority:jepson-ruling-2026-10-18',
        max_attempts: 2,
        simulated: options.simulated ?? false,
      },
      state,
      attempt_id: state === 'ready' ? null : `attempt-${i + 1}`,
      owner: state === 'ready' ? null : `worker-${(i % 8) + 1}`,
      candidate_ref: state === 'awaiting_verification' ? `cand-${i + 1}` : null,
    });
  }

  return {
    schema_version: 1,
    simulated: options.simulated ?? false,
    cursor: {
      recovery_generation: options.recoveryGeneration ?? 'gen-qual-1',
      event_seq: options.eventSeq ?? '9007199254740993',
    },
    projects,
    tasks,
    coordination: {
      queued_messages: options.queuedMessages ?? 7,
      acknowledged_messages: options.acknowledgedMessages ?? 19,
      routine_model_turns: 0,
      measured_tokens: options.measuredTokens === undefined ? null : options.measuredTokens,
    },
    sqlite_version: '3.51.3',
    native_execution_qualified: false,
  };
}

export function createLegacyFixture(projectCount = 4): Record<string, unknown> {
  const order: string[] = [];
  const projects: Record<string, unknown> = {};
  const projHistory: Record<string, unknown> = {};
  const hygieneHistory: Record<string, unknown> = {};

  for (let i = 0; i < projectCount; i++) {
    const id = `legacy-proj-${String(i + 1).padStart(3, '0')}`;
    const name =
      i === 0
        ? 'Alpha Control'
        : i === 1
          ? 'Beta Storage'
          : i === 2
            ? 'Gamma Runtime'
            : `Fleet Project ${i + 1}`;
    order.push(id);
    const done = (i % 5) + 2;
    const inPr = (i % 3) + 1;
    const inProgress = (i % 4) + 1;
    const blocked = i % 2;
    const total = done + inPr + inProgress + blocked;
    if (i < 4) {
      projects[id] = {
        name,
        total,
        ledger_source: 'producer ledger',
        stages: { done, in_pr: inPr, in_progress: inProgress, blocked },
        stage_labels: { done: 'accepted on main' },
        flags: i === 0 ? ['Independent audit required'] : [],
        recent: [
          {
            task: `P${String(i + 1).padStart(2, '0')}`,
            old: 'in_progress',
            new: 'in_pr',
            at: '2026-10-18T20:00:00Z',
            source: 'ledger commit',
          },
        ],
        captain: {
          name: `captain_${id}`,
          model: 'Gemini 4 Argon',
          agent_running: true,
          tail: [`Commodore ${id} observing worker progress`],
        },
        workers: [
          {
            name: `worker_${id}_1`,
            model: 'gpt-6-luna',
            effort: 'high',
            tier: 'T0',
            host: 'xbabe2',
            task: `P${String(i + 1).padStart(2, '0')}`,
            agent_running: true,
            tail: ['Running qualification checks'],
          },
        ],
        pr: {
          number: 100 + i,
          url: `https://example.com/admiralty/pull/${100 + i}`,
          state: 'OPEN',
          head: `sha-${i + 1}`,
          branch: `feat/${id}`,
          ci: { state: i % 2 === 0 ? 'success' : 'pending' },
          exact_head_approved: i % 2 === 0,
        },
      };
    } else {
      projects[id] = {
        name,
        total,
        ledger_source: 'producer ledger',
        stages: { done, in_pr: inPr, in_progress: inProgress, blocked },
      };
    }
    projHistory[id] = { done, total, ip: inProgress, ci: i % 2 === 0 ? 'success' : 'pending' };
    hygieneHistory[id] = 92 - (i % 10);
  }

  return {
    generated_utc: '2026-10-18T20:25:00Z',
    host: 'xbabe2',
    order,
    projects,
    rollout: {
      project: { id: 'admiralty-v2', name: 'Admiralty' },
      producer: 'rollout producer',
      updated_utc: '2026-10-18T20:25:00Z',
      total: 32,
      counts: { review: 1, in_progress: 31 },
      packets: [
        { id: 'P12', title: 'React parity and browser E2E', status: 'in_progress' },
        { id: 'P29', title: 'Operator UX and accessibility', status: 'in_progress' },
      ],
      lanes: [],
      source: { sha256: '7bb2784dc740faa3c07dd8c0ad72769dfa344927' },
      freshness: { state: 'fresh' },
    },
    hierarchy: {
      jepson: 'Jepson Taylor',
      grokbot: { name: 'Grokbot', routines: ['Verify receipts', 'Audit CI'] },
      super_captain: {
        screen: '4060655.super_captain',
        model: 'Gemini 4 Argon',
        state: 'active',
        ticker_alive: true,
      },
      captains: order.slice(0, 6).map((id) => ({
        key: id,
        name: (projects[id] as { name: string }).name,
        screen: `captain_${id}`,
        model: 'Gemini 4 Argon',
        state: 'active',
        done: 2,
        total: 6,
      })),
    },
    super_captain: {
      log_tail: ['Hard-switch cutover coordination active'],
      last_check: { no: 42, utc: '2026-10-18T20:24:00Z' },
    },
    token_efficiency: {
      agents_total: projectCount,
      codex_on_cheapest: projectCount - 1,
      codex_agents: projectCount,
      pct_codex_on_cheapest: 75,
      cost_index_vs_all_luna: 1.4,
      cost_index_avg_output_rate: 12.5,
      quota: {
        used_percent: 38,
        window_minutes: 10080,
        burn_pct_per_hour: 1.25,
        burn_window_min: 60,
        hours_to_100pct: 49.6,
        observed_utc: '2026-10-18T20:24:00Z',
        resets_at_utc: '2026-10-25T00:00:00Z',
        credits_balance: '1420',
      },
      by_model: {
        'gpt-6-luna high': Math.max(1, projectCount - 1),
        'Gemini 4 Argon': 1,
      },
      windows: {
        '60': {
          credits_per_hour_est: 18.4,
          output_tokens: 142000,
          input_tokens: 980000,
          by_model: {
            'gpt-6-luna': { sessions: 4, input: 980000, cached: 820000, output: 142000, credits: 18 },
          },
        },
        '15': {
          credits_per_hour_est: 16.0,
          by_model: {},
        },
      },
      ledger: {
        model_lines: 18,
        escalated: 2,
        lane_lines_with_class: 16,
        outcomes: { pass: 15, escalated: 2 },
        class_at_tier: { T0: 16, T1: 2 },
      },
      ledger_stats: {
        attempts: 18,
        luna_failures_total: 2,
        luna_first_pass_rate: 88.9,
        luna_failures_per_hour_24h: [0, 1, 0, 0, 1, 0],
        funnel: { luna_tasks: 16, luna_to_sol: 2, sol_to_astra: 0 },
      },
    },
    token_yield: {
      window_hours: 24,
      generated_utc: '2026-10-18T20:24:00Z',
      method: 'exact-commit-attribution',
      total: {
        tokens: 1122000,
        integration_tokens: 960000,
        main_tokens: 890000,
        yield_main_pct: 79.3,
        yield_integration_pct: 85.6,
        yield_main_credits_pct: 80.1,
        yield_integration_credits_pct: 86.2,
        coverage_pct: 94.0,
      },
      projects: {
        [order[0]]: {
          tokens: 520000,
          credits: 9,
          yield_integration_credits_pct: 88.0,
          yield_main_credits_pct: 82.0,
          coverage_pct: 96.0,
        },
      },
    },
    global_resources: {
      hosts: [
        { host: 'xbabe1', load: [2.1, 1.9, 1.8] },
        { host: 'xbabe2', load: [3.4, 3.1, 2.9] },
      ],
    },
    history: {
      note: 'Two-minute producer history window',
      rows: [
        {
          ts: 1792354800,
          cost_index: 1.35,
          quota_pct: 36,
          credits_h: 17.2,
          luna_fp_rate: 87.5,
          proj: projHistory,
          hosts: { xbabe1: { load: 24, disk: 41 }, xbabe2: { load: 35, disk: 48 } },
          tiers: { luna: Math.max(1, projectCount - 1), agy: 1 },
          hyg: hygieneHistory,
          ty: { tok: 980000, int: 830000, main: 760000, ym: 77.5, yi: 84.7, cov: 93.0 },
        },
        {
          ts: 1792354920,
          cost_index: 1.4,
          quota_pct: 38,
          credits_h: 18.4,
          luna_fp_rate: 88.9,
          proj: projHistory,
          hosts: { xbabe1: { load: 26, disk: 41 }, xbabe2: { load: 38, disk: 48 } },
          tiers: { luna: Math.max(1, projectCount - 1), agy: 1 },
          hyg: hygieneHistory,
          ty: { tok: 1122000, int: 960000, main: 890000, ym: 79.3, yi: 85.6, cov: 94.0 },
        },
      ],
    },
  };
}

export function createModelsFixture(revision = '1'): ModelsFixture {
  return {
    schema_version: 1,
    revision,
    native_execution_qualified: false,
    models: [
      {
        id: 'ornith-local-t0',
        label: 'Ornith Local Coding',
        provider: 'opencode_ssh',
        provider_model: 'ornith-coder-32b',
        tier: 0,
        enabled: true,
        node_id: 'xbabe4',
        max_concurrency: 8,
        capabilities: ['coding', 'review'],
        task_max_attempts: 2,
        escalate_to: 'codex-luna-t1',
        usage_cost: { status: 'unknown' },
      },
      {
        id: 'codex-luna-t1',
        label: 'Codex Luna Fallback',
        provider: 'codex_cli',
        provider_model: 'gpt-6-luna',
        tier: 1,
        enabled: true,
        node_id: null,
        max_concurrency: 16,
        capabilities: ['coding', 'review', 'verification'],
        task_max_attempts: 2,
        escalate_to: null,
        usage_cost: { status: 'unknown' },
      },
    ],
  };
}

export function createDomainSseFrame(
  seq: string,
  generation = 'gen-qual-1',
  taskId = 'task-001',
): string {
  const payload = {
    seq,
    observed_utc: '2026-10-18T20:26:00Z',
    event: {
      kind: 'task_started',
      task_id: taskId,
      attempt_id: `attempt-live-${seq}`,
    },
  };
  return `event: domain\nid: ${generation}:${seq}\ndata: ${JSON.stringify(payload)}\n\n`;
}
