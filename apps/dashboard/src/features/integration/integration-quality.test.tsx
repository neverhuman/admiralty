import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { AnyFleet } from '../../ui/CommandJourneys';
import IntegrationQuality, { taskBlockers } from './IntegrationQuality';

function fixture(): AnyFleet {
  return {
    schema_version: 2, simulated: false,
    cursor: { recovery_generation: 'test-generation', event_seq: '1' },
    projects: [{ id: 'project', name: 'Project', total: 1, running: 0, awaiting_verification: 0, blocked: 0, accepted: 0 }],
    tasks: [{
      contract: {
        schema_version: 1, project_id: 'project', root_id: 'root', task_id: 'task', revision: 1,
        title: 'Qualify candidate', objective: 'Inspect source evidence', non_goals: [],
        owned_paths: ['src'], forbidden_paths: ['private'], read_first: [], dependencies: [],
        criteria: [{ id: 'criterion', text: 'Check source', check_ids: ['check'] }],
        checks: [{ id: 'check', program: 'cargo', args: ['test'], cwd: '.', timeout_secs: 60 }],
        authority_ref: 'owner', max_attempts: 1, simulated: false,
      }, state: 'ready', attempt_id: null, owner: null, candidate_ref: null,
    }],
    coordination: { queued_messages: 0, acknowledged_messages: 0, routine_model_turns: 0, measured_tokens: null },
    sqlite_version: 'test', native_execution_qualified: false,
  };
}

describe('P29 integration and quality evidence', () => {
  it('reports missing dependencies, check definitions and recorded blocker reasons', () => {
    const fleet = fixture();
    const task = fleet.tasks[0];
    task.state = 'blocked';
    task.contract.dependencies = ['missing-task'];
    task.contract.criteria[0].check_ids = ['missing-check'];
    task.lifecycle = { revision: 1, attempts: {}, closure_digest: null, decision: null, reason: 'Reconcile disconnected owner' };
    expect(taskBlockers(task, fleet.tasks)).toEqual([
      'Reconcile disconnected owner',
      'Dependency missing-task: missing',
      `Missing check missing-check for ${task.contract.criteria[0].id}`,
    ]);
  });

  it('does not treat configured checks or an unreviewed candidate as passing evidence', () => {
    const fleet = fixture();
    const task = fleet.tasks[0];
    task.state = 'awaiting_verification';
    task.candidate_ref = 'candidate-unreviewed';
    task.lifecycle = { revision: 1, attempts: {}, closure_digest: null, decision: null, reason: null };
    const html = renderToStaticMarkup(<IntegrationQuality fleet={fleet} onSelectTask={() => {}} />);
    expect(html).toContain('candidate-unreviewed');
    expect(html).toContain('Candidate awaits a recorded verification decision');
    expect(html).toContain('No decision evidence recorded.');
    expect(html).toContain('configured checks');
  });

  it('bounds large result rendering and escapes supplied asset paths', () => {
    const fleet = fixture();
    const task = fleet.tasks[0];
    fleet.tasks = Array.from({ length: 125 }, (_, i) => ({
      ...task, contract: { ...task.contract, task_id: `task-${i}`, owned_paths: ['<script>alert(1)</script>'] },
    }));
    const html = renderToStaticMarkup(<IntegrationQuality fleet={fleet} onSelectTask={() => {}} />);
    expect(html.match(/<tr>/g)).toHaveLength(51);
    expect(html).toContain('Page 1 of 3');
    expect(html).toContain('&lt;script&gt;');
    expect(html).not.toContain('<script>');
    expect(html).toContain('scope="col"');
    expect(html).toContain('aria-label="Candidate evidence table"');
  });

  it('distinguishes unavailable and empty snapshots', () => {
    expect(renderToStaticMarkup(<IntegrationQuality fleet={null} onSelectTask={() => {}} />)).toContain('Integration snapshot unavailable');
    const fleet = fixture();
    fleet.tasks = [];
    expect(renderToStaticMarkup(<IntegrationQuality fleet={fleet} onSelectTask={() => {}} />)).toContain('No tasks match');
  });
});
