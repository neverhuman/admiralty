import { data, list } from '../../legacy';

export const MAX_REPORT_BYTES = 1024 * 1024;
export const MAX_ROWS = 1024;
const dimensions = ['project', 'task', 'role', 'purpose', 'provider', 'cache', 'cohort', 'maturity', 'outcome', 'rotation'] as const;
export type Dimension = typeof dimensions[number];
export type Observation = Record<Dimension, string> & {
  id: string; unit: string; amount: string | null; coverage: 'complete' | 'estimated' | 'unknown';
  harness_floor: string | null; source: string; version: string;
};
export type Report = { schema_version: 1; observed_utc: string; source_revision: string; rows: Observation[] };
export type Total = { unit: string; source: string; version: string; cohort: string; amount: string | null; coverage: string; known: number; count: number; manager_share: string | null };
export const DIMENSIONS = dimensions;

function boundedText(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0 && value.length <= 256;
}
export function exact(value: unknown): string | null {
  return typeof value === 'string' && /^(0|[1-9]\d{0,38})$/.test(value) && BigInt(value) <= (1n << 128n) - 1n ? value : null;
}
/** Imported observations are a separate series; they never establish native execution or savings. */
export function parseReport(value: unknown): Report {
  const root = data(value);
  if (new TextEncoder().encode(JSON.stringify(value)).length > MAX_REPORT_BYTES || root.schema_version !== 1
    || !boundedText(root.observed_utc) || !Number.isFinite(Date.parse(root.observed_utc))
    || typeof root.source_revision !== 'string' || !/^[0-9a-f]{40}(?:[0-9a-f]{24})?$/.test(root.source_revision)
    || !Array.isArray(root.rows) || root.rows.length > MAX_ROWS) throw Error('Invalid economics report envelope (maximum 1 MiB / 1024 rows).');
  const seen = new Set<string>();
  const rows = list(root.rows).map(value => {
    const row = data(value);
    if (!boundedText(row.id) || seen.has(row.id) || !boundedText(row.unit) || !boundedText(row.source) || !boundedText(row.version)
      || dimensions.some(key => !boundedText(row[key]))
      || !['complete', 'estimated', 'unknown'].includes(String(row.coverage))
      || (row.amount !== null && exact(row.amount) === null)
      || (row.coverage === 'unknown') !== (row.amount === null)
      || (row.harness_floor !== null && exact(row.harness_floor) === null)
      || (row.harness_floor !== null && row.amount !== null && BigInt(String(row.harness_floor)) > BigInt(String(row.amount)))) {
      throw Error('Invalid economics row: exact decimal counters, unique IDs and explicit coverage are required.');
    }
    seen.add(row.id);
    return Object.fromEntries([...dimensions.map(key => [key, row[key]]), ...['id', 'unit', 'amount', 'coverage', 'harness_floor', 'source', 'version'].map(key => [key, row[key]])]) as Observation;
  });
  return { schema_version: 1, observed_utc: root.observed_utc, source_revision: root.source_revision, rows };
}
export function parseReportText(raw: string): Report {
  if (new TextEncoder().encode(raw).length > MAX_REPORT_BYTES) throw Error('Economics report exceeds 1 MiB.');
  return parseReport(JSON.parse(raw));
}
export function share(numerator: bigint, denominator: bigint): string | null {
  if (denominator === 0n) return null;
  const hundredths = numerator * 10000n / denominator;
  return `${hundredths / 100n}.${String(hundredths % 100n).padStart(2, '0')}%`;
}
export function totals(rows: Observation[]): Total[] {
  const units = [...new Set(rows.map(row => JSON.stringify([row.unit, row.source, row.version, row.cohort])))].sort();
  return units.map(key => {
    const [unit, source, version, cohort] = JSON.parse(key) as string[];
    const group = rows.filter(row => row.unit === unit && row.source === source && row.version === version && row.cohort === cohort), known = group.filter(row => row.amount !== null);
    const sum = known.reduce((n, row) => n + BigInt(row.amount!), 0n);
    const complete = group.every(row => row.coverage === 'complete');
    const managers = group.filter(row => ['manager', 'admiral', 'commodore'].includes(row.role));
    return { unit, source, version, cohort, amount: known.length ? String(sum) : null, known: known.length, count: group.length,
      coverage: known.length === 0 ? 'unknown' : known.length < group.length ? 'partial subtotal' : complete ? 'complete' : 'estimated',
      manager_share: complete ? share(managers.reduce((n, row) => n + BigInt(row.amount!), 0n), sum) : null };
  });
}
export function comparison(rows: Observation[], baseline: string, candidate: string): string {
  if (!baseline || !candidate || baseline === candidate) return 'Select two distinct fixed cohorts.';
  const left = rows.filter(row => row.cohort === baseline), right = rows.filter(row => row.cohort === candidate);
  const keys = (group: Observation[]) => [...new Set(group.map(row => JSON.stringify([row.project, row.task, row.maturity, row.outcome, row.source, row.version])))].sort();
  if (!left.length || !right.length || JSON.stringify(keys(left)) !== JSON.stringify(keys(right))) return 'Inconclusive: assignment, maturity or outcome coverage differs; failed roots remain in the denominator.';
  if ([...left, ...right].some(row => row.coverage !== 'complete')) return 'Inconclusive: complete measured coverage is required.';
  const a = totals(left), b = totals(right);
  if (JSON.stringify(a.map(x => [x.unit, x.source, x.version])) !== JSON.stringify(b.map(x => [x.unit, x.source, x.version]))) return 'Inconclusive: units or source versions differ.';
  return a.map((unit, index) => {
    const all = BigInt(unit.amount!), after = BigInt(b[index].amount!);
    const coord = (group: Observation[]) => group.filter(row => row.unit === unit.unit && row.source === unit.source && row.version === unit.version && row.purpose === 'coordination').reduce((n, row) => n + BigInt(row.amount!), 0n);
    const beforeCoord = coord(left), afterCoord = coord(right);
    const reduction = (before: bigint, current: bigint) => before === 0n ? 'Unknown (zero baseline)' : `${current > before ? 'increase' : 'reduction'} ${share(current > before ? current - before : before - current, before)}`;
    return `${unit.unit}: all-in ${reduction(all, after)}; coordination-only ${reduction(beforeCoord, afterCoord)}`;
  }).join(' · ') + '. Observational comparison; no causal savings claim.';
}
export function exportReport(report: Report, rows: Observation[]): string {
  const raw = JSON.stringify({ ...report, rows }, null, 2);
  if (new TextEncoder().encode(raw).length > MAX_REPORT_BYTES) throw Error('Export exceeds 1 MiB. Reduce the selection.');
  return raw;
}
