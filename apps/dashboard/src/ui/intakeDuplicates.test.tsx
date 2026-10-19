import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { DuplicateCandidate, DuplicateCoverage, IntakePreview } from '../../../../packages/contracts/index';
import IntakeDuplicates, { IntakeDuplicateResults } from './IntakeDuplicates';

const candidate: DuplicateCandidate = {
  task_id: '<img src=x onerror=alert(1)>',
  contract_revision: 17,
  contract_digest: 'sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
  reason: 'advisory_raw_content',
  lifecycle: '<script>started</script>',
  freshness: 'observed <recently>',
  evidence_refs: ['ref:<unsafe>', 'opaque:second'],
};

function results(coverage: DuplicateCoverage, candidates: DuplicateCandidate[] = [candidate]) {
  return renderToStaticMarkup(<IntakeDuplicateResults
    candidates={candidates}
    coverage={coverage}
    queryDigest="sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
  />);
}

describe('related work result view', () => {
  it('renders complete coverage and candidate values as escaped evidence text, never links', () => {
    const html = results({ kind: 'complete' });
    expect(html).toContain('Coverage complete for this query.');
    expect(html).toContain('&lt;img src=x onerror=alert(1)&gt;');
    expect(html).toContain('&lt;script&gt;started&lt;/script&gt;');
    expect(html).toContain('ref:&lt;unsafe&gt;');
    expect(html).not.toContain('<img');
    expect(html).not.toContain('<script>');
    expect(html).not.toContain('<a ');
    expect(html).toContain('advisory_raw_content');
  });

  it('keeps partial and incomplete coverage explicit', () => {
    expect(results({ kind: 'partial', continuation: 'cursor-next' })).toContain('Coverage partial. More related work may be available.');
    expect(results({ kind: 'incomplete', reason: 'lineage is bounded' })).toContain('Coverage incomplete: lineage is bounded');
  });

  it('does not imply that an empty page means no duplicates', () => {
    const html = results({ kind: 'incomplete', reason: 'bounded lineage' }, []);
    expect(html).toContain('No related work returned on this page.');
    expect(html).toContain('Coverage describes how much of the lookup was checked.');
    expect(html).not.toContain('No duplicates');
  });

  it('renders an explicit search button without searching during render', () => {
    const preview = {
      authorized_scope: { principal: 'operator', authority_ref: 'grant', authority_generation: '1', project_id: 'project-a', root_id: 'root-a' },
    } as IntakePreview;
    const html = renderToStaticMarkup(<IntakeDuplicates preview={preview} token="owner-secret" intakeToken="intake-secret" />);
    expect(html).toContain('Find related work');
    expect(html).not.toContain('Related work results');
    expect(html).not.toContain('owner-secret');
    expect(html).not.toContain('intake-secret');
  });
});
