import { afterEach, describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { freshness, getLegacy, parseLegacy, projectDetails, safeHref, watchLegacy } from './legacy';
import { LegacyFleetView } from './ui/LegacyDetailed';

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
  it('renders every producer project and rollout through the same ordinary project component', () => {
    const source = snapshot(), projects = projectDetails(source);
    expect(projects.map(project => project.id)).toEqual(['alpha', 'beta', 'gamma', 'delta', 'build-project']);
    const html = renderToStaticMarkup(<LegacyFleetView snapshot={source} />);
    expect((html.match(/data-project-id=/g) ?? []).length).toBe(5);
    for (const text of ['Recently changed tasks', 'Source captain tail', 'source worker model', 'exact-head approval not confirmed',
      'Validator audit', 'Task leases', 'Live status', 'Evolution waves', 'Source coordination log', 'Hierarchy', 'Configured owner',
      'Token efficiency and model mix', 'Fleet resources', 'History and trends', 'Super captain source tail', 'Scoped packet']) expect(html).toContain(text);
    expect(html).not.toContain('iframe'); expect(html).not.toContain('dangerouslySetInnerHTML');
  });
  it('selects one project without promoting the rollout above other project cards', () => {
    const html = renderToStaticMarkup(<LegacyFleetView snapshot={snapshot()} selected="build-project" />);
    expect((html.match(/data-project-id=/g) ?? []).length).toBe(1);
    expect(html).toContain('data-project-id="build-project"'); expect(html).not.toContain('data-project-id="alpha"');
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
  it('preserves source counts and timestamps; unknown total remains unknown', () => {
    const source = snapshot(); delete source.projects.alpha.total;
    const project = projectDetails(source)[0];
    expect(project.total).toBeNull(); expect(project.stages.done).toBe(1); expect(project.observed).toBe(source.generated_utc);
    expect(projectDetails(source).at(-1)?.stages.done).toBeUndefined();
    expect(freshness('2026-10-18T12:00:00Z', Date.parse('2026-10-18T12:10:00Z'))).toContain('stale');
    expect(freshness('2026-10-18T12:10:00Z', Date.parse('2026-10-18T12:00:00Z'))).toBe('future source time');
  });
  it('escapes all source text and excludes unsafe source links', () => {
    const source = snapshot(); source.projects.alpha.name = '<img src=x onerror="alert(1)">';
    source.projects.alpha.pr = { url: 'javascript:alert(1)' };
    const html = renderToStaticMarkup(<LegacyFleetView snapshot={source} />);
    expect(html).not.toContain('<img'); expect(html).toContain('&lt;img'); expect(html).not.toContain('href="javascript:');
    expect(safeHref('https://user:secret@example.com')).toBeUndefined();
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
});
