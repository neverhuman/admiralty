import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { Economics } from './Economics';
import { comparison, exportReport, MAX_REPORT_BYTES, parseReport, parseReportText, totals, type Observation } from './report';

export const observation = (extra: Partial<Observation> = {}): Observation => ({ id: 'one', project: 'harbor', task: 'task-1',
  role: 'worker', purpose: 'task', provider: 'codex', cache: 'uncached', cohort: 'baseline', maturity: 'mature',
  outcome: 'failed', rotation: 'unavailable', unit: 'token:input', amount: '9007199254740993', coverage: 'complete',
  harness_floor: null, source: 'usage-ledger', version: 'v1', ...extra });
const envelope = (rows: Observation[]) => ({ schema_version: 1, observed_utc: '2026-10-19T14:20:00Z', source_revision: 'a'.repeat(40), rows });

describe('economics source-bound reports', () => {
  it('keeps huge counters exact and currencies/quota separate', () => {
    const report = parseReport(envelope([observation(), observation({ id: 'two', amount: '1' }), observation({ id: 'usd', unit: 'USD:micro', amount: '4' }), observation({ id: 'quota', unit: 'quota:credit', amount: '7' })]));
    expect(totals(report.rows).map(row => [row.unit, row.amount])).toEqual([['USD:micro', '4'], ['quota:credit', '7'], ['token:input', '9007199254740994']]);
  });
  it('retains missing/estimated coverage and does not infer manager share', () => {
    expect(totals([observation({ amount: null, coverage: 'unknown' })])[0]).toMatchObject({ amount: null, coverage: 'unknown', manager_share: null });
    expect(totals([observation(), observation({ id: 'missing', amount: null, coverage: 'unknown' })])[0]).toMatchObject({ amount: '9007199254740993', coverage: 'partial subtotal', known: 1, count: 2, manager_share: null });
    expect(totals([observation({ coverage: 'estimated' })])[0].coverage).toBe('estimated');
  });
  it('computes measured manager share using integer arithmetic', () => {
    expect(totals([observation({ amount: '9' }), observation({ id: 'manager', amount: '1', role: 'manager' })])[0].manager_share).toBe('10.00%');
    expect(totals([observation({ amount: '0' })])[0].manager_share).toBeNull();
  });
  it('preserves source-version discontinuities and refuses cross-version comparisons', () => {
    expect(totals([observation(), observation({ id: 'new', version: 'v2' })])).toHaveLength(2);
    expect(comparison([observation(), observation({ id: 'new', version: 'v2', cohort: 'candidate' })], 'baseline', 'candidate')).toContain('Inconclusive');
  });
  it('rejects duplicate IDs, lossy numbers, noncanonical/overflow counters and unknown-zero confusion', () => {
    expect(() => parseReport(envelope([observation(), observation()]))).toThrow();
    for (const amount of [9007199254740992, '-1', '01', '1.5', String(1n << 128n), null]) {
      expect(() => parseReport(envelope([{ ...observation(), amount } as Observation]))).toThrow();
    }
    expect(() => parseReport(envelope([observation({ amount: '0', coverage: 'unknown' })]))).toThrow();
    expect(() => parseReport(envelope([observation({ amount: '1', harness_floor: '2' })]))).toThrow();
  });
  it('bounds imports, records and provenance', () => {
    expect(() => parseReportText(' '.repeat(MAX_REPORT_BYTES + 1))).toThrow('1 MiB');
    expect(() => parseReport(envelope(Array.from({ length: 1025 }, (_, id) => observation({ id: String(id) }))))).toThrow();
    expect(() => parseReport({ ...envelope([]), source_revision: 'main' })).toThrow();
  });
  it('compares all-in and coordination-only amounts separately, keeping failed roots', () => {
    const rows = [observation({ amount: '90' }), observation({ id: 'b-coord', purpose: 'coordination', amount: '10' }),
      observation({ id: 'candidate', cohort: 'candidate', amount: '90' }), observation({ id: 'c-coord', cohort: 'candidate', purpose: 'coordination', amount: '5' })];
    expect(comparison(rows, 'baseline', 'candidate')).toContain('all-in reduction 5.00%; coordination-only reduction 50.00%');
    expect(comparison(rows.slice(0, 2).concat(rows[2], observation({ ...rows[3], maturity: 'immature' })), 'baseline', 'candidate')).toContain('Inconclusive');
    expect(comparison(rows.map(row => row.id === 'candidate' ? { ...row, amount: null, coverage: 'unknown' as const } : row), 'baseline', 'candidate')).toContain('complete measured coverage');
  });
  it('exports an exact bounded selection and renders unavailable source separately from legacy', () => {
    const report = parseReport(envelope([observation()]));
    expect(parseReportText(exportReport(report, report.rows))).toEqual(report);
    const html = renderToStaticMarkup(<Economics />);
    expect(html).toContain('Normalized usage unavailable');
    expect(html).toContain('separate from legacy estimates');
  });
});
