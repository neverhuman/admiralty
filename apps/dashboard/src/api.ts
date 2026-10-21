import type { CandidateDecisionEvidence as I1CandidateDecisionEvidence, TaskContract } from '../../../packages/contracts/index';

export type Cursor = { recovery_generation: string; event_seq: string };
export class ApiError extends Error {
  constructor(message: string, public readonly status: number) { super(message); this.name = 'ApiError'; }
}
export const MAX_FLEET_RESPONSE_BYTES = 8 * 1024 * 1024;

/** Content-Length is an early guard; actual streamed bytes remain authoritative. */
export async function readBoundedJson(response: Response): Promise<unknown> {
  const declared = response.headers.get('Content-Length');
  if (declared && /^\d+$/.test(declared) && Number(declared) > MAX_FLEET_RESPONSE_BYTES) {
    await response.body?.cancel();
    throw new Error('Fleet response exceeds eight MiB');
  }
  if (!response.body) throw new Error('Fleet response has no JSON body');
  const reader = response.body.getReader();
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const chunks: string[] = [];
  let bytes = 0, pending = '';
  try {
    for (;;) {
      const result = await reader.read();
      if (result.done) break;
      bytes += result.value.byteLength;
      if (bytes > MAX_FLEET_RESPONSE_BYTES) throw new Error('Fleet response exceeds eight MiB');
      pending += decoder.decode(result.value, { stream: true });
      // Coalesce tiny transport chunks so bounded bytes also mean bounded
      // bookkeeping memory rather than millions of one-character entries.
      if (pending.length >= 64 * 1024) { chunks.push(pending); pending = ''; }
    }
    chunks.push(pending + decoder.decode());
    return JSON.parse(chunks.join(''));
  } catch (error) {
    await reader.cancel().catch(() => undefined);
    throw error;
  } finally { reader.releaseLock(); }
}

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
    contract: TaskContract;
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

export type NativeTaskState = Fleet['tasks'][number]['state'] | 'accepted' | 'failed' | 'cancelled';
export type CandidateDecisionEvidence = I1CandidateDecisionEvidence;
export type NativeTaskLifecycle = {
  revision: number;
  attempts: Record<string, string>;
  closure_digest: string | null;
  decision: CandidateDecisionEvidence | null;
  reason: string | null;
};
export type NativeTaskView = Omit<Fleet['tasks'][number], 'state'> & {
  state: NativeTaskState;
  /** Exact read-only lifecycle projection published by I1. */
  lifecycle?: NativeTaskLifecycle;
};
export type NativeFleet = Omit<Fleet, 'schema_version' | 'tasks'> & {
  schema_version: 2;
  tasks: NativeTaskView[];
};

export async function getFleet(token?: string, signal?: AbortSignal, fetcher: typeof fetch = fetch): Promise<Fleet> {
  const response = await fetcher('/v1/fleet', {
    headers: token ? { Authorization: `Bearer ${token}` } : undefined,
    signal, cache: 'no-store',
  });
  if (!response.ok) throw new ApiError(`Fleet request failed (${response.status})`, response.status);
  const body = await readBoundedJson(response);
  // Generated schema guards load on first use, outside the initial bundle.
  const { parseFleet } = await import('./validation');
  return parseFleet(body);
}

export async function getNativeFleet(token?: string, signal?: AbortSignal, fetcher: typeof fetch = fetch): Promise<NativeFleet> {
  const response = await fetcher('/v2/fleet', {
    headers: token ? { Authorization: `Bearer ${token}` } : undefined,
    signal, cache: 'no-store',
  });
  if (!response.ok) throw new ApiError(`Fleet request failed (${response.status})`, response.status);
  const body = await readBoundedJson(response);
  const { parseNativeFleet } = await import('./validation');
  return parseNativeFleet(body);
}

export function formatUnknown(value: number | string | null | undefined): string {
  return value === null || value === undefined || value === '' ? 'Unknown' : String(value);
}

export function fleetTotals(fleet: Pick<NativeFleet, 'projects'>) {
  return fleet.projects.reduce((total, project) => ({
    projects: total.projects + 1,
    tasks: total.tasks + project.total,
    running: total.running + project.running,
    blocked: total.blocked + project.blocked,
    accepted: total.accepted + project.accepted,
    verifying: total.verifying + project.awaiting_verification,
  }), { projects: 0, tasks: 0, running: 0, blocked: 0, accepted: 0, verifying: 0 });
}

export function taskStateLabel(state: NativeTaskState): string {
  return state.replaceAll('_', ' ');
}
