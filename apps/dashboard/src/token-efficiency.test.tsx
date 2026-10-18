import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { parseLegacy } from './legacy';
import { TokenEfficiency } from './ui/TokenEfficiency';

const snapshot = () => parseLegacy({
  generated_utc: '2026-10-18T18:15:00Z', host: 'custom-observer-node', projects: {},
  token_efficiency: {
    agents_total: 21, codex_agents: 9, codex_on_cheapest: 4, pct_codex_on_cheapest: 44.4,
    cost_index_vs_all_luna: 20.4, cost_index_avg_output_rate: 255.6,
    cost_index_note: 'Per-agent output rate; not token weighted.',
    by_model: { 'agy Gemini 4 Argon': 9, 'gpt-6.1-sol high': 4, 'gpt-6-luna medium': 4, 'claude haiku': 3, 'gpt-6-astra medium': 1 },
    by_family: { codex: 9, claude: 3, agy: 9 }, codex_tiers: { T2: 4, T0: 4, T4: 1 },
    windows: {
      '60': { credits_per_hour_est: 411.5, input_tokens: 167_000_000, output_tokens: 690_000,
        by_model: { 'gpt-6.1-sol': { sessions: 6, input: 102_600_000, cached: 100_548_000, output: 350_000, credits: 388 },
          'gpt-6-luna': { sessions: 21, input: 64_200_000, cached: 62_916_000, output: 340_000, credits: 24 } } },
      '15': { credits_per_hour_est: 306.5 },
    },
    quota: { used_percent: 41, window_minutes: 10080, observed_utc: '2026-10-18T18:12:00Z',
      resets_at_utc: '2026-10-24T21:40:00Z', credits_balance: '9691', burn_pct_per_hour: 2.68, burn_window_min: 179, hours_to_100pct: 22.1 },
    flags: [{ screen: 'custom_worker', project: 'custom-project', text: 'Model too costly for SIMPLE task' }],
    ledger: { model_lines: 780, escalated: 19, lane_lines_with_class: 31, outcomes: { pass: 744, escalated: 19 },
      class_at_tier: { 'simple@T2': 2 }, note: 'MODEL usage ledger source' },
    ledger_stats: { attempts: 780, luna_first_pass_rate: 95.8, luna_failures_total: 19,
      funnel: { luna_tasks: 143, luna_to_sol: 17, sol_to_astra: 0 } },
    windows_note: 'Real token counts from Codex session events; files limited to the last 3h.',
    quota_note: 'Quota observed in rate_limits events.', claude_note: 'Claude/agy token counts are not tracked here.',
    rates_per_1m: { 'gpt-6-luna': { input: 1, cached: 0.1, output: 12.5 } }, rates_source: 'configured producer rates',
  },
  agents: [{ screen: 'worker-one', host: 'arbitrary-node', model: 'gpt-6-luna', effort: 'medium', family: 'codex', model_source: 'session turn_context' }],
  token_yield: { generated_utc: '2026-10-18T18:00:00Z', window_hours: 48, method: 'Source commit attribution estimate.',
    not_covered: ['non-Codex sessions'], total: { tokens: 10_410_000_000, yield_integration_credits_pct: 50.3, yield_main_credits_pct: 33.2, coverage_pct: 75 },
    projects: { 'custom-project': { tokens: 12_000, credits: 42, yield_integration_credits_pct: 60, yield_main_credits_pct: 30, coverage_pct: 80 } } },
});
const render = (source = snapshot()) => renderToStaticMarkup(<TokenEfficiency snapshot={source} />);

describe('source-faithful token efficiency panel', () => {
  it('renders dense KPIs, bright model bars and source-window provenance', () => {
    const html = render();
    for (const value of ['44.4%', '20.4×', '411.5', '306.5', 'Weekly quota used', '41%', '9,691',
      '2026-10-24T21:40:00Z', 'Observed agents by model', 'te-fill', 'te-luna', 'te-astra',
      'session turn_context', 'custom-observer-node', 'last 3h', 'not token weighted']) expect(html).toContain(value);
    expect(html).toContain('gpt-6-luna baseline');
    expect(html).toContain('agent weighted');
    expect(html).not.toContain('<dl');
  });
  it('keeps measured tokens, cached share and estimated credits separate', () => {
    const html = render();
    for (const value of ['measured tokens · estimated credits', '102.6M', '100.55M', '98%', '350k', '388',
      'Claude/agy token counts are not tracked here.', 'Producer rate assumptions', 'configured producer rates']) expect(html).toContain(value);
    expect(html).not.toContain('NaN'); expect(html).not.toContain('Infinity');
  });
  it('renders actual routing flags, ledger outcomes, escalation funnel and estimated yield', () => {
    const html = render();
    for (const value of ['custom_worker', 'custom-project', 'Model too costly for SIMPLE task', 'MODEL attempts',
      'pass 744', 'simple@T2 2', '95.8%', '143', '17', 'Token yield', '48h source window', '50.3%', '33.2%',
      '75%', 'Source commit attribution estimate.', 'non-Codex sessions', '2026-10-18T18:00:00Z']) expect(html).toContain(value);
    expect(html).not.toContain('acceptance verified');
  });
  it('preserves missing metrics as unknown without fabricating zero use or a weekly window', () => {
    const html = render(parseLegacy({ generated_utc: 'original-source-time', projects: {} }));
    expect(html).toContain('metrics are unavailable'); expect(html).toContain('Unknown');
    expect(html).toContain('Routing flags unavailable'); expect(html).toContain('window unknown');
    expect(html).not.toContain('Weekly quota used'); expect(html).not.toContain('0%');
    expect(html).not.toContain('0 cr/h'); expect(html).not.toContain('None detected');
  });
  it('retains explicit zeros, string zero balance and signed quota observations', () => {
    const source = snapshot();
    source.token_efficiency = { agents_total: 0, by_model: {}, codex_agents: 0, codex_on_cheapest: 0,
      pct_codex_on_cheapest: null, cost_index_vs_all_luna: 0, flags: [],
      windows: { '60': { credits_per_hour_est: 0, input_tokens: 0, output_tokens: 0 } },
      quota: { used_percent: 0, window_minutes: 300, credits_balance: '0', burn_pct_per_hour: -2, burn_window_min: 30 } };
    const html = render(source);
    expect(html).toContain('Quota used · 5h'); expect(html).toContain('0%'); expect(html).toContain('credit balance 0');
    expect(html).toContain('-2%/h over 30m'); expect(html).toContain('None detected by the producer');
    expect(html).not.toContain('Weekly quota used'); expect(html).not.toContain('agy Gemini');
  });
  it('never prices unknown models at zero or treats a priced subtotal as complete spend', () => {
    const source = snapshot();
    source.token_efficiency = { windows: { '60': { credits_per_hour_est: 12,
      by_model: { 'own-unpriced-model': { input: 100, cached: 150, output: 50, sessions: 1, credits: 0, unpriced: true },
        'missing-cost-model': { input: 0, cached: 0, output: 0, sessions: 0 } } } } };
    const html = render(source);
    expect(html).toContain('Estimate · priced subtotal'); expect(html).toContain('Unpriced');
    expect(html).toContain('Their cost is unknown'); expect(html).toContain('share unknown');
    expect(html).not.toContain('150%'); expect(html).not.toContain('NaN'); expect(html).not.toContain('Infinity');
  });
  it('derives only a clearly labelled fallback mix from agent records and preserves source objects', () => {
    const source = parseLegacy({ generated_utc: 'original-time', projects: {}, agents: [
      { model: '__proto__', family: 'own', screen: 'custom-a' },
      { model: 'own-local-model', family: 'own', screen: 'custom-b' },
      { model: 'own-local-model', family: 'own', screen: 'custom-c' },
    ], remote_screens: [{ screen: 'remote-not-an-agent', model: 'must-not-count' }] });
    const before = JSON.stringify(source), html = render(source);
    expect(html).toContain('mix from agent records'); expect(html).toContain('__proto__');
    expect(html).toContain('own-local-model: 2 observed agent processes'); expect(html).not.toContain('must-not-count');
    expect(JSON.stringify(source)).toBe(before);
  });
  it('escapes source-provided model names, flags and notes without executable markup', () => {
    const source = snapshot();
    source.token_efficiency = { by_model: { '<img src=x onerror=alert(1)>': 1 },
      flags: [{ text: '<script>alert(1)</script>' }], windows_note: '<svg onload=alert(1)>' };
    const html = render(source);
    expect(html).toContain('&lt;img'); expect(html).toContain('&lt;script&gt;'); expect(html).toContain('&lt;svg');
    expect(html).not.toContain('<img'); expect(html).not.toContain('<script>'); expect(html).not.toContain('<svg');
  });
});
