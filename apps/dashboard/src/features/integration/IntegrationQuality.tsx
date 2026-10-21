import { useMemo, useState } from 'react';
import type { AnyFleet, AnyFleetTask } from '../../ui/CommandJourneys';
import './integration-quality.css';

export function taskBlockers(task: AnyFleetTask, tasks: readonly AnyFleetTask[]): string[] {
  const blockers: string[] = [];
  if (task.lifecycle?.reason) blockers.push(task.lifecycle.reason);
  for (const id of task.contract.dependencies) {
    const dependency = tasks.find(item => item.contract.task_id === id);
    if (dependency?.state !== 'accepted') blockers.push(`Dependency ${id}: ${dependency?.state ?? 'missing'}`);
  }
  const checks = new Set(task.contract.checks.map(check => check.id));
  for (const criterion of task.contract.criteria) {
    for (const id of criterion.check_ids) {
      if (!checks.has(id)) blockers.push(`Missing check ${id} for ${criterion.id}`);
    }
  }
  if (task.state === 'awaiting_verification' && !task.lifecycle?.decision) {
    blockers.push('Candidate awaits a recorded verification decision');
  }
  if (task.state === 'blocked' && blockers.length === 0) blockers.push('Blocker details unavailable in this snapshot');
  return blockers;
}

const PAGE_SIZE = 50;

export default function IntegrationQuality({ fleet, onSelectTask }: {
  fleet: AnyFleet | null;
  onSelectTask: (taskId: string) => void;
}) {
  const [query, setQuery] = useState('');
  const [project, setProject] = useState('');
  const [page, setPage] = useState(0);
  const filtered = useMemo(() => {
    const search = query.trim().toLowerCase();
    return (fleet?.tasks ?? []).filter(task =>
      (!project || task.contract.project_id === project) &&
      [task.contract.task_id, task.contract.title, task.candidate_ref ?? '', ...taskBlockers(task, fleet?.tasks ?? [])]
        .some(value => value.toLowerCase().includes(search)));
  }, [fleet, project, query]);
  const lastPage = Math.max(0, Math.ceil(filtered.length / PAGE_SIZE) - 1);
  const currentPage = Math.min(page, lastPage);
  const visible = filtered.slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE);

  if (!fleet) return <div className="loading-card" role="status">Integration snapshot unavailable. Reconnect to inspect recorded evidence.</div>;

  return <section className="panel integration-quality" aria-labelledby="integration-quality-title">
    <div className="panel-head"><div><div className="eyebrow">Integration and quality</div>
      <h2 id="integration-quality-title">Candidates, blockers &amp; evidence</h2></div>
      <a href="#/atlas">Fleet inventory &amp; node observations</a></div>
    <p>Open a task to inspect its verification evidence and available authorized actions.</p>
    {fleet.simulated && <p role="status">Simulated snapshot · these rows are fixture evidence.</p>}
    <div className="integration-filters">
      <label>Search candidates or blockers<input value={query} onChange={event => { setQuery(event.target.value); setPage(0); }} /></label>
      <label>Integration project<select value={project} onChange={event => { setProject(event.target.value); setPage(0); }}>
        <option value="">All visible projects</option>
        {fleet.projects.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
      </select></label>
    </div>
    <p role="status">{filtered.length} matching tasks · snapshot event {fleet.cursor.event_seq}</p>
    {visible.length === 0 ? <p>No tasks match the current filters.</p> : <div className="integration-table" tabIndex={0} role="region" aria-label="Candidate evidence table">
      <table><caption>Recorded candidate and quality evidence</caption>
        <thead><tr><th scope="col">Task</th><th scope="col">State &amp; blockers</th><th scope="col">Candidate &amp; decision</th><th scope="col">Checks &amp; assets</th></tr></thead>
        <tbody>{visible.map(task => {
          const blockers = taskBlockers(task, fleet.tasks);
          const decision = task.lifecycle?.decision;
          return <tr key={task.contract.task_id}>
            <td><button type="button" className="text-button" onClick={() => onSelectTask(task.contract.task_id)}>{task.contract.title}</button>
              <div>{task.contract.task_id}</div><small>{task.contract.project_id} · revision {task.contract.revision}</small></td>
            <td><strong>{task.state.replaceAll('_', ' ')}</strong>
              {blockers.length ? <ul>{blockers.map((blocker, index) => <li key={index}>{blocker}</li>)}</ul> : <p>No blocker recorded.</p>}</td>
            <td><div>Candidate: <code>{task.candidate_ref ?? 'Not recorded'}</code></div>
              {decision ? <details><summary>Recorded decision evidence</summary>
                <dl><dt>Source commit</dt><dd><code>{decision.source_commit_sha}</code></dd>
                  <dt>Reviewer</dt><dd>{decision.verifier}</dd>
                  <dt>Receipt</dt><dd><code>{decision.receipt_digest}</code></dd>
                  <dt>Verification policy</dt><dd><code>{decision.verification_policy_digest}</code></dd></dl>
              </details> : <p>No decision evidence recorded.</p>}</td>
            <td><div>{task.contract.checks.length} configured checks</div>
              <details><summary>Checks &amp; owned assets</summary><ul>{task.contract.checks.map(check => <li key={check.id}>{check.id}</li>)}</ul>
                <p>Owned paths</p><ul>{task.contract.owned_paths.map(path => <li key={path}><code>{path}</code></li>)}</ul>
                <p>Protected paths</p><ul>{task.contract.forbidden_paths.map(path => <li key={path}><code>{path}</code></li>)}</ul>
              </details></td>
          </tr>;
        })}</tbody>
      </table></div>}
    <nav className="integration-pages" aria-label="Candidate result pages">
      <button type="button" disabled={currentPage === 0} onClick={() => setPage(currentPage - 1)}>Previous candidates</button>
      <span>Page {currentPage + 1} of {lastPage + 1}</span>
      <button type="button" disabled={currentPage === lastPage} onClick={() => setPage(currentPage + 1)}>Next candidates</button>
    </nav>
  </section>;
}
