import { useEffect, useState } from 'react';
import { clockState, fetchBrief, HealthAccessError, type FleetBrief } from './api';
import './health.css';

export function HealthReport({ brief, now = Date.now() }: { brief: FleetBrief; now?: number }) {
  const [onlyAttention, setOnlyAttention] = useState(false);
  const [query, setQuery] = useState('');
  const totals = brief.totals;
  const state = clockState(brief.source_clock, now, brief.stale_after_seconds);
  const rows = brief.projects.filter(p => p.id.toLowerCase().includes(query.toLowerCase()) &&
    (!onlyAttention || p.git_state !== 'healthy' || clockState(p.git_clock, now, brief.stale_after_seconds) !== 'fresh' || (p.dirty_checkouts ?? 0) > 0));
  const json = JSON.stringify(brief);
  const bytes = new TextEncoder().encode(json).length;
  return <div className="brief-health-report">
    <section className={`brief-health-source brief-health-${state}`} aria-label="Producer freshness">
      <div><span className="eyebrow">PRODUCER CLOCK</span><h2>{state === 'fresh' ? 'Observations are current' : state === 'stale' ? 'Producer observations are stale' : state === 'future' ? 'Producer clock is ahead' : 'Producer time is unknown'}</h2>
        <p>{brief.source_clock.observed_utc ?? 'No valid source timestamp'} · stale after {brief.stale_after_seconds}s</p></div>
      <div className="brief-health-payload"><strong>{bytes.toLocaleString()} B</strong><span>agent brief / {brief.source_bytes.toLocaleString()} B source</span></div>
    </section>
    <section className="brief-health-metrics" aria-label="Git health summary">
      {[
        ['Need Git review', totals.git_review_projects, `${totals.conflicted_checkouts} observed checkouts with conflicts`, 'attention'],
        ['Unknown or missing', totals.git_unknown + totals.git_missing, `${totals.git_disabled} policies disabled`, 'unknown'],
        ['Reports not fresh', totals.git_not_fresh, `Across ${totals.projects} observed projects`, 'stale'],
        ['Dirty checkouts', totals.dirty_checkouts, `${totals.unknown_checkouts} working states unknown`, 'neutral'],
      ].map(([label, count, detail, tone]) => <article key={label} className={`brief-health-metric brief-health-${tone}`}><span>{label}</span><strong>{count}</strong><small>{detail}</small></article>)}
    </section>
    {brief.diagnostics.length > 0 && <div className="brief-health-warning" role="status">Collector diagnostics: {brief.diagnostics.join(', ')}. Inspect the detailed producer evidence.</div>}
    <section className="panel brief-health-projects" aria-labelledby="brief-health-project-title">
      <div className="panel-head"><div><div className="eyebrow">EVIDENCE BEFORE ACTION</div><h2 id="brief-health-project-title">Project health</h2><p>Observed Git findings and source task counts. Dirty work alone can be normal work in progress.</p></div>
        <a className="text-button" href="#/git">Open Git Flow →</a></div>
      <div className="brief-health-controls"><label>Find a project<input type="search" value={query} onChange={e => setQuery(e.target.value)} placeholder="Project ID" /></label>
        <label className="brief-health-checkbox"><input type="checkbox" checked={onlyAttention} onChange={e => setOnlyAttention(e.target.checked)} />Show review and coverage gaps</label></div>
      <div className="brief-health-table-scroll"><table><caption>Git coverage and checkout findings</caption><thead><tr><th scope="col">Project</th><th scope="col">Git state</th><th scope="col">Git clock</th><th scope="col">Done / total</th><th scope="col">Blocked</th><th scope="col">Diverged branches</th><th scope="col">Dirty / conflicts</th><th scope="col">Issue codes</th></tr></thead>
        <tbody>{rows.map(p => <tr key={p.id}><th scope="row">{p.id}</th><td><span className={`brief-health-tag brief-health-${p.git_state}`}>{p.git_state}</span></td><td><span title={p.git_clock.observed_utc ?? 'No observation timestamp'}>{clockState(p.git_clock, now, brief.stale_after_seconds)}</span></td><td>{p.done ?? 'unknown'} / {p.total ?? 'unknown'}</td><td>{p.blocked ?? 'unknown'}</td><td>{p.diverged_branches ?? 'unknown'}<small> of {p.observed_branches ?? 'unknown'} observed</small></td><td>{p.dirty_checkouts ?? 'unknown'} / {p.conflicted_checkouts ?? 'unknown'}</td><td>{Object.entries(p.issue_codes).map(([code, count]) => <span className="brief-health-code" key={code}>{code} ×{count}</span>)}{Object.keys(p.issue_codes).length === 0 && <span>{p.git_state === 'healthy' ? 'No reported issues' : 'No issue codes reported'}</span>}</td></tr>)}</tbody>
      </table></div>
      {rows.length === 0 && <p className="brief-health-empty">No projects match this view.</p>}
      <p className="brief-health-coverage">{brief.projects.length} project rows included · {brief.omitted_projects} omitted by the context budget. Summary counts cover all {totals.projects} source projects.</p>
    </section>
    <details className="panel brief-health-agent"><summary>Agent context · bounded JSON · {bytes.toLocaleString()} bytes</summary><p>Fixed codes, exact counters and original source clocks. Full evidence remains available in Fleet detail and Git Flow.</p><pre tabIndex={0} aria-label="Agent JSON brief">{json}</pre><small>Source identity: {brief.source_sha256}</small></details>
  </div>;
}

export default function FleetHealth({ token, onAccess }: { token?: string; onAccess: () => void }) {
  const [result, setResult] = useState<{ token?: string; brief: FleetBrief } | null>(null);
  const [failure, setFailure] = useState<{ token?: string; message: string } | null>(null);
  const [now, setNow] = useState(Date.now());
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    let stopped = false, active = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let request: AbortController | undefined;
    const poll = async () => {
      setNow(Date.now());
      if (stopped || document.hidden || active) return;
      active = true; request = new AbortController();
      let accessDenied = false;
      const deadline = setTimeout(() => request?.abort(), 12_000);
      try {
        const brief = await fetchBrief(token, request.signal);
        if (!stopped) { setResult({ token, brief }); setFailure(null); }
      } catch (error) {
        accessDenied = error instanceof HealthAccessError;
        if (!stopped) {
          if (accessDenied) setResult(null);
          setFailure({ token, message: error instanceof Error ? error.message : 'Agent brief is unavailable.' });
        }
      } finally {
        active = false; clearTimeout(deadline);
        if (!stopped && !document.hidden && !accessDenied) timer = setTimeout(poll, 15_000);
      }
    };
    const visibility = () => { clearTimeout(timer); if (!document.hidden) void poll(); };
    document.addEventListener('visibilitychange', visibility);
    void poll();
    return () => { stopped = true; request?.abort(); clearTimeout(timer); document.removeEventListener('visibilitychange', visibility); };
  }, [token, retry]);
  // A changed session can never display the previous session's protected data.
  const brief = result?.token === token ? result?.brief : undefined;
  const error = failure?.token === token ? failure?.message : undefined;
  return <div className="fleet-health">
    {error && <div className="brief-health-warning" role="status"><span>{error}{brief ? ' Showing the last successful brief; check source clocks.' : ''}</span><button onClick={onAccess}>Session access</button><button onClick={() => setRetry(n => n + 1)}>Retry</button></div>}
    {brief ? <HealthReport brief={brief} now={now} /> : !error && <div role="status">Loading fleet health…</div>}
  </div>;
}
