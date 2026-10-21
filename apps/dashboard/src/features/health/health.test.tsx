import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { parseLegacy } from '../../legacy';
import { age, countText, findings, gitRows, hostRows, parsePrometheus } from './health';
import { Attention, GitTable, Host, readinessFrom } from './FleetHealth';
import { CommandPalette, rankCommands, type PaletteCommand } from '../../ui/CommandPalette';

const NOW = Date.parse('2026-10-21T08:00:30Z');

function snapshot(extra: Record<string, unknown> = {}) {
  return parseLegacy({
    generated_utc: '2026-10-21T08:00:00Z', interval_s: 15, order: ['alpha', 'beta'],
    projects: { alpha: { name: 'Alpha' } },
    project_catalog: [{ id: 'beta', name: 'Beta from catalog' }],
    git_health: {
      alpha: { state: 'attention', observed_utc: '2026-10-21T08:00:00Z', integration_ref: 'refs/heads/main', divergence_threshold: 20,
        issues: [{ code: 'merge_conflicts' }, { code: 'merge_conflicts' }, { code: 'diverged_branch' }],
        metrics: { unmerged_branches: 3, merged_branches: 2, diverged_branches: 1, remote_unmerged_refs: 1,
          max_unmerged_behind: { value: 140, lower_bound: false }, max_unmerged_ahead: { value: 5, lower_bound: true },
          oldest_unmerged_tip_age_s: 7200, dirty_checkouts: 2, conflicted_checkouts: 1, detached_checkouts: 0,
          cleanup_candidate_checkouts: ['lane-a', 'lane-b'], integration_tip_age_s: 600,
          integration_commits_24h: { value: 4, lower_bound: false }, integration_commits_7d: { value: 31, lower_bound: true },
          integration_merges_7d: { value: 1, lower_bound: false }, integration_reverts_7d: { value: 0, lower_bound: false },
          integration_fixups_7d: { value: 2, lower_bound: false } } },
      beta: { state: 'unknown', issues: [{ code: 'shallow_history' }] },
    },
    global_resources: { hosts: [
      { host: 'build', reachable: true, nproc: 8, load: [10, 8, 6],
        mem: { avail_pct: 6.5, avail_gb: 2, total_gb: 31, swap_used_gb: 1, swap_total_gb: 8 },
        disks: [{ mount: '/', pct: 88, size: 100e9, avail: 12e9, growth_gb_per_h: 4, hours_to_full: 3, level: 'red' },
          { mount: '/data', pct: 10, size: 1000e9, avail: 900e9, growth_gb_per_h: -0.5, hours_to_full: null, level: 'ok' }],
        pressure: { cpu: { some_avg60: 12.5 }, memory: { full_avg60: 25 }, io: { full_avg60: 1 } }, d_state_procs: 2,
        top_rss: [{ pid: 42, user: 'builder', rss_gb: 8, pcpu: 190, comm: 'rustc' }],
        gpus: [{ idx: 0, name: 'RTX', util: 50, mem_used_mib: 1000, mem_total_mib: 24000, temp_c: 60 }],
        warnings: [{ level: 'red', text: 'build / 88.0% used, 12 GB free, ~3.0 h to full at 4.0 GB/h' },
          { level: 'amber', text: 'build memory pressure: all tasks stalled 25.0% of the last minute' }] },
      { host: 'remote', reachable: false, error: 'resource probe unavailable or incomplete',
        warnings: [{ level: 'amber', text: 'resource probe unavailable or incomplete' }] },
    ] },
    ...extra,
  });
}

describe('fleet health derivations', () => {
  it('builds Git rows in producer order with names, deduplicated issues and lower bounds', () => {
    const rows = gitRows(snapshot());
    expect(rows.map(row => [row.id, row.name, row.state])).toEqual([['alpha', 'Alpha', 'attention'], ['beta', 'Beta from catalog', 'unknown']]);
    expect(rows[0].issues).toEqual(['merge_conflicts', 'diverged_branch']);
    expect(countText(rows[0].commits7d)).toBe('≥31');
    expect(countText(rows[0].maxAhead)).toBe('≥5');
    expect(rows[0].cleanup).toEqual(['lane-a', 'lane-b']);
    expect(rows[1].unmerged).toBeNull();
    expect(rows[1].cleanup).toBeNull();
    expect(countText(rows[1].commits7d)).toBe('unknown');
  });

  it('keeps unreachable hosts free of invented numbers and preserves signed growth', () => {
    const [build, remote] = hostRows(snapshot());
    expect(build.loadPct).toBe(125);
    expect(build.disks[1].growthGbPerHour).toBe(-0.5);
    expect(build.pressure).toEqual({ cpuSome: 12.5, memoryFull: 25, ioFull: 1 });
    expect(build.top[0]).toMatchObject({ pid: 42, comm: 'rustc', rssGb: 8 });
    expect(remote.reachable).toBe(false);
    expect(remote.loadPct).toBeNull();
    expect(remote.pressure).toBeNull();
  });

  it('orders findings by severity and gives every one an action', () => {
    const items = findings(snapshot(), { status: 'not_ready', detail: 'Store check: timeout.' }, NOW);
    expect(items[0]).toMatchObject({ severity: 'critical', subject: 'admiraltyd' });
    const severities = items.map(item => item.severity);
    expect(severities).toEqual([...severities].sort((a, b) => ['critical', 'warning', 'info'].indexOf(a) - ['critical', 'warning', 'info'].indexOf(b)));
    const text = items.map(item => `${item.subject}: ${item.message}`);
    expect(text).toContain('build: build / 88.0% used, 12 GB free, ~3.0 h to full at 4.0 GB/h');
    expect(text).toContain('remote: Unreachable: resource probe unavailable or incomplete.');
    expect(text.filter(line => line.startsWith('remote:'))).toHaveLength(1);
    expect(text).toContain('Alpha: 1 merge commit reached refs/heads/main this week.');
    expect(text).toContain('Alpha: An unmerged lane trails integration by 140 commits.');
    expect(text).toContain('Alpha: 2 worktrees are merged and clean.');
    expect(text).toContain('Beta from catalog: Git health is unknown: shallow_history.');
    expect(items.find(item => item.message.includes('trails'))?.severity).toBe('warning');
    expect(items.every(item => item.action.length > 0)).toBe(true);
    expect(items.find(item => item.subject === 'Alpha')?.href).toBe('#/git?project=alpha');
  });

  it('flags a stale observer feed and stays quiet when everything is fine', () => {
    const stale = findings(snapshot(), { status: 'ready', detail: '' }, Date.parse('2026-10-21T08:30:00Z'));
    expect(stale.some(item => item.subject === 'Observer' && item.message.includes('30 min ago'))).toBe(true);
    const calm = findings(parseLegacy({ generated_utc: '2026-10-21T08:00:00Z', projects: {} }), { status: 'ready', detail: '' }, NOW);
    expect(calm).toEqual([]);
    expect(findings(null, { status: 'checking', detail: '' })).toEqual([]);
  });

  it('summarizes Prometheus text per route with errors and p95 bucket', () => {
    const summary = parsePrometheus([
      '# HELP admiralty_http_requests_total x',
      'admiralty_http_requests_total{route="/v2/views",class="2xx"} 90',
      'admiralty_http_requests_total{route="/v2/views",class="5xx"} 10',
      'admiralty_http_requests_total{route="/q\\"x",class="4xx"} 3',
      'admiralty_http_request_duration_seconds_bucket{route="/v2/views",le="0.01"} 50',
      'admiralty_http_request_duration_seconds_bucket{route="/v2/views",le="0.1"} 94',
      'admiralty_http_request_duration_seconds_bucket{route="/v2/views",le="1"} 100',
      'admiralty_http_request_duration_seconds_bucket{route="/v2/views",le="+Inf"} 100',
      'admiralty_uptime_seconds 3600.5',
      'garbage line',
    ].join('\n'));
    expect(summary.routes[0]).toEqual({ route: '/v2/views', requests: 100, clientErrors: 0, serverErrors: 10, p95LeSeconds: 1 });
    expect(summary.routes[1]).toMatchObject({ route: '/q"x', clientErrors: 3 });
    expect(summary.gauges.admiralty_uptime_seconds).toBe(3600.5);
  });

  it('formats ages and readiness responses', () => {
    expect([age(null), age(30), age(600), age(7200), age(259200)]).toEqual(['unknown', '30s', '10m', '2h', '3d']);
    expect(readinessFrom(null).status).toBe('checking');
    expect(readinessFrom({ status: 0, body: '', error: 'refused' })).toEqual({ status: 'unreachable', detail: 'Readiness check failed: refused.' });
    expect(readinessFrom({ status: 200, body: '{"status":"ready","checks":{"store_ms":3}}', error: '' }).status).toBe('ready');
    expect(readinessFrom({ status: 503, body: '{"status":"not_ready","checks":{"store":"timeout","draining":false}}', error: '' }))
      .toEqual({ status: 'not_ready', detail: 'Store check: timeout.' });
    expect(readinessFrom({ status: 503, body: '{"status":"not_ready","checks":{"draining":true}}', error: '' }).detail).toContain('draining');
  });
});

describe('fleet health rendering', () => {
  it('renders an accessible Git table and host meters with the 24-hour projection', () => {
    const table = renderToStaticMarkup(<GitTable rows={gitRows(snapshot())} />);
    expect(table).toContain('<caption class="sr-only">');
    expect(table).toContain('href="#/git?project=alpha"');
    expect(table).toContain('≥31');
    expect(table).not.toMatch(/NaN|undefined|Infinity/);
    const [build, remote] = hostRows(snapshot());
    const host = renderToStaticMarkup(<Host host={build} />);
    expect(host).toContain('role="meter"');
    expect(host).toContain('aria-valuetext="88% · 12 GB free · +4 GB/h, full in 3 h"');
    expect(host).toContain('class="health-meter-projection"');
    expect(host).toContain('not growing');
    expect(host).toContain('2 blocked in I/O');
    expect(renderToStaticMarkup(<Host host={remote} />)).toContain('No values are shown rather than guessed');
  });

  it('labels severity in text, not only color', () => {
    const html = renderToStaticMarkup(<Attention items={findings(snapshot(), { status: 'ready', detail: '' }, NOW)} observed="2026-10-21T08:00:00Z" />);
    expect(html).toContain('Critical');
    expect(html).toContain('Warning');
    expect(html).toContain('Note');
    expect(renderToStaticMarkup(<Attention items={[]} observed="" />)).toContain('Nothing needs attention');
  });
});

describe('command palette ranking', () => {
  const commands: PaletteCommand[] = ['Fleet detail', 'Fleet health', 'Git Flow', 'Fleet Atlas', 'Models', 'Switch to light theme']
    .map(label => ({ id: label, label, group: 'Observe', run: () => undefined }));
  commands[1].keywords = 'hosts disk memory';
  it('prefers label prefixes, then word prefixes, then keywords', () => {
    expect(rankCommands(commands, '').length).toBe(commands.length);
    expect(rankCommands(commands, 'fleet').map(c => c.label)).toEqual(['Fleet detail', 'Fleet health', 'Fleet Atlas']);
    expect(rankCommands(commands, 'atlas')[0].label).toBe('Fleet Atlas');
    expect(rankCommands(commands, 'disk').map(c => c.label)).toEqual(['Fleet health']);
    expect(rankCommands(commands, 'light')[0].label).toBe('Switch to light theme');
    expect(rankCommands(commands, 'zzz')).toEqual([]);
  });

  it('labels the dialog by a heading and points the active option at a valid id', () => {
    const html = renderToStaticMarkup(<CommandPalette commands={commands} onClose={() => undefined} />);
    const heading = /<h2 id="([^"]+)"[^>]*>Go to a view or run a command<\/h2>/.exec(html);
    expect(heading).not.toBeNull();
    expect(html).toContain(`aria-labelledby="${heading![1]}"`);
    const active = /aria-activedescendant="([^"]+)"/.exec(html)![1];
    expect(active).not.toMatch(/\s/);
    expect(html).toContain(`id="${active}" role="option" aria-selected="true"`);
    expect(html).not.toContain('aria-label="Go to');
  });
});
