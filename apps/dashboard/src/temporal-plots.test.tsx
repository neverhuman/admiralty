import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { parseLegacy } from './legacy';
import { buildTemporalMetrics, TemporalPlots } from './ui/TemporalPlots';

// Field names and nesting captured from GET /v1/legacy: ts/proj/hosts/tiers/hyg/ty,
// token_efficiency.ledger_stats, and independently timestamped quota/token_yield.
function snapshot() {
  return parseLegacy({ generated_utc: '2026-10-18T11:04:22+00:00', order: ['alpha', 'beta'],
    projects: { alpha: { name: 'Project Alpha', done: 4, total: 10, counts: { in_progress: 3 }, pr: { ci: { state: 'success' } } },
      beta: { name: 'Project Beta', done: null, total: null, counts: null, pr: { ci: { state: 'pending' } } } },
    history: { note: 'one snapshot per ~2 min; 7-day retention', rows: [
      { ts: 1792321200, cost_index: 4.2, cost_basis: 'tokens (last 15 min, real token counts; Codex only)', credits_h: 220,
        proj: { alpha: { done: 3, total: 10, ip: 4, ci: 'failure' }, beta: { done: 1, total: 4, ip: 2, ci: 'pending' } },
        hosts: { 'arbitrary-node': { load: 7, disk: 80 } }, tiers: { luna: 2, sol: 1, claude: 1, agy: 1 }, hyg: {},
        luna_fp_rate: 92, ty: { tok: 1000, int: 600, main: 500, ym: 50, yi: 60, cov: 80 } },
      { ts: 1792321320, cost_index: 4.51, cost_basis: 'tokens (last 15 min, real token counts; Codex only)', credits_h: 226.6,
        proj: { alpha: { done: 4, total: 10, ip: 3, ci: 'success' }, beta: { done: null, total: null, ip: null, ci: 'pending' } },
        hosts: { 'arbitrary-node': { load: 12, disk: 81 } }, tiers: { luna: 3, sol: 1, astra: 1, claude: 1, agy: 1 },
        hyg: { alpha: 100 }, luna_fp_rate: 95.5, ty: { tok: 2000, int: 1100, main: 900, ym: 45, yi: 55, cov: 75 } },
    ] },
    token_efficiency: { cost_index_vs_all_luna: 11.3,
      ledger_stats: { attempts: 888, luna_failures_total: 20, luna_first_pass_rate: 95.5,
        luna_failures_per_hour_24h: [0, 1, 4, 0], funnel: { luna_tasks: 157, luna_to_sol: 18, sol_to_astra: 0 } },
      quota: { used_percent: 47, observed_utc: '2026-10-18T11:04:00Z', resets_at_utc: '2026-10-24T20:40:51Z',
        history: [['2026-10-18T11:02:00Z', 46], ['2026-10-18T11:04:00Z', 47]] } },
    token_yield: { generated_utc: '2026-10-18T10:56:08Z', window_hours: 48, method: 'Source attribution is an estimate',
      total: { tokens: 2000, integration_tokens: 1100, main_tokens: 900, yield_main_pct: 45, yield_integration_pct: 55,
        yield_main_credits_pct: 47.4, yield_integration_credits_pct: 51.6, coverage_pct: 75 } },
  });
}

describe('dense temporal dashboard strip', () => {
  it('restores all thirteen old metrics from the actual collector field names', () => {
    const model = buildTemporalMetrics(snapshot());
    expect(model.metrics.map(metric => metric.id)).toEqual(['cost-index', 'luna-failures', 'burn-up', 'in-progress', 'agent-tiers',
      'escalation', 'cpu-load', 'root-disk', 'integration-ci', 'hygiene', 'tokens', 'token-yield', 'weekly-quota']);
    const html = renderToStaticMarkup(<TemporalPlots snapshot={snapshot()} />);
    expect((html.match(/data-metric=/g) ?? []).length).toBe(13);
    for (const label of ['History and trends', 'Cost index vs all-Luna', 'Luna failures', 'first-pass', 'burn-up', 'In progress per project',
      'Agents by tier', 'Escalation funnel', 'Load % of CPUs', 'Root disk % used', 'Integration PR CI history', 'Hygiene score',
      'Tokens · generated / integration / main', 'Token yield %', 'Codex weekly quota']) expect(html).toContain(label);
    expect((html.match(/aria-expanded="false"/g) ?? []).length).toBe(13);
    expect(html).toContain('Scrollable temporal metric cards');
    expect(html).not.toMatch(/NaN|Infinity/);
  });
  it('keeps token-weighted cost distinct from the separate live-agent average', () => {
    const metric = buildTemporalMetrics(snapshot()).metrics[0];
    expect(metric.values[0].value).toBe('4.51×');
    expect(metric.series[0].points.map(point => point.value)).toEqual([4.2, 4.51]);
    expect(metric.note).toContain('real token counts');
  });
  it('retains arbitrary project/node identity across all historical series and CI rows', () => {
    const source = snapshot();
    const metrics = buildTemporalMetrics(source).metrics;
    for (const id of ['burn-up', 'in-progress', 'hygiene']) {
      const series = metrics.find(metric => metric.id === id)!.series;
      expect(series.map(value => value.id)).toEqual(['alpha', 'beta']);
      expect(series[0].label).toBe('Project Alpha');
    }
    expect(metrics.find(metric => metric.id === 'cpu-load')!.series[0]).toMatchObject({ id: 'arbitrary-node', points: [{ value: 7 }, { value: 12 }] });
    expect(metrics.find(metric => metric.id === 'root-disk')!.series[0].points.map(point => point.value)).toEqual([80, 81]);
    expect(metrics.find(metric => metric.id === 'integration-ci')!.ci![0].points.map(point => point.state)).toEqual(['failure', 'success']);
    expect(metrics.find(metric => metric.id === 'burn-up')!.series[1].points[1].value).toBeNull();
  });
  it('shows unknown counts/gaps instead of manufacturing zeros or pending CI', () => {
    const source = parseLegacy({ generated_utc: 'original source timestamp', projects: { fresh: {} }, history: { rows: [{ ts: 1 }] } });
    const metrics = buildTemporalMetrics(source).metrics;
    expect(metrics.find(metric => metric.id === 'agent-tiers')!.values.every(value => value.value === 'unknown')).toBe(true);
    expect(metrics.find(metric => metric.id === 'burn-up')!.values[0].value).toBe('unknown/unknown');
    expect(metrics.find(metric => metric.id === 'integration-ci')!.values[0].value).toBe('unknown');
    expect(metrics.find(metric => metric.id === 'integration-ci')!.ci![0].points[0].state).toBe('unknown');
    const html = renderToStaticMarkup(<TemporalPlots snapshot={source} />);
    expect(html).toContain('history unknown'); expect(html).toContain('as of unknown');
    expect(html).not.toContain('0 total'); expect(html).not.toContain('0%');
  });
  it('keeps explicit zero measurements and hourly failures separate from unknown', () => {
    const source = snapshot();
    source.token_efficiency = { ledger_stats: { attempts: 0, luna_failures_total: 0, luna_first_pass_rate: 0,
      luna_failures_per_hour_24h: [0, null, 0], funnel: { luna_tasks: 0, luna_to_sol: 0, sol_to_astra: 0 } } };
    const metric = buildTemporalMetrics(source).metrics.find(item => item.id === 'luna-failures')!;
    expect(metric.bars!.map(bar => bar.value)).toEqual([0, null, 0]);
    expect(metric.values.map(value => value.value)).toEqual(['0 total', '0% first-pass']);
    expect(renderToStaticMarkup(<TemporalPlots snapshot={source} />)).toContain('bucket before source: 0');
  });
  it('preserves independent source times and distinguishes token from credit yields', () => {
    const source = snapshot(), metrics = buildTemporalMetrics(source).metrics;
    expect(metrics.find(metric => metric.id === 'tokens')!.sourceTime).toBe('2026-10-18T10:56:08Z');
    const quota = metrics.find(metric => metric.id === 'weekly-quota')!;
    expect(quota.sourceTime).toBe('2026-10-18T11:04:00Z'); expect(quota.series[0].points.at(-1)!.time).toBe(Date.parse(quota.sourceTime) / 1000);
    const yieldMetric = metrics.find(metric => metric.id === 'token-yield')!;
    expect(yieldMetric.values[0].value).toBe('main 45%'); expect(yieldMetric.note).toContain('main 47.4%');
    expect(yieldMetric.note).toContain('not acceptance');
    const html = renderToStaticMarkup(<TemporalPlots snapshot={source} />);
    expect(html).toContain('dateTime="2026-10-18T10:56:08Z"');
  });
  it('breaks missing-data lines, sorts source time and safely escapes hostile labels', () => {
    const source = snapshot(); source.projects.alpha.name = '<img src=x onerror=alert(1)>';
    source.history = { rows: [{ ts: 3, cost_index: 3 }, { ts: 1, cost_index: 1 }, { ts: 2, cost_index: null }, { ts: 'invalid', cost_index: 100 }] };
    const metric = buildTemporalMetrics(source).metrics[0];
    expect(metric.series[0].points.map(point => point.value)).toEqual([1, null, 3]);
    const html = renderToStaticMarkup(<TemporalPlots snapshot={source} />);
    const cost = html.split('data-metric="cost-index"')[1].split('</article>')[0];
    expect((cost.match(/<circle/g) ?? []).length).toBe(2); expect(cost).not.toContain('<polyline');
    expect(html).not.toContain('<img'); expect(html).toContain('&lt;img');
  });
});
