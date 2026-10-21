import { data, list, text, type LegacySnapshot } from '../../legacy';
import { parseGitHealth, type GitHealthReport } from '../live-views/gitHealth';

export type SourceAge = 'current' | 'stale' | 'future' | 'unknown';
export function sourceAge(stamp: string, now: number, interval: unknown): SourceAge {
  const at = Date.parse(stamp);
  if (!Number.isFinite(at)) return 'unknown';
  if (at > now + 30_000) return 'future';
  const cadence = typeof interval === 'number' && Number.isFinite(interval) && interval > 0 ? interval : 0;
  return now - at > Math.max(180_000, cadence * 3_000) ? 'stale' : 'current';
}
export type ProjectHealth = {
  id: string; name: string; report: GitHealthReport | null; invalid: boolean;
  age: SourceAge; attention: boolean; incomplete: boolean; codes: string[];
};
export function projectHealth(snapshot: LegacySnapshot, now: number): ProjectHealth[] {
  const names = new Map<string, string>();
  for (const entry of list(snapshot.project_catalog).map(data)) {
    if (typeof entry.id === 'string') names.set(entry.id, text(entry.name) || entry.id);
  }
  for (const [id, entry] of Object.entries(snapshot.projects)) names.set(id, text(entry.name) || names.get(id) || id);
  const rollout = data(data(snapshot.rollout).project);
  if (typeof rollout.id === 'string') names.set(rollout.id, text(rollout.name) || rollout.id);
  const reports = data(snapshot.git_health);
  for (const id of Object.keys(reports)) if (!names.has(id)) names.set(id, id);
  return [...names].map(([id, name]): ProjectHealth => {
    let report: GitHealthReport | null = null, invalid = false;
    if (Object.hasOwn(reports, id)) {
      try { report = parseGitHealth({ schema_version: 1, error: null, reports: { [id]: reports[id] } }).reports[id]; }
      catch { invalid = true; }
    }
    const codes = [...new Set(report?.issues.map(issue => /^[a-z0-9_]{1,96}$/.test(issue.code) ? issue.code : 'unrecognized_issue') ?? [])].sort();
    return { id, name, report, invalid, age: sourceAge(report?.observed_utc ?? '', now, snapshot.interval_s),
      attention: report?.actionable === true || report?.state === 'attention',
      incomplete: !report || report.state === 'unknown' || report.shallow || report.truncated || report.observation_error || report.missing_parent_oids.length > 0,
      codes };
  }).sort((a, b) => Number(b.attention) - Number(a.attention) || Number(b.incomplete) - Number(a.incomplete) || a.id.localeCompare(b.id));
}

/** Explicitly lossy decision brief; original reports stay reachable in Git Flow.
 * It omits free-form details, paths, transcripts, subjects, credentials and task claims.
 * The brief always covers the full source inventory, independently of table filters.
 */
export function agentBrief(snapshot: LegacySnapshot, rows: ProjectHealth[]): string {
  const selected = rows.filter(row => row.attention || row.incomplete || row.age !== 'current' || row.report?.state === 'disabled');
  const limit = 20;
  const bounded = selected.filter(row => new TextEncoder().encode(row.id).length <= 128);
  const included = bounded.slice(0, limit);
  return JSON.stringify({ schema_version: 1, kind: 'fleet_operations_brief', recorded_only: true,
    source_observed_utc: snapshot.generated_utc,
    projects: rows.length, attention: rows.filter(row => row.attention).length,
    incomplete: rows.filter(row => row.incomplete).length,
    noncurrent_git_sources: rows.filter(row => row.age !== 'current').length,
    disabled: rows.filter(row => row.report?.state === 'disabled').length,
    exceptions: included.map(row => ({ project: row.id,
      state: row.report?.state ?? 'unknown', source_age: row.age, observed_utc: row.report && row.report.observed_utc.length <= 64 ? row.report.observed_utc : null,
      fingerprint: row.report && row.report.fingerprint.length <= 128 ? row.report.fingerprint : null,
      codes: row.codes.slice(0, 8), omitted_codes: Math.max(0, row.codes.length - 8),
      reason: row.invalid ? 'invalid_report' : !row.report ? 'report_unavailable' : null })),
    omitted_exceptions: selected.length - included.length,
    evidence: { route: '#/git', source: '/v1/legacy/detail/initial' },
    acceptance_granted: false,
  });
}
