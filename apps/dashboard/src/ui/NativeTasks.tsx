import { useEffect, useMemo, useState } from 'react';
import type { NativeFleet, NativeTaskState } from '../api';
import { taskStateLabel } from '../api';
import type { ConnectionState } from '../events';

type Task = NativeFleet['tasks'][number];
type TaskState = NativeTaskState;
type TaskFilters = { query?: string; projectId?: string; state?: TaskState | '' };

const taskStates: TaskState[] = ['ready', 'blocked', 'claimed', 'running', 'awaiting_verification'];

export function filterNativeTasks(tasks: Task[], filters: TaskFilters = {}): Task[] {
  const query = filters.query?.trim().toLowerCase() ?? '';
  return tasks.filter(({ contract, state }) =>
    (!filters.projectId || contract.project_id === filters.projectId)
    && (!filters.state || state === filters.state)
    && (!query || `${contract.title} ${contract.objective} ${contract.task_id} ${contract.project_id}`.toLowerCase().includes(query)));
}

export default function NativeTasks({ fleet, error, connection }: {
  fleet: NativeFleet | null;
  error: string;
  connection: ConnectionState;
}) {
  const [query, setQuery] = useState('');
  const [projectId, setProjectId] = useState('');
  const [state, setState] = useState<TaskState | ''>('');
  const projects = useMemo(() => new Map(fleet?.projects.map(project => [project.id, project.name]) ?? []), [fleet]);
  const visibleTasks = useMemo(() => fleet ? filterNativeTasks(fleet.tasks, { query, projectId, state }) : [], [fleet, query, projectId, state]);

  useEffect(() => {
    if (projectId && !projects.has(projectId)) setProjectId('');
  }, [projectId, projects]);

  return <section className="panel workplans-panel native-task-panel" aria-labelledby="native-task-title">
    <div className="panel-head">
      <div><div className="eyebrow">NATIVE TASKS / SNAPSHOT PROJECTION</div><h2 id="native-task-title">Task inspection</h2></div>
      {fleet && <span className="tiny-badge">{fleet.simulated ? 'SIMULATED SNAPSHOT' : 'AUTHENTICATED V2 SNAPSHOT'} · EXECUTION NOT QUALIFIED</span>}
    </div>
    {!fleet ? <div className="native-task-unavailable" role="status">
      <strong>Task snapshot unavailable</strong>
      <p>{error || `Waiting for the /v1/fleet snapshot (${connection}). No task records have been received.`}</p>
    </div> : <>
      {error && <p className="native-task-refresh-error" role="status">Snapshot refresh issue: {error}. The last received snapshot remains visible.</p>}
      {fleet.simulated && <p className="native-task-simulation">Demo data only. These task rows are synthetic and do not represent native acceptance or execution.</p>}
      <div className="task-inspection-controls">
        <label className="search"><span aria-hidden="true">⌕</span><input aria-label="Filter tasks" placeholder="Filter tasks" value={query} onChange={event => setQuery(event.target.value)} /></label>
        <label>Project<select aria-label="Filter by project" value={projectId} onChange={event => setProjectId(event.target.value)}>
          <option value="">All projects</option>
          {fleet.projects.map(project => <option value={project.id} key={project.id}>{project.name}</option>)}
        </select></label>
        <label>State<select aria-label="Filter by task state" value={state} onChange={event => setState(event.target.value as TaskState | '')}>
          <option value="">All states</option>
          {taskStates.map(taskState => <option value={taskState} key={taskState}>{taskStateLabel(taskState)}</option>)}
        </select></label>
        <span className="native-task-count">{visibleTasks.length} of {fleet.tasks.length} tasks</span>
      </div>
      <div className="table-scroll"><table><thead><tr><th>Task</th><th>Project</th><th>State</th><th>Active attempt</th><th>Candidate reference</th></tr></thead><tbody>
        {visibleTasks.map(task => <tr key={`${task.contract.task_id}:${task.contract.revision}`}>
          <td><strong>{task.contract.title}</strong><small>{task.contract.task_id} · rev {task.contract.revision}</small></td>
          <td>{projects.get(task.contract.project_id) ?? task.contract.project_id}</td>
          <td><span className={`state-pill ${task.state}`}>{taskStateLabel(task.state)}</span></td>
          <td>{task.attempt_id ?? <span className="muted">No active attempt</span>}{task.owner && <small>Owner: {task.owner}</small>}</td>
          <td>{task.candidate_ref ?? <span className="muted">No candidate reference</span>}</td>
        </tr>)}
      </tbody></table></div>
      {!visibleTasks.length && <div className="empty-state"><span>⌁</span>{fleet.tasks.length && (query || projectId || state)
        ? 'No snapshot tasks match these filters.'
        : fleet.tasks.length ? 'No snapshot tasks match this filter.' : 'The current fleet snapshot contains no task records.'}</div>}
      <p className="native-task-source">Source: current {fleet.simulated ? 'simulated' : 'authenticated'} <code>/v2/fleet</code> snapshot. This panel is read only; native execution is not qualified.</p>
    </>}
  </section>;
}
