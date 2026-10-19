import { useEffect, useRef, useState, type CSSProperties } from 'react';
import type { DuplicateCandidate, DuplicateCoverage, IntakePreview } from '../../../../packages/contracts/index';
import {
  buildDuplicateQuery,
  duplicateQueryDigest,
  queryIntakeDuplicates,
} from '../intakeDuplicates';

const panel: CSSProperties = {
  background: '#050505',
  color: '#f2f2f2',
  border: '1px solid #777',
  padding: '1rem',
  minWidth: 0,
};
const tableWrap: CSSProperties = { overflowX: 'auto', maxWidth: '100%' };
const table: CSSProperties = { width: '100%', borderCollapse: 'collapse', textAlign: 'left' };
const cell: CSSProperties = {
  borderBottom: '1px solid #666',
  padding: '.5rem',
  verticalAlign: 'top',
  overflowWrap: 'anywhere',
};
const button: CSSProperties = {
  background: '#18212b',
  color: '#fff',
  border: '1px solid #9aa9bb',
  borderRadius: 4,
  padding: '.5rem .75rem',
  font: 'inherit',
  cursor: 'pointer',
};

const PAGE_LIMIT = 20;
const MAX_PAGES = 5;
const MAX_ROWS = 100;

export type IntakeDuplicatesProps = {
  preview: IntakePreview;
  token: string;
  intakeToken: string;
  fetcher?: typeof fetch;
  disabled?: boolean;
};

type ResultsState = {
  previewIdentity: string;
  token: string;
  intakeToken: string;
  fetcher: typeof fetch;
  candidates: DuplicateCandidate[];
  coverage: DuplicateCoverage;
  queryDigest: string;
  pages: number;
  seenCursors: string[];
  loading: boolean;
  error: string | null;
  previous: boolean;
};

function coverageLabel(coverage: DuplicateCoverage): string {
  switch (coverage.kind) {
    case 'complete': return 'Coverage complete for this query.';
    case 'partial': return 'Coverage partial. More related work may be available.';
    case 'incomplete': return `Coverage incomplete: ${coverage.reason}`;
  }
}

export function IntakeDuplicateResults({
  candidates,
  coverage,
  queryDigest,
  previous = false,
}: {
  candidates: DuplicateCandidate[];
  coverage: DuplicateCoverage;
  queryDigest: string;
  previous?: boolean;
}) {
  return <section aria-label="Related work results" style={panel}>
    <h3>Related work</h3>
    {previous && <p role="status">Showing previous results; the latest page request failed.</p>}
    <p>{coverageLabel(coverage)}</p>
    <p>Latest page query digest: <code style={{ overflowWrap: 'anywhere' }}>{queryDigest}</code></p>
    {candidates.length === 0
      ? <p>No related work returned on this page. Coverage describes how much of the lookup was checked.</p>
      : <div style={tableWrap}>
        <table style={table}>
          <caption>Related work candidates. Values are evidence text only.</caption>
          <thead><tr>
            <th scope="col" style={cell}>Task</th>
            <th scope="col" style={cell}>Contract</th>
            <th scope="col" style={cell}>Reason</th>
            <th scope="col" style={cell}>Lifecycle</th>
            <th scope="col" style={cell}>Freshness</th>
            <th scope="col" style={cell}>Evidence references</th>
          </tr></thead>
          <tbody>{candidates.map((candidate, index) => <tr key={`${candidate.task_id}:${candidate.contract_revision}:${index}`}>
            <td style={cell}>{candidate.task_id}</td>
            <td style={cell}><code>{candidate.contract_revision}</code><br /><code>{candidate.contract_digest}</code></td>
            <td style={cell}>{candidate.reason}</td>
            <td style={cell}>{candidate.lifecycle}</td>
            <td style={cell}>{candidate.freshness}</td>
            <td style={cell}>{candidate.evidence_refs.length === 0
              ? 'No evidence references supplied.'
              : candidate.evidence_refs.map((reference, refIndex) => <div key={`${refIndex}:${reference}`}><code>{reference}</code></div>)}</td>
          </tr>)}</tbody>
        </table>
      </div>}
    <p>Related-work lookup is advisory and is not an exhaustive similarity search.</p>
  </section>;
}

export default function IntakeDuplicates({
  preview,
  token,
  intakeToken,
  fetcher = fetch,
  disabled = false,
}: IntakeDuplicatesProps) {
  const [state, setState] = useState<ResultsState | null>(null);
  const generation = useRef(0);
  const controller = useRef<AbortController | null>(null);
  const previewIdentity = [
    preview.manifest_digest,
    preview.authorized_scope.project_id ?? '',
    preview.authorized_scope.root_id ?? '',
    preview.authorized_scope.principal,
    preview.authorized_scope.authority_ref,
    preview.authorized_scope.authority_generation,
  ].join('\u0000');

  useEffect(() => () => {
    generation.current += 1;
    controller.current?.abort();
    controller.current = null;
  }, []);

  useEffect(() => {
    generation.current += 1;
    controller.current?.abort();
    controller.current = null;
    setState(null);
  }, [previewIdentity, token, intakeToken, fetcher, disabled]);

  const currentState = state?.previewIdentity === previewIdentity
    && state.token === token
    && state.intakeToken === intakeToken
    && state.fetcher === fetcher ? state : null;

  async function load(cursor: string | null, append: boolean) {
    if (disabled || !token || !intakeToken) return;
    if (controller.current) return;
    const projectId = preview.authorized_scope.project_id;
    const rootId = preview.authorized_scope.root_id;
    if (!projectId || !rootId) {
      setState({
        previewIdentity,
        token,
        intakeToken,
        fetcher,
        candidates: [],
        coverage: { kind: 'incomplete', reason: 'The preview does not contain a project and root scope.' },
        queryDigest: '',
        pages: 0,
        seenCursors: [],
        loading: false,
        error: 'Cannot search without the preview project and root scope.',
        previous: false,
      });
      return;
    }
    const prior = append ? currentState : null;
    if (append && !prior) return;
    const activeController = new AbortController();
    controller.current = activeController;
    const requestGeneration = ++generation.current;
    if (prior) setState({ ...prior, loading: true, error: null, previous: false });
    else setState({
      previewIdentity,
      token,
      intakeToken,
      fetcher,
      candidates: [],
      coverage: { kind: 'incomplete', reason: 'Search is in progress.' },
      queryDigest: '',
      pages: 0,
      seenCursors: [],
      loading: true,
      error: null,
      previous: false,
    });
    try {
      const query = buildDuplicateQuery(preview, cursor, PAGE_LIMIT);
      const expectedDigest = await duplicateQueryDigest(query);
      if (requestGeneration !== generation.current || activeController.signal.aborted) return;
      const page = await queryIntakeDuplicates(
        { project_id: projectId, root_id: rootId, query },
        token,
        intakeToken,
        fetcher,
        activeController.signal,
      );
      if (requestGeneration !== generation.current || activeController.signal.aborted) return;
      if (page.query_digest !== expectedDigest) throw new Error('The related-work response did not match this query.');
      const candidates = append && prior
        ? [...prior.candidates, ...page.candidates].slice(0, MAX_ROWS)
        : page.candidates.slice(0, MAX_ROWS);
      const seenCursors = append && prior && cursor !== null
        ? [...prior.seenCursors, cursor]
        : [];
      setState({
        previewIdentity,
        token,
        intakeToken,
        fetcher,
        candidates,
        coverage: page.coverage,
        queryDigest: page.query_digest,
        pages: append && prior ? prior.pages + 1 : 1,
        seenCursors,
        loading: false,
        error: null,
        previous: false,
      });
    } catch {
      if (requestGeneration !== generation.current || activeController.signal.aborted) return;
      if (append && prior) {
        setState({ ...prior, loading: false, error: 'Could not load the next page.', previous: true });
      } else {
        setState({
          previewIdentity,
          token,
          intakeToken,
          fetcher,
          candidates: [],
          coverage: { kind: 'incomplete', reason: 'The lookup could not be completed.' },
          queryDigest: '',
          pages: 0,
          seenCursors: [],
          loading: false,
          error: 'Could not search related work.',
          previous: false,
        });
      }
    } finally {
      if (controller.current === activeController) controller.current = null;
    }
  }

  const continuation = currentState?.coverage.kind === 'partial'
    ? currentState.coverage.continuation
    : null;
  const cursorCycle = continuation !== null && currentState?.seenCursors.includes(continuation) === true;
  const mayContinue = Boolean(currentState && continuation && !cursorCycle
    && currentState.pages < MAX_PAGES && currentState.candidates.length < MAX_ROWS);

  return <section aria-labelledby="related-work-title" style={panel}>
    <h3 id="related-work-title">Related work lookup</h3>
    <p>Search uses the current preview scope and does not create, change, or accept work.</p>
    <button
      type="button"
      style={button}
      disabled={disabled || !token || !intakeToken || currentState?.loading === true}
      onClick={() => void load(null, false)}
    >{currentState?.loading && currentState.pages === 0 ? 'Searching…' : 'Find related work'}</button>
    {!token || !intakeToken ? <p>Both owner access and the separate intake token are required.</p> : null}
    {currentState?.loading && <p role="status" aria-live="polite">
      {currentState.pages > 0 ? 'Loading next page…' : 'Searching related work…'}
    </p>}
    {currentState?.error && <p role="alert">{currentState.error}</p>}
    {currentState && currentState.pages > 0 && <>
      <IntakeDuplicateResults
        candidates={currentState.candidates}
        coverage={currentState.coverage}
        queryDigest={currentState.queryDigest}
        previous={currentState.previous}
      />
      {cursorCycle && <p role="status">The server repeated a continuation cursor. Pagination stopped safely.</p>}
      {currentState.pages >= MAX_PAGES && currentState.coverage.kind === 'partial'
        && <p role="status">The five-page lookup limit was reached; more related work may remain.</p>}
      {currentState.candidates.length >= MAX_ROWS && currentState.coverage.kind === 'partial'
        && <p role="status">The 100-result display limit was reached; more related work may remain.</p>}
      {mayContinue && <button
        type="button"
        style={button}
        disabled={currentState.loading}
        onClick={() => void load(continuation, true)}
      >{currentState.loading ? 'Loading…' : 'Load next page'}</button>}
    </>}
  </section>;
}
