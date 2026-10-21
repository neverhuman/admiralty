import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { parseLegacy } from '../../legacy';
import { parseRoute, routeHref } from '../../router';
import { agentBrief, projectHealth, sourceAge } from './model';
import { OperationsView } from './Operations';

const stamp = '2026-10-21T12:00:00Z', now = Date.parse(stamp);
function report(patch: Record<string, unknown> = {}) {
  return { schema_version: 1, instance_id: 'fixture', observed_utc: stamp, integration_ref: 'refs/heads/main',
    integration_oid: 'a'.repeat(40), state: 'healthy', actionable: false, observed_commits: 12, observed_refs: 2,
    observed_checkouts: 1, missing_parent_oids: [], shallow: false, truncated: false, observation_error: false,
    fingerprint: 'fixture-fingerprint', issues: [], branches: [], ...patch };
}
function source() {
  return parseLegacy({ generated_utc: stamp, interval_s: 45,
    project_catalog: [{ id: 'missing', name: 'No report' }], projects: {
      healthy: { name: 'Healthy' }, unknown: { name: 'Partial' }, disabled: { name: 'Disabled' }, attention: { name: 'Needs review' },
    }, git_health: { healthy: report(), unknown: report({ state: 'unknown', truncated: true, actionable: true }),
      disabled: report({ state: 'disabled', integration_ref: null, integration_oid: null }),
      attention: report({ state: 'attention', actionable: true, issues: [
        { code: 'integration_worktree_dirty', severity: 'attention', ref_name: 'refs/heads/main', checkout_id: 'checkout', detail: 'private transcript and path' },
      ] }),
    }, collector: { implementation: 'rust', duration_ms: 12.34567, stages_ms: { git: 0 } },
    global_resources: { hosts: [{ host: 'fixture-node', reachable: false, nproc: null, load: null, mem: null }] },
    secret: 'never-copy-this', tail: 'raw session transcript',
  });
}
describe('fleet operations evidence', () => {
  it('keeps stale, future and unknown clocks separate and honors slow collection cadence', () => {
    expect(sourceAge(stamp, now, 45)).toBe('current');
    expect(sourceAge(stamp, now + 180_001, 45)).toBe('stale');
    expect(sourceAge(stamp, now + 180_001, 120)).toBe('current');
    expect(sourceAge(stamp, now - 30_001, 45)).toBe('future');
    expect(sourceAge('invalid', now, 45)).toBe('unknown');
  });
  it('prioritizes actionable partial evidence without promoting missing or disabled reports', () => {
    const rows = projectHealth(source(), now);
    expect(rows.map(row => row.id)).toEqual(['unknown', 'attention', 'missing', 'disabled', 'healthy']);
    expect(rows.find(row => row.id === 'unknown')).toMatchObject({ attention: true, incomplete: true });
    expect(rows.find(row => row.id === 'missing')).toMatchObject({ report: null, age: 'unknown', incomplete: true });
    expect(rows.find(row => row.id === 'disabled')).toMatchObject({ attention: false, incomplete: false });
  });
  it('isolates malformed reports instead of hiding the remaining fleet', () => {
    const input = source(); input.git_health = { ...(input.git_health as object), broken: report({ observed_commits: -1 }) };
    const rows = projectHealth(input, now);
    expect(rows.find(row => row.id === 'broken')).toMatchObject({ invalid: true, report: null, incomplete: true });
    expect(rows.find(row => row.id === 'healthy')?.report?.state).toBe('healthy');
  });
  it('bounds the agent exception inventory and records omissions without leaking source prose', () => {
    const input = source(); input.projects = Object.fromEntries(Array.from({ length: 1000 }, (_, i) => [`p${i}`, {}]));
    const rows = projectHealth(input, now), brief = agentBrief(input, rows), packet = JSON.parse(brief);
    expect(packet.exceptions).toHaveLength(20);
    expect(packet.omitted_exceptions).toBe(rows.filter(row => row.attention || row.incomplete || row.age !== 'current' || row.report?.state === 'disabled').length - 20);
    expect(new TextEncoder().encode(brief).length).toBeLessThan(10_000);
    expect(packet.acceptance_granted).toBe(false);
    for (const secret of ['never-copy-this', 'raw session transcript', 'private transcript and path']) expect(brief).not.toContain(secret);
  });
  it('preserves source time and report fingerprints in a deterministic brief', () => {
    const input = source(), rows = projectHealth(input, now), packet = JSON.parse(agentBrief(input, rows));
    expect(packet.source_observed_utc).toBe(stamp);
    expect(packet.exceptions.find((row: { project: string }) => row.project === 'attention')).toMatchObject({ fingerprint: 'fixture-fingerprint', codes: ['integration_worktree_dirty'] });
    expect(agentBrief(input, rows)).toBe(agentBrief(input, rows));
  });
  it('renders useful timings, source ages and unknown capacity with no raw source text', () => {
    const html = renderToStaticMarkup(<OperationsView snapshot={source()} now={now + 200_000} />);
    expect(html).toContain('Producer source: stale');
    expect(html).toContain('12.35'); expect(html).toContain('0'); expect(html).toContain('Unknown');
    expect(html).toContain('Incomplete coverage'); expect(html).toContain('No report observed');
    expect(html).toContain('Unreachable at source time'); expect(html).toContain('acceptance_granted');
    expect(html).not.toContain('never-copy-this'); expect(html).not.toContain('private transcript and path');
    expect(html).toContain('#/git?project=attention');
  });
  it('bounds rendered project rows and escapes names', () => {
    const input = source(); input.projects = Object.fromEntries(Array.from({ length: 300 }, (_, i) => [`p${i}`, { name: '<script>alert(1)</script>' }]));
    const html = renderToStaticMarkup(<OperationsView snapshot={input} now={now} />);
    expect((html.match(/<th scope="row">/g) ?? [])).toHaveLength(50);
    expect(html).toContain('&lt;script&gt;'); expect(html).not.toContain('<script>');
    expect(html).toContain('page 1 of 7');
  });
  it('supports direct links without changing the default route', () => {
    expect(parseRoute('#/operations').name).toBe('operations');
    expect(parseRoute('#/').name).toBe('detail');
    expect(routeHref('operations')).toBe('#/operations');
  });
});
