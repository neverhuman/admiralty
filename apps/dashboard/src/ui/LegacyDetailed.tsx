import { useEffect, useMemo, useState } from 'react';
import { data, list, text, label, freshness, safeHref, stageLabel, stageClass, projectDetails, watchLegacy, type LegacySnapshot, type ProjectDetailModel } from '../legacy';
import { TemporalPlots } from './TemporalPlots';
import { LegacyHierarchy } from './LegacyHierarchy';
import { TokenEfficiency } from './TokenEfficiency';
import { Economics } from '../features/economics/Economics';
import { ModelBadge } from './ModelBadge';
import { routeHref } from '../router';
import './legacy-evidence.css';

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
  return <details className="legacy-section" open={expanded} onToggle={event => setExpanded(event.currentTarget.open)}><summary>{title}</summary>{expanded && (title === 'Work packets' ? <PacketTable values={list(value)} /> : title === 'Coordination log' && Array.isArray(data(value).tail) ? <><div className="legacy-provenance">{text(data(value).mtime_utc)}</div><pre>{list(data(value).tail).map(text).join('\n')}</pre></> : <DataValue value={value} />)}</details>;
}
function PacketTable({ values }: { values: unknown[] }) {
  const [shown, setShown] = useState(12);
  return <><div className="legacy-table-scroll"><table><thead><tr><th>Packet</th><th>Work</th><th>Stage</th></tr></thead><tbody>{values.slice(0,shown).map((value,index) => {const packet=data(value); return <tr key={text(packet.id)||index}><td><b>{text(packet.id)}</b></td><td>{text(packet.title)}<details><summary>Evidence and scope</summary><DataValue value={packet}/></details></td><td>{stageLabel(text(packet.status))}</td></tr>;})}</tbody></table></div>{values.length>shown && <button className="text-button" onClick={()=>setShown(values.length)}>Show all {values.length} packets</button>}</>;
}
function Screen({ value }: { value: unknown }) {
  const screen = data(value);
  if (!Object.keys(screen).length) return <div className="muted">No Commodore reported by producer.</div>;
  return <div className="legacy-screen"><div><b>{text(screen.name ?? screen.screen ?? screen.id) || 'Commodore'}</b> <span className={`source-state ${screen.agent_running === true || screen.state === 'active' ? 'active' : ''}`}>{screen.agent_running === true ? 'agent running' : text(screen.state) || (screen.alive === true ? 'screen alive; agent unconfirmed' : 'unknown')}</span></div><div><ModelBadge model={text(screen.model) || 'Unknown'} effort={text(screen.effort)} /> · {text(screen.screen_changed_utc ?? screen.last_token_utc) || 'Activity time unknown'}</div>{list(screen.tail).length > 0 && <pre>{list(screen.tail).map(text).join('\n')}</pre>}</div>;
}
function Workers({ values }: { values: unknown[] }) {
  return <div className="legacy-table-scroll"><table><thead><tr><th>Screen / node</th><th>Model / effort</th><th>Task</th><th>Observed state</th></tr></thead><tbody>{values.map((value, index) => {
    const worker = data(value);
    return <tr key={`${text(worker.id ?? worker.screen ?? worker.name)}:${index}`}><td>{text(worker.name ?? worker.screen ?? worker.id) || 'Unknown'}<small>{text(worker.host ?? worker.node) || 'Node not reported'}</small></td><td><ModelBadge model={text(worker.model) || 'Unknown'} effort={text(worker.effort)} tier={text(worker.tier)} /></td><td>{text(worker.task) || 'Unassigned'}{worker.tail !== undefined && <details><summary>Screen tail</summary><pre>{list(worker.tail).map(text).join('\n')}</pre></details>}</td><td>{text(worker.state) || (worker.agent_running === true ? 'agent running' : worker.agent_running === false ? 'agent not running' : 'unknown')}<small>{text(worker.screen_changed_utc)}</small></td></tr>;
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
    <header className="legacy-project-heading"><h2>{project.name}</h2><a className="legacy-live-link" href={routeHref('git', { project: project.id })}>Git Flow ↗</a><span>{total === null ? 'Unknown total' : `${total} tasks`}</span></header>
    <p className="legacy-provenance">{project.source}</p>
    <div className="legacy-stage-bar" role="img" aria-label={`${project.name} reported task stages`}>{entries.map(([stage, count]) => <span key={stage} className={stageClass(stage)} style={{ width: `${total && total > 0 ? Math.min(100, count / total * 100) : 0}%` }} title={`${stageLabel(stage, project.labels)}: ${count}`} />)}</div>
    <div className="legacy-stage-counts">{entries.map(([stage, count]) => <span key={stage}><i className={stageClass(stage)} /><b>{count}</b> {stageLabel(stage, project.labels)}</span>)}</div>
    <div className="legacy-provenance">Producer observation: {project.observed} ({freshness(project.observed)}). Source claims and accepted counts retain their original meanings.</div>
    {Object.hasOwn(project.stages, 'verified') && <p className="legacy-evidence-note">Recorded checks describe component evidence. Integration and accepted completion require separate proof.</p>}
    {project.alerts.map((alert, index) => <div className={`legacy-alert legacy-alert-${alert.kind}`} key={`${alert.sourceMessage}:${index}`}>
      {alert.taskIds.length > 0 && <b>{alert.taskIds.length} {alert.taskIds.length === 1 ? 'packet' : 'packets'} · </b>}{alert.message}
      {alert.count > 1 && !alert.taskIds.length && <b> ({alert.count} records)</b>}
      {(alert.taskIds.length > 0 || alert.sourceMessage !== alert.message) && <details><summary>Affected packets and source evidence{alert.count > 1 ? ` (${alert.count} records)` : ''}</summary>
        <p>{alert.sourceMessage}</p>{alert.taskIds.length > 0 && <p className="legacy-alert-task-ids">{alert.taskIds.join(', ')}</p>}
      </details>}
    </div>)}
    {project.recent.length > 0 && <section><h3>Recently changed tasks</h3><div className="legacy-table-scroll"><table><thead><tr><th>Task</th><th>Change</th><th>Observed</th><th>Source</th></tr></thead><tbody>{project.recent.map((value, index) => { const row = data(value); return <tr key={index}><td>{text(row.task)}</td><td>{text(row.old) || '∅'} → {text(row.new)}</td><td>{text(row.at)}</td><td>{text(row.source)}</td></tr>; })}</tbody></table></div></section>}
    <section><h3>Commodore</h3><Screen value={project.captain} /></section>
    <section><h3>Worker screens and models ({project.workers.length})</h3><Workers values={project.workers} /></section>
    <section><h3>Integration PR, CI and review</h3><PullRequest value={project.pr} /></section>
    {project.sections.map((section, index) => <SourceSection key={`${section.title}:${index}`} {...section} />)}
    <SourceSection title="Complete producer project record" value={project.raw} />
  </article>;
}
export function LegacyFleetView({ snapshot, selected = 'all' }: { snapshot: LegacySnapshot; selected?: string }) {
  const projects = projectDetails(snapshot), visible = selected === 'all' ? projects : projects.filter(project => project.id === selected);
  return <>
    <TemporalPlots snapshot={snapshot} />
    <div className="legacy-source-status">Source host: <b>{text(snapshot.host) || 'Unknown'}</b> · Produced {snapshot.generated_utc} ({freshness(snapshot.generated_utc)}) · Read-only observation every 15 seconds</div>
    <div className="legacy-layout">
      <div className="legacy-project-grid">{visible.map(project => <ProjectDetail key={project.id} project={project} compact={selected === 'all'} />)}{!visible.length && <div className="loading-card">Selected project is no longer present in this snapshot.</div>}</div>
      <aside className="legacy-side">
        <a className="text-button" href={routeHref('atlas')}>Open full Fleet Atlas ↗</a><LegacyHierarchy snapshot={snapshot} />
        <TokenEfficiency snapshot={snapshot} />
        <Economics />
        <div className="legacy-auxiliary panel legacy-side-panel">
          <SourceSection title="Fleet resources" value={snapshot.global_resources} />
          <SourceSection title="Admiral" value={snapshot.super_captain} />
          <SourceSection title="Token yield source and coverage" value={snapshot.token_yield} />
          <SourceSection title="Complete history source" value={snapshot.history} />
          <SourceSection title="All agent observations" value={snapshot.agents} />
          <SourceSection title="Remote screens" value={snapshot.remote_screens} />
        </div>
      </aside>
    </div>
  </>;
}
export default function LegacyDetailed({ token }: { token?: string }) {
  const [snapshot, setSnapshot] = useState<LegacySnapshot | null>(null), [error, setError] = useState('');
  const [loaded, setLoaded] = useState(false), [selected, setSelected] = useState('all'), [density, setDensity] = useState<'dense' | 'comfortable'>('dense');
  const [retry, setRetry] = useState(0);
  useEffect(() => { setSnapshot(null); setLoaded(false); setError(''); return watchLegacy({ token, onSnapshot: value => { setSnapshot(value); setLoaded(true); }, onError: value => { setError(value); if (value) setLoaded(true); } }); }, [token, retry]);
  const projects = useMemo(() => snapshot ? projectDetails(snapshot) : [], [snapshot]);
  return <div className={`legacy-detailed density-${density}`}><div className="legacy-toolbar"><div className="legacy-project-tabs" role="tablist" aria-label="Observed projects"><button role="tab" aria-selected={selected === 'all'} onClick={() => setSelected('all')}>All projects</button>{projects.map(project => <button role="tab" key={project.id} aria-selected={selected === project.id} onClick={() => setSelected(project.id)}><span className="legacy-tab-title">{project.name}<small>{project.total === null ? 'Unknown total' : `${project.total} tasks`}</small></span><span className="legacy-tab-metrics">{Object.entries(project.stages).filter(([stage, count]) => count > 0 && ['done', 'in_progress', 'in_pr', 'blocked', 'review', 'verified', 'integrated'].includes(stage)).map(([stage, count]) => <span key={stage}><b>{count}</b> {stageLabel(stage, project.labels)}</span>)}</span><span className="legacy-stage-bar" aria-hidden="true">{Object.entries(project.stages).map(([stage, count]) => <span key={stage} className={stageClass(stage)} style={{width:`${project.total ? Math.min(100,count/project.total*100) : 0}%`}} />)}</span></button>)}</div><label>Density <select value={density} onChange={event => setDensity(event.target.value as 'dense' | 'comfortable')}><option value="dense">Dense</option><option value="comfortable">Comfortable</option></select></label><button className="text-button" onClick={() => setRetry(value => value + 1)}>Refresh source</button><a href="/legacy/" target="_blank" rel="noopener noreferrer">Original detailed dashboard ↗</a></div>
    {error && <div className="error-banner" role="status">{error}. Existing snapshot remains visible when available; source age is preserved.</div>}
    {!loaded && <div className="loading-card">Loading detailed producer snapshot…</div>}
    {loaded && !snapshot && <div className="loading-card">Detailed producer data unavailable on this server. Native simulation and its synthetic projects remain separate in the overview.</div>}
    {snapshot && <LegacyFleetView snapshot={snapshot} selected={selected} />}
  </div>;
}
