import { useEffect, useRef, useState } from 'react';
import './incidents.css';
import { incidentRequest, parseScope, parseSnapshot, validateReceipt, IncidentHttpError, type Scope, type Snapshot } from './api';

type ProposalCommand = { scope: Scope; command_id: string; expected_revision: number; operation: { kind: 'propose'; episode_id: string; reproduction: { fields: Record<string, string> } } };

export function IncidentHistory({ snapshot }: { snapshot: Snapshot }) {
  return <div className="incident-history">
    <p>Recorded revision {snapshot.revision}. Missing attribution stays unknown. Local proposals require review before any action.</p>
    {!snapshot.state?.episodes.length && <p>No recorded incidents in this scope.</p>}
    {snapshot.state?.episodes.map(e => <article className="panel" key={e.episode_id}>
      <h3>{e.family}</h3><p>{e.unknown_attribution ? 'Attribution unknown' : 'Explicit causal lineage recorded'}</p>
      <ul>{e.events.map(event => <li key={event.event_id}>{event.kind} · {event.event_id}{event.cause_parent ? ` · caused by ${event.cause_parent}` : ' · cause unknown'}<ul>{event.evidence_digests.map((d, i) => <li key={i}><code>{d}</code></li>)}</ul></li>)}</ul>
    </article>)}
    <h3>Local improvement proposals</h3>
    {!snapshot.proposals.length && <p>No local proposals recorded.</p>}
    {snapshot.proposals.map(({ command_id, proposal: p }) => <article className="panel" key={command_id}>
      <h4>{p.title}</h4><p>{p.diagnosis}</p><p>{p.uncertainty}</p>
      <p>Proposed investigation: at most {p.proposed_investigator_max_steps} steps and {p.proposed_investigator_max_model_turns} model turn. Execution is not granted. External publication is not granted.</p>
      <p>Episode evidence: <code>{p.episode_digest}</code></p>
      <dl>{Object.entries(p.reproduction_contract.fields).map(([k, v]) => <div key={k}><dt>{k.replaceAll('_', ' ')}</dt><dd>{v}</dd></div>)}</dl>
      <ul>{p.evidence_digests.map(d => <li key={d}><code>{d}</code></li>)}</ul>
    </article>)}
  </div>;
}
export default function Incidents({ token }: { token: string }) {
  const [scopes, setScopes] = useState<Scope[]>([]), [selection, setSelection] = useState('');
  const [loaded, setLoaded] = useState<{ snapshot: Snapshot; token: string; selection: string } | null>(null), [error, setError] = useState('');
  const [refresh, setRefresh] = useState(0), [busy, setBusy] = useState(false), [notice, setNotice] = useState('');
  const [episode, setEpisode] = useState(''), [fields, setFields] = useState<Record<string, string>>({});
  const generation = useRef(0);
  const snapshot = loaded?.token === token && loaded.selection === selection ? loaded.snapshot : null;
  const [pending, setPending] = useState<ProposalCommand | null>(null);
  useEffect(() => {
    const abort = new AbortController(); ++generation.current;
    setScopes([]); setSelection(''); setLoaded(null); setPending(null); setFields({}); setError(''); setNotice(''); setBusy(false);
    if (token) incidentRequest('scopes', {}, token, abort.signal).then(value => {
      if (!Array.isArray(value) || value.length > 128) throw new Error('Incident scope list is invalid.');
      if (!abort.signal.aborted) setScopes(value.map(parseScope));
    }).catch(e => { if (!abort.signal.aborted) setError(e instanceof Error ? e.message : 'Incident access unavailable.'); });
    return () => abort.abort();
  }, [token]);
  useEffect(() => {
    const abort = new AbortController(); ++generation.current;
    setLoaded(null); setPending(null); setFields({}); setEpisode(''); setNotice(''); setError(''); setBusy(false);
    const scope = scopes[Number(selection)];
    if (selection && scope) incidentRequest('read', scope, token, abort.signal).then(value => { if (!abort.signal.aborted) setLoaded({ snapshot: parseSnapshot(value, scope), token, selection }); }).catch(e => { if (!abort.signal.aborted) setError(e instanceof Error ? e.message : 'Incident history unavailable.'); });
    return () => abort.abort();
  }, [selection, scopes, token, refresh]);
  async function propose() {
    if (!snapshot || !episode || busy) return;
    const turn = generation.current;
    const command: ProposalCommand = pending ?? { scope: snapshot.scope, command_id: crypto.randomUUID(), expected_revision: snapshot.revision, operation: { kind: 'propose', episode_id: episode, reproduction: { fields: { scope: `${snapshot.scope.project}/${snapshot.scope.root}/${snapshot.scope.task} revision ${snapshot.scope.contract_revision}`, ...fields } } } };
    setPending(command); setBusy(true); setError('');
    try {
      const value = await incidentRequest('commands', command, token);
      if (turn !== generation.current) return;
      validateReceipt(value, command);
      setPending(null); setRefresh(v => v + 1);
    } catch (e) { if (turn === generation.current) { setError(e instanceof Error ? e.message : 'Proposal outcome unknown.'); if (e instanceof IncidentHttpError && e.status < 500 && e.status !== 408) { setPending(null); setNotice(''); } else setNotice('Keep this request to retry after an uncertain response, or refresh to inspect history.'); } }
    finally { if (turn === generation.current) setBusy(false); }
  }
  return <section className="panel" aria-label="Scoped incidents">
    <h2>Incident history</h2><p>Inspect an authorized task scope and prepare a local improvement proposal.</p>
    {!token && <p>Apply a session token in access settings to load your incident scopes.</p>}
    {token && <label>Task scope <select value={selection} onChange={e => setSelection(e.target.value)}><option value="">Choose a scope</option>{scopes.map((s, i) => <option value={String(i)} key={i}>{s.project} / {s.root} / {s.task} · revision {s.contract_revision}</option>)}</select></label>}
    {token && !scopes.length && !error && <p>No authorized incident scopes loaded.</p>}
    {error && <p role="alert">{error}</p>}{notice && <p role="status">{notice}</p>}
    {selection && <button className="secondary-button" onClick={() => setRefresh(v => v + 1)}>Refresh history</button>}
    {snapshot && <><IncidentHistory snapshot={snapshot} /><form onSubmit={e => { e.preventDefault(); void propose(); }}>
      <h3>Prepare a local proposal</h3><label>Episode <select required disabled={!!pending || busy} value={episode} onChange={e => setEpisode(e.target.value)}><option value="">Choose an episode</option>{snapshot.state?.episodes.map(e => <option key={e.episode_id} value={e.episode_id}>{e.family} · {e.events[0]?.event_id}</option>)}</select></label>
      {(['steps', 'expected', 'actual', 'tried_approaches', 'candidate_fix', 'measurable_outcome'] as const).map(k => <label key={k}>{({ steps: 'Reproduction steps', expected: 'Expected result', actual: 'Observed result', tried_approaches: 'Approaches already tried', candidate_fix: 'Suggested change', measurable_outcome: 'Measurable follow-up' })[k]}<textarea maxLength={4096} required={['steps', 'expected', 'actual'].includes(k)} disabled={!!pending || busy} value={fields[k] ?? ''} onChange={e => setFields(f => { const next = { ...f }; if (e.target.value) next[k] = e.target.value; else delete next[k]; return next; })} /></label>)}
      <button className="primary-button" disabled={busy || !episode}>{busy ? 'Recording…' : pending ? 'Retry proposal' : 'Record local proposal'}</button>
    </form></>}
  </section>;
}
