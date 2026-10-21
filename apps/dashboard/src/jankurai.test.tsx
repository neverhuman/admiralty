import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { JankuraiBadge, JankuraiSection, jankuraiFor, jankuraiStanding } from './features/jankurai/Jankurai';
import type { LegacySnapshot } from './legacy';

const snapshot = (projects: Record<string, unknown>): LegacySnapshot => ({
  generated_utc: '2026-10-21T14:00:00Z',
  projects: { core: {}, ui: {} },
  jankurai: { report_path: '.jankurai/repo-score.json', projects },
});

describe('jankurai scores from the observer', () => {
  it('keeps the auditor decision, minimum, findings and weakest dimension', () => {
    const summary = jankuraiFor(snapshot({ core: {
      status: 'observed', score: 56, decision: 'advisory', minimum_score: 85, passed: false,
      findings: { critical: 1, high: 52, medium: 27, low: 0, info: 0, total: 80 }, caps_total: 17,
      dimensions: [{ name: 'Code shape', score: 0, weight: 12 }, { name: 'Security', score: 88, weight: 12 }],
      weakest_dimension: 'Code shape', audited_head: '2f5b93157c3b0a2194929f08d166d18c1abaad43', generated_utc: '2026-10-21T13:55:48Z',
    } }), 'core');
    expect(summary?.status).toBe('observed');
    expect(jankuraiStanding(summary!)).toEqual({ tone: 'below', label: 'Below minimum 85' });
    const markup = renderToStaticMarkup(<JankuraiSection summary={summary!} />);
    expect(markup).toContain('Below minimum 85');
    expect(markup).toContain('Code shape (weakest)');
    expect(markup).toContain('audited 2f5b93157');
    expect(renderToStaticMarkup(<JankuraiBadge summary={summary!} />)).toContain('Jankurai 56');
  });

  it('never turns a missing or unreadable report into a score', () => {
    const missing = jankuraiFor(snapshot({}), 'ui');
    expect(missing?.status).toBe('missing');
    expect(missing?.score).toBeNull();
    expect(renderToStaticMarkup(<JankuraiBadge summary={missing!} />)).toContain('Jankurai unknown');
    expect(renderToStaticMarkup(<JankuraiSection summary={missing!} />)).toContain('No jankurai report');
    const invalid = jankuraiFor(snapshot({ ui: { status: 'invalid', reason: 'source exceeds configured byte limit' } }), 'ui');
    expect(renderToStaticMarkup(<JankuraiSection summary={invalid!} />)).toContain('source exceeds configured byte limit');
  });

  it('is absent when the observer predates score ingestion', () => {
    expect(jankuraiFor({ generated_utc: 'x', projects: {} }, 'core')).toBeNull();
  });
});
