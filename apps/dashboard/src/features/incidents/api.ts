export type Scope = { project: string; root: string; task: string; contract_revision: number; contract_digest: string; protected_oracle_digest: string };
export type Proposal = { scope: Scope; episode_digest: string; title: string; reproduction_contract: { fields: Record<string, string> }; evidence_digests: string[]; uncertainty: string; diagnosis: string; external_publication_allowed: false; execution_granted: false; proposed_investigator_max_steps: number; proposed_investigator_max_model_turns: number };
export type Episode = { scope: Scope; episode_id: string; family: string; unknown_attribution: boolean; events: { event_id: string; kind: string; cause_parent: string | null; evidence_digests: string[] }[] };
export type Snapshot = { scope: Scope; revision: number; state: { scope: Scope; episodes: Episode[] } | null; proposals: { command_id: string; proposal: Proposal }[] };
const digest = (v: unknown): v is string => typeof v === 'string' && /^sha256:[0-9a-f]{64}$/.test(v);
const id = (v: unknown): v is string => typeof v === 'string' && v.length > 0 && v.length <= 128 && v !== '.' && v !== '..' && /^[A-Za-z0-9_.-]+$/.test(v);
function object(v: unknown): Record<string, unknown> {
  if (!v || typeof v !== 'object' || Array.isArray(v)) throw new Error('Incident response is invalid.');
  return v as Record<string, unknown>;
}
export function parseScope(v: unknown): Scope {
  const s = object(v);
  if (!id(s.project) || !id(s.root) || !id(s.task) || !Number.isInteger(s.contract_revision) || (s.contract_revision as number) < 1 || (s.contract_revision as number) > 4294967295 || !digest(s.contract_digest) || !digest(s.protected_oracle_digest)) throw new Error('Incident scope is invalid.');
  return { project: s.project, root: s.root, task: s.task, contract_revision: s.contract_revision as number, contract_digest: s.contract_digest, protected_oracle_digest: s.protected_oracle_digest };
}
export const sameScope = (a: Scope, b: Scope) => JSON.stringify(parseScope(a)) === JSON.stringify(parseScope(b));
const strings = (v: unknown): v is string[] => Array.isArray(v) && v.length <= 1024 && v.every(digest);
export function parseSnapshot(v: unknown, scope: Scope): Snapshot {
  const s = object(v);
  if (!sameScope(parseScope(s.scope), scope) || !Number.isInteger(s.revision) || (s.revision as number) < 0 || (s.revision as number) > 1024 || !Array.isArray(s.proposals) || s.proposals.length > 32) throw new Error('Incident snapshot binding is invalid.');
  if (s.state !== null) {
    const state = object(s.state);
    if (!sameScope(parseScope(state.scope), scope) || !Array.isArray(state.episodes) || state.episodes.length > 1024) throw new Error('Incident state is invalid.');
    let count = 0;
    for (const value of state.episodes) {
      const e = object(value);
      if (!sameScope(parseScope(e.scope), scope) || !id(e.episode_id) || !id(e.family) || typeof e.unknown_attribution !== 'boolean' || !Array.isArray(e.events) || !e.events.length || (count += e.events.length) > 1024) throw new Error('Incident episode is invalid.');
      for (const value of e.events) {
        const event = object(value);
        if (!id(event.event_id) || !id(event.kind) || !(event.cause_parent === null || id(event.cause_parent)) || !strings(event.evidence_digests)) throw new Error('Incident event is invalid.');
      }
    }
  }
  for (const value of s.proposals) {
    const stored = object(value), p = object(stored.proposal), fields = object(object(p.reproduction_contract).fields);
    if (!id(stored.command_id) || !sameScope(parseScope(p.scope), scope) || !digest(p.episode_digest) || !strings(p.evidence_digests) || p.external_publication_allowed !== false || p.execution_granted !== false || p.proposed_investigator_max_steps !== 8 || p.proposed_investigator_max_model_turns !== 1 || Object.keys(fields).length > 32 || Object.values(fields).some(v => typeof v !== 'string' || v.length > 4096) || ['title', 'uncertainty', 'diagnosis'].some(k => typeof p[k] !== 'string' || (p[k] as string).length > 4096)) throw new Error('Local proposal is invalid.');
  }
  return s as unknown as Snapshot;
}
export class IncidentHttpError extends Error {
  constructor(public readonly status: number, message: string) { super(message); }
}
export function validateReceipt(value: unknown, command: { command_id: string; expected_revision: number }): void {
  const result = object(value), receipt = object(result.receipt);
  if (typeof result.replayed !== 'boolean' || receipt.command_id !== command.command_id || receipt.revision !== command.expected_revision + 1 || !digest(receipt.snapshot_digest)) throw new Error('Proposal receipt is unavailable.');
}
export async function incidentRequest(path: 'scopes' | 'read' | 'commands', value: unknown, token: string, signal?: AbortSignal): Promise<unknown> {
  const response = await fetch(`/v2/incidents/${path}`, { method: 'POST', headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', 'x-admiralty-requested-with': 'incidents' }, body: JSON.stringify(value), signal: signal ? AbortSignal.any([signal, AbortSignal.timeout(15000)]) : AbortSignal.timeout(15000), cache: 'no-store', redirect: 'error', credentials: 'omit' });
  if (!response.ok) throw new IncidentHttpError(response.status, response.status === 403 ? 'Access to this incident scope was denied.' : response.status === 409 ? 'Incident history changed. Refresh before proposing again.' : `Incident request unavailable (${response.status}).`);
  if (!response.body) throw new Error('Incident response is unavailable.');
  const reader = response.body.getReader(); const chunks: Uint8Array[] = []; let bytes = 0;
  try {
    for (;;) { const { value, done } = await reader.read(); if (done) break; bytes += value.length; if (bytes > 2 * 1024 * 1024) throw new Error('Incident response exceeds its bound.'); chunks.push(value); }
  } catch (error) { await reader.cancel(); throw error; }
  finally { reader.releaseLock(); }
  const data = new Uint8Array(bytes); let offset = 0; for (const chunk of chunks) { data.set(chunk, offset); offset += chunk.length; }
  return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(data));
}
