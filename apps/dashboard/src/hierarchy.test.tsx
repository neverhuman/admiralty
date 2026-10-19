import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { parseLegacy } from './legacy';
import { LegacyHierarchy } from './ui/LegacyHierarchy';
import { ModelBadge, modelFamily } from './ui/ModelBadge';

function source() {
  return parseLegacy({ generated_utc: '2026-10-18T11:05:51Z', host: 'controller-a', projects: {},
    hierarchy: { jepson: 'Configured owner', grokbot: { name: 'Configured watcher', routines: ['One bounded source routine', 'Another source routine'] },
      super_captain: { screen: '902.super_captain', alive: true, model: 'agy Gemini 4 Argon', state: 'idle', ticker_alive: true },
      captains: [{ key: 'project-a', name: 'Project A', screen: '904.captain_a', model: 'agy Gemini 4 Argon', state: 'idle',
        counts: { done: 7, in_progress: 2, blocked: 1 }, done: 7, total: 10,
        workers: [
          { screen: 'worker_local', host: 'controller-a', model: 'ornith/Ornith-1.5-35B-A3B', effort: null, tier: 'T0', task: 'P-LOCAL', state: 'active', flags: [] },
          { screen: 'worker_luna', host: 'node-q', model: 'gpt-6-luna', effort: 'medium', tier: 'T0', task: 'P-LUNA', state: 'idle', flags: [] },
          { screen: 'worker_remote', host: 'node-r', model: 'unknown (remote: names only)', effort: null, tier: '—', task: null, state: 'remote', flags: [] },
          { screen: 'worker_blocked', host: 'node-s', model: 'claude haiku', tier: 'T1', task: 'P-BLOCKED', state: 'blocked', flags: ['Quota source warning'] },
          { screen: 'worker_finished', host: 'controller-a', model: 'unknown', task: null, state: 'finished', flags: [] },
          { screen: 'worker_gone', host: 'controller-a', model: 'unknown', task: null, state: 'gone', flags: [] },
        ] }], unassigned: [{ screen: 'source_unassigned', model: 'gpt-6-astra', state: 'unknown' }] },
    super_captain: { last_check: { no: '15', utc: 'source-check-time' }, status: { captains: { 'project-a': { hygiene: { grade: 'A', score: 100, notes: ['Recorded hygiene note'] } } } } },
  });
}

describe('source-faithful fleet hierarchy', () => {
  it('renders the real owner/watcher/super-captain shape and compact source worker columns', () => {
    const html = renderToStaticMarkup(<LegacyHierarchy snapshot={source()} />);
    for (const value of ['Configured owner', 'Configured watcher', '2 routines', 'One bounded source routine', '902.super_captain',
      'Admiral', 'Commodores', 'ticker ✓', 'check #15', 'source-check-time', 'data-captain-key="project-a"', '7/10 done', '2 in progress',
      'Hygiene', 'A 100', 'Recorded hygiene note', 'worker_local', 'P-LOCAL', 'medium', 'T0', 'node-q', 'node-r', 'node-s',
      '1 active', '1 idle', '1 remote', '1 blocked', 'Quota source warning', '+2 finished / ended screens', 'source_unassigned']) expect(html).toContain(value);
    for (const family of ['local', 'luna', 'gemini', 'claude', 'astra', 'unknown']) expect(html).toContain(`model-badge--${family}`);
    expect(html).toContain('grid-template-columns:repeat(1, minmax(150px, 1fr))');
    expect(html).not.toContain('<table'); expect(html).not.toContain('<iframe');
    expect(html).not.toContain('30 min each');
  });

  it('fits the supplied Grokbot asset in the named watcher card without branding other watchers', () => {
    const snapshot = source(); snapshot.hierarchy = { ...snapshot.hierarchy as Record<string, unknown>, grokbot: { name: 'Grok Bot (ENG:watcher)' } };
    const html = renderToStaticMarkup(<LegacyHierarchy snapshot={snapshot} />);
    expect(html).toContain('src="/brand/grokbot.png"'); expect(html).toContain('alt="Grokbot"');
    expect(html).toContain('width="44" height="44"'); expect(html).toContain('Configured owner profile picture');
    expect(renderToStaticMarkup(<LegacyHierarchy snapshot={source()} />)).not.toContain('/brand/grokbot.png');
  });

  it('includes a manifest project captain and lanes as ordinary columns with exact reported state', () => {
    const snapshot = source();
    snapshot.agents = [{ project: 'custom-build', screen: 'captain_custom-build', model: 'gpt-6-luna', effort: 'medium', tier: 'T0', state: 'unknown' },
      { project: 'unassigned', screen: 'custom_lane', model: 'observed model', state: 'unknown' }];
    snapshot.rollout = { project: { id: 'custom-build', name: 'Configured build' }, total: 32, counts: { in_progress: 0, review: 1 },
      lanes: [{ id: 'lane-1', screen: 'custom_lane', node: 'node-z', model: 'declared model', effort: 'medium', tier: 'T0', packet_id: 'P14', status: 'stale; reported in_progress; screen present' }] };
    const html = renderToStaticMarkup(<LegacyHierarchy snapshot={snapshot} />);
    expect((html.match(/data-captain-key=/g) ?? []).length).toBe(2);
    for (const value of ['data-captain-key="custom-build"', 'Configured build', 'captain_custom-build', 'custom_lane', 'observed model', 'P14',
      'node-z', '32 tasks · done not reported', '0 in progress', 'stale; reported in_progress; screen present']) expect(html).toContain(value);
    expect(html).not.toContain('0/32 done');
    expect(html).not.toContain('model-badge--luna" title="declared model');
  });

  it('keeps missing observations unknown, preserves zero counts, and never invents grades or active screens', () => {
    const snapshot = parseLegacy({ generated_utc: 'now', projects: {}, hierarchy: { super_captain: { alive: true },
      captains: [{ key: 'empty', name: 'Empty project', total: 0, done: 0, workers: [] },
        { key: 'unknown', name: 'Unknown project', workers: [{ screen: 'screen_only', alive: true, agent_running: false }] }] } });
    const html = renderToStaticMarkup(<LegacyHierarchy snapshot={snapshot} />);
    for (const value of ['Owner not reported', 'Watcher not reported', 'ticker not reported', '0/0 done', 'No workers reported', 'agent not running']) expect(html).toContain(value);
    expect(html).not.toContain('Hygiene'); expect(html).not.toContain('A 100');
    expect(html).not.toContain('1 active'); expect(html).not.toContain('1 remote');
    expect(html).not.toContain('0/? done');
  });

  it('escapes hierarchy, model, host, flags and task strings without HTML or state-class injection', () => {
    const snapshot = source(), attack = '<img src=x onerror="alert(1)">';
    snapshot.hierarchy = { owner: { name: attack }, watcher: { name: attack }, captains: [{ key: attack, name: attack, state: '__proto__', model: attack,
      workers: [{ screen: attack, host: attack, model: attack, task: attack, state: 'active injected-class', flags: [attack] }] }] };
    const html = renderToStaticMarkup(<LegacyHierarchy snapshot={snapshot} />);
    expect(html).toContain('&lt;img'); expect(html).not.toContain('<img'); expect(html).not.toContain('class="legacy-hierarchy-dot legacy-hierarchy-dot--active injected-class');
    expect(html).toContain('legacy-hierarchy-dot--unknown');
    expect(html).not.toContain('[object Object]'); expect(html).not.toContain('dangerouslySetInnerHTML');
  });

  it('bounds initially expanded worker rows with a visible continuation and keeps ended screens separate', () => {
    const snapshot = parseLegacy({ generated_utc: 'now', projects: {}, hierarchy: { captains: [{ key: 'many',
      workers: Array.from({ length: 45 }, (_, index) => ({ screen: `worker-${index}`, state: 'idle', model: 'unknown' })) }] } });
    const html = renderToStaticMarkup(<LegacyHierarchy snapshot={snapshot} />);
    expect((html.match(/data-worker-screen=/g) ?? []).length).toBe(40);
    expect(html).toContain('Show more screens (5)'); expect(html).toContain('45 idle');
    expect(html).not.toContain('finished / ended screens');
  });
});

describe('model family badges', () => {
  it('colors observed Claude short labels while keeping unavailable identities unknown', () => {
    for (const model of ['Haiku 4.5', 'Sonnet 4.5', 'Opus 4']) {
      expect(renderToStaticMarkup(<ModelBadge model={model} />)).toContain('model-badge--claude');
    }
    for (const model of ['unknown', 'Gemini (model unknown)', 'claude model unknown']) {
      expect(modelFamily(model)).toBe('unknown');
    }
  });
  it('colors configurable family names and preserves reported model/effort/tier labels', () => {
    for (const [model, family] of [['gpt-6-luna', 'luna'], ['gpt-6.1-sol', 'sol'], ['gpt-6-astra', 'astra'], ['agy Gemini 4 Argon', 'gemini'],
      ['google/gemini-custom', 'gemini'], ['anthropic/claude-haiku', 'claude'], ['ornith/Ornith-1.5-35B-A3B', 'local'],
      ['local/custom-checkpoint', 'local'], ['ollama/custom', 'local'], ['unregistered-provider/model', 'unknown']]) expect(modelFamily(model)).toBe(family);
    const html = renderToStaticMarkup(<ModelBadge model="gpt-6.1-sol" effort="xhigh" tier="T3" />);
    expect(html).toContain('model-badge--sol'); expect(html).toContain('gpt-6.1-sol xhigh · T3');
    expect(renderToStaticMarkup(<ModelBadge model=" " tier="—" />)).toContain('>unknown</span>');
    expect(renderToStaticMarkup(<ModelBadge model="Configured custom model" tier="custom tier" />)).toContain('Configured custom model · custom tier');
  });
});
