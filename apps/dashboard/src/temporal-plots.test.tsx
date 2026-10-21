import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { parseLegacy } from './legacy';
import { TokenEfficiency } from './ui/TokenEfficiency';
import { LegacyHierarchy } from './ui/LegacyHierarchy';
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
  it('does not label a fully covered cost sample as partial', () => {
    const source = snapshot();
    source.token_efficiency = { cost_index_vs_all_luna: 1.25, cost_index_observed_vs_all_luna: 1.25,
      cost_index_note: 'Full measured token coverage', cost_index_observed_note: 'Same observed events' };
    const cost = buildTemporalMetrics(source).metrics[0];
    expect(cost.values.map(value => value.value)).toEqual(['1.25×', '1.25× observed']);
    expect(cost.values.map(value => value.label).join(' ')).not.toContain('partial');
    expect(cost.series.map(value => value.label).join(' ')).not.toContain('partial');
  });
  it('never resurrects historical full cost after the current producer explicitly reports unknown', () => {
    const source = snapshot();
    source.token_efficiency = { cost_index_vs_all_luna: null, cost_index_observed_vs_all_luna: 1.234567 };
    const metric = buildTemporalMetrics(source).metrics[0];
    expect(metric.values.map(value => value.value)).toEqual(['1.23× observed']);
    expect(metric.series[0].points.map(point => point.value)).toEqual([4.2, 4.51]);
    const html = renderToStaticMarkup(<TokenEfficiency snapshot={source} />);
    expect(html).toContain('Cost index · observed sample');
    expect(html).not.toContain('4.51×');
  });
  it('shows current lower-bound counts and same-sample ratios before history sampling and keeps resets unknown', () => {
    const source = snapshot(); source.history = { rows: [] };
    source.token_yield = { generated_utc: source.generated_utc, total: {
      tokens: null, integration_tokens: null, main_tokens: null, attributed_tokens: null,
      lower_bound_tokens: 300, lower_bound_integration_tokens: 150, lower_bound_main_tokens: 100, lower_bound_attributed_tokens: 225,
    } };
    const metric = (id: string) => buildTemporalMetrics(source).metrics.find(item => item.id === id)!;
    expect(metric('tokens').values.map(value => value.value)).toEqual(['≥ 300 observed', '≥ 150 observed', '≥ 100 observed', '≥ 225 observed']);
    expect(metric('token-yield').values.slice(3).map(value => value.value)).toEqual(['33.33%', '50%', '75%']);
    expect(metric('tokens').sourceTime).toBe(source.generated_utc);
    expect(metric('tokens').series.every(item => item.points.length === 0)).toBe(true);
    const html = renderToStaticMarkup(<TokenEfficiency snapshot={source} />);
    expect(html).toContain('≥ 150 observed'); expect(html).toContain('observed-sample main yield');
    const total = (source.token_yield as { total: Record<string, unknown> }).total;
    total.lower_bound_tokens = 0; total.lower_bound_main_tokens = 0;
    expect(metric('token-yield').values[3].value).toBe('unknown');
    total.lower_bound_tokens = null; total.lower_bound_main_tokens = null;
    expect(metric('tokens').values[0].value).toBe('unknown');
    expect(metric('token-yield').values[3].value).toBe('unknown');
  });
  it('uses the hygiene audit clock instead of subsequent fleet or status refreshes', () => {
    const source = snapshot();
    source.super_captain = { status_mtime_utc: '2026-10-18T11:04:00Z', status: { captains: {
      alpha: { hygiene: { score: 87.654321, observed_utc: '2026-10-18T10:00:00Z' } },
    } } };
    source.history = { rows: [1792321200, 1792321320].map(ts => ({ ts,
      hyg_at: { alpha: Date.parse('2026-10-18T10:00:00Z') / 1000 }, hyg: { alpha: 87.654321 } })) };
    const metric = buildTemporalMetrics(source).metrics.find(item => item.id === 'hygiene')!;
    expect(metric.values[0].label).toContain('2026-10-18T10:00:00Z');
    expect(metric.series[0].points).toEqual([{ time: Date.parse('2026-10-18T10:00:00Z') / 1000, value: 87.654321 }]);
    expect(metric.values[0].value).toContain('87.65');
  });
  it('reports real partial hygiene ranges in current cards and retained history without inventing a final score', () => {
    const source = snapshot(), at = '2026-10-18T10:00:00Z';
    const partial = { score: null, grade: null, possible_score_range: [30.123456, 80.987654], known_component_subtotal: 80.987654, observed_utc: at };
    source.super_captain = { status_mtime_utc: source.generated_utc, status: { captains: { alpha: { hygiene: partial } } } };
    source.hierarchy = { captains: [{ key: 'alpha', name: 'Project Alpha', workers: [] }] };
    source.history = { rows: [1792321200, 1792321320].map(ts => ({ ts, hyg_at: { alpha: Date.parse(at) / 1000 }, hyg: { alpha: partial } })) };
    const metric = () => buildTemporalMetrics(source).metrics.find(item => item.id === 'hygiene')!;
    expect(metric().values[0].value).toContain('30.12–80.99 range · partial audit · known subtotal 80.99');
    expect(metric().values[0].label).toContain(at);
    expect(metric().series[0].points).toEqual([{ time: Date.parse(at) / 1000, value: null }]);
    expect(metric().series.find(item => item.label === 'Project Alpha · partial minimum')!.points).toEqual([{ time: Date.parse(at) / 1000, value: 30.123456 }]);
    expect(metric().series.find(item => item.label === 'Project Alpha · partial maximum')!.points).toEqual([{ time: Date.parse(at) / 1000, value: 80.987654 }]);
    const html = renderToStaticMarkup(<><TemporalPlots snapshot={source} /><LegacyHierarchy snapshot={source} /></>);
    expect(html).toContain('30.12–80.99 range');
    expect(html).toContain('partial audit');
    expect(html).not.toContain('30.123456');
    expect(html).not.toContain('80.987654');
    source.super_captain = {};
    expect(metric().values[0].label).toContain('history at 2026-10-18T10:00:00.000Z');
    expect(metric().values[0].value).toContain('30.12–80.99 range');
  });
  it('keeps malformed hygiene ranges unknown and retains a measured zero range', () => {
    for (const range of [[90, 80], [-1, 80], [0, 101], [null, 80], ['0', 80], [0, 80, 90], []]) {
      const source = snapshot(); source.history = { rows: [] };
      source.super_captain = { status: { captains: { alpha: { hygiene: { score: null, possible_score_range: range, known_component_subtotal: 80 } } } } };
      expect(buildTemporalMetrics(source).metrics.find(item => item.id === 'hygiene')!.values[0].value).toBe('Not reported by Admiral');
    }
    const source = snapshot(); source.history = { rows: [] };
    source.super_captain = { status: { captains: { alpha: { hygiene: { score: null, possible_score_range: [0, 0], known_component_subtotal: 0 } } } } };
    expect(buildTemporalMetrics(source).metrics.find(item => item.id === 'hygiene')!.values[0].value).toContain('0–0 range · partial audit · known subtotal 0');
  });
  it('does not render a final grade or grade color for an explicitly partial audit', () => {
    for (const grade of ['A', 'D']) {
      const source = snapshot(); source.hierarchy = { captains: [{ key: 'alpha', name: 'Project Alpha', workers: [] }] };
      source.super_captain = { status: { captains: { alpha: { hygiene: { score: null, grade, possible_score_range: [30, 80], known_component_subtotal: 80 } } } } };
      const html = renderToStaticMarkup(<LegacyHierarchy snapshot={source} />);
      expect(html).toContain('30–80 range · partial audit · known subtotal 80');
      expect(html).not.toMatch(/>[AD]\s|legacy-hierarchy-good|legacy-hierarchy-bad/);
    }
  });
  it('allocates unique range series IDs even when reported project IDs occupy generated names', () => {
    const source = snapshot(), reserved = '["hygiene-partial","alpha",0]';
    source.order = ['alpha', 'alpha-partial-0', reserved];
    source.projects = Object.fromEntries(source.order.map(id => [id, { name: id }]));
    source.history = { rows: [{ ts: 1792321200, hyg: { alpha: { score: null, possible_score_range: [30, 80] } } }] };
    const series = buildTemporalMetrics(source).metrics.find(item => item.id === 'hygiene')!.series;
    expect(series.slice(0, 3).map(item => item.id)).toEqual(source.order);
    expect(series).toHaveLength(5);
    expect(new Set(series.map(item => item.id)).size).toBe(series.length);
    expect(series.find(item => item.label === 'alpha · partial minimum')!.points[0].value).toBe(30);
  });
  it('uses the same small percentages in live trends and the token summary', () => {
    const source = snapshot(); source.token_yield = { total: {
      tokens: null, lower_bound_tokens: 10000, lower_bound_main_tokens: 1,
      lower_bound_integration_tokens: 2, lower_bound_attributed_tokens: 42,
    } };
    const values = buildTemporalMetrics(source).metrics.find(item => item.id === 'token-yield')!.values;
    expect(values.slice(3).map(item => item.value)).toEqual(['0.01%', '0.02%', '0.42%']);
    const visible = renderToStaticMarkup(<TokenEfficiency snapshot={source} />).replace(/<[^>]*>/g, '');
    expect(visible).toContain('0.01% observed-sample main yield');
    expect(visible).toContain('0.02% observed-sample integration yield');
    expect(visible).toContain('0.42% observed-sample coverage');
  });
  it('explains unavailable Admiral status while keeping retained audit values at their original time', () => {
    const source = snapshot(); source.super_captain = { status: null,
      status_error: 'Admiral status unavailable (128 KiB source limit): source exceeds configured byte limit',
      status_mtime_utc: source.generated_utc };
    source.history = { rows: [{ ts: 1792321200, hyg_at: { alpha: 1792321140 }, hyg: { alpha: 72 } }] };
    const metric = buildTemporalMetrics(source).metrics.find(item => item.id === 'hygiene')!;
    expect(metric.values[0].value).toContain('72');
    expect(metric.values[0].label).toContain('history at 2026-10-18T10:59:00.000Z');
    expect(metric.values[1].value).toBe('Admiral status unavailable');
    expect(metric.note).toContain('source exceeds configured byte limit');
  });
  it('restores all thirteen original metrics and adds memory and Git hygiene trends', () => {
    const model = buildTemporalMetrics(snapshot());
    expect(model.metrics.map(metric => metric.id)).toEqual(['cost-index', 'luna-failures', 'burn-up', 'in-progress', 'agent-tiers',
      'escalation', 'cpu-load', 'root-disk', 'host-memory', 'integration-ci', 'git-unmerged', 'hygiene', 'tokens', 'token-yield', 'weekly-quota']);
    const html = renderToStaticMarkup(<TemporalPlots snapshot={snapshot()} />);
    expect((html.match(/data-metric=/g) ?? []).length).toBe(15);
    for (const label of ['History and trends', 'Cost index vs all-Luna', 'Luna failures', 'first-pass', 'burn-up', 'In progress per project',
      'Agents by tier', 'Escalation funnel', 'Load % of CPUs', 'Root disk % used', 'Integration PR CI history', 'Hygiene score',
      'Tokens · generated / integration / main', 'Token yield %', 'Codex weekly quota']) expect(html).toContain(label);
    expect((html.match(/aria-expanded="false"/g) ?? []).length).toBe(15);
    expect(html).toContain('Scrollable temporal metric cards');
    expect(html).not.toMatch(/NaN|Infinity/);
  });
  it('keeps token-weighted cost distinct from the separate live-agent average', () => {
    const metric = buildTemporalMetrics(snapshot()).metrics[0];
    expect(metric.values[0].value).toBe('11.3×');
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
    expect(yieldMetric.values[0].value).toBe('45%'); expect(yieldMetric.values[0].label).toBe('Full main yield'); expect(yieldMetric.note).toContain('main 47.4%');
    expect(yieldMetric.note).toContain('estimates from the same observed events');
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

  it('keeps full and observed cost and yield bases separate across mixed history', () => {
    const source = snapshot();
    source.token_efficiency = { cost_index_vs_all_luna: null, cost_index_observed_vs_all_luna: 1.234,
      quota: { credits_balance: '123456789012345678.999' },
      cost_index_observed_note: 'Partial priced Codex sample · not full spend' };
    source.history = { rows: [
      { ts: 1792321200, cost_index: 4.25, cost_index_observed: null,
        ty: { tok: 1000, int: 600, main: 400, ym: 40, yi: 60, cov: 80,
          observed: { tok: 900, int: 500, main: 350, attributed: 300, ym: null, yi: null, cov: null } } },
      { ts: 1792321320, cost_index: null, cost_index_observed: 1.2345,
        cost_observed_basis: 'Priced observed samples only',
        ty: { tok: null, int: null, main: null, ym: null, yi: null, cov: null,
          observed: { tok: 80, int: 45, main: 30, attributed: 25, ym: 0, yi: 87.64277401681029, cov: null } } },
    ] };
    source.token_yield = { generated_utc: '2026-10-18T11:04:00Z', method: 'event counts',
      total: { tokens: null, integration_tokens: null, main_tokens: null, attributed_tokens: null,
        lower_bound_tokens: 120, lower_bound_integration_tokens: 70, lower_bound_main_tokens: 50, lower_bound_attributed_tokens: 42,
        yield_main_pct: null, yield_integration_pct: null, coverage_pct: null } };

    const model = buildTemporalMetrics(source).metrics;
    const cost = model.find(metric => metric.id === 'cost-index')!;
    expect(cost.values.map(value => value.value)).toEqual(['1.23× observed']);
    expect(cost.series.map(item => item.id)).toEqual(['cost-full', 'cost-observed']);
    expect(cost.series[0].points.map(point => point.value)).toEqual([4.25, null]);
    expect(cost.series[1].points.map(point => point.value)).toEqual([null, 1.2345]);
    expect(cost.note).toContain('Partial priced Codex sample');

    const tokens = model.find(metric => metric.id === 'tokens')!;
    expect(tokens.values.map(value => value.value)).toEqual(['≥ 120 observed', '≥ 70 observed', '≥ 50 observed', '≥ 42 observed']);
    expect(tokens.series.find(item => item.id === 'token-observed-main')!.points.map(point => point.value)).toEqual([350, 30]);
    const yieldMetric = model.find(metric => metric.id === 'token-yield')!;
    expect(yieldMetric.values.slice(0, 3).map(value => value.value)).toEqual(['unknown', 'unknown', 'unknown']);
    expect(yieldMetric.values.slice(3).map(value => value.value)).toEqual(['41.67%', '58.33%', '35%']);
    expect(yieldMetric.series.find(item => item.id === 'yield-main-observed')!.points.map(point => point.value)).toEqual([null, 0]);
    expect(yieldMetric.series.find(item => item.id === 'yield-main-full')!.points.map(point => point.value)).toEqual([40, null]);

    const html = renderToStaticMarkup(<><TemporalPlots snapshot={source} /><TokenEfficiency snapshot={source} /></>);
    expect(html).toContain('Observed sample');
    expect(html).toContain('Partial priced Codex sample');
    expect(html).toContain('≥ 120 observed');
    expect(html).toContain('Observed-sample main yield');
    expect(html).toContain('123,456,789,012,345,679');
    expect(html).not.toContain('87.642774');
  });

  it('shows current and historical hygiene with original per-project source age, including missing sources', () => {
    const source = snapshot();
    source.hierarchy = { captains: [{ key: 'alpha', name: 'Project Alpha', state: 'idle', workers: [] },
      { key: 'beta', name: 'Project Beta', state: 'idle', workers: [] }] };
    source.super_captain = { status_mtime_utc: '2026-10-18T10:00:00Z', status: { captains: {
      alpha: { hygiene: { grade: 'A', score: 96.42857142857143, notes: ['reported'] } }, beta: { hygiene: null },
    } } };
    source.history = { rows: [
      { ts: 1792321200, hyg_at: { beta: 1792321140 }, hyg: { beta: 13.593749999999998 } },
      { ts: 1792321320, hyg_at: {}, hyg: {} },
    ] };
    const metric = buildTemporalMetrics(source).metrics.find(item => item.id === 'hygiene')!;
    expect(metric.values[0].value).toContain('96.43');
    expect(metric.values[0].label).toContain('2026-10-18T10:00:00Z');
    expect(metric.values[1].value).toContain('13.59');
    expect(metric.values[1].label).toContain('2026-10-18T10:59:00.000Z');
    const html = renderToStaticMarkup(<LegacyHierarchy snapshot={source} />);
    expect(html).toContain('Hygiene');
    expect(html).toContain('96.43');
    expect(html).toContain('2026-10-18T10:00:00Z');
    expect(html).toContain('Not reported by Admiral');
    expect(html).not.toContain('Hygiene 0');
  });
  it('trends memory headroom and unmerged lanes, preferring live Git metrics and keeping gaps unknown', () => {
    const source = parseLegacy({ generated_utc: '2026-10-21T08:00:00Z', order: ['alpha', 'beta'],
      projects: { alpha: { name: 'Alpha' }, beta: { name: 'Beta' } },
      global_resources: { hosts: [{ host: 'node-a' }] },
      git_health: { alpha: { state: 'attention', metrics: { unmerged_branches: 4, dirty_checkouts: 1 } } },
      history: { rows: [
        { ts: 1792540800, hosts: { 'node-a': { load: 10, disk: 50, mem: 40.5 } }, git: { alpha: { st: 'healthy', un: 2, dt: 0 }, beta: { st: 'unknown', un: null } } },
        { ts: 1792540920, hosts: { 'node-a': { load: 11, disk: 50 } }, git: { alpha: { st: 'attention', un: 3, dt: 1 }, beta: { st: 'healthy', un: 1, dt: 2 } } },
      ] } });
    const metrics = buildTemporalMetrics(source).metrics;
    const memory = metrics.find(metric => metric.id === 'host-memory')!;
    expect(memory.max).toBe(100);
    expect(memory.values).toEqual([{ label: 'node-a', value: 'unknown', color: expect.any(String) }]);
    expect(memory.series[0].points.map(point => point.value)).toEqual([40.5, null]);
    const git = metrics.find(metric => metric.id === 'git-unmerged')!;
    expect(git.values.map(value => [value.label, value.value])).toEqual([
      ['Alpha', '4 unmerged · 1 dirty'],
      ['Beta', '1 unmerged · 2 dirty'],
    ]);
    expect(git.series.map(series => series.points.map(point => point.value))).toEqual([[2, 3], [null, 1]]);
  });
});
