import { useEffect, useMemo, useState } from 'react';
import { ApiError } from '../../api';
import { data, getLegacy, list, text, type LegacySnapshot } from '../../legacy';
import { formatMeasurement } from '../../formatNumbers';
import { routeHref } from '../../router';
import { agentBrief, projectHealth, sourceAge } from './model';
import './operations.css';

export default function Operations({ token }: { token?: string }) {
  const [state, setState] = useState<{ token?: string; snapshot: LegacySnapshot | null; error: string } | null>(null);
  const [retry, setRetry] = useState(0);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    let stopped = false, request: AbortController | undefined, timer: ReturnType<typeof setTimeout> | undefined;
    let inFlight = false, denied = false;
    setState(null);
    async function poll() {
      clearTimeout(timer);
      if (stopped || denied || inFlight || document.visibilityState === 'hidden') return;
      inFlight = true;
      const controller = new AbortController(); request = controller;
      const timeout = setTimeout(() => controller.abort(), 12_000);
      try {
        // The initial projection excludes graph/history/tails; no new collection is triggered.
        const snapshot = await getLegacy(token, controller.signal, undefined, 'initial');
        if (!stopped) setState({ token, snapshot, error: snapshot ? '' : 'Producer diagnostics unavailable. Configure an observation feed.' });
      } catch (failure) {
        denied = failure instanceof ApiError && [401, 403].includes(failure.status);
        if (!stopped) setState(previous => ({ token, snapshot: denied ? null : previous && previous.token === token ? previous.snapshot : null,
          error: denied ? 'Session access required. Apply a token and retry.' : 'Observation request failed. Retaining the last source clock; retrying.' }));
      } finally {
        clearTimeout(timeout); inFlight = false;
        if (!stopped && !denied) timer = setTimeout(poll, 15_000);
      }
    }
    const visible = () => { if (document.visibilityState === 'visible') void poll(); else clearTimeout(timer); };
    void poll();
    document.addEventListener('visibilitychange', visible);
    const clock = setInterval(() => setNow(Date.now()), 10_000);
    return () => { stopped = true; request?.abort(); clearTimeout(timer); clearInterval(clock); document.removeEventListener('visibilitychange', visible); };
  }, [token, retry]);
  const current = state?.token === token ? state : null;
  return <div className="operations">
    {current?.error && <div className="error-banner" role="status">{current.error}<button onClick={() => setRetry(value => value + 1)}>Retry diagnostics</button></div>}
    {current?.snapshot ? <OperationsView snapshot={current.snapshot} now={now} /> : !current?.error && <p role="status">Loading fleet diagnostics…</p>}
  </div>;
}

export function OperationsView({ snapshot, now }: { snapshot: LegacySnapshot; now: number }) {
  const rows = useMemo(() => projectHealth(snapshot, now), [snapshot, now]);
  const brief = useMemo(() => agentBrief(snapshot, rows), [snapshot, rows]);
  const [query, setQuery] = useState(''), [filter, setFilter] = useState('all'), [page, setPage] = useState(0);
  const [copied, setCopied] = useState('');
  useEffect(() => setCopied(''), [brief]);
  const filtered = rows.filter(row => `${row.id} ${row.name}`.toLowerCase().includes(query.toLowerCase()) &&
    (filter === 'all' || filter === 'attention' && row.attention || filter === 'incomplete' && row.incomplete || filter === 'stale' && row.age !== 'current'));
  const lastPage = Math.max(0, Math.ceil(filtered.length / 50) - 1), currentPage = Math.min(page, lastPage);
  const collector = data(snapshot.collector), stages = data(collector.stages_ms);
  const hosts = list(data(snapshot.global_resources).hosts).map(data);
  const metrics = [
    ['Needs attention', rows.filter(row => row.attention).length, 'Proved Git findings'],
    ['Incomplete evidence', rows.filter(row => row.incomplete).length, 'Missing or partial Git observations'],
    ['Source clock gaps', rows.filter(row => row.age !== 'current').length, 'Stale, future or unknown Git clocks'],
    ['Policy disabled', rows.filter(row => row.report?.state === 'disabled').length, 'Integration policy not configured'],
  ] as const;
  return <>
    {snapshot.simulated === true && <p className="simulation-note">SIMULATION · Synthetic observations</p>}
    <div className="ops-source" role="status"><b>Producer source: {sourceAge(snapshot.generated_utc, now, snapshot.interval_s)}</b><time>{snapshot.generated_utc}</time><span>{rows.length} projects in scope</span></div>
    <section className="metric-grid" aria-label="Operations summary">{metrics.map(([label, value, detail]) => <article className="metric-card" key={label}><span>{label}</span><strong>{value}</strong><small>{detail}</small></article>)}</section>
    <section className="panel ops-health" aria-labelledby="ops-health-title">
      <div className="panel-head"><div><div className="eyebrow">GIT / OBSERVED EVIDENCE</div><h2 id="ops-health-title">Fleet health</h2></div><span>Attention first · no repository mutations</span></div>
      <div className="ops-filters"><label>Search projects<input type="search" value={query} onChange={event => { setQuery(event.target.value); setPage(0); }} /></label><label>Show<select value={filter} onChange={event => { setFilter(event.target.value); setPage(0); }}><option value="all">All projects</option><option value="attention">Needs attention</option><option value="incomplete">Incomplete evidence</option><option value="stale">Source clock gaps</option></select></label></div>
      <div className="ops-table-scroll" tabIndex={0} role="region" aria-label="Project Git health"><table><caption>Cached observations; healthy does not establish merge safety.</caption><thead><tr><th scope="col">Project</th><th scope="col">Git state</th><th scope="col">Evidence</th><th scope="col">Source time</th><th scope="col">Inspect</th></tr></thead><tbody>{filtered.slice(currentPage * 50, currentPage * 50 + 50).map(row => <tr key={row.id}><th scope="row">{row.name}<small>{row.id}</small></th><td data-label="Git state"><span className={`ops-state state-${row.report?.state ?? 'unknown'}`}>{row.report?.state ?? 'unknown'}</span>{row.attention && row.report?.state !== 'attention' && <small>Actionable findings</small>}</td><td data-label="Evidence">{row.report ? <><span>{row.report.observed_commits} commits · {row.report.observed_checkouts} checkouts</span><small>{row.incomplete ? 'Incomplete coverage' : row.report.state === 'disabled' ? 'Policy not configured' : 'Complete observed coverage'}</small><small>{row.codes.slice(0, 3).join(' · ')}{row.codes.length > 3 ? ` +${row.codes.length - 3} codes` : ''}</small></> : row.invalid ? 'Invalid report; refresh source' : 'No report observed'}</td><td data-label="Source time"><span>{row.age}</span><small>{row.report?.observed_utc ?? 'Unknown'}</small></td><td data-label="Inspect"><a href={routeHref('git', { project: row.id })}>Open Git ↗</a></td></tr>)}</tbody></table></div>
      {!filtered.length && <p className="ops-empty">No projects match this view.</p>}
      <div className="ops-pagination"><span>{filtered.length} matching projects · page {currentPage + 1} of {lastPage + 1}</span><button disabled={!currentPage} onClick={() => setPage(currentPage - 1)}>Previous</button><button disabled={currentPage === lastPage} onClick={() => setPage(currentPage + 1)}>Next</button></div>
    </section>
    <div className="ops-bottom"><section className="panel" aria-labelledby="ops-pipeline-title"><div className="eyebrow">OBSERVATION PIPELINE</div><h2 id="ops-pipeline-title">Collection cost</h2><dl className="ops-timings"><div><dt>Implementation</dt><dd>{text(collector.implementation) || 'Unknown'} {text(collector.version)}</dd></div><div><dt>Before publication</dt><dd>{formatMeasurement(collector.duration_ms)} ms</dd></div>{['fleet', 'projects', 'git', 'usage', 'history'].map(stage => <div key={stage}><dt>{stage}</dt><dd>{formatMeasurement(stages[stage])} ms</dd></div>)}</dl><p>Stage wall times exclude final serialization, publication and HTTP delivery. Missing timings remain unknown.</p></section>
      <section className="panel" aria-labelledby="ops-host-title"><div className="eyebrow">OBSERVED CAPACITY</div><h2 id="ops-host-title">Host pressure</h2>{hosts.length ? hosts.slice(0, 64).map((host, index) => <div className="ops-host" key={`${text(host.host)}:${index}`}><b>{text(host.host) || 'Unknown host'}</b><span>{host.reachable === true ? 'Reachable at source time' : host.reachable === false ? 'Unreachable at source time' : 'Reachability unknown'}</span><small>Load: {formatMeasurement(list(host.load)[0])} / {formatMeasurement(host.nproc, 0)} CPUs · Available memory: {formatMeasurement(data(host.mem).avail_gb)} GiB</small><small>{Array.isArray(host.warnings) ? host.warnings.length : 'Unknown'} reported pressure warnings · source {sourceAge(snapshot.generated_utc, now, snapshot.interval_s)}</small></div>) : <p>No host measurements observed.</p>}{hosts.length > 64 && <p>{hosts.length - 64} additional hosts omitted from this summary.</p>}</section></div>
    <section className="panel ops-brief" aria-labelledby="ops-brief-title"><div className="panel-head"><div><div className="eyebrow">AGENT CONTEXT</div><h2 id="ops-brief-title">Compact operations brief</h2></div><button onClick={() => { if (!navigator.clipboard) { setCopied('Copy unavailable; select the JSON below.'); return; } void navigator.clipboard.writeText(brief).then(() => setCopied('Copied current brief'), () => setCopied('Copy unavailable; select the JSON below.')); }}>Copy brief</button></div><p>All {rows.length} projects · up to 20 exception records · {new TextEncoder().encode(brief).length.toLocaleString('en-US')} UTF-8 bytes. Omitted exceptions are counted. This summary is lossy; use Git Flow for full evidence.</p><span role="status">{copied}</span><details><summary>Inspect agent JSON</summary><pre>{brief}</pre></details></section>
  </>;
}
