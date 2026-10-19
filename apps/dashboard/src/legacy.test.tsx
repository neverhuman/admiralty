import { afterEach, describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { freshness, getLegacy, parseLegacy, projectDetails, hierarchyCaptains, safeHref, watchLegacy } from './legacy';
import { LegacyFleetView, ProjectDetail } from './ui/LegacyDetailed';

const snapshot = () => parseLegacy({ generated_utc: '2026-10-18T12:00:00Z', host: 'arbitrary-node', order: ['alpha', 'beta', 'gamma', 'delta'],
  projects: Object.fromEntries(['alpha', 'beta', 'gamma', 'delta'].map(id => [id, { name: `Project ${id}`, total: 10,
    ledger_source: 'producer ledger', stages: { done: 1, in_pr: 4, in_progress: 2, blocked: 3 },
    stage_labels: { done: 'accepted on main' }, flags: ['Independent audit required'],
    recent: [{ task: 'T1', old: 'open', new: 'review', at: 'source timestamp', source: 'ledger commit' }],
    captain: { name: `captain_${id}`, model: 'source captain model', agent_running: true, tail: ['Source captain tail'] },
    workers: [{ name: 'worker', model: 'source worker model', host: 'arbitrary-worker-node', task: 'T1', agent_running: false, tail: ['Worker tail'] }],
    pr: { number: 1, url: 'https://example.com/project/pull/1', state: 'OPEN', head: 'exact-revision', ci: { state: 'pending' }, exact_head_approved: false },
    validator: { verified_mentions: 1 }, leases: [{ owner: 'worker', tasks: ['T1'] }],
    live_status: { active_task: 'T1' }, waves: { running: ['wave-1'] }, coord_log: { tail: ['Source coordination log'], mtime_utc: 'producer timestamp' },
  }])),
  rollout: { project: { id: 'build-project', name: 'Admiralty' }, producer: 'rollout producer', updated_utc: 'source rollout timestamp', total: 2,
    counts: { review: 1, planned: 1 }, packets: [{ id: 'P01', title: 'Scoped packet', status: 'review' }], lanes: [], source: { sha256: 'source hash' }, freshness: { state: 'stale' } },
  hierarchy: { jepson: 'Configured owner', grokbot: { name: 'Configured watcher' }, super_captain: { screen: 'configured-super' }, captains: [] },
  super_captain: { log_tail: ['Super captain source tail'] }, token_efficiency: { quota: { used_percent: 44 }, by_model: { 'arbitrary-model': 2 } },
  token_yield: { total: { tokens: 123 }, method: 'source accounting method' }, global_resources: { hosts: [{ host: 'arbitrary-node', load: [1, 2, 3] }] },
  history: { rows: [{ ts: 1, cost_index: 2.5, quota_pct: 44 }] },
});

describe('detailed project source adapters', () => {
  it('preserves ledger source age even when publication and observation are fresh', () => {
    const source = snapshot();
    source.projects.alpha.ledger_updated_utc = '2026-10-17T01:00:00Z';
    source.projects.alpha.observed_utc = '2026-10-18T12:00:00Z';
    expect(projectDetails(source)[0].observed).toBe('2026-10-17T01:00:00Z');
  });
  it('renders every producer project and rollout through the same ordinary project component', () => {
    const source = snapshot(), projects = projectDetails(source);
    expect(projects.map(project => project.id)).toEqual(['alpha', 'beta', 'gamma', 'delta', 'build-project']);
    const html = renderToStaticMarkup(<LegacyFleetView snapshot={source} />);
    expect((html.match(/<article[^>]*data-project-id=/g) ?? []).length).toBe(5);
    for (const text of ['Recently changed tasks', 'Source captain tail', 'source worker model', 'exact-head approval not confirmed',
      'Validator audit', 'Task leases', 'Live status', 'Evolution waves', 'Source coordination log', 'Hierarchy', 'Configured owner',
      'Token efficiency', 'Fleet resources', 'History and trends', 'Admiral', 'Scoped packet']) expect(html).toContain(text);
    expect(html).not.toContain('iframe'); expect(html).not.toContain('dangerouslySetInnerHTML');
  });
  it('selects one project without promoting the rollout above other project cards', () => {
    const html = renderToStaticMarkup(<LegacyFleetView snapshot={snapshot()} selected="build-project" />);
    expect((html.match(/<article[^>]*data-project-id=/g) ?? []).length).toBe(1);
    expect(html).toMatch(/<article[^>]*data-project-id="build-project"/); expect(html).not.toMatch(/<article[^>]*data-project-id="alpha"/);
  });
  it('merges matching producer and rollout identities without duplicate project tabs', () => {
    const source = snapshot(); source.rollout = { ...source.rollout as object, project: { id: 'alpha', name: 'Alternate title' } };
    const projects = projectDetails(source);
    expect(projects).toHaveLength(4); expect(projects[0].name).toBe('Project alpha');
    expect(projects[0].sections.some(section => section.title === 'Work packets')).toBe(true);
  });
  it('maps rollout screens to their project even when the old classifier says unassigned', () => {
    const source = snapshot();
    source.agents = [{ screen: 'custom_lane', project: 'unassigned', model: 'observed model', state: 'unknown' }];
    source.rollout = { ...source.rollout as object, lanes: [{ id: 'lane-1', screen: 'custom_lane', node: 'arbitrary-node',
      model: 'declared model', packet_id: 'P13A', status: 'stale; reported in_progress; screen present', source_updated_utc: 'original time' }] };
    const project = projectDetails(source).at(-1)!;
    expect(project.workers).toHaveLength(1);
    expect(project.workers[0]).toMatchObject({ screen: 'custom_lane', model: 'observed model', task: 'P13A',
      state: 'stale; reported in_progress; screen present', screen_changed_utc: 'original time' });
  });
  it('selects the explicitly pinned observed manager instead of an older Luna session', () => {
    const source = snapshot();
    source.agents = [
      { id: '101.captain_build-project', screen: 'captain_build-project', project: 'build-project', model: 'gpt-6-luna', effort: 'medium', tier: 'T0' },
      { id: '202.agy_manager', screen: 'agy_manager', project: 'unassigned', model: 'agy Gemini 4 Argon', tier: 'planner', state: 'idle' },
    ];
    source.rollout = { ...source.rollout as object, project: { id: 'build-project', name: 'Admiralty', captain_screen: 'agy_manager', captain_screen_pid: 202 },
      live_observation: { captain: { identity_state: 'observed', id: '202.agy_manager', pid: 202, name: 'agy_manager', model: 'Gemini 4 Argon', agent_running: true } } };
    const project = projectDetails(source).at(-1)!;
    expect(project.captain).toMatchObject({ id: '202.agy_manager', screen: 'agy_manager', model: 'agy Gemini 4 Argon', tier: 'planner', state: 'idle' });
    expect(project.captain.effort).toBeUndefined();
    expect(project.workers).toHaveLength(1);
    expect(project.workers[0]).toMatchObject({ model: 'gpt-6-luna' });
    expect(hierarchyCaptains(source).at(-1)).toMatchObject({ name: 'Admiralty', screen: 'agy_manager', model: 'agy Gemini 4 Argon' });
    const html = renderToStaticMarkup(<LegacyFleetView snapshot={source} selected="build-project" />);
    expect(html).toContain('agy Gemini 4 Argon');
  });
  it('preserves the observed manager model when the same-session roster model is unknown', () => {
    const source = snapshot();
    source.agents = [{ id: '202.agy_manager', screen: 'agy_manager', project: 'build-project', model: 'agy Gemini (model unknown)' }];
    source.rollout = { ...source.rollout as object, project: { id: 'build-project', captain_screen: '202.agy_manager' },
      live_observation: { captain: { identity_state: 'observed', id: '202.agy_manager', pid: 202, name: 'agy_manager', model: 'Gemini 4 Argon' } } };
    expect(projectDetails(source).at(-1)!.captain.model).toBe('Gemini 4 Argon');
    expect(projectDetails(source).at(-1)!.workers).toHaveLength(0);
  });
  it('keeps a missing or mismatched configured manager unknown without promoting a worker', () => {
    for (const observed of [undefined, { identity_state: 'absent' },
      { identity_state: 'observed', id: '303.agy_manager', pid: 303, name: 'agy_manager', model: 'wrong session' },
      { identity_state: 'observed', id: '202.wrong_manager', pid: 202, name: 'wrong_manager', model: 'wrong name' },
      { identity_state: 'observed', id: '202.agy_manager', pid: 303, name: 'agy_manager', model: 'inconsistent PID' }]) {
      const source = snapshot();
      source.agents = [{ id: '101.captain_build-project', screen: 'captain_build-project', project: 'build-project', model: 'gpt-6-luna' }];
      source.rollout = { ...source.rollout as object, project: { id: 'build-project', captain_screen: 'agy_manager', captain_screen_pid: 202 },
        live_observation: { captain: observed } };
      expect(projectDetails(source).at(-1)!.captain).toMatchObject({ screen: 'agy_manager', model: 'unknown', state: 'unknown' });
      expect(projectDetails(source).at(-1)!.workers).toHaveLength(1);
    }
  });
  it('preserves source counts and timestamps; unknown total remains unknown', () => {
    const source = snapshot(); delete source.projects.alpha.total;
    const project = projectDetails(source)[0];
    expect(project.total).toBeNull(); expect(project.stages.done).toBe(1); expect(project.observed).toBe(source.generated_utc);
    expect(projectDetails(source).at(-1)?.stages.done).toBeUndefined();
    expect(freshness('2026-10-18T12:00:00Z', Date.parse('2026-10-18T12:10:00Z'))).toContain('stale');
    expect(freshness('2026-10-18T12:10:00Z', Date.parse('2026-10-18T12:00:00Z'))).toBe('future source time');
  });
  it('keeps independently observed worker activity separate from its lane report', () => {
    const source = snapshot();
    source.agents = [{ screen: 'observed_worker', project: 'build-project', state: 'active', last_token_utc: 'token source time' }];
    source.rollout = { ...source.rollout as object, lanes: [{ id: 'lane', screen: 'observed_worker', status: 'stale; reported in_progress; screen present' }] };
    const worker = projectDetails(source).at(-1)!.workers[0];
    expect(worker).toMatchObject({ state: 'active', lane_report_status: 'stale; reported in_progress; screen present', screen_changed_utc: 'token source time' });
  });
  it('includes additional project captains in the hierarchy without duplicating producer entries', () => {
    const source = snapshot();
    source.hierarchy = { captains: [{ key: 'alpha', name: 'Authoritative producer captain' }] };
    source.agents = [{ project: 'build-project', screen: 'captain_build-project', model: 'observed captain' }];
    const captains = hierarchyCaptains(source);
    expect(captains.filter(captain => captain.key === 'alpha')).toHaveLength(1);
    expect(captains.find(captain => captain.key === 'alpha')?.name).toBe('Authoritative producer captain');
    expect(captains.find(captain => captain.key === 'build-project')).toMatchObject({ name: 'Admiralty', model: 'observed captain' });
  });
  it('escapes all source text and excludes unsafe source links', () => {
    const source = snapshot(); source.projects.alpha.name = '<img src=x onerror="alert(1)">';
    source.projects.alpha.pr = { url: 'javascript:alert(1)' };
    const html = renderToStaticMarkup(<LegacyFleetView snapshot={source} />);
    expect(html).not.toContain('<img'); expect(html).toContain('&lt;img'); expect(html).not.toContain('href="javascript:');
    expect(safeHref('https://user:secret@example.com')).toBeUndefined();
  });
});

describe('recorded rollout evidence and compact warnings', () => {
  const provenance = 'Integration provenance missing; displayed as verification recorded.';
  function rolloutWith32Packets() {
    const source = snapshot();
    source.rollout = { ...source.rollout as object, total: 32, updated_utc: '2026-10-18T21:24:00Z',
      counts: { verified: 32, integrated: 0 }, freshness: { state: 'stale' },
      packets: Array.from({ length: 32 }, (_, i) => ({ id: `P${String(i).padStart(2, '0')}`, title: `Packet ${i}`, status: 'verified', warnings: [provenance] })) };
    return source;
  }
  it('groups 32 provenance warnings once while retaining every affected packet and stale source time', () => {
    const project = projectDetails(rolloutWith32Packets()).at(-1)!;
    const evidence = project.alerts.filter(alert => alert.kind === 'evidence');
    expect(evidence).toHaveLength(1); expect(evidence[0].count).toBe(32);
    expect(evidence[0].taskIds).toEqual(Array.from({ length: 32 }, (_, i) => `P${String(i).padStart(2, '0')}`));
    expect(evidence[0].sourceMessage).toBe(provenance);
    expect(project.observed).toBe('2026-10-18T21:24:00Z');
    expect(project.stages).toEqual({ verified: 32, integrated: 0 }); expect(project.stages.done).toBeUndefined();
    const html = renderToStaticMarkup(<ProjectDetail project={project} />);
    expect((html.match(/class="legacy-alert legacy-alert-evidence"/g) ?? []).length).toBe(1);
    expect(html).toContain('32 packets'); expect(html).toContain('Affected packets and source evidence (32 records)');
    expect(html).toContain('P31'); expect(html).toContain(provenance);
    expect(html).toContain('Rollout evidence is stale'); expect(html).toContain('2026-10-18T21:24:00Z');
  });
  it('preserves separate project and rollout timestamps when their identities merge', () => {
    const source = rolloutWith32Packets();
    source.rollout = { ...source.rollout as object, project: { id: 'alpha', name: 'Admiralty' } };
    const projects = projectDetails(source), project = projects.find(value => value.id === 'alpha')!;
    expect(projects).toHaveLength(4);
    expect(project.observed).toBe(source.generated_utc);
    expect(project.stages).toEqual({ done: 1, in_pr: 4, in_progress: 2, blocked: 3 });
    expect(project.sections.find(section => section.title === 'Manifest provenance')?.value).toMatchObject({ updated_utc: '2026-10-18T21:24:00Z', freshness: { state: 'stale' } });
    expect(project.alerts.find(alert => alert.kind === 'evidence')).toMatchObject({ count: 32, taskIds: expect.arrayContaining(['P00', 'P31']) });
    expect(project.alerts.some(alert => alert.message === 'Independent audit required')).toBe(true);
    expect(project.alerts.some(alert => alert.kind === 'warning' && alert.message.includes('stale'))).toBe(true);
  });
  it('retains distinct failures and counts repeated failures without mixing them with missing integration evidence', () => {
    const source = rolloutWith32Packets();
    const rollout = source.rollout as { error?: string; packets: Array<{ id: string; warnings: string[] }> };
    rollout.error = 'Manifest could not fetch review evidence';
    rollout.packets[3].warnings.push('Rust checks failed: target mismatch');
    rollout.packets[4].warnings.push('Rust checks failed: target mismatch');
    rollout.packets[5].warnings.push('React assertion failed');
    const project = projectDetails(source).at(-1)!;
    expect(project.alerts).toHaveLength(5);
    expect(project.alerts.find(alert => alert.message === 'Rust checks failed: target mismatch')).toMatchObject({ count: 2, taskIds: ['P03', 'P04'], kind: 'error' });
    expect(project.alerts.find(alert => alert.message === 'React assertion failed')).toMatchObject({ count: 1, taskIds: ['P05'], kind: 'error' });
    const html = renderToStaticMarkup(<ProjectDetail project={project} />);
    for (const message of ['Rust checks failed: target mismatch', 'React assertion failed', 'Manifest could not fetch review evidence']) expect(html).toContain(message);
    expect((html.match(/class="legacy-alert legacy-alert-error"/g) ?? []).length).toBe(3);
  });
  it('presents recorded component checks in blue evidence styling, separately from green accepted completion', () => {
    const project = projectDetails(rolloutWith32Packets()).at(-1)!;
    const html = renderToStaticMarkup(<ProjectDetail project={project} />);
    expect(html).toContain('stage-evidence-recorded'); expect(html).not.toContain('stage-verified'); expect(html).not.toContain('stage-done');
    expect(html).toContain('checks recorded'); expect(html).toContain('0</b> integration recorded');
    expect(html).toContain('Recorded checks describe component evidence. Integration and accepted completion require separate proof.');
    expect(html).not.toContain('32 done');
    const original = projectDetails(snapshot())[0];
    const originalHtml = renderToStaticMarkup(<ProjectDetail project={original} />);
    expect(originalHtml).toContain('stage-done'); expect(originalHtml).toContain('accepted on main');
  });
});

describe('read-only detailed snapshot transport', () => {
  afterEach(() => vi.useRealTimers());
  it('passes bearer only in headers and tolerates an optional missing demo endpoint', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(new Response('', { status: 404 }));
    await expect(getLegacy('private-token', undefined, fetcher)).resolves.toBeNull();
    expect(fetcher.mock.calls[0][0]).toBe('/v1/legacy');
    expect(new Headers(fetcher.mock.calls[0][1]?.headers).get('Authorization')).toBe('Bearer private-token');
  });
  it('rejects malformed source envelopes', () => {
    for (const value of [{}, { generated_utc: 'now', projects: [] }, { generated_utc: 'now', projects: { a: null } }]) expect(() => parseLegacy(value)).toThrow();
  });
  it('polls sequentially every 15 seconds and cancels after unmount', async () => {
    vi.useFakeTimers();
    const fetcher = vi.fn<typeof fetch>().mockImplementation(async () => Response.json(snapshot()));
    const onSnapshot = vi.fn(), stop = watchLegacy({ fetcher, onSnapshot, onError: vi.fn() });
    await vi.advanceTimersByTimeAsync(0); expect(fetcher).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(15_000); expect(fetcher).toHaveBeenCalledTimes(2);
    stop(); await vi.advanceTimersByTimeAsync(30_000); expect(fetcher).toHaveBeenCalledTimes(2);
  });
  it('does not repeatedly poll a missing demo source', async () => {
    vi.useFakeTimers();
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(new Response('', { status: 404 }));
    const stop = watchLegacy({ fetcher, onSnapshot: vi.fn(), onError: vi.fn() });
    await vi.advanceTimersByTimeAsync(60_000); expect(fetcher).toHaveBeenCalledTimes(1); stop();
  });
  it('reports the bounded timeout clearly and recovers on the next sequential poll', async () => {
    vi.useFakeTimers();
    const fetcher = vi.fn<typeof fetch>().mockImplementationOnce((_url, options) =>
      new Promise((_resolve, reject) => options?.signal?.addEventListener('abort', () =>
        reject(new DOMException('signal is aborted without reason', 'AbortError')))))
      .mockResolvedValue(Response.json(snapshot()));
    const onError = vi.fn(), onSnapshot = vi.fn();
    const stop = watchLegacy({ fetcher, onError, onSnapshot });
    await vi.advanceTimersByTimeAsync(12_000);
    expect(onError).toHaveBeenLastCalledWith(expect.stringContaining('within 12 seconds'));
    expect(onSnapshot).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(15_000);
    expect(fetcher).toHaveBeenCalledTimes(2);
    expect(onSnapshot).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenLastCalledWith('');
    stop();
  });
});
