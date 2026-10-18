import { useEffect, useMemo, useState } from 'react';
import { data, list, text, label, number, freshness, safeHref, projectDetails, watchLegacy, type LegacySnapshot, type ProjectDetailModel } from '../legacy';

/** Semantic React views of producer facts. Unknown values never become zeros. */
export function DataValue({ value, depth = 0 }: { value: unknown; depth?: number }) {
  const [shown, setShown] = useState(30), [expanded, setExpanded] = useState(false);
  if (value === null || value === undefined) return <span className="muted">Unknown</span>;
  if (typeof value !== 'object') return <span className="source-text">{text(value)}</span>;
  if (depth >= 5) return <details onToggle={event => setExpanded(event.currentTarget.open)}><summary>Nested source details</summary>{expanded && <pre>{JSON.stringify(value, null, 2)}</pre>}</details>;
  const entries = Array.isArray(value) ? value.map((item, index) => [String(index + 1), item] as const) : Object.entries(value);
  if (!entries.length) return <span className="muted">None reported</span>;
  return <><dl className="source-data">{entries.slice(0, shown).map(([key, item]) => <div key={key}><dt>{Array.isArray(value) ? key : label(key)}</dt><dd><DataValue value={item} depth={depth + 1} /></dd></div>)}</dl>{entries.length > shown && <button className="text-button" onClick={() => setShown(count => count + 100)}>Show more ({entries.length - shown} remaining)</button>}</>;
}
function SourceSection({ title, value, open = false }: { title: string; value: unknown; open?: boolean }) {
  const [expanded, setExpanded] = useState(open);
  return <details className="legacy-section" open={expanded} onToggle={event => setExpanded(event.currentTarget.open)}><summary>{title}</summary>{expanded && <DataValue value={value} />}</details>;
}
function Screen({ value }: { value: unknown }) {
  const screen = data(value);
  if (!Object.keys(screen).length) return <div className="muted">No captain reported by producer.</div>;
  return <div className="legacy-screen"><div><b>{text(screen.name ?? screen.screen ?? screen.id) || 'Captain'}</b> <span className={`source-state ${screen.agent_running === true || screen.state === 'active' ? 'active' : ''}`}>{screen.agent_running === true ? 'agent running' : text(screen.state) || (screen.alive === true ? 'screen alive; agent unconfirmed' : 'unknown')}</span></div><div>{text(screen.model) || 'Model unknown'} {text(screen.effort)} · {text(screen.screen_changed_utc) || 'Activity time unknown'}</div>{list(screen.tail).length > 0 && <pre>{list(screen.tail).map(text).join('\n')}</pre>}</div>;
}
function Workers({ values }: { values: unknown[] }) {
  return <div className="legacy-table-scroll"><table><thead><tr><th>Screen / node</th><th>Model / effort</th><th>Task</th><th>Observed state</th></tr></thead><tbody>{values.map((value, index) => {
    const worker = data(value);
    return <tr key={`${text(worker.id ?? worker.screen ?? worker.name)}:${index}`}><td>{text(worker.name ?? worker.screen ?? worker.id) || 'Unknown'}<small>{text(worker.host ?? worker.node) || 'Node not reported'}</small></td><td>{text(worker.model) || 'Unknown'}<small>{text(worker.effort)} {text(worker.tier)}</small></td><td>{text(worker.task) || 'Unassigned'}{worker.tail !== undefined && <details><summary>Screen tail</summary><pre>{list(worker.tail).map(text).join('\n')}</pre></details>}</td><td>{text(worker.state) || (worker.agent_running === true ? 'agent running' : worker.agent_running === false ? 'agent not running' : 'unknown')}<small>{text(worker.screen_changed_utc)}</small></td></tr>;
  })}</tbody></table>{!values.length && <div className="muted">No workers reported.</div>}</div>;
}
function PullRequest({ value }: { value: unknown }) {
  const pr = data(value), ci = data(pr.ci), link = safeHref(pr.url);
  if (!Object.keys(pr).length) return <div className="muted">No PR metadata reported.</div>;
  return <div className="legacy-pr">{link ? <a href={link} target="_blank" rel="noopener noreferrer">{pr.number ? `PR #${text(pr.number)}` : 'Integration PR'}</a> : <b>PR not linked</b>} · {text(pr.state) || 'State unknown'}{pr.draft === true ? ' (draft)' : ''}<p>{text(pr.title)}</p><div>Head <code>{text(pr.head) || 'Unknown'}</code> · {text(pr.branch)}</div><div>CI: <b>{text(ci.state) || 'Unknown'}</b> · Review: {text(pr.review_decision) || 'Unknown'} · {pr.exact_head_approved === true ? 'exact head approved (source report)' : 'exact-head approval not confirmed'}</div><DataValue value={{ checks: ci.counts, failing: ci.failing, last_review: pr.last_review, fetched_utc: pr.fetched_utc, merged_at: pr.merged_at, note: pr.note }} /></div>;
}
export function ProjectDetail({ project, compact = false }: { project: ProjectDetailModel; compact?: boolean }) {
  const total = project.total, entries = Object.entries(project.stages);
  return <article className={`legacy-project panel ${compact ? 'compact' : ''}`} data-project-id={project.id}>
    <header className="legacy-project-heading"><h2>{project.name}</h2><span>{total === null ? 'Unknown total' : `${total} tasks`}</span></header>
    <p className="legacy-provenance">{project.source}</p>
    <div className="legacy-stage-bar" aria-label={`${project.name} reported task stages`}>{entries.map(([stage, count]) => <span key={stage} className={`stage-${stage}`} style={{ width: `${total && total > 0 ? Math.min(100, count / total * 100) : 0}%` }} title={`${project.labels[stage] || label(stage)}: ${count}`} />)}</div>
    <div className="legacy-stage-counts">{entries.map(([stage, count]) => <span key={stage}><i className={`stage-${stage}`} /><b>{count}</b> {project.labels[stage] || label(stage)}</span>)}</div>
    <div className="legacy-provenance">Producer observation: {project.observed} ({freshness(project.observed)}). Source claims and accepted counts retain their original meanings.</div>
    {project.alerts.map((alert, index) => <div className="legacy-alert" key={index}>{alert}</div>)}
    {project.recent.length > 0 && <section><h3>Recently changed tasks</h3><div className="legacy-table-scroll"><table><thead><tr><th>Task</th><th>Change</th><th>Observed</th><th>Source</th></tr></thead><tbody>{project.recent.map((value, index) => { const row = data(value); return <tr key={index}><td>{text(row.task)}</td><td>{text(row.old) || '∅'} → {text(row.new)}</td><td>{text(row.at)}</td><td>{text(row.source)}</td></tr>; })}</tbody></table></div></section>}
    <section><h3>Captain</h3><Screen value={project.captain} /></section>
    <section><h3>Worker screens and models ({project.workers.length})</h3><Workers values={project.workers} /></section>
    <section><h3>Integration PR, CI and review</h3><PullRequest value={project.pr} /></section>
    {project.sections.map((section, index) => <SourceSection key={`${section.title}:${index}`} {...section} />)}
    <SourceSection title="Complete producer project record" value={project.raw} />
  </article>;
}
function Hierarchy({ snapshot }: { snapshot: LegacySnapshot }) {
  const hierarchy = data(snapshot.hierarchy), captains = list(hierarchy.captains);
  return <section className="legacy-side-panel panel"><h2>Hierarchy</h2><div className="legacy-command-chain"><span>{text(hierarchy.jepson ?? hierarchy.owner) || 'Owner not reported'}</span><span>→ {text(data(hierarchy.grokbot).name) || 'Watcher not reported'}</span><span>→ {text(data(hierarchy.super_captain).screen) || 'Super captain not reported'}</span></div><div className="legacy-captain-columns">{captains.map((value, index) => { const captain = data(value); return <div key={index}><b>{text(captain.name ?? captain.key)}</b><div>{text(captain.model)}</div><span>{text(captain.state)}</span><Workers values={list(captain.workers)} /></div>; })}</div><SourceSection title="Full hierarchy source" value={snapshot.hierarchy} /></section>;
}
function Trends({ snapshot }: { snapshot: LegacySnapshot }) {
  const rows = list(data(snapshot.history).rows).map(data), metrics = ['cost_index', 'quota_pct', 'credits_h', 'luna_fail_total', 'luna_fp_rate'];
  return <section className="legacy-side-panel panel"><h2>History and trends</h2><div className="legacy-trends">{metrics.map(key => {
    const values = rows.map(row => number(row[key])).filter((value): value is number => value !== null);
    const max = Math.max(...values, 1), min = Math.min(...values, 0);
    const points = values.map((value, index) => `${index / Math.max(1, values.length - 1) * 200},${38 - (value - min) / Math.max(1, max - min) * 34}`).join(' ');
    return <div key={key}><span>{label(key)}</span><b>{values.length ? values.at(-1) : 'Unknown'}</b><svg viewBox="0 0 200 42" role="img" aria-label={`${label(key)} source history`}><polyline points={points} fill="none" stroke="currentColor" strokeWidth="2" /></svg></div>;
  })}</div><SourceSection title={`History source rows (${rows.length})`} value={snapshot.history} /></section>;
}
export function LegacyFleetView({ snapshot, selected = 'all' }: { snapshot: LegacySnapshot; selected?: string }) {
  const projects = projectDetails(snapshot), visible = selected === 'all' ? projects : projects.filter(project => project.id === selected);
  return <><div className="legacy-source-status">Source host: <b>{text(snapshot.host) || 'Unknown'}</b> · Produced {snapshot.generated_utc} ({freshness(snapshot.generated_utc)}) · Read-only observation every 15 seconds</div>
    <div className="legacy-layout"><div className="legacy-project-grid">{visible.map(project => <ProjectDetail key={project.id} project={project} compact={selected === 'all'} />)}{!visible.length && <div className="loading-card">Selected project is no longer present in this snapshot.</div>}</div>
    <aside className="legacy-side"><Hierarchy snapshot={snapshot} /><section className="legacy-side-panel panel"><h2>Token efficiency and model mix</h2><DataValue value={snapshot.token_efficiency} /></section><section className="legacy-side-panel panel"><h2>Token yield and coverage</h2><DataValue value={snapshot.token_yield} /></section><section className="legacy-side-panel panel"><h2>Fleet resources</h2><DataValue value={snapshot.global_resources} /></section><Trends snapshot={snapshot} /><section className="legacy-side-panel panel"><h2>Super captain</h2><DataValue value={snapshot.super_captain} /></section><SourceSection title="All agent observations" value={snapshot.agents} /><SourceSection title="Remote screens" value={snapshot.remote_screens} /></aside></div></>;
}
export default function LegacyDetailed({ token }: { token?: string }) {
  const [snapshot, setSnapshot] = useState<LegacySnapshot | null>(null), [error, setError] = useState('');
  const [loaded, setLoaded] = useState(false), [selected, setSelected] = useState('all'), [density, setDensity] = useState<'dense' | 'comfortable'>('dense');
  const [retry, setRetry] = useState(0);
  useEffect(() => { setSnapshot(null); setLoaded(false); setError(''); return watchLegacy({ token, onSnapshot: value => { setSnapshot(value); setLoaded(true); }, onError: value => { setError(value); if (value) setLoaded(true); } }); }, [token, retry]);
  const projects = useMemo(() => snapshot ? projectDetails(snapshot) : [], [snapshot]);
  return <div className={`legacy-detailed density-${density}`}><div className="legacy-toolbar"><div className="legacy-project-tabs" role="tablist" aria-label="Observed projects"><button role="tab" aria-selected={selected === 'all'} onClick={() => setSelected('all')}>All projects</button>{projects.map(project => <button role="tab" key={project.id} aria-selected={selected === project.id} onClick={() => setSelected(project.id)}>{project.name}</button>)}</div><label>Density <select value={density} onChange={event => setDensity(event.target.value as 'dense' | 'comfortable')}><option value="dense">Dense</option><option value="comfortable">Comfortable</option></select></label><button className="text-button" onClick={() => setRetry(value => value + 1)}>Refresh source</button><a href="/legacy/" target="_blank" rel="noopener noreferrer">Original detailed dashboard ↗</a></div>
    {error && <div className="error-banner" role="status">{error}. Existing snapshot remains visible when available; source age is preserved.</div>}
    {!loaded && <div className="loading-card">Loading detailed producer snapshot…</div>}
    {loaded && !snapshot && <div className="loading-card">Detailed producer data unavailable on this server. Native simulation and its synthetic projects remain separate in the overview.</div>}
    {snapshot && <LegacyFleetView snapshot={snapshot} selected={selected} />}
  </div>;
}
