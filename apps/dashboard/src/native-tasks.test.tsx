import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { NativeFleet } from './api';
import { parseNativeFleet } from './validation';
import NativeTasks, { filterNativeTasks } from './ui/NativeTasks';

const contract = (taskId: string, projectId: string, title: string) => ({
  authority_ref: 'fixture:authority', checks: [], criteria: [], dependencies: [], forbidden_paths: [], max_attempts: 1,
  non_goals: [], objective: `${title} objective`, owned_paths: [], project_id: projectId, read_first: [], revision: 1,
  root_id: `root-${taskId}`, schema_version: 1, simulated: false, task_id: taskId, title,
});

const task = (patch: Partial<NativeFleet['tasks'][number]> = {}): NativeFleet['tasks'][number] => ({
  contract: contract('task-a', 'project-a', 'Review release evidence'), state: 'running',
  attempt_id: 'attempt-a', owner: 'worker-a', candidate_ref: 'artifact:sha256:abc123', ...patch,
});

const fleet = (patch: Partial<NativeFleet> = {}): NativeFleet => ({
  schema_version: 2, simulated: false, cursor: { recovery_generation: 'generation-1', event_seq: '4' },
  projects: [{ id: 'project-a', name: 'Project A', total: 1, running: 1, awaiting_verification: 0, blocked: 0, accepted: 0 },
    { id: 'project-b', name: 'Project B', total: 1, running: 0, awaiting_verification: 0, blocked: 0, accepted: 0 }],
  tasks: [task(), task({ contract: contract('task-b', 'project-b', 'Check package'), state: 'awaiting_verification', attempt_id: null, owner: null, candidate_ref: null })],
  coordination: { queued_messages: 0, acknowledged_messages: 0, routine_model_turns: 0, measured_tokens: null },
  sqlite_version: '3', native_execution_qualified: false, ...patch,
});

describe('native task inspection panel', () => {
  it('filters only actual snapshot rows by project, state and text', () => {
    const snapshot = fleet();
    expect(filterNativeTasks(snapshot.tasks, { projectId: 'project-b' }).map(row => row.contract.task_id)).toEqual(['task-b']);
    expect(filterNativeTasks(snapshot.tasks, { state: 'running' }).map(row => row.contract.task_id)).toEqual(['task-a']);
    expect(filterNativeTasks(snapshot.tasks, { query: 'release', state: 'running' }).map(row => row.contract.task_id)).toEqual(['task-a']);
    expect(filterNativeTasks(snapshot.tasks, { projectId: 'project-a', state: 'blocked' })).toEqual([]);
  });

  it('renders projection state and only present attempt and candidate references', () => {
    const html = renderToStaticMarkup(<NativeTasks fleet={fleet()} error="" connection="connected" />);
    expect(html).toContain('AUTHENTICATED V2 SNAPSHOT');
    expect(html).toContain('Review release evidence');
    expect(html).toContain('running');
    expect(html).toContain('attempt-a');
    expect(html).toContain('artifact:sha256:abc123');
    expect(html).toContain('No active attempt');
    expect(html).toContain('No candidate reference');
    expect(html).not.toContain('<button');
  });

  it('distinguishes an empty real snapshot from an unavailable snapshot', () => {
    const empty = renderToStaticMarkup(<NativeTasks fleet={fleet({ tasks: [], projects: [] })} error="" connection="connected" />);
    expect(empty).toContain('current fleet snapshot contains no task records');
    const unavailable = renderToStaticMarkup(<NativeTasks fleet={null} error="Fleet request failed (503)" connection="paused" />);
    expect(unavailable).toContain('Task snapshot unavailable');
    expect(unavailable).toContain('Fleet request failed (503)');
  });

  it('labels demo rows as synthetic and keeps execution unqualified', () => {
    const demo = fleet({ simulated: true, tasks: [task({ contract: { ...contract('demo', 'project-a', 'Synthetic task'), simulated: true } })] });
    const html = renderToStaticMarkup(<NativeTasks fleet={demo} error="" connection="connected" />);
    expect(html).toContain('SIMULATED SNAPSHOT');
    expect(html).toContain('synthetic and do not represent native acceptance or execution');
    expect(html).toContain('EXECUTION NOT QUALIFIED');
  });

  it('validates v2 lifecycle projections and terminal task states without interpreting claims', () => {
    const snapshot = fleet({ tasks: [task({ state: 'accepted', lifecycle: {
      revision: 3, attempts: { 'attempt-a': 'worker-a' }, closure_digest: 'sha256:abc', decision: null, reason: null,
    } })] });
    expect(parseNativeFleet(snapshot).tasks[0].state).toBe('accepted');
    expect(() => parseNativeFleet({ ...snapshot, schema_version: 1 })).toThrow('fleet envelope');
    expect(() => parseNativeFleet({ ...snapshot, tasks: [{ ...snapshot.tasks[0], lifecycle: [] }] })).toThrow('expected object');
    expect(() => parseNativeFleet({ ...snapshot, tasks: [{ ...snapshot.tasks[0], state: 'self_accepted' }] })).toThrow('task projection');
  });

  it('shows closure and rejection reason exactly as reported by the lifecycle projection', () => {
    const snapshot = fleet({ tasks: [task({ state: 'failed', lifecycle: {
      revision: 4, attempts: { 'attempt-a': 'worker-a' }, closure_digest: 'sha256:closure', decision: null, reason: 'verification rejected',
    } })] });
    const html = renderToStaticMarkup(<NativeTasks fleet={snapshot} error="" connection="connected" />);
    expect(html).toContain('failed');
    expect(html).toContain('Closure: sha256:closure');
    expect(html).toContain('Lifecycle reason: verification rejected');
    expect(html).toContain('EXECUTION NOT QUALIFIED');
  });
});
