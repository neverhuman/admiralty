import { readBoundedJson } from '../live-views/operator-chat';

export type Clock = { observed_utc: string | null; age_seconds: number | null; state: 'fresh' | 'stale' | 'future' | 'unknown' };
export type ProjectHealth = {
  id: string; total: string | null; done: string | null; blocked: string | null;
  git_state: 'healthy' | 'attention' | 'unknown' | 'disabled' | 'missing' | 'invalid'; git_clock: Clock;
  observed_branches: number | null; diverged_branches: number | null;
  dirty_checkouts: number | null; conflicted_checkouts: number | null; unknown_checkouts: number | null;
  issue_codes: Record<string, number>;
};
export type FleetBrief = {
  schema_version: 1; read_only: true; source_sha256: string; source_bytes: number;
  stale_after_seconds: number; source_clock: Clock; collector_duration_ms: number | null;
  diagnostics: string[];
  totals: { projects: number; git_review_projects: number; git_attention: number; git_unknown: number; git_disabled: number; git_missing: number; git_healthy: number; git_not_fresh: number; dirty_checkouts: number; conflicted_checkouts: number; unknown_checkouts: number };
  projects: ProjectHealth[]; omitted_projects: number; detail_path: '/v1/legacy';
};

export class HealthAccessError extends Error {
  constructor() { super('Session access is required for this producer feed.'); }
}

function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid fleet health object');
  return value as Record<string, unknown>;
}
function count(value: unknown) {
  if (!Number.isSafeInteger(value) || (value as number) < 0) throw new Error('Invalid fleet health count');
}
function clock(value: unknown) {
  const c = object(value);
  if (!['fresh', 'stale', 'future', 'unknown'].includes(String(c.state)) ||
    !(c.observed_utc === null || typeof c.observed_utc === 'string' && c.observed_utc.length <= 64 && Number.isFinite(Date.parse(c.observed_utc))) ||
    !(c.age_seconds === null || Number.isSafeInteger(c.age_seconds))) throw new Error('Invalid fleet health clock');
}
export function parseBrief(value: unknown): FleetBrief {
  const b = object(value), totals = object(b.totals);
  if (b.schema_version !== 1 || b.read_only !== true || b.detail_path !== '/v1/legacy' ||
    typeof b.source_sha256 !== 'string' || !/^sha256:[0-9a-f]{64}$/.test(b.source_sha256) ||
    !Array.isArray(b.projects) || b.projects.length > 64 || !Array.isArray(b.diagnostics) ||
    b.diagnostics.some(c => !['usage_error', 'history_error'].includes(String(c)))) throw new Error('Invalid fleet health response');
  for (const key of ['projects', 'git_review_projects', 'git_attention', 'git_unknown', 'git_disabled', 'git_missing', 'git_healthy', 'git_not_fresh', 'dirty_checkouts', 'conflicted_checkouts', 'unknown_checkouts']) count(totals[key]);
  count(b.source_bytes); count(b.stale_after_seconds); count(b.omitted_projects);
  if (Number(totals.projects) > 2000 || Number(b.source_bytes) > 32 * 1024 * 1024) throw new Error('Fleet health scope exceeds its bound');
  if (b.collector_duration_ms !== null) count(b.collector_duration_ms);
  clock(b.source_clock);
  const ids = new Set<string>();
  for (const raw of b.projects) {
    const p = object(raw);
    if (typeof p.id !== 'string' || !/^[a-zA-Z0-9_.:-]{1,128}$/.test(p.id) || ids.has(p.id) ||
      !['healthy', 'attention', 'unknown', 'disabled', 'missing', 'invalid'].includes(String(p.git_state))) throw new Error('Invalid project health');
    ids.add(p.id); clock(p.git_clock);
    for (const key of ['total', 'done', 'blocked']) if (!(p[key] === null || typeof p[key] === 'string' && /^(0|[1-9][0-9]{0,19})$/.test(p[key] as string) && BigInt(p[key] as string) <= 18446744073709551615n)) throw new Error('Invalid project counter');
    for (const key of ['observed_branches', 'diverged_branches', 'dirty_checkouts', 'conflicted_checkouts', 'unknown_checkouts']) if (p[key] !== null) count(p[key]);
    const codes = object(p.issue_codes);
    if (Object.keys(codes).length > 32) throw new Error('Invalid issue code scope');
    for (const [code, n] of Object.entries(codes)) { if (!/^[a-z_]{1,64}$/.test(code)) throw new Error('Invalid issue code'); count(n); }
  }
  if (b.projects.length + Number(b.omitted_projects) !== totals.projects ||
    ['git_attention', 'git_unknown', 'git_disabled', 'git_missing', 'git_healthy'].reduce((n, k) => n + Number(totals[k]), 0) !== totals.projects) throw new Error('Inconsistent fleet health coverage');
  return value as FleetBrief;
}

export async function fetchBrief(token: string | undefined, signal?: AbortSignal): Promise<FleetBrief> {
  const response = await fetch('/v1/legacy/brief', { signal, cache: 'no-store', redirect: 'error',
    headers: token ? { Authorization: `Bearer ${token}` } : {} });
  if (response.status === 401 || response.status === 403) throw new HealthAccessError();
  if (!response.ok) throw new Error(response.status === 404 ? 'Configure a producer feed to use fleet health.' : 'Fleet health is unavailable.');
  return parseBrief(await readBoundedJson(response, 16 * 1024));
}

// Recompute age locally so a stopped collector or retained response never looks
// fresh merely because the last HTTP request succeeded.
export function clockState(clock: Clock, now: number, staleAfter: number): Clock['state'] {
  if (!clock.observed_utc) return 'unknown';
  const age = (now - Date.parse(clock.observed_utc)) / 1000;
  return !Number.isFinite(age) ? 'unknown' : age < 0 ? 'future' : age > staleAfter ? 'stale' : 'fresh';
}
