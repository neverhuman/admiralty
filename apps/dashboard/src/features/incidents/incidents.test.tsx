import { afterEach, describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { IncidentHistory } from './Incidents';
import { incidentRequest, parseSnapshot, parseScope, validateReceipt, type Scope, type Snapshot } from './api';

const scope: Scope = { project: 'p', root: 'r', task: 't', contract_revision: 1, contract_digest: `sha256:${'a'.repeat(64)}`, protected_oracle_digest: `sha256:${'b'.repeat(64)}` };
const snapshot: Snapshot = { scope, revision: 1, state: { scope, episodes: [{ scope, episode_id: 'episode-one', family: 'unknown-failure', unknown_attribution: true, events: [{ event_id: 'failure', kind: 'failure', cause_parent: null, evidence_digests: [] }] }] }, proposals: [] };
afterEach(() => vi.unstubAllGlobals());
describe('scoped incident inspection', () => {
  it('keeps unknown attribution and an empty proposal set visible', () => {
    const html = renderToStaticMarkup(<IncidentHistory snapshot={parseSnapshot(snapshot, scope)} />);
    expect(html).toContain('Attribution unknown'); expect(html).toContain('cause unknown');
    expect(html).toContain('No local proposals recorded.'); expect(html).not.toContain('href=');
  });
  it('rejects foreign scope, malformed evidence and an execution grant', () => {
    expect(() => parseSnapshot(snapshot, { ...scope, root: 'foreign' })).toThrow();
    expect(() => parseScope({ ...scope, contract_revision: 0 })).toThrow();
    const bad = structuredClone(snapshot); bad.state!.episodes[0].events[0].evidence_digests = ['raw transcript'];
    expect(() => parseSnapshot(bad, scope)).toThrow();
    const proposal = { command_id: 'p1', proposal: { scope, episode_digest: scope.contract_digest, title: 'Investigate recorded excursion', reproduction_contract: { fields: { steps: '<script>never execute</script>' } }, evidence_digests: [], uncertainty: 'Cause unknown', diagnosis: 'Bounded investigation', external_publication_allowed: false as const, execution_granted: false as const, proposed_investigator_max_steps: 8, proposed_investigator_max_model_turns: 1 } };
    const valid = parseSnapshot({ ...snapshot, proposals: [proposal] }, scope);
    const html = renderToStaticMarkup(<IncidentHistory snapshot={valid} />);
    expect(html).toContain('&lt;script&gt;'); expect(html).toContain('Execution is not granted'); expect(html).toContain('External publication is not granted');
    expect(() => parseSnapshot({ ...snapshot, proposals: [{ ...proposal, proposal: { ...proposal.proposal, execution_granted: true } }] }, scope)).toThrow();
  });
  it('uses fixed local endpoints and header-only credentials', async () => {
    const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify(snapshot), { status: 200 })); vi.stubGlobal('fetch', fetch);
    expect(parseSnapshot(await incidentRequest('read', scope, 'private-test-token'), scope)).toEqual(snapshot);
    const [url, options] = fetch.mock.calls[0]; expect(url).toBe('/v2/incidents/read');
    expect(options.headers.Authorization).toBe('Bearer private-test-token'); expect(options.body).not.toContain('private-test-token');
    expect(options.redirect).toBe('error'); expect(options.cache).toBe('no-store');
  });
  it('accepts only a receipt for the exact submitted command and revision', () => {
    const command = { command_id: 'p1', expected_revision: 2 };
    const receipt = { command_id: 'p1', revision: 3, snapshot_digest: scope.contract_digest };
    expect(() => validateReceipt({ replayed: true, receipt }, command)).not.toThrow();
    expect(() => validateReceipt({ replayed: false, receipt: { ...receipt, command_id: 'foreign' } }, command)).toThrow();
    expect(() => validateReceipt({ replayed: false, receipt: { ...receipt, revision: 4 } }, command)).toThrow();
  });
  it('bounds streamed response bytes and hides untrusted server error text', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('s'.repeat(2 * 1024 * 1024 + 1))));
    await expect(incidentRequest('read', scope, 'token')).rejects.toThrow('exceeds its bound');
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('secret=private', { status: 403 })));
    await expect(incidentRequest('read', scope, 'token')).rejects.toThrow('Access to this incident scope was denied.');
  });
});
