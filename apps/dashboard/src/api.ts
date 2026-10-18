export type Fleet = {
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
      schema_version: 1;
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
    state: 'ready' | 'blocked' | 'claimed' | 'running' | 'awaiting_verification';
    attempt_id: string | null;
    owner: string | null;
    candidate_ref: string | null;
  }>;
  coordination: {
    queued_messages: number;
    acknowledged_messages: number;
    routine_model_turns: 0;
    measured_tokens: null | string;
  };
  sqlite_version: string;
  native_execution_qualified: false;
};

export async function getFleet(token?: string, signal?: AbortSignal): Promise<Fleet> {
  const response = await fetch('/v1/fleet', {
    headers: token ? { Authorization: `Bearer ${token}` } : undefined,
    signal,
  });
  if (!response.ok) throw new Error(`Fleet request failed (${response.status})`);
  return response.json() as Promise<Fleet>;
}

export function formatUnknown(value: number | string | null | undefined): string {
  return value === null || value === undefined || value === '' ? 'Unknown' : String(value);
}

export function fleetTotals(fleet: Fleet) {
  return fleet.projects.reduce((total, project) => ({
    projects: total.projects + 1,
    tasks: total.tasks + project.total,
    running: total.running + project.running,
    blocked: total.blocked + project.blocked,
    accepted: total.accepted + project.accepted,
    verifying: total.verifying + project.awaiting_verification,
  }), { projects: 0, tasks: 0, running: 0, blocked: 0, accepted: 0, verifying: 0 });
}

export function taskStateLabel(state: Fleet['tasks'][number]['state']): string {
  return state.replaceAll('_', ' ');
}
