import { data, list, number, text, type LegacySnapshot } from '../../legacy';
import './jankurai.css';

/** Observer summary of one repository's latest `jankurai audit` report. */
export type JankuraiSummary = {
  status: 'observed' | 'missing' | 'invalid';
  score: number | null;
  decision: string;
  minimum: number | null;
  passed: boolean | null;
  findings: Record<string, number>;
  capsTotal: number | null;
  dimensions: Array<{ name: string; score: number | null; weight: number | null }>;
  weakest: string;
  auditedHead: string;
  generated: string;
  reason: string;
};

/** Read the observer's `jankurai.projects[id]`; absent data is `missing`, never zero. */
export function jankuraiFor(snapshot: LegacySnapshot, projectId: string): JankuraiSummary | null {
  const source = data(snapshot.jankurai);
  if (!Object.keys(source).length) return null;
  const entry = data(data(source.projects)[projectId]);
  const status = text(entry.status);
  const findings: Record<string, number> = {};
  for (const [key, value] of Object.entries(data(entry.findings))) {
    const count = number(value);
    if (count !== null) findings[key] = count;
  }
  return {
    status: status === 'observed' || status === 'invalid' ? status : 'missing',
    score: number(entry.score),
    decision: text(entry.decision),
    minimum: number(entry.minimum_score),
    passed: typeof entry.passed === 'boolean' ? entry.passed : null,
    findings,
    capsTotal: number(entry.caps_total),
    dimensions: list(entry.dimensions).map(value => {
      const dimension = data(value);
      return { name: text(dimension.name), score: number(dimension.score), weight: number(dimension.weight) };
    }).filter(dimension => dimension.name),
    weakest: text(entry.weakest_dimension),
    auditedHead: text(entry.audited_head),
    generated: text(entry.generated_utc),
    reason: text(entry.reason),
  };
}

/** Standing relative to the auditor's own minimum: shape and word, not colour alone. */
export function jankuraiStanding(summary: JankuraiSummary): { tone: 'pass' | 'below' | 'unknown'; label: string } {
  if (summary.status !== 'observed' || summary.score === null) return { tone: 'unknown', label: summary.status === 'invalid' ? 'Report unreadable' : 'No report' };
  if (summary.minimum === null) return { tone: 'unknown', label: summary.decision || 'Observed' };
  return summary.score >= summary.minimum
    ? { tone: 'pass', label: `At or above minimum ${summary.minimum}` }
    : { tone: 'below', label: `Below minimum ${summary.minimum}` };
}

export function JankuraiBadge({ summary }: { summary: JankuraiSummary }) {
  const standing = jankuraiStanding(summary);
  return <span className={`jk-badge jk-${standing.tone}`} title={standing.label}>
    <i aria-hidden="true" />Jankurai {summary.score === null ? 'unknown' : `${summary.score}`}
    <span className="jk-visually-hidden"> ({standing.label})</span>
  </span>;
}

export function JankuraiSection({ summary }: { summary: JankuraiSummary }) {
  const standing = jankuraiStanding(summary);
  if (summary.status !== 'observed') {
    return <section className="jk-section"><h3>Coding standard</h3><p className="jk-empty">{summary.status === 'invalid' ? `Jankurai report unreadable: ${summary.reason || 'unknown reason'}.` : 'No jankurai report in this repository yet. Run `jankurai audit` there to publish one.'}</p></section>;
  }
  const findingOrder = ['critical', 'high', 'medium', 'low'];
  return <section className="jk-section" aria-label="Jankurai coding standard">
    <h3>Coding standard</h3>
    <div className="jk-headline">
      <b className="jk-score">{summary.score ?? 'Unknown'}</b>
      <span className={`jk-standing jk-${standing.tone}`}>{standing.label}</span>
      <span className="jk-meta">{summary.decision || 'decision not reported'}{summary.auditedHead ? `, audited ${summary.auditedHead.slice(0, 9)}` : ''}{summary.generated ? ` at ${summary.generated}` : ''}</span>
    </div>
    <dl className="jk-findings">
      {findingOrder.map(severity => <div key={severity}><dt>{severity}</dt><dd>{summary.findings[severity] ?? 'Unknown'}</dd></div>)}
      <div><dt>score caps</dt><dd>{summary.capsTotal ?? 'Unknown'}</dd></div>
    </dl>
    {summary.dimensions.length > 0 && <ul className="jk-dimensions">
      {summary.dimensions.map(dimension => <li key={dimension.name} className={dimension.name === summary.weakest ? 'jk-weakest' : ''}>
        <span className="jk-dimension-name">{dimension.name}{dimension.name === summary.weakest ? ' (weakest)' : ''}</span>
        <span className="jk-track" aria-hidden="true"><span style={{ width: `${Math.max(0, Math.min(100, dimension.score ?? 0))}%` }} /></span>
        <span className="jk-dimension-score">{dimension.score ?? 'Unknown'}</span>
      </li>)}
    </ul>}
  </section>;
}
