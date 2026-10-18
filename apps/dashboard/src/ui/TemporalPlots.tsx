import { useState } from 'react';
import { data, list, number, text, type Data, type LegacySnapshot } from '../legacy';
import './temporal-plots.css';

type Point = { time: number; value: number | null };
type Value = { label: string; value: string; color?: string };
export type TemporalSeries = { id: string; label: string; color: string; points: Point[] };
type Bar = { label: string; value: number | null; color: string };
type CiRow = { id: string; label: string; color: string; points: { time: number; state: string }[] };
export type TemporalMetric = {
  id: string; title: string; values: Value[]; note: string; sourceTime: string;
  series: TemporalSeries[]; kind?: 'stacked' | 'bars' | 'ci'; bars?: Bar[]; ci?: CiRow[];
  secondary?: TemporalSeries[]; min?: number; max?: number;
};
const projectColors = ['#39e065', '#35d9e5', '#ffd23f', '#bf83ff', '#58a6ff', '#ff7184', '#ffa657'];
const tierColors: Record<string, string> = { luna: '#39e065', sol: '#f2c335', astra: '#ff535c', claude: '#bf83ff', agy: '#58a6ff', other: '#8b949e' };
const tierLabels: Record<string, string> = { luna: 'Luna', sol: 'Sol', astra: 'Astra', claude: 'Claude', agy: 'agy', other: 'other' };
const unique = (values: string[]) => [...new Set(values.filter(Boolean))];
const numeric = (value: unknown) => number(value);
const format = (value: unknown, suffix = '') => numeric(value) === null ? 'unknown' : `${value}${suffix}`;
const compact = (value: unknown): string => {
  const n = numeric(value);
  return n === null ? 'unknown' : n >= 1e9 ? `${(n / 1e9).toFixed(2)}B` : n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${(n / 1e3).toFixed(0)}k` : String(n);
};
const utc = (value: number) => new Date(value * 1000).toISOString();
const sourceTime = (row: Data) => numeric(row.ts) !== null && Number.isFinite(new Date(Number(row.ts) * 1000).getTime()) ? utc(Number(row.ts)) : '';
const ciColor = (state: string) => state === 'success' ? '#39e065' : state === 'failure' ? '#ff535c' : state === 'pending' ? '#f2c335' : '#4b5563';
const idColor = (id: string) => projectColors[Array.from(id).reduce((hash, char) => (hash * 31 + char.charCodeAt(0)) >>> 0, 0) % projectColors.length];

/** Uses the legacy collector's actual ts/proj/hosts/tiers/hyg/ty schema. No observation is inferred from absence. */
export function buildTemporalMetrics(snapshot: LegacySnapshot): { metrics: TemporalMetric[]; rows: Data[]; note: string; omittedRows: number } {
  const history = data(snapshot.history), sourceRows = list(history.rows).map(data);
  const validRows = sourceRows.filter(row => sourceTime(row)).sort((a, b) => Number(a.ts) - Number(b.ts));
  const rows = validRows.slice(-4096), last = rows.at(-1) ?? {}, historicalTime = sourceTime(last);
  const projects = unique([...(snapshot.order ?? []), ...Object.keys(snapshot.projects), ...rows.flatMap(row => Object.keys(data(row.proj)))]);
  const hosts = unique([...rows.flatMap(row => Object.keys(data(row.hosts))), ...list(data(snapshot.global_resources).hosts).map(host => text(data(host).host))]);
  const te = data(snapshot.token_efficiency), ledger = data(te.ledger_stats), funnel = data(ledger.funnel);
  const yieldSource = data(snapshot.token_yield), total = data(yieldSource.total), quota = data(te.quota);
  const projectName = (id: string) => text(data(snapshot.projects[id]).name) || id;
  const projectColor = (id: string) => projectColors[projects.indexOf(id) % projectColors.length] ?? idColor(id);
  const series = (ids: string[], get: (row: Data, id: string) => unknown, labels = (id: string) => id, colors = idColor): TemporalSeries[] =>
    ids.map(id => ({ id, label: labels(id), color: colors(id), points: rows.map(row => ({ time: Number(row.ts), value: numeric(get(row, id)) })) }));
  const single = (id: string, label: string, color: string, get: (row: Data) => unknown): TemporalSeries[] =>
    [{ id, label, color, points: rows.map(row => ({ time: Number(row.ts), value: numeric(get(row)) })) }];
  const pv = (get: (project: Data) => string): Value[] => projects.map(id => ({ label: projectName(id), value: get(data(snapshot.projects[id])), color: projectColor(id) }));
  const hv = (field: string): Value[] => hosts.map(id => ({ label: id, value: format(data(data(last.hosts)[id])[field], '%'), color: idColor(id) }));
  const make = (id: string, title: string, values: Value[], graph: TemporalSeries[], note: string, at = historicalTime): TemporalMetric =>
    ({ id, title, values, series: graph, note, sourceTime: at, min: 0 });
  const tiers = unique(['luna', 'sol', 'astra', 'claude', 'agy', ...rows.flatMap(row => Object.keys(data(row.tiers)))]);
  const hygiene = [...rows].reverse().find(row => Object.keys(data(row.hyg)).length) ?? {};
  const quotaSeries = single('quota', 'used quota', '#58a6ff', row => row.quota_pct);
  const samples = list(quota.history).filter(Array.isArray).flatMap(row => {
    const time = typeof row[0] === 'string' ? Date.parse(row[0]) / 1000 : NaN;
    return Number.isFinite(time) ? [{ time, value: numeric(row[1]) }] : [];
  }).sort((a, b) => a.time - b.time);
  // The quota source has its own timestamped stream, often newer than the two-minute fleet history.
  if (samples.length) quotaSeries[0].points = samples;
  const metrics: TemporalMetric[] = [
    make('cost-index', 'Cost index vs all-Luna (1.0 = all Luna)', [{ label: 'token-weighted cost index', value: format(last.cost_index, '×'), color: '#f2c335' }],
      single('cost', 'cost index', '#f2c335', row => row.cost_index), text(last.cost_basis) || 'Cost basis unknown'),
    { ...make('luna-failures', 'Luna failures → escalated · first-pass', [
      { label: 'failures', value: `${format(ledger.luna_failures_total)} total`, color: '#ff535c' },
      { label: 'first-pass', value: `${format(ledger.luna_first_pass_rate, '%')} first-pass`, color: '#39e065' },
    ], [], `${format(ledger.attempts)} ledger attempts · hourly failures, last 24h · lower line: first-pass %`, snapshot.generated_utc), kind: 'bars',
    bars: list(ledger.luna_failures_per_hour_24h).map((value, index, all) => ({ label: `${all.length - index}h bucket before source`, value: numeric(value), color: '#ff535c' })),
    secondary: single('first-pass', 'first-pass %', '#39e065', row => row.luna_fp_rate) },
    make('burn-up', 'ToDos done % of total · burn-up', pv(project => `${format(project.done)}/${format(project.total)}`),
      series(projects, (row, id) => { const p = data(data(row.proj)[id]), done = numeric(p.done), count = numeric(p.total); return done !== null && count !== null && count > 0 ? 100 * done / count : null; }, projectName, projectColor),
      'Producer done/total · missing or zero denominator remains unknown', snapshot.generated_utc),
    make('in-progress', 'In progress per project', pv(project => format(data(project.counts).in_progress)),
      series(projects, (row, id) => data(data(row.proj)[id]).ip, projectName, projectColor), 'Producer task counts', snapshot.generated_utc),
    { ...make('agent-tiers', 'Agents by tier · stacked reported counts', tiers.map(id => ({ label: tierLabels[id] ?? id, value: format(data(last.tiers)[id]), color: tierColors[id] ?? idColor(id) })),
      series(tiers, (row, id) => data(row.tiers)[id], id => tierLabels[id] ?? id, id => tierColors[id] ?? idColor(id)), 'Missing tier counts are unknown; stacks include reported counts only'), kind: 'stacked' },
    { ...make('escalation', 'Escalation funnel · tasks', [
      { label: 'Luna tasks', value: `Luna ${format(funnel.luna_tasks)}`, color: '#39e065' },
      { label: 'Luna → Sol', value: `→ Sol ${format(funnel.luna_to_sol)}`, color: '#f2c335' },
      { label: 'Sol → Astra', value: `→ Astra ${format(funnel.sol_to_astra)}`, color: '#ff535c' },
    ], [], 'Task transitions from the model ledger; this funnel is not a time series', snapshot.generated_utc), kind: 'bars',
    bars: [ ['Luna tasks', funnel.luna_tasks, '#39e065'], ['Luna → Sol', funnel.luna_to_sol, '#f2c335'], ['Sol → Astra', funnel.sol_to_astra, '#ff535c'] ].map(([label, value, color]) => ({ label: String(label), value: numeric(value), color: String(color) })) },
    make('cpu-load', 'Load % of CPUs', hv('load'), series(hosts, (row, id) => data(data(row.hosts)[id]).load), 'Normalized CPU load from each recorded host'),
    make('root-disk', 'Root disk % used', hv('disk'), series(hosts, (row, id) => data(data(row.hosts)[id]).disk), 'Root filesystem use · source hosts discovered dynamically'),
    { ...make('integration-ci', 'Integration PR CI history', projects.map(id => {
      const state = text(data(data(data(snapshot.projects[id]).pr).ci).state) || 'unknown';
      return { label: projectName(id), value: state, color: ciColor(state) };
    }), [], 'Rows retain project identity · green pass · red fail · amber pending · gray unknown', snapshot.generated_utc), kind: 'ci',
    ci: projects.map(id => ({ id, label: projectName(id), color: projectColor(id), points: rows.map(row => ({ time: Number(row.ts), state: text(data(data(row.proj)[id]).ci) || 'unknown' })) })) },
    { ...make('hygiene', 'Hygiene score per Commodore', projects.map(id => ({ label: projectName(id), value: format(data(hygiene.hyg)[id]), color: projectColor(id) })),
      series(projects, (row, id) => data(row.hyg)[id], projectName, projectColor), 'Unscored Commodores remain unknown', sourceTime(hygiene)), max: 100 },
    make('tokens', 'Tokens · generated / integration / main', [
      { label: 'generated', value: compact(total.tokens), color: '#c1c9d2' },
      { label: 'integration', value: compact(total.integration_tokens), color: '#f2c335' },
      { label: 'main', value: compact(total.main_tokens), color: '#39e065' },
    ], [...single('generated', 'generated', '#c1c9d2', row => data(row.ty).tok), ...single('integration', 'integration', '#f2c335', row => data(row.ty).int), ...single('main', 'main', '#39e065', row => data(row.ty).main)],
    `${format(yieldSource.window_hours)}h window · ${text(yieldSource.method) || 'Attribution method unknown'}`, text(yieldSource.generated_utc)),
    { ...make('token-yield', 'Token yield % · estimate / coverage', [
      { label: 'main tokens', value: `main ${format(total.yield_main_pct, '%')}`, color: '#39e065' },
      { label: 'integration tokens', value: `int ${format(total.yield_integration_pct, '%')}`, color: '#f2c335' },
      { label: 'coverage', value: `cov ${format(total.coverage_pct, '%')}`, color: '#a3adba' },
    ], [...single('yield-main', 'main tokens', '#39e065', row => data(row.ty).ym), ...single('yield-integration', 'integration tokens', '#f2c335', row => data(row.ty).yi), ...single('coverage', 'coverage', '#a3adba', row => data(row.ty).cov)],
    `Estimated credit yield: main ${format(total.yield_main_credits_pct, '%')} · integration ${format(total.yield_integration_credits_pct, '%')}. Curves show token yield and coverage; attribution is not acceptance.`, text(yieldSource.generated_utc)), max: 100 },
    { ...make('weekly-quota', 'Codex weekly quota used % · real', [
      { label: 'quota used', value: format(quota.used_percent, '%'), color: '#58a6ff' },
      { label: 'credits per hour estimate', value: `${format(last.credits_h)} cr/h est.`, color: '#c1c9d2' },
    ], quotaSeries, `Resets ${text(quota.resets_at_utc) || 'unknown'} · credits/hour is an estimate from the fleet history, not a quota balance`, text(quota.observed_utc)), max: 100 },
  ];
  return { metrics, rows, note: text(history.note), omittedRows: Math.max(0, validRows.length - rows.length) };
}

function extent(series: TemporalSeries[], min?: number, max?: number) {
  let x0 = Infinity, x1 = -Infinity, y0 = min ?? Infinity, y1 = max ?? -Infinity;
  for (const value of series) for (const point of value.points) if (point.value !== null) {
    x0 = Math.min(x0, point.time); x1 = Math.max(x1, point.time);
    y0 = Math.min(y0, point.value); y1 = Math.max(y1, point.value);
  }
  if (x0 === Infinity) return null;
  if (x0 === x1) { x0 -= 1; x1 += 1; }
  if (y0 === y1) { y0 = Math.min(0, y0); y1 += 1; }
  return { x: (time: number) => 3 + (time - x0) / (x1 - x0) * 294, y: (value: number) => 57 - (value - y0) / (y1 - y0) * 52 };
}
function Spark({ series, title, min, max, small = false }: { series: TemporalSeries[]; title: string; min?: number; max?: number; small?: boolean }) {
  const scale = extent(series, min, max);
  if (!scale) return <div className="temporal-empty">history unknown</div>;
  return <svg className={small ? 'temporal-spark temporal-spark--secondary' : 'temporal-spark'} viewBox="0 0 300 62" preserveAspectRatio="none" role="img" aria-label={title}>
    <title>{title}</title><path className="temporal-grid" d="M0 57H300 M0 31H300 M0 5H300" />
    {series.map(value => {
      const segments: Point[][] = []; let segment: Point[] = [];
      for (const point of value.points) { if (point.value === null) { if (segment.length) segments.push(segment); segment = []; } else segment.push(point); }
      if (segment.length) segments.push(segment);
      return <g key={value.id} data-series-id={value.id}><title>{`${value.label}: ${value.points.filter(point => point.value !== null).length} recorded samples; gaps are unknown`}</title>
        {segments.map((points, index) => points.length === 1
          ? <circle key={index} cx={scale.x(points[0].time)} cy={scale.y(points[0].value!)} r="2.3" fill={value.color} />
          : <polyline key={index} points={points.map(point => `${scale.x(point.time).toFixed(2)},${scale.y(point.value!).toFixed(2)}`).join(' ')} fill="none" stroke={value.color} strokeWidth="1.8" vectorEffect="non-scaling-stroke" />)}</g>;
    })}
  </svg>;
}
function Bars({ bars, title }: { bars: Bar[]; title: string }) {
  const known = bars.filter(bar => bar.value !== null);
  if (!known.length) return <div className="temporal-empty">ledger history unknown</div>;
  const max = Math.max(1, ...known.map(bar => bar.value!)), width = 300 / bars.length;
  return <svg className="temporal-spark" viewBox="0 0 300 62" preserveAspectRatio="none" role="img" aria-label={title}><title>{title}</title>
    <path className="temporal-grid" d="M0 57H300" />{bars.map((bar, index) => bar.value === null
      ? <text key={index} x={(index + .5) * width} y="55" textAnchor="middle" fill="#8b949e" fontSize="9">?<title>{`${bar.label}: unknown`}</title></text>
      : <rect key={index} x={index * width + 1} width={Math.max(.5, width - 2)} y={57 - bar.value / max * 52} height={bar.value === 0 ? .7 : bar.value / max * 52} fill={bar.color}><title>{`${bar.label}: ${bar.value}`}</title></rect>)}
  </svg>;
}
function Stacked({ series, title }: { series: TemporalSeries[]; title: string }) {
  const rows = series[0]?.points ?? [];
  const totals = rows.map((_, index) => series.reduce((sum, item) => sum + (item.points[index]?.value ?? 0), 0));
  if (!series.some(item => item.points.some(point => point.value !== null))) return <div className="temporal-empty">tier history unknown</div>;
  const max = Math.max(1, ...totals), width = 300 / Math.max(1, rows.length);
  return <svg className="temporal-spark" viewBox="0 0 300 62" preserveAspectRatio="none" role="img" aria-label={`${title}; only reported counts are stacked; missing tiers are unknown`}><title>{`${title} · reported counts only`}</title>
    {rows.flatMap((row, index) => { let base = 0; return series.map(item => {
      const value = item.points[index]?.value;
      if (value === undefined || value === null) return null;
      base += value;
      return <rect key={`${index}-${item.id}`} data-series-id={item.id} x={index * width} width={width + .1} y={57 - base / max * 52} height={value / max * 52} fill={item.color} opacity=".92"><title>{`${item.label}: ${value} · ${utc(row.time)}; unknown tiers excluded from stack`}</title></rect>;
    }).filter(Boolean); })}
  </svg>;
}
function CiHistory({ rows }: { rows: CiRow[] }) {
  if (!rows.some(row => row.points.length)) return <div className="temporal-empty">CI history unknown</div>;
  const height = 62 / Math.max(1, rows.length);
  return <svg className="temporal-spark temporal-ci" viewBox="0 0 300 62" preserveAspectRatio="none" role="img" aria-label="Integration PR CI history; rows retain project identity; missing states are unknown"><title>Integration PR CI history</title>
    {rows.map((row, rowIndex) => <g key={row.id} data-project-id={row.id}><title>{row.label}</title>{row.points.map((point, index) => <rect key={index} x={index * 300 / row.points.length} width={300 / row.points.length + .1} y={rowIndex * height + 1} height={Math.max(1, height - 2)} fill={ciColor(point.state)}><title>{`${row.label}: ${point.state} · ${utc(point.time)}`}</title></rect>)}</g>)}
  </svg>;
}
function MetricCard({ metric, first, last }: { metric: TemporalMetric; first: string; last: string }) {
  const [expanded, setExpanded] = useState(false);
  const legend = metric.kind === 'ci' ? metric.ci?.map(row => ({ id: row.id, label: row.label, color: row.color })) : metric.series;
  return <article className={`temporal-plot${expanded ? ' temporal-plot--expanded' : ''}`} data-metric={metric.id}>
    <button className="temporal-toggle" type="button" aria-expanded={expanded} onClick={() => setExpanded(value => !value)} title={`${metric.note}\nSource: ${metric.sourceTime || 'unknown'}\nHistory: ${first || 'unknown'} → ${last || 'unknown'}`}>
      <span className="temporal-title">{metric.title}<span className="temporal-expand" data-icon={expanded ? '−' : '+'} aria-hidden="true" /></span>
      <span className="temporal-values">{metric.values.length ? metric.values.map(value => <span key={value.label} title={value.label} style={{ color: value.color }}>{value.value}</span>) : <span>unknown</span>}</span>
      <span className="temporal-chart">{metric.kind === 'bars' ? <Bars bars={metric.bars ?? []} title={metric.title} /> : metric.kind === 'stacked' ? <Stacked series={metric.series} title={metric.title} /> : metric.kind === 'ci' ? <CiHistory rows={metric.ci ?? []} /> : <Spark series={metric.series} title={metric.title} min={metric.min} max={metric.max} />}
        {metric.secondary && <Spark series={metric.secondary} title="Luna first-pass percentage history" min={0} max={100} small />}</span>
      <span className="temporal-legend">{legend?.map(value => <span key={value.id}><i style={{ background: value.color }} />{value.label}</span>)}</span>
      <span className="temporal-source">as of {metric.sourceTime ? <time dateTime={metric.sourceTime} title={metric.sourceTime}>{metric.sourceTime.slice(11, 19)} UTC</time> : 'unknown'}</span>
    </button>
    {expanded && <div className="temporal-explanation"><p>{metric.note}</p><p>Source <time dateTime={metric.sourceTime || undefined}>{metric.sourceTime || 'unknown'}</time></p><p>History {first || 'unknown'} → {last || 'unknown'}. Missing observations remain unknown.</p></div>}
  </article>;
}

export function TemporalPlots({ snapshot }: { snapshot: LegacySnapshot }) {
  const { metrics, rows, note, omittedRows } = buildTemporalMetrics(snapshot);
  const first = sourceTime(rows[0] ?? {}), last = sourceTime(rows.at(-1) ?? {});
  return <section className="temporal-plots" aria-label="History and trends · fleet temporal metrics">
    <div className="temporal-heading"><strong>Live trends</strong><span>{rows.length} historical samples · click a metric to expand · scroll for all {metrics.length}</span><span title={note}>History <time dateTime={first || undefined}>{first || 'unknown'}</time> → <time dateTime={last || undefined}>{last || 'unknown'}</time>{omittedRows ? ` · ${omittedRows} earlier rows outside display bound` : ''}</span></div>
    <div className="temporal-strip" tabIndex={0} role="group" aria-label="Scrollable temporal metric cards">{metrics.map(metric => <MetricCard key={metric.id} metric={metric} first={first} last={last} />)}</div>
  </section>;
}
