import admiraltyMark from '../assets/admiralty-mark.webp';
import { lazy, Suspense, useEffect, useMemo, useState } from 'react';
import { fleetTotals, formatUnknown, type NativeFleet } from '../api';
import { watchFleet, type ConnectionState } from '../events';
import LegacyDetailed from './LegacyDetailed';
import NativeTasks from './NativeTasks';
import Models from './Models';
import { useProfile } from '../profile';
import { ProfileAvatar, ProfileSettings } from './Profile';
import { navigate, useRoute, type RouteName } from '../router';

const FleetHealth = lazy(() => import('../features/health/FleetHealth'));
const LiveViews = lazy(() => import('../features/live-views/LiveViews'));
const CommandJourneys = lazy(() => import('./CommandJourneys'));
const Intake = lazy(() => import('./Intake'));
const Incidents = lazy(() => import('../features/incidents/Incidents'));
type View = 'Fleet health' | 'Legacy detailed view' | 'Overview' | 'Workplans' | 'Coordination' | 'Commands' | 'Intake' | 'Incidents' | 'Models' | 'Git Flow' | 'Fleet Atlas';

function App() {
  const route = useRoute();
  const destinations: Record<RouteName, View> = { health: 'Fleet health', detail: 'Legacy detailed view', overview: 'Overview', workplans: 'Workplans', coordination: 'Coordination', commands: 'Commands', intake: 'Intake', incidents: 'Incidents', models: 'Models', git: 'Git Flow', atlas: 'Fleet Atlas' };
  const view = destinations[route.name];
  const setView = (value: View) => navigate((Object.keys(destinations) as RouteName[]).find(key => destinations[key] === value) ?? 'detail');
  const visualView = route.name === 'git' || route.name === 'atlas';
  const [fleet, setFleet] = useState<NativeFleet | null>(null);
  const [error, setError] = useState('');
  const [updated, setUpdated] = useState<Date | null>(null);
  const [token, setToken] = useState('');
  const [tokenDraft, setTokenDraft] = useState('');
  const [showAccess, setShowAccess] = useState(false);
  const [showProfile, setShowProfile] = useState(false);
  const profile = useProfile();
  const profileName = profile.displayName || 'Operator';
  const [theme, setTheme] = useState<'dark' | 'light'>('dark');
  const [connection, setConnection] = useState<ConnectionState>('connecting');
  const [retry, setRetry] = useState(0);
  const healthView = view === 'Fleet health';
  const legacyView = view === 'Legacy detailed view';
  const modelsView = view === 'Models';
  const commandsView = view === 'Commands';
  const intakeView = view === 'Intake';
  const incidentsView = view === 'Incidents';
  const [hasOpenedIntake, setHasOpenedIntake] = useState(intakeView);

  useEffect(() => {
    if (intakeView) setHasOpenedIntake(true);
  }, [intakeView]);

  useEffect(() => {
    setFleet(null); setUpdated(null); setError('');
    if (visualView || healthView || intakeView || incidentsView) return;
    return watchFleet({ token: token || undefined,
      onFleet: (result) => {
        setFleet(result);
        setUpdated(new Date());
      }, onState: setConnection, onError: setError,
    });
  }, [token, retry, visualView, healthView, intakeView, incidentsView]);

  const totals = useMemo(() => fleet ? fleetTotals(fleet) : null, [fleet]);

  return <div className={`app theme-${theme} ${legacyView ? 'fleet-console' : ''} ${visualView ? 'visual-console' : ''}`}>
    <aside className="sidebar">
      <a className="brand" href="#/detail">
        <img className="brand-mark" src={admiraltyMark} fetchPriority="low" decoding="async" alt="Admiralty" /><span><strong>ADMIRALTY</strong> <small>FLEET CONTROL</small></span>
      </a>
      <div className="workspace-label">WORKSPACE</div>
      <div className="workspace"><span className="workspace-dot" /> Fleet overview <span className="chevron" data-icon="⌄" aria-hidden="true" /></div>
      <nav aria-label="Main navigation">
        <div className="nav-label">MONITOR</div>
        {(['Fleet health', 'Legacy detailed view', 'Git Flow', 'Fleet Atlas', 'Overview', 'Workplans', 'Coordination', 'Commands', 'Intake', 'Incidents', 'Models'] as View[]).map((item, index) => <button key={item} className={`nav-item ${view === item ? 'active' : ''}`} title={item} aria-current={view === item ? 'page' : undefined} onClick={() => setView(item)}>
          <span className="nav-icon" data-icon={['◈', '▦', '⑂', '✦', '◫', '≡', '◉', '⌘', '↧', '!', '⚙'][index]} aria-hidden="true" />{item === 'Overview' && fleet?.simulated ? 'Demo overview' : item}
          {item === 'Coordination' && fleet && <span className="nav-count">{fleet.coordination.queued_messages}</span>}
        </button>)}
      </nav>
      <div className="sidebar-spacer" />
      <div className="brand-slot"><img className="brand-slot-mark" src={admiraltyMark} fetchPriority="low" decoding="async" alt="Admiralty mascot" /><span>Admiralty<br /><small>Coordinate. Build. Verify.</small></span></div>
      <div className="sidebar-footer"><ProfileAvatar name={profileName} profile={profile} /><span><b>{profileName}</b><small>Local session</small></span><button aria-label="Access settings" onClick={() => setShowAccess(true)}><span data-icon="···" aria-hidden="true" /></button></div>
    </aside>

    <main className="main-area">
      <header className="topbar">
        <div className="breadcrumbs">Fleet <span data-icon="/" aria-hidden="true" /> <strong>{view}</strong></div>
        <div className="top-actions"><span className={`connection ${!healthView && !legacyView && !intakeView && !incidentsView && connection === 'connected' ? 'online' : !healthView && !legacyView && !intakeView && !incidentsView && connection === 'paused' ? 'offline' : ''}`} aria-live="polite"><i />{healthView ? 'Producer health view' : intakeView || incidentsView ? 'Scoped operator access' : legacyView ? 'Detailed producer view' : connection === 'connected' ? 'Live updates' : connection === 'paused' ? 'Updates paused' : connection === 'reconnecting' ? 'Reconnecting' : 'Connecting'}</span>
          <button className="icon-button" aria-label={`Switch to ${theme === 'dark' ? 'light' : 'dark'} theme`} onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}><span data-icon={theme === 'dark' ? '☼' : '◐'} aria-hidden="true" /></button>
          <button className="operator-button" title="Your profile" onClick={() => setShowProfile(true)}><ProfileAvatar name={profileName} className="small" profile={profile} /><span>{profileName}</span><span data-icon="⌄" aria-hidden="true" /></button>
        </div>
      </header>

      <div className={`content ${legacyView ? 'legacy-content' : ''}`}>
        <section className="page-heading"><div><div className="eyebrow">ADMIRALTY / {legacyView || healthView ? 'OBSERVED PRODUCER FLEET' : 'NATIVE CONTROL PLANE'}</div><h1>{legacyView ? 'Fleet detail' : view === 'Overview' && fleet?.simulated ? 'Demo overview' : view}</h1><p>{healthView ? 'Source freshness · Git review · coverage gaps · compact agent context' : legacyView ? 'Live project progress · hierarchy · model mix · token efficiency' : view === 'Overview' ? 'Native task projections; separate from the detailed producer fleet.' : view === 'Workplans' ? 'Native task stages and current ownership.' : commandsView ? 'Create work, revise pending contracts and inspect task evidence.' : incidentsView ? 'Scoped incident history and local improvement proposals.' : intakeView ? 'Preserve an original request, inspect unresolved facts and record an immutable capture.' : view === 'Models' ? 'Provider and model configuration.' : 'Compact native coordination activity and message acknowledgements.'}</p></div>
          <div className="heading-meta"><span className={`mode-pill ${!healthView && !legacyView && !visualView && !intakeView && !incidentsView && fleet?.simulated ? 'simulation' : ''}`}>{healthView ? 'OBSERVED HEALTH' : incidentsView ? 'SCOPED INCIDENTS' : intakeView ? 'SCOPED INTAKE' : legacyView ? 'PRODUCER OBSERVATIONS' : fleet?.simulated ? 'SIMULATION · SYNTHETIC TASKS' : fleet ? 'NATIVE SNAPSHOT' : 'AWAITING DATA'}</span>{!healthView && !legacyView && !intakeView && !incidentsView && <span className="freshness"><i />{updated ? `Updated ${updated.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}` : 'No snapshot yet'}</span>}</div>
        </section>
        {healthView && <Suspense fallback={<div role="status">Loading fleet health…</div>}><FleetHealth token={token || undefined} onAccess={() => setShowAccess(true)} /></Suspense>}
        {visualView && <Suspense fallback={<div className="loading-card" role="status">Loading observatory…</div>}><LiveViews mode={route.name === 'atlas' ? 'atlas' : 'git'} token={token || undefined} onAccess={() => setShowAccess(true)} /></Suspense>}
        {incidentsView && <Suspense fallback={<div role="status">Loading incident history…</div>}><Incidents token={token} /></Suspense>}
        {legacyView && <LegacyDetailed token={token || undefined} />}
        {!healthView && !legacyView && !visualView && !intakeView && !incidentsView && fleet?.simulated && <div className="simulation-note">Simulation: these synthetic projects and tasks are separate from the real producer fleet.</div>}
        {!healthView && !legacyView && !visualView && !modelsView && !commandsView && !intakeView && !incidentsView && error && <div className="error-banner" role="status"><span>{connection === 'paused' ? 'Updates paused' : 'Connection issue'}</span><span>{error}{connection === 'reconnecting' ? ' · Reconnecting automatically.' : ''}</span><button className="text-button" onClick={() => setRetry(value => value + 1)}>Retry connection</button></div>}
        {!healthView && !legacyView && !visualView && !modelsView && !commandsView && !intakeView && !incidentsView && !fleet && !error && <div className="loading-card">Connecting to the local native service…</div>}
        {fleet && view === 'Overview' && <Overview fleet={fleet} totals={totals!} onWorkplans={() => setView('Workplans')} />}
        {view === 'Workplans' && <NativeTasks fleet={fleet} error={error} connection={connection} />}
        {fleet && view === 'Coordination' && <Coordination fleet={fleet} />}
        {view === 'Models' && <Models token={token || undefined} onAccess={() => setShowAccess(true)} />}
        {commandsView && <Suspense fallback={<div className="loading-card" role="status">Loading task commands…</div>}><CommandJourneys token={token || undefined} onAccess={() => setShowAccess(true)} onRefreshSnapshot={() => setRetry(value => value + 1)} /></Suspense>}
        {(intakeView || hasOpenedIntake) && <div hidden={!intakeView}><Suspense fallback={<div className="loading-card" role="status">Loading request intake…</div>}><Intake token={token || undefined} onAccess={() => setShowAccess(true)} /></Suspense></div>}
        <footer className="page-footer"><span>{healthView ? 'READ ONLY · PRODUCER HEALTH BRIEF' : incidentsView ? 'SCOPED INCIDENT HISTORY · LOCAL PROPOSALS' : intakeView ? 'RAW REQUEST INTAKE · SEPARATE SCOPED ACCESS' : route.name === 'git' ? 'GIT OBSERVATIONS · OPERATOR CHAT' : commandsView ? 'OPERATOR COMMANDS · SESSION ACCESS' : modelsView ? 'MODEL POLICY CONFIGURATION' : `READ ONLY · SOURCE: ${legacyView ? 'PRODUCER SNAPSHOT' : 'NATIVE FLEET API'}`}</span><span>{healthView ? 'Coverage gaps and original source clocks preserved' : incidentsView ? 'Review required before action' : intakeView ? 'No execution or acceptance grant' : legacyView ? 'Source provenance and uncertainty preserved' : <>Native execution {fleet?.native_execution_qualified ? 'qualified' : 'not qualified'} <b data-icon="·" aria-hidden="true" /> Usage {formatUnknown(fleet?.coordination.measured_tokens)}</>}</span></footer>
      </div>
    </main>
    {showProfile && <ProfileSettings onClose={() => setShowProfile(false)} onAccess={() => { setShowProfile(false); setShowAccess(true); }} />}
    {showAccess && <div className="modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) setShowAccess(false); }}>
      <section className="access-modal" role="dialog" aria-modal="true" aria-labelledby="access-title"><button className="modal-close" onClick={() => setShowAccess(false)} aria-label="Close access settings"><span data-icon="×" aria-hidden="true" /></button><div className="eyebrow">PRIVATE MODE</div><h2 id="access-title">Session access</h2><p>Bearer token stays in this page's memory and is sent only as an authorization header. It is never stored in the URL.</p><label htmlFor="bearer-token">Bearer token</label><input id="bearer-token" type="password" autoComplete="off" value={tokenDraft} onChange={(event) => setTokenDraft(event.target.value)} placeholder="Paste token" onKeyDown={(event) => { if (event.key === 'Enter') { setToken(tokenDraft.trim()); setShowAccess(false); } }} /><div className="modal-actions"><button className="secondary-button" onClick={() => { setToken(''); setTokenDraft(''); setShowAccess(false); }}>Clear session</button><button className="primary-button" onClick={() => { setToken(tokenDraft.trim()); setShowAccess(false); }}>Apply token</button></div></section>
    </div>}
  </div>;
}

function Overview({ fleet, totals, onWorkplans }: { fleet: NativeFleet; totals: ReturnType<typeof fleetTotals>; onWorkplans: () => void }) {
  const stageItems = [
    ['Accepted', totals.accepted, 'accepted'], ['Claimed or running', totals.running, 'running'], ['Awaiting verification', totals.verifying, 'verifying'], ['Blocked', totals.blocked, 'blocked'],
  ] as const;
  return <>
    <section className="metric-grid" aria-label="Fleet summary">
      <Metric label="Active projects" value={totals.projects} detail="Across current fleet" icon="⌘" />
      <Metric label="Tasks in scope" value={totals.tasks} detail="Tracked work items" icon="▤" />
      <Metric label="Claimed or running" value={totals.running} detail="Includes claims; execution unqualified" icon="◷" tone="blue" />
      <Metric label="Blocked" value={totals.blocked} detail="Needs attention" icon="⏸" tone="gold" />
    </section>
    <section className="overview-grid">
      <div className="panel project-panel"><div className="panel-head"><div><div className="eyebrow">PROJECTS</div><h2>Fleet at a glance</h2></div><button className="text-button" onClick={onWorkplans}>View workplans <span data-icon="→" aria-hidden="true" /></button></div>
        <div className="project-list">{fleet.projects.length ? fleet.projects.map((project) => {
          const progress = project.total ? Math.min(100, Math.round((project.accepted / project.total) * 100)) : 0;
          return <article className="project-card" key={project.id}><div className="project-card-top"><span className="project-avatar">{project.name.slice(0, 1).toUpperCase()}</span><div className="project-name"><h3>{project.name}</h3><small>{project.id}</small></div><span className="project-total">{project.total} <small>tasks</small></span></div><div className="progress-track" role="progressbar" aria-label={`${project.name} accepted tasks`} aria-valuenow={progress} aria-valuemin={0} aria-valuemax={100}><span style={{ width: `${progress}%` }} /></div><div className="project-stages"><span><i className="dot accepted" />{project.accepted} accepted</span><span><i className="dot running" />{project.running} claimed / running</span><span><i className="dot blocked" />{project.blocked} blocked</span><span><i className="dot verifying" />{project.awaiting_verification} awaiting verification</span></div></article>;
        }) : <Empty message="No projects in the current snapshot." />}</div>
      </div>
      <div className="panel delivery-panel"><div className="panel-head"><div><div className="eyebrow">DELIVERY</div><h2>Task stages</h2></div><span className="tiny-badge">{totals.tasks} TOTAL</span></div><div className="stage-list">{stageItems.map(([label, count, kind]) => <div className="stage-row" key={label}><span className={`stage-icon ${kind}`} data-icon={kind === 'accepted' ? '✓' : kind === 'running' ? '↗' : kind === 'blocked' ? '!' : '◷'} aria-hidden="true" /><span className="stage-label">{label}</span><strong>{count}</strong><span className="stage-track"><i className={kind} style={{ width: `${totals.tasks ? Math.max(count ? 3 : 0, count / totals.tasks * 100) : 0}%` }} /></span></div>)}</div><div className="delivery-note"><span>Quality gate</span><b>Not yet qualified</b><small>Candidate reports await independent verification. Native execution remains disabled.</small></div></div>
    </section>
    <section className="panel activity-panel"><div className="panel-head"><div><div className="eyebrow">COORDINATION</div><h2>Message pulse</h2></div><span className="panel-caption">No routine model turns</span></div><div className="coord-stats"><div><span className="stat-label">Recorded messages</span><strong>{fleet.coordination.queued_messages}</strong><small>Durably queued in snapshot</small></div><div><span className="stat-label">Acknowledged</span><strong>{fleet.coordination.acknowledged_messages}</strong><small>Recorded in snapshot</small></div><div><span className="stat-label">Routine turns</span><strong>{fleet.coordination.routine_model_turns}</strong><small>Current API contract</small></div><div><span className="stat-label">Measured tokens</span><strong>{formatUnknown(fleet.coordination.measured_tokens)}</strong><small>Unknown is distinct from zero</small></div></div></section>
  </>;
}

function Metric({ label, value, detail, icon, tone = '' }: { label: string; value: number; detail: string; icon: string; tone?: string }) {
  return <article className="metric-card"><div className="metric-top"><span>{label}</span><span className={`metric-icon ${tone}`} data-icon={icon} aria-hidden="true" /></div><strong>{value}</strong><small>{detail}</small><div className={`metric-baseline ${tone}`} /></article>;
}

function Coordination({ fleet }: { fleet: NativeFleet }) {
  const items = [
    { label: 'Recorded messages', value: fleet.coordination.queued_messages, detail: 'Message entries recorded', color: 'gold' },
    { label: 'Acknowledged messages', value: fleet.coordination.acknowledged_messages, detail: 'Acknowledgements recorded', color: 'blue' },
    { label: 'Routine model turns', value: fleet.coordination.routine_model_turns, detail: 'No routine turns observed', color: 'green' },
  ];
  return <div className="coordination-layout"><section className="panel coordination-summary"><div className="panel-head"><div><div className="eyebrow">COORDINATION / SNAPSHOT</div><h2>Message statistics</h2></div><span className="tiny-badge">CURRENT</span></div><div className="coordination-cards">{items.map((item) => <article className="coordination-card" key={item.label}><span className={`coordination-icon ${item.color}`} data-icon={item.color === 'gold' ? '◌' : item.color === 'blue' ? '✓' : '↔'} aria-hidden="true" /><div><span>{item.label}</span><strong>{item.value}</strong><small>{item.detail}</small></div></article>)}</div></section><section className="panel accounting-panel"><div className="eyebrow">USAGE ACCOUNTING</div><h2>Measured usage</h2><div className="accounting-value">{formatUnknown(fleet.coordination.measured_tokens)} <span>tokens</span></div><p>The API reports measured token usage when available. Missing usage remains unknown; it is not treated as zero.</p></section><div className="read-only-note"><span className="lock-icon" data-icon="⌑" aria-hidden="true" /><div><b>Coordination is read only</b><span>This dashboard displays observed message counts. Sending, replying and changing task state are not available here.</span></div></div></div>;
}

function Empty({ message }: { message: string }) { return <div className="empty-state"><span data-icon="⌁" aria-hidden="true" />{message}</div>; }

export default App;
