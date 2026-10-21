import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { ApiError } from '../../api';
import { watchLegacy, type LegacySnapshot } from '../../legacy';
import { buildHealthModel, countText, duration, figure, type CycleStage, type HealthModel, type HealthState, type HostHealth, type Level, type ProjectHealth } from './health';
import './health.css';

const STATE_LABEL: Record<HealthState, string> = {
  healthy: 'Healthy',
  attention: 'Needs attention',
  unknown: 'Unknown',
  disabled: 'No policy',
  not_reported: 'Not reported',
};
const LEVEL_LABEL: Record<Level, string> = { ok: 'OK', amber: 'Watch', red: 'At risk', unknown: 'Unknown' };

function StateChip({ state }: { state: HealthState }) {
  return <span className={`health-chip health-chip--${state}`}>{STATE_LABEL[state]}</span>;
}

function LevelChip({ level }: { level: Level }) {
  return <span className={`health-chip health-chip--level-${level}`}>{LEVEL_LABEL[level]}</span>;
}

function Readout({ value, label, detail, tone }: { value: string; label: string; detail?: string; tone?: 'attention' | 'risk' | 'quiet' }) {
  return <div className={`health-readout ${tone ? `health-readout--${tone}` : ''}`}>
    <dt>{label}</dt>
    <dd><strong>{value}</strong>{detail && <span>{detail}</span>}</dd>
  </div>;
}

function branchText(project: ProjectHealth): string[] {
  const b = project.branches;
  if (!b) return ['Not reported'];
  const lines = [`${b.compared} compared`];
  const parts = [b.merged && `${b.merged} merged`, b.ahead && `${b.ahead} ahead`, b.diverged && `${b.diverged} diverged`, b.unknown && `${b.unknown} unknown`].filter(Boolean) as string[];
  if (parts.length) lines.push(parts.join(', '));
  if (b.older30d) lines.push(`${b.older30d} unmerged tip${b.older30d === 1 ? '' : 's'} older than 30 days`);
  if (project.unintegrated) lines.push(`${countText(project.unintegrated)} commits not in integration`);
  return lines;
}

function worktreeText(project: ProjectHealth): string[] {
  const w = project.worktrees;
  if (!w) return ['Not reported'];
  const lines = [`${w.total} total, ${w.dirty} with changes`];
  if (w.unknown) lines.push(`${w.unknown} with unknown state`);
  if (w.conflicts) lines.push(`${w.conflicts} with conflicts`);
  if (w.detached) lines.push(`${w.detached} detached`);
  return lines;
}

function RepositoryTable({ projects }: { projects: ProjectHealth[] }) {
  if (!projects.length) return <p className="health-empty">The producer snapshot lists no repositories. Configure projects in the observer to see their hygiene here.</p>;
  // Focusable so keyboard users can scroll the table on narrow screens.
  return <div className="health-table-wrap" tabIndex={0} role="region" aria-label="Repository hygiene table">
    <table className="health-table">
      <caption className="visually-hidden">Repository hygiene by project</caption>
      <thead><tr><th scope="col">Repository</th><th scope="col">Git health</th><th scope="col">Branches</th><th scope="col">Worktrees</th><th scope="col">Integration</th></tr></thead>
      <tbody>{projects.map(project => <tr key={project.id} className={`health-row health-row--${project.state}`}>
        <th scope="row"><span className="health-name">{project.name}</span>{project.integrationRef && <code>{project.integrationRef.replace(/^refs\/(heads|remotes)\//, '')}</code>}</th>
        <td data-label="Git health"><StateChip state={project.state} />
          {project.issues.length > 0 && <ul className="health-issues">{project.issues.map(issue => <li key={issue}>{issue.replaceAll('_', ' ')}</li>)}</ul>}
          {project.complete === false && <p className="health-note">Partial scan: counts may grow.</p>}
        </td>
        <td data-label="Branches">{branchText(project).map(line => <p key={line}>{line}</p>)}</td>
        <td data-label="Worktrees">{worktreeText(project).map(line => <p key={line}>{line}</p>)}</td>
        <td data-label="Integration"><p>Tip {project.tipAgeSeconds === null ? 'age unknown' : `${duration(project.tipAgeSeconds)} old`}</p>
          <p>{countText(project.commits24h)} commits in 24 h, {countText(project.commits7d)} in 7 d</p>
          {project.fingerprint && <p className="health-note">Evidence <code>{project.fingerprint}</code></p>}
        </td>
      </tr>)}</tbody>
    </table>
  </div>;
}

function HostTable({ hosts }: { hosts: HostHealth[] }) {
  if (!hosts.length) return <p className="health-empty">No host resources are in this snapshot.</p>;
  return <div className="health-table-wrap" tabIndex={0} role="region" aria-label="Host resources table">
    <table className="health-table health-table--hosts">
      <caption className="visually-hidden">Host resource pressure</caption>
      <thead><tr><th scope="col">Host</th><th scope="col">State</th><th scope="col">Load</th><th scope="col">Memory free</th><th scope="col">Root disk</th><th scope="col">Disk outlook</th></tr></thead>
      <tbody>{hosts.map(host => <tr key={host.host}>
        <th scope="row">{host.host}</th>
        <td data-label="State">{host.reachable ? <LevelChip level={host.level} /> : <span className="health-chip health-chip--unknown">Unreachable</span>}</td>
        <td className="health-num" data-label="Load">{figure(host.loadPercent, '%')}{host.gpuUtilization.length > 0 && <small>GPU {host.gpuUtilization.map(u => `${figure(u)}%`).join(', ')}</small>}</td>
        <td className="health-num" data-label="Memory free">{figure(host.memoryAvailablePercent, '%')}{host.swapUsedPercent !== null && <small>swap {figure(host.swapUsedPercent, '%')} used</small>}</td>
        <td className="health-num" data-label="Root disk">{figure(host.rootUsedPercent, '%')}{host.rootGrowthGbPerHour !== null && <small>{host.rootGrowthGbPerHour >= 0 ? '+' : ''}{figure(host.rootGrowthGbPerHour)} GB/h</small>}</td>
        <td data-label="Disk outlook">{host.worstDisk ? (host.worstDisk.hoursToFull !== null
          ? <p>{host.worstDisk.mount} full in about {duration(host.worstDisk.hoursToFull * 3600)}</p>
          : <p>{host.worstDisk.growthGbPerHour === null ? 'No projection yet' : 'Not growing'}</p>) : 'unknown'}</td>
      </tr>)}</tbody>
    </table>
  </div>;
}

/** Parallel collection lanes, then the serial tail, drawn to scale. */
function CycleAnatomy({ model }: { model: HealthModel }) {
  const { collect, git, serial, durationMs } = model.cycle;
  const known = (stages: CycleStage[]) => stages.reduce((sum, s) => sum + (s.ms ?? 0), 0);
  const parallel = Math.max(known(collect), known(git));
  const total = Math.max(1, parallel + known(serial));
  const reported = [...collect, ...git, ...serial].some(stage => stage.ms !== null);
  if (!reported) return <p className="health-empty">This observer does not report stage timings yet. Durations appear after the observer is upgraded.</p>;
  const width = (ms: number | null) => `${Math.max(ms ? 1.5 : 0, ((ms ?? 0) / total) * 100)}%`;
  const lane = (stages: CycleStage[]) => stages.filter(s => s.ms !== null).map(s => <span key={s.id} className={`health-seg health-seg--${s.id}`} style={{ width: width(s.ms) }} title={`${s.label}: ${s.ms} ms`}><b>{s.label}</b> {s.ms} ms</span>);
  const summary = [...collect, ...git, ...serial].map(s => `${s.label} ${s.ms ?? 'unknown'} ms`).join(', ');
  return <figure className="health-cycle" aria-label={`Observer cycle ${durationMs ?? 'unknown'} ms: ${summary}`}>
    <div className="health-lanes" aria-hidden="true">
      <div className="health-lane"><span className="health-lane-label">Fleet and projects</span><div className="health-track">{lane(collect)}</div></div>
      <div className="health-lane"><span className="health-lane-label">Git</span><div className="health-track">{lane(git)}</div></div>
      <div className="health-lane"><span className="health-lane-label">Then</span><div className="health-track"><span className="health-gap" style={{ width: `${(parallel / total) * 100}%` }} />{lane(serial)}</div></div>
    </div>
    <figcaption>Fleet and Git observation run side by side, so a cycle costs the slower lane plus the short serial tail. Last cycle took {durationMs === null ? 'an unknown time' : `${durationMs} ms`} with {model.cycle.modelTurns ?? 'unknown'} model turns.</figcaption>
  </figure>;
}

export function AgentBrief({ token, revision }: { token?: string; revision: string | null }) {
  const [budget, setBudget] = useState(2000);
  const [state, setState] = useState<{ text: string; error: string; omitted: number | null }>({ text: '', error: '', omitted: null });
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!revision) return;
    const controller = new AbortController();
    void (async () => {
      try {
        const response = await fetch(`/v1/brief?budget=${budget}`, { signal: controller.signal, cache: 'no-store', headers: token ? { Authorization: `Bearer ${token}` } : undefined });
        if (!response.ok) throw new ApiError(response.status === 404 ? 'This server does not provide the agent brief yet.' : `Brief request failed (${response.status})`, response.status);
        const omitted = Number(response.headers.get('x-admiralty-brief-omitted'));
        setState({ text: (await response.text()).slice(0, 64 * 1024), error: '', omitted: Number.isFinite(omitted) ? omitted : null });
      } catch (error) {
        if (!controller.signal.aborted) setState(previous => ({ ...previous, error: error instanceof Error ? error.message : 'Brief unavailable' }));
      }
    })();
    return () => controller.abort();
  }, [token, budget, revision]);
  const copy = async () => {
    try { await navigator.clipboard.writeText(state.text); setCopied(true); setTimeout(() => setCopied(false), 2000); } catch { setCopied(false); }
  };
  return <section className="panel health-panel health-brief" aria-labelledby="health-brief-title">
    <div className="health-panel-head"><h2 id="health-brief-title">Agent brief</h2>
      <div className="health-brief-controls">
        <label htmlFor="health-brief-budget">Size</label>
        <select id="health-brief-budget" value={budget} onChange={event => setBudget(Number(event.target.value))}>
          <option value={1000}>1 KB</option><option value={2000}>2 KB</option><option value={4000}>4 KB</option>
        </select>
        <button type="button" className="secondary-button" disabled={!state.text} onClick={() => void copy()}>{copied ? 'Copied' : 'Copy brief'}</button>
      </div>
    </div>
    <p className="health-lede">What a captain or watcher reads instead of the full snapshot: urgent facts first, unknowns shown as <code>?</code>, never zero.</p>
    {state.error ? <p className="health-empty" role="status">{state.error}</p>
      : <pre className="health-brief-text" tabIndex={0} role="region" aria-label="Agent brief text">{state.text || 'Loading brief…'}</pre>}
    <p className="health-note">{state.omitted ? `${state.omitted} lower-priority lines left out at this size. ` : ''}The same text is available from <code>GET /v1/brief</code> and <code>admiralty brief</code>.</p>
  </section>;
}

export function HealthView({ model, brief }: { model: HealthModel; brief?: ReactNode }) {
  const s = model.summary;
  const feed = model.ageSeconds === null ? 'unknown' : `${duration(model.ageSeconds)} old`;
  return <div className="health">
    <dl className="health-bridge" aria-label="Fleet health summary">
      <Readout label="Producer feed" value={model.stale ? 'Stale' : feed} detail={`${model.stale ? `${feed}, ` : ''}from ${model.host}${model.intervalSeconds ? `, every ${model.intervalSeconds} s` : ''}`} tone={model.stale ? 'risk' : undefined} />
      <Readout label="Repositories needing attention" value={String(s.attention)} detail={`${s.unknown} unknown, ${s.healthy} healthy of ${s.projects}`} tone={s.attention ? 'attention' : 'quiet'} />
      <Readout label="Worktrees with changes" value={s.dirtyWorktrees === null ? 'unknown' : String(s.dirtyWorktrees)} detail={s.unmergedOlder30d === null ? undefined : `${s.unmergedOlder30d} unmerged branch tips older than 30 days`} />
      <Readout label="Hosts at risk" value={String(s.hostsAtRisk)} detail={s.hostsUnreachable ? `${s.hostsUnreachable} unreachable` : 'All reachable'} tone={s.hostsAtRisk || s.hostsUnreachable ? 'risk' : 'quiet'} />
      <Readout label="Observer cycle" value={model.cycle.durationMs === null ? 'unknown' : `${model.cycle.durationMs} ms`} detail={`${model.cycle.modelTurns ?? 'unknown'} model turns`} />
    </dl>
    <section className="panel health-panel" aria-labelledby="health-repos-title">
      <div className="health-panel-head"><h2 id="health-repos-title">Repository hygiene</h2></div>
      <p className="health-lede">Read from the observer's last Git scan. Health decides what is actionable; the other columns describe pressure and activity without judging intent.</p>
      <RepositoryTable projects={model.projects} />
    </section>
    <div className="health-split">
      <section className="panel health-panel" aria-labelledby="health-hosts-title">
        <div className="health-panel-head"><h2 id="health-hosts-title">Hosts</h2></div>
        <p className="health-lede">From the observer's own probes. Disk outlook projects the last 20 minutes to 3 hours of growth; until then it shows no projection.</p>
        <HostTable hosts={model.hosts} />
      </section>
      {brief}
    </div>
    <section className="panel health-panel" aria-labelledby="health-cycle-title">
      <div className="health-panel-head"><h2 id="health-cycle-title">Observer cycle</h2></div>
      <CycleAnatomy model={model} />
    </section>
  </div>;
}

export default function FleetHealth({ token, onAccess }: { token?: string; onAccess: () => void }) {
  const [snapshot, setSnapshot] = useState<LegacySnapshot | null>(null);
  const [missing, setMissing] = useState(false);
  const [error, setError] = useState('');
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    setError('');
    return watchLegacy({ token, view: 'detail', initialOnly: true,
      onSnapshot: value => { if (value === null) setMissing(true); else { setSnapshot(value); setMissing(false); } setNow(Date.now()); },
      onError: setError });
  }, [token]);
  useEffect(() => {
    const tick = setInterval(() => setNow(Date.now()), 10_000);
    return () => clearInterval(tick);
  }, []);
  const model = useMemo(() => (snapshot ? buildHealthModel(snapshot, now) : null), [snapshot, now]);
  const needsAccess = /\((401|403)\)/.test(error);
  return <>
    {error && <div className="error-banner" role="status"><span>{needsAccess ? 'Session access required' : 'Connection issue'}</span><span>{error}</span>{needsAccess && <button className="text-button" onClick={onAccess}>Connect session access</button>}</div>}
    {missing && <div className="loading-card" role="status">This server has no producer feed configured. Start admiraltyd with an observer status file to see fleet health.</div>}
    {!model && !missing && !error && <div className="loading-card" role="status">Loading the latest producer snapshot…</div>}
    {model && <HealthView model={model} brief={<AgentBrief token={token} revision={model.generated} />} />}
  </>;
}
