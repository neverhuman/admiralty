import { useRef, useState } from 'react';
import { comparison, DIMENSIONS, exportReport, MAX_REPORT_BYTES, parseReportText, totals, type Dimension, type Report } from './report';
import './economics.css';

export function Economics() {
  const [report, setReport] = useState<Report | null>(null), [error, setError] = useState('');
  const [dimension, setDimension] = useState<Dimension>('role'), [selection, setSelection] = useState('');
  const [baseline, setBaseline] = useState(''), [candidate, setCandidate] = useState('');
  const request = useRef(0);
  const rows = report?.rows.filter(row => !selection || row[dimension] === selection) ?? [];
  const values = [...new Set(report?.rows.map(row => row[dimension]) ?? [])].sort();
  const cohorts = [...new Set(report?.rows.map(row => row.cohort) ?? [])].sort();
  const groups = [...new Set(rows.map(row => row[dimension]))].sort();
  async function load(file: File | undefined) {
    const sequence = ++request.current;
    setReport(null); setError(''); setSelection(''); setBaseline(''); setCandidate('');
    if (!file) return;
    try {
      if (file.size > MAX_REPORT_BYTES) throw Error('Economics report exceeds 1 MiB.');
      const parsed = parseReportText(await file.text());
      if (sequence === request.current) setReport(parsed);
    } catch (failure) { if (sequence === request.current) setError(failure instanceof Error ? failure.message : 'Unable to read report.'); }
  }
  function download() {
    if (!report) return;
    try {
      const url = URL.createObjectURL(new Blob([exportReport(report, rows)], { type: 'application/json' }));
      const link = document.createElement('a'); link.href = url; link.download = 'economics-selection.json'; link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (failure) { setError(String(failure)); }
  }
  return <section className="economics panel" aria-label="Economics explorer">
    <h2>Economics explorer</h2>
    <p>Imported observations · separate from legacy estimates. Reports stay in this browser session.</p>
    <label>Open economics report <input type="file" accept="application/json,.json" onChange={event => { void load(event.target.files?.[0]); event.target.value = ''; }} /></label>
    {error && <p role="alert">{error}</p>}
    {!report ? <p>Normalized usage unavailable. Open a source-bound report to inspect role, purpose, provider, project and cache coverage.</p> : <>
      <p className="economics-provenance">Observed {report.observed_utc} · source {report.source_revision} · {report.rows.length} observations</p>
      <div className="economics-controls">
        <label>Break down by <select value={dimension} onChange={event => { setDimension(event.target.value as Dimension); setSelection(''); }}>{DIMENSIONS.map(key => <option key={key}>{key}</option>)}</select></label>
        <label>Selected group <select value={selection} onChange={event => setSelection(event.target.value)}><option value="">All groups</option>{values.map(value => <option key={value}>{value}</option>)}</select></label>
        <button className="text-button" type="button" onClick={download}>Export selection (JSON)</button>
        <button className="text-button" type="button" onClick={() => { ++request.current; setReport(null); setError(''); }}>Clear report</button>
      </div>
      <div className="table-scroll"><table aria-label="Economics breakdown"><thead><tr><th>{dimension}</th><th>Unit / cohort / source version</th><th>Amount</th><th>Coverage</th><th>Manager share</th></tr></thead><tbody>
        {groups.flatMap(group => totals(rows.filter(row => row[dimension] === group)).map(total => <tr key={JSON.stringify([group, total.unit, total.source, total.version, total.cohort])}><td>{group}</td><td>{total.unit} / {total.cohort} / {total.source} / {total.version}</td><td>{total.amount ?? 'Unavailable'}</td><td>{total.coverage} · {total.known}/{total.count}</td><td>{total.manager_share ?? 'Unavailable'}</td></tr>))}
        {!rows.length && <tr><td colSpan={5}>No observations in this selection.</td></tr>}
      </tbody></table></div>
      <h3>Fixed-cohort comparison</h3>
      <div className="economics-controls">{(['Baseline', 'Candidate'] as const).map(label => <label key={label}>{label} cohort <select value={label === 'Baseline' ? baseline : candidate} onChange={event => (label === 'Baseline' ? setBaseline : setCandidate)(event.target.value)}><option value="">Select cohort</option>{cohorts.map(value => <option key={value}>{value}</option>)}</select></label>)}</div>
      <p role="status">{comparison(report.rows, baseline, candidate)}</p>
      <details><summary>Task spend, harness floor and context rotation</summary><div className="table-scroll"><table aria-label="Economics observations"><thead><tr><th>Task / cohort</th><th>Outcome / maturity</th><th>Spend / unit</th><th>Harness floor</th><th>Rotation / cache</th><th>Source / version</th></tr></thead><tbody>{rows.slice(0, 128).map(row => <tr key={row.id}><td>{row.task} / {row.cohort}</td><td>{row.outcome} / {row.maturity}</td><td>{row.amount ?? 'Unavailable'} / {row.unit} ({row.coverage})</td><td>{row.harness_floor ?? 'Unavailable'} {row.unit}</td><td>{row.rotation} / {row.cache}</td><td>{row.source} / {row.version}</td></tr>)}</tbody></table></div><p>Showing {Math.min(rows.length, 128)} of {rows.length} rows. Export includes the full selection.</p></details>
    </>}
    <details><summary>Metric definitions and report format</summary>
      <p>Amount is an exact nonnegative decimal string in the named unit, including currency and scale (for example USD:micro, quota:credit, token:input). Units are never added together. Null means unavailable. Estimated values and partial subtotals retain their coverage. Manager share includes roles manager, admiral and commodore and requires complete coverage.</p>
      <p>Each observation is a disjoint usage increment after upstream correction and parent/child reconciliation. Duplicate IDs are rejected. Harness floor is the source estimate in the same unit. Cohorts retain every fixed task, including failed, cancelled and reopened work; maturity, cache and rotation are source labels. No interpolation crosses source versions or missing observations.</p>
      <p>Report envelope: schema_version: 1, observed_utc: ISO timestamp, source_revision: full Git SHA, rows: up to 1024 observations. Each row requires id, project, task, role, purpose, provider, cache, cohort, maturity, outcome, rotation, unit, amount (decimal string or null), coverage (complete, estimated or unknown), harness_floor (decimal string or null), source and version. Maximum import and export: 1 MiB. Source labels are declared provenance, not authenticated measurements.</p>
    </details>
  </section>;
}
