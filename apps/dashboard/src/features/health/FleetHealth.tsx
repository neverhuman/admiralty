import { useEffect, useMemo, useState } from 'react';
import { freshness, watchLegacy, type LegacySnapshot } from '../../legacy';
import { formatMeasurement } from '../../formatNumbers';
import { routeHref } from '../../router';
import {
  age, countText, findings, gitRows, hostRows, parsePrometheus,
  type DiskRow, type Finding, type GitRow, type HostRow, type MetricsSummary, type Readiness,
} from './health';
import './health.css';

const n = (value: number | null, digits = 1) => value === null ? 'unknown' : formatMeasurement(value, digits);
const severityLabel: Record<Finding['severity'], string> = { critical: 'Critical', warning: 'Warning', info: 'Note' };
/** Findings shown before "Show more"; critical and warning items sort first. */
const ATTENTION_LIMIT = 8;

/** Poll a small text endpoint; returns status and body, or an error string.
 *  A 401/403 stops polling until the token changes (a refused token will not
 *  start working on its own), and a hidden tab skips requests until it is shown. */
function usePoll(path: string | null, token: string | undefined, intervalMs: number, nonce = 0) {
  const [state, setState] = useState<{ status: number; body: string; error: string; at: number } | null>(null);
  useEffect(() => {
    setState(null);
    if (!path) return;
    let stopped = false, timer: ReturnType<typeof setTimeout> | undefined, controller: AbortController | undefined;
    const poll = async () => {
      if (typeof document !== 'undefined' && document.hidden) { timer = setTimeout(() => void poll(), intervalMs); return; }
      controller = new AbortController();
      const deadline = setTimeout(() => controller?.abort(), 10_000);
      let refused = false;
      try {
        const response = await fetch(path, { signal: controller.signal, cache: 'no-store', headers: token ? { Authorization: `Bearer ${token}` } : {} });
        refused = response.status === 401 || response.status === 403;
        const body = (await response.text()).slice(0, 256 * 1024);
        if (!stopped) setState({ status: response.status, body, error: '', at: Date.now() });
      } catch (error) {
        if (!stopped) setState({ status: 0, body: '', error: error instanceof Error && error.name !== 'AbortError' ? error.message : 'Request timed out', at: Date.now() });
      } finally {
        clearTimeout(deadline);
        if (!stopped && !refused) timer = setTimeout(() => void poll(), intervalMs);
      }
    };
    void poll();
    return () => { stopped = true; controller?.abort(); clearTimeout(timer); };
  }, [path, token, intervalMs, nonce]);
  return state;
}

export function readinessFrom(state: { status: number; body: string; error: string } | null): Readiness {
  if (!state) return { status: 'checking', detail: 'Checking readiness…' };
  if (state.status === 0) return { status: 'unreachable', detail: `Readiness check failed: ${state.error || 'no response'}.` };
  if (state.status === 404) return { status: 'checking', detail: 'This server predates /readyz.' };
  try {
    const body = JSON.parse(state.body) as { status?: string; checks?: Record<string, unknown> };
    if (state.status === 200 && body.status === 'ready') return { status: 'ready', detail: `Store answered in ${String(body.checks?.store_ms ?? '?')} ms.` };
    const store = String(body.checks?.store ?? 'unknown'), draining = body.checks?.draining === true;
    return { status: 'not_ready', detail: draining ? 'Service is draining for shutdown.' : `Store check: ${store}.` };
  } catch {
    return { status: 'not_ready', detail: `Readiness returned HTTP ${state.status}.` };
  }
}

export function Attention({ items, observed }: { items: Finding[]; observed: string }) {
  const [expanded, setExpanded] = useState(false);
  const shown = expanded ? items : items.slice(0, ATTENTION_LIMIT);
  return <section className="health-panel health-attention" aria-labelledby="health-attention-title">
    <h2 id="health-attention-title">Needs attention</h2>
    {items.length === 0
      ? <p className="health-quiet">Nothing needs attention. {observed ? `Last observation ${freshness(observed)}.` : 'Waiting for the first observation.'}</p>
      : <><ol className="health-findings">{shown.map((item, index) => <li key={`${item.subject}:${item.message}:${index}`} className={`health-finding health-${item.severity}`}>
        <span className="health-severity"><i aria-hidden="true" />{severityLabel[item.severity]}</span>
        <span className="health-finding-body"><b>{item.subject}</b> {item.message}<small>{item.action}{item.href && <> <a href={item.href}>Open Git Flow</a></>}</small></span>
      </li>)}</ol>
        {items.length > shown.length && <button className="text-button health-more" onClick={() => setExpanded(true)}>Show {items.length - shown.length} more</button>}</>}
  </section>;
}

function Meter({ label, used, projected, valueText, tone }: { label: string; used: number | null; projected?: number | null; valueText: string; tone: string }) {
  const value = used === null ? 0 : Math.min(100, Math.max(0, used));
  const ahead = projected === null || projected === undefined ? 0 : Math.max(0, Math.min(100, projected) - value);
  return <div className={`health-meter health-tone-${tone}`}>
    <span className="health-meter-label" title={label}>{label}</span>
    <span className="health-meter-track" role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={used === null ? undefined : Math.round(value)} aria-valuetext={valueText}>
      {used !== null && <i className="health-meter-fill" style={{ width: `${value}%` }} />}
      {ahead > 0 && <i className="health-meter-projection" style={{ left: `${value}%`, width: `${ahead}%` }} />}
    </span>
    <span className="health-meter-value">{valueText}</span>
  </div>;
}

function diskTone(disk: DiskRow) {
  return disk.level === 'red' ? 'red' : disk.level === 'amber' ? 'amber' : 'ok';
}

function DiskMeter({ disk }: { disk: DiskRow }) {
  const projected = disk.pct !== null && disk.growthGbPerHour !== null && disk.size
    ? disk.pct + (disk.growthGbPerHour * 24 * 1e9) / disk.size * 100 : null;
  const growth = disk.growthGbPerHour === null ? 'growth unknown'
    : disk.growthGbPerHour <= 0 ? 'not growing' : `+${formatMeasurement(disk.growthGbPerHour, 2)} GB/h`;
  const full = disk.hoursToFull === null ? '' : `, full in ${formatMeasurement(disk.hoursToFull, 1)} h`;
  return <Meter label={disk.mount} used={disk.pct} projected={projected} tone={diskTone(disk)}
    valueText={`${n(disk.pct)}% · ${disk.avail === null ? '?' : formatMeasurement(disk.avail / 1e9, 0)} GB free · ${growth}${full}`} />;
}

export function Host({ host }: { host: HostRow }) {
  if (!host.reachable) return <article className="health-host health-host-down"><h3>{host.id}</h3><p>Unreachable: {host.error || 'reason unknown'}. No values are shown rather than guessed.</p></article>;
  const memUsed = host.memAvailPct === null ? null : 100 - host.memAvailPct;
  return <article className="health-host">
    <h3>{host.id}<small>{host.nproc === null ? '' : `${host.nproc} CPUs`}{host.dState ? ` · ${host.dState} blocked in I/O` : ''}</small></h3>
    <Meter label="CPU load" used={host.loadPct} tone={host.loadPct !== null && host.loadPct > 100 ? 'red' : host.loadPct !== null && host.loadPct > 85 ? 'amber' : 'ok'}
      valueText={host.load1 === null ? 'unknown' : `${formatMeasurement(host.load1, 1)} on ${host.nproc ?? '?'} CPUs`} />
    <Meter label="Memory" used={memUsed} tone={host.memAvailPct !== null && host.memAvailPct < 8 ? 'red' : host.memAvailPct !== null && host.memAvailPct < 15 ? 'amber' : 'ok'}
      valueText={`${n(host.memAvailGb)} of ${n(host.memTotalGb, 0)} GB free${host.swapTotalGb ? ` · swap ${n(host.swapUsedGb)}/${n(host.swapTotalGb, 0)} GB` : ''}`} />
    {host.disks.map(disk => <DiskMeter key={disk.mount} disk={disk} />)}
    {host.pressure && <p className="health-pressure" title="Pressure stall information, 60-second average: share of time tasks waited">
      Stalled time, last minute: CPU {n(host.pressure.cpuSome)}% · memory {n(host.pressure.memoryFull)}% · I/O {n(host.pressure.ioFull)}%</p>}
    {host.gpus.map(gpu => <p className="health-gpu" key={gpu.index}>GPU {gpu.index} {gpu.name}: {n(gpu.util, 0)}% busy · {n(gpu.memUsed, 0)}/{n(gpu.memTotal, 0)} MiB · {n(gpu.temp, 0)} °C</p>)}
    {host.top.length > 0 && <details className="health-top"><summary>Largest processes</summary><table><caption className="sr-only">Largest processes on {host.id} by resident memory</caption>
      <thead><tr><th scope="col">Process</th><th scope="col">User</th><th scope="col" className="num">Memory</th><th scope="col" className="num">CPU</th></tr></thead>
      <tbody>{host.top.map(item => <tr key={item.pid}><td>{item.comm} <small>{item.pid}</small></td><td>{item.user}</td><td className="num">{n(item.rssGb)} GB</td><td className="num">{n(item.pcpu, 0)}%</td></tr>)}</tbody></table></details>}
  </article>;
}

function stateClass(state: string) {
  return state === 'healthy' ? 'ok' : state === 'attention' ? 'amber' : state === 'unknown' ? 'muted' : 'off';
}

export function GitTable({ rows }: { rows: GitRow[] }) {
  if (!rows.length) return <p className="health-quiet">No Git health reports yet. Configure <code>git_health.integration_ref</code> per project in the observer.</p>;
  return <div className="health-table-scroll"><table className="health-git">
    <caption className="sr-only">Git hygiene by project. Counts marked ≥ are lower bounds.</caption>
    <thead><tr>
      <th scope="col">Project</th><th scope="col">State</th>
      <th scope="col" className="num" title="Local branches with work not in integration">Unmerged</th>
      <th scope="col" className="num" title="Commits the most-behind unmerged lane is missing">Lane lag</th>
      <th scope="col" className="num" title="Local branches fully contained in integration">Merged</th>
      <th scope="col" className="num" title="Worktrees with uncommitted files">Dirty</th>
      <th scope="col" className="num" title="Linked worktrees that are clean and fully merged">Removable</th>
      <th scope="col" className="num" title="First-parent commits on integration in 7 days">Commits 7d</th>
      <th scope="col" className="num" title="Merge / revert / fixup commits on integration in 7 days">M / R / F</th>
      <th scope="col" className="num" title="Age of the integration tip commit">Tip age</th>
    </tr></thead>
    <tbody>{rows.map(row => <tr key={row.id}>
      <th scope="row"><a href={routeHref('git', { project: row.id })}>{row.name}</a><small>{row.integrationRef.replace(/^refs\/(heads|remotes)\//, '') || 'no policy'}</small></th>
      <td data-label="State"><span className={`health-state health-tone-${stateClass(row.state)}`}>{row.state}</span>{row.issues.length > 0 && <small>{row.issues.join(', ')}</small>}</td>
      <td className="num" data-label="Unmerged">{n(row.unmerged, 0)}{row.diverged ? <small>{row.diverged} diverged</small> : null}</td>
      <td className="num" data-label="Lane lag">{countText(row.maxBehind)}{row.oldestUnmergedAgeS !== null && <small>oldest {age(row.oldestUnmergedAgeS)}</small>}</td>
      <td className="num" data-label="Merged">{n(row.merged, 0)}</td>
      <td className="num" data-label="Dirty">{n(row.dirty, 0)}{row.conflicted ? <small>{row.conflicted} conflicted</small> : null}</td>
      <td className="num" data-label="Removable" title={row.cleanup?.join('\n')}>{row.cleanup === null ? 'unknown' : row.cleanup.length}</td>
      <td className="num" data-label="Commits 7d">{countText(row.commits7d)}<small>{countText(row.commits24h)} in 24 h</small></td>
      <td className="num" data-label="M / R / F">{countText(row.merges7d)} / {countText(row.reverts7d)} / {countText(row.fixups7d)}</td>
      <td className="num" data-label="Tip age">{age(row.tipAgeS)}</td>
    </tr>)}</tbody>
  </table></div>;
}

function Service({ readiness, metrics, metricsStatus, onAccess }: { readiness: Readiness; metrics: MetricsSummary | null; metricsStatus: number | null; onAccess: () => void }) {
  return <section className="health-panel health-service" aria-labelledby="health-service-title">
    <h2 id="health-service-title">Service</h2>
    <p className={`health-ready health-tone-${readiness.status === 'ready' ? 'ok' : readiness.status === 'checking' ? 'muted' : 'red'}`} role="status">
      <i aria-hidden="true" />{readiness.status === 'ready' ? 'Ready' : readiness.status === 'checking' ? 'Checking' : readiness.status === 'unreachable' ? 'Unreachable' : 'Not ready'}
      <small>{readiness.detail}</small></p>
    {metricsStatus === 401 || metricsStatus === 503
      ? <p className="health-quiet">Request metrics need session access. <button className="text-button" onClick={onAccess}>Add access token</button></p>
      : metrics && metrics.routes.length > 0
        ? <table className="health-routes"><caption className="sr-only">Busiest routes since the server started</caption>
          <thead><tr><th scope="col">Route</th><th scope="col" className="num">Requests</th><th scope="col" className="num">Errors</th><th scope="col" className="num" title="95th percentile falls at or below this latency">p95 ≤</th></tr></thead>
          <tbody>{metrics.routes.slice(0, 6).map(route => <tr key={route.route}><td><code>{route.route}</code></td><td className="num">{formatMeasurement(route.requests, 0)}</td>
            <td className="num">{route.serverErrors ? <b className="health-tone-red">{route.serverErrors}</b> : 0}{route.clientErrors ? <small>{route.clientErrors} 4xx</small> : null}</td>
            <td className="num">{route.p95LeSeconds === null ? '> 10 s' : route.p95LeSeconds < 1 ? `${formatMeasurement(route.p95LeSeconds * 1000, 0)} ms` : `${formatMeasurement(route.p95LeSeconds, 1)} s`}</td></tr>)}</tbody></table>
        : <p className="health-quiet">{metricsStatus === 404 ? 'This server predates /metrics.' : 'No request metrics yet.'}</p>}
    {metrics && metrics.gauges.admiralty_uptime_seconds !== undefined && <p className="health-footnote">Up {age(metrics.gauges.admiralty_uptime_seconds)} · {metrics.gauges.admiralty_event_streams_active ?? 0} of {metrics.gauges.admiralty_event_streams_limit ?? '?'} live streams · {metrics.gauges.admiralty_read_permits_available ?? '?'} read slots free</p>}
  </section>;
}

function Digest({ token, onAccess }: { token?: string; onAccess: () => void }) {
  const [budget, setBudget] = useState(600), [level, setLevel] = useState('info'), [copied, setCopied] = useState(false);
  const path = `/v1/logs/digest?budget_tokens=${budget}&min_level=${level}`;
  const state = usePoll(path, token, 30_000);
  const copy = async () => {
    if (!state?.body) return;
    try { await navigator.clipboard.writeText(state.body); setCopied(true); setTimeout(() => setCopied(false), 2000); } catch { setCopied(false); }
  };
  return <section className="health-panel health-digest" aria-labelledby="health-digest-title">
    <div className="health-panel-head"><h2 id="health-digest-title">Log digest for agents</h2>
      <label>Budget <select value={budget} onChange={event => setBudget(Number(event.target.value))}>{[300, 600, 1200, 2400].map(value => <option key={value} value={value}>{value} tokens</option>)}</select></label>
      <label>Level <select value={level} onChange={event => setLevel(event.target.value)}>{['error', 'warn', 'info', 'debug'].map(value => <option key={value} value={value}>{value}</option>)}</select></label>
      <button className="secondary-button" onClick={copy} disabled={!state || state.status !== 200}>{copied ? 'Copied' : 'Copy digest'}</button></div>
    {!state ? <p className="health-quiet">Loading digest…</p>
      : state.status === 401 || state.status === 503 ? <p className="health-quiet">The digest needs session access. <button className="text-button" onClick={onAccess}>Add access token</button></p>
        : state.status === 404 ? <p className="health-quiet">This server predates log digests.</p>
          : state.status !== 200 ? <p className="health-quiet">Digest unavailable ({state.error || `HTTP ${state.status}`}).</p>
            : <pre className="health-digest-text" tabIndex={0} role="region" aria-label="Log digest text">{state.body}</pre>}
    <p className="health-footnote">Grouped by event code with repeats counted (I×6 means six info events), within the token budget. Ranges such as ms=3..80 summarize varying values; a trailing ? marks a field missing from some events. Paste it into an agent instead of raw logs.</p>
  </section>;
}

export default function FleetHealth({ token, onAccess }: { token?: string; onAccess: () => void }) {
  const [snapshot, setSnapshot] = useState<LegacySnapshot | null>(null), [error, setError] = useState(''), [loaded, setLoaded] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => { setSnapshot(null); setLoaded(false); setError('');
    return watchLegacy({ token, view: 'detail', onSnapshot: value => { setSnapshot(value); setLoaded(true); }, onError: value => { setError(value); if (value) setLoaded(true); } }); }, [token]);
  useEffect(() => { const timer = setInterval(() => setNow(Date.now()), 15_000); return () => clearInterval(timer); }, []);
  const ready = usePoll('/readyz', undefined, 15_000);
  const metricsState = usePoll('/metrics', token, 30_000);
  const readiness = readinessFrom(ready);
  const metrics = useMemo(() => metricsState?.status === 200 ? parsePrometheus(metricsState.body) : null, [metricsState]);
  const items = useMemo(() => findings(snapshot, readiness, now), [snapshot, readiness.status, readiness.detail, now]);
  const git = useMemo(() => snapshot ? gitRows(snapshot) : [], [snapshot]);
  const hosts = useMemo(() => snapshot ? hostRows(snapshot) : [], [snapshot]);
  return <div className="fleet-health">
    {error && <div className="error-banner" role="status">{error} {snapshot ? 'Showing the last observation.' : ''}</div>}
    <div className="health-top-grid">
      <Attention items={items} observed={snapshot?.generated_utc ?? ''} />
      <Service readiness={readiness} metrics={metrics} metricsStatus={metricsState?.status ?? null} onAccess={onAccess} />
    </div>
    <section className="health-panel" aria-labelledby="health-git-title">
      <div className="health-panel-head"><h2 id="health-git-title">Git hygiene</h2><a href={routeHref('git')}>Open Git Flow</a></div>
      {!loaded ? <p className="health-quiet">Loading observations…</p> : !snapshot ? <p className="health-quiet">No producer observations on this server. Start admiralty-observe and pass --legacy-status-file to admiraltyd.</p> : <GitTable rows={git} />}
    </section>
    <section className="health-panel" aria-labelledby="health-hosts-title">
      <div className="health-panel-head"><h2 id="health-hosts-title">Hosts</h2>
        <span className="health-legend"><i className="health-legend-used" aria-hidden="true" />used <i className="health-legend-projected" aria-hidden="true" />next 24 h at the current growth rate</span></div>
      {hosts.length ? <div className="health-hosts">{hosts.map(host => <Host key={host.id} host={host} />)}</div>
        : <p className="health-quiet">{loaded ? 'No host observations reported.' : 'Loading observations…'}</p>}
    </section>
    <Digest token={token} onAccess={onAccess} />
  </div>;
}
