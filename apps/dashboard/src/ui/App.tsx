import admiraltyMark from '../assets/admiralty-mark.webp';
import { lazy, Suspense, useEffect, useMemo, useState } from 'react';
import { CommandPalette, type PaletteCommand } from './CommandPalette';
import './shell.css';
import { fleetTotals, formatUnknown, type NativeFleet } from '../api';
import { watchFleet, type ConnectionState } from '../events';
import LegacyDetailed from './LegacyDetailed';
import NativeTasks from './NativeTasks';
import Models from './Models';
import { useProfile } from '../profile';
import { ProfileAvatar, ProfileSettings } from './Profile';
import { navigate, useRoute, type RouteName } from '../router';

const Operations = lazy(() => import('../features/operations/Operations'));
const AgentBrief = lazy(() => import('../features/brief-health/FleetHealth'));
const LiveViews = lazy(() => import('../features/live-views/LiveViews'));
const CommandJourneys = lazy(() => import('./CommandJourneys'));
const Intake = lazy(() => import('./Intake'));
const Incidents = lazy(() => import('../features/incidents/Incidents'));
const FleetHealth = lazy(() => import('../features/health/FleetHealth'));
type View = 'Fleet detail' | 'Fleet health' | 'Fleet operations' | 'Agent brief' | 'Overview' | 'Workplans' | 'Coordination' | 'Commands' | 'Intake' | 'Incidents' | 'Models' | 'Git Flow' | 'Fleet Atlas';
const destinations: Record<RouteName, View> = { detail: 'Fleet detail', health: 'Fleet health', operations: 'Fleet operations', brief: 'Agent brief', overview: 'Overview', workplans: 'Workplans', coordination: 'Coordination', commands: 'Commands', intake: 'Intake', incidents: 'Incidents', models: 'Models', git: 'Git Flow', atlas: 'Fleet Atlas' };

/** Navigation grouped by what the operator is doing: watching, acting or configuring. */
const NAV_GROUPS: ReadonlyArray<{ label: string; items: ReadonlyArray<{ view: View; icon: string }> }> = [
  { label: 'Observe', items: [{ view: 'Fleet detail', icon: '▦' }, { view: 'Fleet health', icon: '♥' }, { view: 'Fleet operations', icon: '⚑' }, { view: 'Agent brief', icon: '≡' }, { view: 'Git Flow', icon: '⑂' }, { view: 'Fleet Atlas', icon: '✦' }] },
  { label: 'Operate', items: [{ view: 'Overview', icon: '◫' }, { view: 'Workplans', icon: '≡' }, { view: 'Coordination', icon: '◉' }, { view: 'Commands', icon: '⌘' }, { view: 'Intake', icon: '↧' }, { view: 'Incidents', icon: '!' }] },
  { label: 'Configure', items: [{ view: 'Models', icon: '⚙' }] },
];
const groupOf = (view: View) => NAV_GROUPS.find(group => group.items.some(item => item.view === view))?.label ?? 'Observe';
const DESCRIPTIONS: Partial<Record<View, string>> = {
  'Fleet health': 'Host, Git and service observations, with a bounded log digest.',
  'Fleet operations': 'Project progress, source freshness and operational findings from the existing producer feed.',
  'Agent brief': 'A bounded producer summary with original clocks and explicit source coverage.',
  'Fleet detail': 'Project progress, command hierarchy, model mix and token efficiency from the producer feed.',
  Overview: 'Native task projections; separate from the detailed producer fleet.',
  Workplans: 'Native task stages and who currently holds each task.',
  Coordination: 'Native coordination activity and message acknowledgements.',
  Commands: 'Create work, revise pending contracts and inspect task evidence.',
  Incidents: 'Scoped incident history and local improvement proposals.',
  Intake: 'Preserve an original request, inspect unresolved facts and record an immutable capture.',
  Models: 'Provider and model configuration.',
};
const THEME_KEY = 'admiralty.theme';
function storedTheme(): 'dark' | 'light' {
  // An explicit choice persists per browser; the product default stays dark.
  try { return window.localStorage.getItem(THEME_KEY) === 'light' ? 'light' : 'dark'; } catch { return 'dark'; }
}

function App() {
  const route = useRoute();
  const view = destinations[route.name];
  const setView = (value: View) => navigate((Object.keys(destinations) as RouteName[]).find(key => destinations[key] === value) ?? 'detail');
  const visualView = route.name === 'git' || route.name === 'atlas';
  const healthView = route.name === 'health' || route.name === 'operations' || route.name === 'brief';
  const [fleet, setFleet] = useState<NativeFleet | null>(null);
  const [error, setError] = useState('');
  const [updated, setUpdated] = useState<Date | null>(null);
  const [token, setToken] = useState('');
  const [tokenDraft, setTokenDraft] = useState('');
  const [showAccess, setShowAccess] = useState(false);
  const [showProfile, setShowProfile] = useState(false);
  const profile = useProfile();
  const profileName = profile.displayName || 'Operator';
  const [theme, setThemeState] = useState<'dark' | 'light'>(storedTheme);
  const setTheme = (next: 'dark' | 'light') => {
    setThemeState(next);
    try { window.localStorage.setItem(THEME_KEY, next); } catch { /* storage unavailable: keep in memory */ }
  };
  const [showPalette, setShowPalette] = useState(false);
  const [connection, setConnection] = useState<ConnectionState>('connecting');
  const [retry, setRetry] = useState(0);
  const legacyView = view === 'Fleet detail';
  const modelsView = view === 'Models';
  const commandsView = view === 'Commands';
  const intakeView = view === 'Intake';
  const incidentsView = view === 'Incidents';

  const [hasOpenedIntake, setHasOpenedIntake] = useState(intakeView);
  // Views that never read the native fleet do not hold its event stream open.
  const fleetFree = visualView || intakeView || incidentsView || healthView;

  useEffect(() => {
    if (intakeView) setHasOpenedIntake(true);
  }, [intakeView]);

  useEffect(() => {
    setFleet(null); setUpdated(null); setError('');
    if (fleetFree) return;
    return watchFleet({ token: token || undefined,
      onFleet: (result) => {
        setFleet(result);
        setUpdated(new Date());
      }, onState: setConnection, onError: setError,
    });
  }, [token, retry, fleetFree]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        setShowPalette(open => !open);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  useEffect(() => {
    if (!showAccess) return;
    const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape') setShowAccess(false); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [showAccess]);

  const totals = useMemo(() => fleet ? fleetTotals(fleet) : null, [fleet]);
  const paletteCommands = useMemo<PaletteCommand[]>(() => [
    ...NAV_GROUPS.flatMap(group => group.items.map(item => ({
      id: `view-${item.view}`, label: item.view === 'Fleet detail' ? 'Fleet detail' : item.view, group: `${group.label.charAt(0)}${group.label.slice(1).toLowerCase()}`,
      keywords: item.view === 'Fleet detail' ? 'legacy detailed view projects trends' : item.view === 'Fleet health' ? 'hosts disk memory git hygiene logs metrics readiness' : undefined,
      run: () => navigate((Object.keys(destinations) as RouteName[]).find(key => destinations[key] === item.view) ?? 'detail'),
    }))),
    { id: 'theme', label: theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme', group: 'Preferences', keywords: 'appearance color mode', run: () => setTheme(theme === 'dark' ? 'light' : 'dark') },
    { id: 'access', label: 'Session access', group: 'Preferences', keywords: 'token bearer login authenticate', run: () => setShowAccess(true) },
    { id: 'profile', label: 'Profile', group: 'Preferences', keywords: 'name avatar', run: () => setShowProfile(true) },
  ], [theme]);

  return <div className={`app theme-${theme} ${legacyView ? 'fleet-console' : ''} ${visualView ? 'visual-console' : ''}`}>
    <aside className="sidebar">
      <a className="brand" href="#/detail">
        <img className="brand-mark" src={admiraltyMark} fetchPriority="low" decoding="async" alt="Admiralty" /><span><strong>Admiralty</strong> <small>Fleet control</small></span>
      </a>
      <nav aria-label="Main navigation">
        {NAV_GROUPS.map(group => <div className="nav-group" role="group" aria-label={group.label} key={group.label}>
          {group.items.map(({ view: item, icon }) => <button key={item} className={`nav-item ${view === item ? 'active' : ''}`} title={item} aria-current={view === item ? 'page' : undefined} onClick={() => setView(item)}>
            <span className="nav-icon" data-icon={icon} aria-hidden="true" /><span className="nav-text">{item === 'Overview' && fleet?.simulated ? 'Demo overview' : item}</span>
            {item === 'Coordination' && fleet && <span className="nav-count">{fleet.coordination.queued_messages}</span>}
          </button>)}
        </div>)}
      </nav>
      <div className="sidebar-spacer" />
      <div className="sidebar-footer"><ProfileAvatar name={profileName} profile={profile} /><span><b>{profileName}</b><small>Local session</small></span><button aria-label="Access settings" onClick={() => setShowAccess(true)}><span data-icon="···" aria-hidden="true" /></button></div>
    </aside>

    <main className="main-area">
      <header className="topbar">
        <div className="breadcrumbs"><span className="crumb-group">{groupOf(view)}</span> <span data-icon="/" aria-hidden="true" /> <strong>{view === 'Overview' && fleet?.simulated ? 'Demo overview' : view}</strong></div>
        <div className="top-actions"><button className="palette-button" title="Go to a view or run a command" aria-keyshortcuts="Control+K Meta+K" aria-haspopup="dialog" aria-expanded={showPalette} onClick={() => setShowPalette(true)}><span>Go to…</span><kbd>Ctrl K</kbd></button>
          <span className={`connection ${!legacyView && !intakeView && !incidentsView && !healthView && connection === 'connected' ? 'online' : !legacyView && !intakeView && !incidentsView && !healthView && connection === 'paused' ? 'offline' : ''}`} aria-live="polite"><i />{intakeView || incidentsView ? 'Scoped operator access' : legacyView ? 'Detailed producer view' : route.name === 'brief' || route.name === 'operations' ? 'Producer observations' : healthView ? 'Observations and service checks' : connection === 'connected' ? 'Live updates' : connection === 'paused' ? 'Updates paused' : connection === 'reconnecting' ? 'Reconnecting' : 'Connecting'}</span>
          <button className="icon-button" aria-label={`Switch to ${theme === 'dark' ? 'light' : 'dark'} theme`} onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}><span data-icon={theme === 'dark' ? '☼' : '◐'} aria-hidden="true" /></button>
          <button className="operator-button" title="Your profile" onClick={() => setShowProfile(true)}><ProfileAvatar name={profileName} className="small" profile={profile} /><span>{profileName}</span><span data-icon="⌄" aria-hidden="true" /></button>
        </div>
      </header>

      <div className={`content ${legacyView ? 'legacy-content' : ''}`}>
        <section className="page-heading"><div><h1>{legacyView ? 'Fleet detail' : view === 'Overview' && fleet?.simulated ? 'Demo overview' : view}</h1><p>{DESCRIPTIONS[view]}</p></div>
          <div className="heading-meta"><span className={`mode-pill ${!legacyView && !visualView && !intakeView && !incidentsView && !healthView && fleet?.simulated ? 'simulation' : ''}`}>{incidentsView ? 'Scoped incidents' : intakeView ? 'Scoped intake' : legacyView || healthView ? 'Producer observations' : fleet?.simulated ? 'Simulation: synthetic tasks' : fleet ? 'Native snapshot' : 'Awaiting data'}</span>{!legacyView && !intakeView && !incidentsView && !healthView && <span className="freshness"><i />{updated ? `Updated ${updated.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}` : 'No snapshot yet'}</span>}</div>
        </section>
        {visualView && <Suspense fallback={<div className="loading-card" role="status">Loading observatory…</div>}><LiveViews mode={route.name === 'atlas' ? 'atlas' : 'git'} token={token || undefined} onAccess={() => setShowAccess(true)} /></Suspense>}
        {route.name === 'health' && <Suspense fallback={<div className="loading-card" role="status">Loading fleet health…</div>}><FleetHealth token={token || undefined} onAccess={() => setShowAccess(true)} /></Suspense>}
        {incidentsView && <Suspense fallback={<div role="status">Loading incident history…</div>}><Incidents token={token} /></Suspense>}
        {route.name === 'operations' && <Suspense fallback={<div className="loading-card" role="status">Loading fleet operations…</div>}><Operations token={token || undefined} /></Suspense>}
        {route.name === 'brief' && <Suspense fallback={<div className="loading-card" role="status">Loading agent brief…</div>}><AgentBrief token={token || undefined} onAccess={() => setShowAccess(true)} /></Suspense>}
        {legacyView && <LegacyDetailed token={token || undefined} />}
        {!legacyView && !visualView && !intakeView && !incidentsView && !healthView && fleet?.simulated && <div className="simulation-note">Simulation: these synthetic projects and tasks are separate from the real producer fleet.</div>}
        {!legacyView && !visualView && !modelsView && !commandsView && !intakeView && !incidentsView && !healthView && error && <div className="error-banner" role="status"><span>{connection === 'paused' ? 'Updates paused' : 'Connection issue'}</span><span>{error}{connection === 'reconnecting' ? ' · Reconnecting automatically.' : ''}</span><button className="text-button" onClick={() => setRetry(value => value + 1)}>Retry connection</button></div>}
        {!legacyView && !visualView && !modelsView && !commandsView && !intakeView && !incidentsView && !healthView && !fleet && !error && <div className="loading-card">Connecting to the local native service…</div>}
        {fleet && view === 'Overview' && <Overview fleet={fleet} totals={totals!} onWorkplans={() => setView('Workplans')} />}
        {view === 'Workplans' && <NativeTasks fleet={fleet} error={error} connection={connection} />}
        {fleet && view === 'Coordination' && <Coordination fleet={fleet} />}
        {view === 'Models' && <Models token={token || undefined} onAccess={() => setShowAccess(true)} />}
        {commandsView && <Suspense fallback={<div className="loading-card" role="status">Loading task commands…</div>}><CommandJourneys token={token || undefined} onAccess={() => setShowAccess(true)} onRefreshSnapshot={() => setRetry(value => value + 1)} /></Suspense>}
        {(intakeView || hasOpenedIntake) && <div hidden={!intakeView}><Suspense fallback={<div className="loading-card" role="status">Loading request intake…</div>}><Intake token={token || undefined} onAccess={() => setShowAccess(true)} /></Suspense></div>}
        <footer className="page-footer"><span>{incidentsView ? 'Scoped incident history and local proposals' : intakeView ? 'Separate scoped access for raw requests' : route.name === 'git' ? 'Git observations and operator chat' : commandsView ? 'Operator commands require session access' : modelsView ? 'Model policy configuration' : route.name === 'brief' || route.name === 'operations' ? 'Read only. Source: producer snapshot' : healthView ? 'Read only. Source: producer snapshot and service checks' : `Read only. Source: ${legacyView ? 'producer snapshot' : 'native fleet API'}`}</span><span>{incidentsView ? 'Review required before action' : intakeView ? 'No execution or acceptance grant' : healthView ? 'Findings describe observations; no action is taken' : legacyView ? 'Source provenance and uncertainty preserved' : <>Native execution {fleet?.native_execution_qualified ? 'qualified' : 'not qualified'} <b data-icon="·" aria-hidden="true" /> Usage {formatUnknown(fleet?.coordination.measured_tokens)}</>}</span></footer>
      </div>
    </main>
    {showProfile && <ProfileSettings onClose={() => setShowProfile(false)} onAccess={() => { setShowProfile(false); setShowAccess(true); }} />}
    {showAccess && <div className="modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) setShowAccess(false); }}>
      <section className="access-modal" role="dialog" aria-modal="true" aria-labelledby="access-title"><button className="modal-close" onClick={() => setShowAccess(false)} aria-label="Close access settings"><span data-icon="×" aria-hidden="true" /></button><h2 id="access-title">Session access</h2><p>Bearer token stays in this page's memory and is sent only as an authorization header. It is never stored in the URL.</p><label htmlFor="bearer-token">Bearer token</label><input id="bearer-token" type="password" autoComplete="off" autoFocus value={tokenDraft} onChange={(event) => setTokenDraft(event.target.value)} placeholder="Paste token" onKeyDown={(event) => { if (event.key === 'Enter') { setToken(tokenDraft.trim()); setShowAccess(false); } }} /><div className="modal-actions"><button className="secondary-button" onClick={() => { setToken(''); setTokenDraft(''); setShowAccess(false); }}>Clear session</button><button className="primary-button" onClick={() => { setToken(tokenDraft.trim()); setShowAccess(false); }}>Apply token</button></div></section>
    </div>}
    {showPalette && <CommandPalette commands={paletteCommands} onClose={() => setShowPalette(false)} />}
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
      <div className="panel project-panel"><div className="panel-head"><div><h2>Fleet at a glance</h2></div><button className="text-button" onClick={onWorkplans}>View workplans <span data-icon="→" aria-hidden="true" /></button></div>
        <div className="project-list">{fleet.projects.length ? fleet.projects.map((project) => {
          const progress = project.total ? Math.min(100, Math.round((project.accepted / project.total) * 100)) : 0;
          return <article className="project-card" key={project.id}><div className="project-card-top"><span className="project-avatar">{project.name.slice(0, 1).toUpperCase()}</span><div className="project-name"><h3>{project.name}</h3><small>{project.id}</small></div><span className="project-total">{project.total} <small>tasks</small></span></div><div className="progress-track" role="progressbar" aria-label={`${project.name} accepted tasks`} aria-valuenow={progress} aria-valuemin={0} aria-valuemax={100}><span style={{ width: `${progress}%` }} /></div><div className="project-stages"><span><i className="dot accepted" />{project.accepted} accepted</span><span><i className="dot running" />{project.running} claimed / running</span><span><i className="dot blocked" />{project.blocked} blocked</span><span><i className="dot verifying" />{project.awaiting_verification} awaiting verification</span></div></article>;
        }) : <Empty message="No projects in the current snapshot." />}</div>
      </div>
      <div className="panel delivery-panel"><div className="panel-head"><div><h2>Task stages</h2></div><span className="tiny-badge">{totals.tasks} total</span></div><div className="stage-list">{stageItems.map(([label, count, kind]) => <div className="stage-row" key={label}><span className={`stage-icon ${kind}`} data-icon={kind === 'accepted' ? '✓' : kind === 'running' ? '↗' : kind === 'blocked' ? '!' : '◷'} aria-hidden="true" /><span className="stage-label">{label}</span><strong>{count}</strong><span className="stage-track"><i className={kind} style={{ width: `${totals.tasks ? Math.max(count ? 3 : 0, count / totals.tasks * 100) : 0}%` }} /></span></div>)}</div><div className="delivery-note"><span>Quality gate</span><b>Not yet qualified</b><small>Candidate reports await independent verification. Native execution remains disabled.</small></div></div>
    </section>
    <section className="panel activity-panel"><div className="panel-head"><div><h2>Message pulse</h2></div><span className="panel-caption">No routine model turns</span></div><div className="coord-stats"><div><span className="stat-label">Recorded messages</span><strong>{fleet.coordination.queued_messages}</strong><small>Durably queued in snapshot</small></div><div><span className="stat-label">Acknowledged</span><strong>{fleet.coordination.acknowledged_messages}</strong><small>Recorded in snapshot</small></div><div><span className="stat-label">Routine turns</span><strong>{fleet.coordination.routine_model_turns}</strong><small>Current API contract</small></div><div><span className="stat-label">Measured tokens</span><strong>{formatUnknown(fleet.coordination.measured_tokens)}</strong><small>Unknown is distinct from zero</small></div></div></section>
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
  return <div className="coordination-layout"><section className="panel coordination-summary"><div className="panel-head"><div><h2>Message statistics</h2></div><span className="tiny-badge">Current snapshot</span></div><div className="coordination-cards">{items.map((item) => <article className="coordination-card" key={item.label}><span className={`coordination-icon ${item.color}`} data-icon={item.color === 'gold' ? '◌' : item.color === 'blue' ? '✓' : '↔'} aria-hidden="true" /><div><span>{item.label}</span><strong>{item.value}</strong><small>{item.detail}</small></div></article>)}</div></section><section className="panel accounting-panel"><h2>Measured usage</h2><div className="accounting-value">{formatUnknown(fleet.coordination.measured_tokens)} <span>tokens</span></div><p>The API reports measured token usage when available. Missing usage remains unknown; it is not treated as zero.</p></section><div className="read-only-note"><span className="lock-icon" data-icon="⌑" aria-hidden="true" /><div><b>Coordination is read only</b><span>This dashboard displays observed message counts. Sending, replying and changing task state are not available here.</span></div></div></div>;
}

function Empty({ message }: { message: string }) { return <div className="empty-state"><span data-icon="⌁" aria-hidden="true" />{message}</div>; }

export default App;
