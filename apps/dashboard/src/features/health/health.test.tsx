import { afterEach, describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { clockState, fetchBrief, HealthAccessError, parseBrief, type FleetBrief } from './api';
import { HealthReport } from './FleetHealth';

const fixture = (): FleetBrief => ({ schema_version: 1, read_only: true, source_sha256: `sha256:${'a'.repeat(64)}`, source_bytes: 100000,
  stale_after_seconds: 180, source_clock: { observed_utc: '2026-10-21T11:00:00Z', age_seconds: 3600, state: 'stale' }, collector_duration_ms: 42, diagnostics: [],
  totals: { projects: 1, git_review_projects: 0, git_attention: 0, git_unknown: 0, git_disabled: 0, git_missing: 1, git_healthy: 0, git_not_fresh: 0, dirty_checkouts: 0, conflicted_checkouts: 0, unknown_checkouts: 0 },
  projects: [{ id: 'project-a', total: '9007199254740993', done: '1', blocked: null, git_state: 'missing', git_clock: { observed_utc: null, age_seconds: null, state: 'unknown' }, observed_branches: null, diverged_branches: null, dirty_checkouts: null, conflicted_checkouts: null, unknown_checkouts: null, issue_codes: {} }],
  omitted_projects: 0, detail_path: '/v1/legacy' });
afterEach(() => vi.unstubAllGlobals());
describe('fleet health evidence', () => {
  it('shows stale clocks, missing data and exact counters without implying a clean fleet', () => {
    const html = renderToStaticMarkup(<HealthReport brief={parseBrief(fixture())} now={Date.parse('2026-10-21T12:00:00Z')} />);
    expect(html).toContain('Producer observations are stale'); expect(html).toContain('9007199254740993');
    expect(html).toContain('unknown'); expect(html).toContain('missing'); expect(html).not.toContain('No reported issues');
    expect(html).toContain('Summary counts cover all 1 source projects');
  });
  it('recomputes freshness for a retained response and distinguishes future/unknown clocks', () => {
    const c = { ...fixture().source_clock, state: 'fresh' as const };
    expect(clockState(c, Date.parse('2026-10-21T12:00:00Z'), 180)).toBe('stale');
    expect(clockState(c, Date.parse('2026-10-21T10:00:00Z'), 180)).toBe('future');
    expect(clockState({ observed_utc: null, state: 'unknown', age_seconds: null }, Date.now(), 180)).toBe('unknown');
  });
  it('rejects malformed counters, coverage and injected issue text', () => {
    const bad = fixture(); bad.projects[0].total = '18446744073709551616'; expect(() => parseBrief(bad)).toThrow();
    const omitted = fixture(); omitted.omitted_projects = 2; expect(() => parseBrief(omitted)).toThrow();
    const code = fixture(); code.projects[0].issue_codes = { '<script>': 1 }; expect(() => parseBrief(code)).toThrow();
    const grant = fixture(); (grant as {read_only: boolean}).read_only = false; expect(() => parseBrief(grant)).toThrow();
  });
  it('uses a bounded local request, header credentials and generic error text', async () => {
    const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify(fixture()))); vi.stubGlobal('fetch', fetch);
    expect(await fetchBrief('secret-token')).toEqual(fixture());
    const [url, options] = fetch.mock.calls[0]; expect(url).toBe('/v1/legacy/brief');
    expect(options.headers.Authorization).toBe('Bearer secret-token'); expect(options.redirect).toBe('error');
    fetch.mockResolvedValue(new Response('private raw server message', { status: 500 }));
    await expect(fetchBrief(undefined)).rejects.toThrow('Fleet health is unavailable.');
    fetch.mockResolvedValue(new Response('s'.repeat(16385)));
    await expect(fetchBrief(undefined)).rejects.toThrow('API response too large');
    fetch.mockResolvedValue(new Response('private reason', { status: 403 }));
    await expect(fetchBrief('revoked')).rejects.toBeInstanceOf(HealthAccessError);
  });
});
