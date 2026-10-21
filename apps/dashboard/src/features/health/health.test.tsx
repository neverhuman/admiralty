import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { LegacySnapshot } from '../../legacy';
import { HealthView } from './FleetHealth';
import { buildHealthModel, countText, duration, figure } from './health';

const NOW = Date.parse('2026-10-21T08:00:20Z');

function snapshot(): LegacySnapshot {
  return {
    generated_utc: '2026-10-21T08:00:00Z', interval_s: 15, host: 'xbabe2', order: ['dope', 'joil', 'quiet'],
    projects: { dope: { name: 'DOPE' }, joil: { name: 'Joil' }, quiet: { name: 'Quiet' } },
    collector: { duration_ms: 1834, model_turns: 0, stages_ms: { fleet: 420, projects: 300, git: 900, git_health: 3, usage: 150, history: 20 } },
    git_health: {
      dope: { state: 'attention', actionable: true, fingerprint: 'sha256:3f2a91c4ffffffff', integration_ref: 'refs/heads/main',
        issues: [{ code: 'merge_conflicts' }, { code: 'diverged_branch' }, { code: 'merge_conflicts' }] },
      joil: { state: 'unknown', actionable: true, fingerprint: 'sha256:aaaa', issues: [{ code: 'shallow_history' }] },
    },
    git_metrics: {
      dope: { complete: true, integration_ref: 'refs/heads/main',
        branches: { compared: 14, merged: 9, ahead_only: 3, diverged: 2, unknown: 0, unmerged_tips_older_30d: 1, unintegrated_commits: { value: 7, lower_bound: false } },
        checkouts: { total: 6, dirty: 2, state_unknown: 1, with_conflicts: 1, detached: 0 },
        integration: { tip_age_seconds: 10800 },
        activity: [{ window_hours: 24, integration_first_parent: { value: 4, lower_bound: false } }, { window_hours: 168, integration_first_parent: { value: 19, lower_bound: true } }] },
      joil: { complete: false, branches: { compared: 2, merged: 0, ahead_only: 0, diverged: 0, unknown: 2, unmerged_tips_older_30d: 0 },
        checkouts: { total: 1, dirty: 1, state_unknown: 0, with_conflicts: 0, detached: 0 }, integration: { tip_age_seconds: null }, activity: [] },
    },
    global_resources: { hosts: [
      { host: 'xbabe2', reachable: true, nproc: 100, load: [55.1, 1, 1], load_level: 'ok', mem: { avail_pct: 61, level: 'ok', swap_total_gb: 8, swap_used_gb: 2 },
        disks: [{ mount: '/', pct: 91.2, level: 'amber', growth_gb_per_h: 2.1, hours_to_full: 11.4 }, { mount: '/data', pct: 40, level: 'ok', growth_gb_per_h: null, hours_to_full: null }],
        gpus: [{ util: 43 }] },
      { host: 'xbabe1', reachable: false },
    ] },
  } as LegacySnapshot;
}

describe('fleet health model', () => {
  it('summarizes Git, hosts and freshness without turning unknowns into zero', () => {
    const model = buildHealthModel(snapshot(), NOW);
    expect(model.ageSeconds).toBe(20);
    expect(model.stale).toBe(false);
    expect(model.summary).toMatchObject({ projects: 3, attention: 1, unknown: 1, healthy: 0, disabled: 1, hostsAtRisk: 1, hostsUnreachable: 1 });
    // Only projects with metrics contribute; all known, so the sum is known.
    expect(model.summary.dirtyWorktrees).toBe(3);
    const dope = model.projects[0];
    expect(dope.state).toBe('attention');
    expect(dope.issues).toEqual(['diverged_branch', 'merge_conflicts']);
    expect(dope.fingerprint).toBe('3f2a91c4');
    expect(dope.commits7d).toEqual({ value: 19, lowerBound: true });
    expect(model.projects[1].tipAgeSeconds).toBeNull();
    expect(model.projects[2].state).toBe('not_reported');
    expect(model.projects[2].branches).toBeNull();
    const [unreachable, busy] = model.hosts;
    expect(unreachable.host).toBe('xbabe1');
    expect(unreachable.level).toBe('unknown');
    expect(busy.loadPercent).toBeCloseTo(55.1);
    expect(busy.swapUsedPercent).toBe(25);
    expect(busy.worstDisk).toMatchObject({ mount: '/', hoursToFull: 11.4 });
    expect(model.cycle.git[0].ms).toBe(900);
  });
  it('marks a source older than three cycles as stale and keeps a missing clock unknown', () => {
    expect(buildHealthModel(snapshot(), Date.parse('2026-10-21T08:05:00Z')).stale).toBe(true);
    const unknownClock = buildHealthModel({ ...snapshot(), generated_utc: 'not a time' }, NOW);
    expect(unknownClock.ageSeconds).toBeNull();
    expect(unknownClock.stale).toBeNull();
  });
  it('formats with at most two decimals and explicit lower bounds', () => {
    expect(figure(96.42857142857143, '%')).toBe('96.43%');
    expect(figure(null)).toBe('unknown');
    expect(countText({ value: 4, lowerBound: true })).toBe('4+');
    expect(countText(null)).toBe('unknown');
    expect(duration(10800)).toBe('3 h');
    expect(duration(-5)).toBe('clock skew');
  });
});

describe('fleet health view', () => {
  it('renders named states, the cycle lanes and accessible tables', () => {
    const html = renderToStaticMarkup(<HealthView model={buildHealthModel(snapshot(), NOW)} />);
    expect(html).toContain('Repositories needing attention');
    expect(html).toContain('Needs attention');
    expect(html).toContain('Not reported');
    expect(html).toContain('diverged branch');
    expect(html).toContain('Partial scan: counts may grow.');
    expect(html).toContain('7 commits not in integration');
    expect(html).toContain('4 commits in 24 h, 19+ in 7 d');
    expect(html).toContain('Unreachable');
    expect(html).toContain('/ full in about 11 h');
    expect(html).toContain('+2.1 GB/h');
    expect(html).not.toContain('No projection yet');
    expect(html).toContain('<caption class="visually-hidden">Repository hygiene by project</caption>');
    expect(html).toContain('aria-label="Observer cycle 1834 ms: Fleet 420 ms, Projects 300 ms, Git scan 900 ms');
    expect(html).not.toContain('NaN');
  });
  it('distinguishes a missing projection from a disk that is not growing', () => {
    const legacy = snapshot();
    const hosts = (legacy.global_resources as { hosts: Array<Record<string, unknown>> }).hosts;
    hosts[0].disks = [{ mount: '/', pct: 50, level: 'ok', growth_gb_per_h: null, hours_to_full: null }];
    expect(renderToStaticMarkup(<HealthView model={buildHealthModel(legacy, NOW)} />)).toContain('No projection yet');
    hosts[0].disks = [{ mount: '/', pct: 50, level: 'ok', growth_gb_per_h: -0.5, hours_to_full: null }];
    expect(renderToStaticMarkup(<HealthView model={buildHealthModel(legacy, NOW)} />)).toContain('Not growing');
  });
  it('explains missing stage timings instead of drawing empty lanes', () => {
    const legacy = snapshot();
    legacy.collector = { duration_ms: 50 };
    const html = renderToStaticMarkup(<HealthView model={buildHealthModel(legacy, NOW)} />);
    expect(html).toContain('does not report stage timings yet');
  });
});
