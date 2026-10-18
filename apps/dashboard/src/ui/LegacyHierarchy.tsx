import { useState } from 'react';
import { data, hierarchyCaptains, list, number, text, type Data, type LegacySnapshot } from '../legacy';
import { ModelBadge } from './ModelBadge';
import { useProfile } from '../profile';
import { ProfileAvatar } from './Profile';
import './legacy-hierarchy.css';

const endedStates = new Set(['finished', 'gone', 'dead', 'exited']);
const stateClasses: Record<string, string> = { active: 'active', idle: 'idle', blocked: 'blocked', quota: 'blocked',
  remote: 'remote', finished: 'finished', gone: 'finished', dead: 'finished', exited: 'finished',
  running: 'active', in_progress: 'active', claimed: 'idle', 'agent running': 'active',
};
const stateClass = (state: string): string => Object.hasOwn(stateClasses, state) ? stateClasses[state] : 'unknown';

function reportedState(record: Data): string {
  return text(record.state ?? record.status) || (record.agent_running === true ? 'agent running'
    : record.agent_running === false ? 'agent not running' : 'unknown');
}
function identity(value: unknown): string {
  const record = data(value);
  return text(record.name ?? record.label ?? record.id) || text(value);
}
function screenName(record: Data): string {
  return text(record.screen ?? record.name ?? record.id) || 'Screen not reported';
}
function State({ state }: { state: string }) {
  return <span className="legacy-hierarchy-state" title={`Producer state: ${state}`}><i aria-hidden="true" className={`legacy-hierarchy-dot legacy-hierarchy-dot--${stateClass(state)}`} />{state}</span>;
}
function Host({ record }: { record: Data }) {
  const host = text(record.host ?? record.node ?? record.node_id);
  return host ? <span className="legacy-hierarchy-host" title="Reported host">{host}</span> : null;
}
function Worker({ record }: { record: Data }) {
  const flags = list(record.flags).map(text).filter(Boolean), state = reportedState(record);
  return <div className="legacy-hierarchy-worker" data-worker-screen={screenName(record)}>
    <div className="legacy-hierarchy-worker-heading"><i aria-hidden="true" className={`legacy-hierarchy-dot legacy-hierarchy-dot--${stateClass(state)}`} /><span className="legacy-hierarchy-screen">{screenName(record)}</span> <Host record={record} />{flags.length > 0 && <span className="legacy-hierarchy-flag" title={flags.join('; ')} aria-label={flags.join('; ')}>⚑</span>}</div>
    <div><ModelBadge model={text(record.model) || 'unknown'} effort={text(record.effort)} tier={text(record.tier)} />{text(record.task ?? record.packet_id) && <> <span className="legacy-hierarchy-task">{text(record.task ?? record.packet_id)}</span></>}</div>
    <div className="legacy-hierarchy-worker-state" title={`Producer state: ${state}`}>{state}</div>
  </div>;
}
function WorkerRows({ workers }: { workers: Data[] }) {
  const [shown, setShown] = useState(40);
  return <>{workers.slice(0, shown).map((worker, index) => <Worker key={`${screenName(worker)}:${text(worker.host ?? worker.node)}:${index}`} record={worker} />)}{workers.length > shown && <button className="legacy-hierarchy-more" onClick={() => setShown(count => count + 40)}>Show more screens ({workers.length - shown})</button>}</>;
}
function Hygiene({ value }: { value: unknown }) {
  const hygiene = data(value), grade = text(hygiene.grade), score = text(hygiene.score);
  if (!grade && !score) return null;
  const color = ['A', 'B'].includes(grade) ? 'good' : grade === 'C' ? 'warn' : ['D', 'E', 'F'].includes(grade) ? 'bad' : 'unknown';
  return <div className="legacy-hierarchy-hygiene" title={list(hygiene.notes).map(text).join('; ')}>Hygiene <b className={`legacy-hierarchy-${color}`}>{[grade, score].filter(Boolean).join(' ')}</b></div>;
}
function Captain({ captain, project, status }: { captain: Data; project: Data; status: Data }) {
  const workers = list(captain.workers).map(data), live = workers.filter(worker => !endedStates.has(reportedState(worker)));
  const ended = workers.filter(worker => endedStates.has(reportedState(worker)));
  const summary = new Map<string, number>();
  for (const worker of live) { const state = reportedState(worker); summary.set(state, (summary.get(state) ?? 0) + 1); }
  const counts = { ...data(project.stages ?? project.counts), ...data(captain.counts) };
  const done = number(captain.done) ?? number(counts.done), total = number(captain.total) ?? number(project.total);
  const inProgress = number(counts.in_progress), state = reportedState(captain);
  return <article className="legacy-hierarchy-captain" data-captain-key={text(captain.key ?? captain.project ?? captain.id)}>
    <header className="legacy-hierarchy-captain-heading"><div><b title="Project Commodore">{text(captain.name ?? captain.key ?? captain.project) || 'Commodore'}</b> <State state={state} /></div>
      <ModelBadge model={text(captain.model) || 'unknown'} effort={text(captain.effort)} tier={text(captain.tier)} />
      <div className="legacy-hierarchy-screen legacy-hierarchy-muted">{screenName(captain)} <Host record={captain} /></div>
      {(done !== null || total !== null || inProgress !== null) && <div className="legacy-hierarchy-progress">{done !== null ? `${done}/${total ?? '?'} done` : total !== null ? `${total} tasks · done not reported` : ''}{inProgress !== null && <span className="legacy-hierarchy-warn">{done !== null || total !== null ? ' · ' : ''}{inProgress} in progress</span>}</div>}
      <Hygiene value={captain.hygiene ?? status.hygiene} />
      {summary.size > 0 && <div className="legacy-hierarchy-worker-summary">{[...summary].map(([reported, count]) => <span key={reported} title={`${count} screen(s), producer state: ${reported}`}><i aria-hidden="true" className={`legacy-hierarchy-dot legacy-hierarchy-dot--${stateClass(reported)}`} /><span>{count} {reported}</span></span>)}</div>}
      {!workers.length && <div className="legacy-hierarchy-muted">No workers reported</div>}
    </header>
    <WorkerRows workers={live} />
    {ended.length > 0 && <details className="legacy-hierarchy-ended"><summary>+{ended.length} finished / ended screens</summary><WorkerRows workers={ended} /></details>}
  </article>;
}

/** A compact view of source observations, including ordinary manifest-backed project lanes. */
export function LegacyHierarchy({ snapshot }: { snapshot: LegacySnapshot }) {
  const profile = useProfile();
  const hierarchy = data(snapshot.hierarchy), captains = hierarchyCaptains(snapshot), superSource = data(snapshot.super_captain);
  const superCaptain = Object.keys(data(hierarchy.super_captain)).length ? data(hierarchy.super_captain) : data(superSource.screen);
  const owner = identity(hierarchy.owner ?? hierarchy.jepson) || 'Owner not reported';
  const watcher = data(hierarchy.watcher ?? hierarchy.grokbot), watcherName = identity(hierarchy.watcher ?? hierarchy.grokbot) || 'Watcher not reported';
  const isGrokbot = /grok\s*bot/i.test(watcherName);
  const routines = list(watcher.routines), lastCheck = data(superSource.last_check), superStatus = data(superSource.status);
  const ticker = typeof superCaptain.ticker_alive === 'boolean' ? superCaptain.ticker_alive : data(superSource.ticker).alive;
  const rollout = data(snapshot.rollout), rolloutId = text(data(rollout.project).id), unassigned = list(hierarchy.unassigned).map(data);
  return <section className="legacy-hierarchy" aria-label="Fleet hierarchy">
    <div className="legacy-hierarchy-heading"><h2>Hierarchy</h2><span className="legacy-hierarchy-muted">Owner → watcher → Admiral → Commodores → agents</span></div>
    <div className="legacy-hierarchy-chain-scroll"><div className="legacy-hierarchy-chain">
      <div className="legacy-hierarchy-node legacy-hierarchy-person"><ProfileAvatar name={owner} profile={profile} className="legacy-hierarchy-avatar" /><div><b>{owner}</b><div className="legacy-hierarchy-muted">owner</div></div></div>
      <span className="legacy-hierarchy-arrow" aria-hidden="true">→</span>
      <div className="legacy-hierarchy-node legacy-hierarchy-person">{isGrokbot && <img className="legacy-hierarchy-grokbot" src="/brand/grokbot.png" alt="Grokbot" width="44" height="44" />}<div><b>{watcherName}</b>{routines.length > 0 ? <details className="legacy-hierarchy-routines"><summary>{routines.length} routines</summary><ul>{routines.map((routine, index) => <li key={index}>{identity(routine) || text(data(routine).description) || 'Routine details not reported'}</li>)}</ul></details> : <div className="legacy-hierarchy-muted">watcher</div>}</div></div>
      <span className="legacy-hierarchy-arrow" aria-hidden="true">→</span>
      <div className="legacy-hierarchy-node legacy-hierarchy-super"><div><b>Admiral</b> <State state={reportedState(superCaptain)} /></div><ModelBadge model={text(superCaptain.model) || 'unknown'} effort={text(superCaptain.effort)} tier={text(superCaptain.tier)} /><div className="legacy-hierarchy-muted legacy-hierarchy-screen">{screenName(superCaptain)}</div><div className="legacy-hierarchy-muted">{ticker === true ? 'ticker ✓' : ticker === false ? 'ticker not running' : 'ticker not reported'}{text(lastCheck.no) ? ` · check #${text(lastCheck.no)}` : ''}{text(lastCheck.utc ?? superStatus.updated_utc) ? ` · ${text(lastCheck.utc ?? superStatus.updated_utc)}` : ''}</div></div>
    </div></div>
    {captains.length > 0 ? <div className="legacy-hierarchy-columns-scroll"><div className="legacy-hierarchy-columns" style={{ gridTemplateColumns: `repeat(${captains.length}, minmax(150px, 1fr))` }}>{captains.map((captain, index) => {
      const key = text(captain.key ?? captain.project ?? captain.id);
      return <Captain key={`${key}:${index}`} captain={captain} project={data(snapshot.projects[key] ?? (key === rolloutId ? rollout : undefined))} status={data(data(superStatus.captains)[key])} />;
    })}</div></div> : <div className="legacy-hierarchy-muted">No Commodores reported.</div>}
    {unassigned.length > 0 && <details className="legacy-hierarchy-unassigned"><summary>Producer unassigned screens ({unassigned.length})</summary><WorkerRows workers={unassigned} /></details>}
    <div className="legacy-hierarchy-legend"><span><i className="legacy-hierarchy-dot legacy-hierarchy-dot--active" />active</span><span><i className="legacy-hierarchy-dot legacy-hierarchy-dot--idle" />idle</span><span><i className="legacy-hierarchy-dot legacy-hierarchy-dot--blocked" />blocked / quota</span><span><i className="legacy-hierarchy-dot legacy-hierarchy-dot--remote" />remote</span><span><i className="legacy-hierarchy-dot legacy-hierarchy-dot--unknown" />unknown</span><span>State and model labels are producer observations.</span></div>
  </section>;
}

export default LegacyHierarchy;
