import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import goldenText from '../../../packages/contracts/fixtures/intake-duplicate-query-golden.json?raw';
import {
  validateDuplicateQuery,
  type DuplicateCandidate,
  type DuplicatePage,
  type DuplicateQuery,
  type IntakePreview,
} from '../../../packages/contracts/index';
import { buildDuplicateQuery, duplicateQueryDigest, queryIntakeDuplicates } from './intakeDuplicates';

const digest = (character: string) => `sha256:${character.repeat(64)}`;
const golden = JSON.parse(goldenText) as Array<{
  query: DuplicateQuery;
  query_digest: string;
}>;
const ownerToken = 'owner-test-token';
const intakeToken = 'intake-test-token';
const browserCrypto = globalThis.crypto;

function preview(): IntakePreview {
  return {
    schema_version: 1,
    parser_revision: 'parser-1',
    policy_revision: 'policy-1',
    policy_digest: digest('c'),
    format: 'plain_text',
    raw_digest: digest('a'),
    canonical_digest: digest('b'),
    raw_byte_length: 4,
    provenance: {
      schema_version: 1,
      source_id: 'capture-command-namespace',
      revision: 'source-revision',
      captured_at: '2026-10-19T11:55:00Z',
      requester: { kind: 'unknown' },
      origin: { system: 'tracker', record_id: 'record-1', revision: 'revision-1' },
    },
    authorized_scope: { principal: 'principal-1', project_id: 'my-project', root_id: 'my-root', authority_ref: 'authority-1', authority_generation: '1' },
    requests: [],
    manifest_digest: digest('d'),
  };
}
function request(query = buildDuplicateQuery(preview())) {
  return { project_id: query.project_id, root_id: 'my-root', query };
}
function candidate(): DuplicateCandidate {
  return {
    task_id: 'task-1', contract_revision: 1, contract_digest: digest('e'),
    reason: 'advisory_raw_content', lifecycle: 'awaiting_verification',
    freshness: 'unknown', evidence_refs: ['lineage:outcome-1'],
  };
}
async function page(query = buildDuplicateQuery(preview())): Promise<DuplicatePage> {
  return { schema_version: 1, query_digest: await duplicateQueryDigest(query), candidates: [candidate()], coverage: { kind: 'incomplete', reason: 'Bounded lineage; older provenance may be absent.' } };
}
function envelope(data: DuplicatePage) {
  return { schema_version: 1, data, execution_granted: false, acceptance_granted: false };
}
function returning(value: unknown): typeof fetch {
  return vi.fn(async () => Response.json(value));
}

beforeEach(() => vi.stubGlobal('crypto', browserCrypto));
afterEach(() => vi.unstubAllGlobals());

describe('duplicate query identity', () => {
  it('builds content lookup from preview scope with explicit nulls, without reusing capture identity', () => {
    const query = buildDuplicateQuery(preview());
    expect(query).toEqual(golden[0].query);
    expect(buildDuplicateQuery(preview(), 'page-2', 7)).toMatchObject({ cursor: 'page-2', limit: 7 });
    const missingRoot = preview();
    missingRoot.authorized_scope.root_id = null;
    expect(() => buildDuplicateQuery(missingRoot)).toThrow('query is invalid');
  });

  it.each(golden)('matches a typed Rust identity_digest golden fixture ($query_digest)', async fixture => {
    expect(validateDuplicateQuery(fixture.query)).toBe(true);
    expect(await duplicateQueryDigest(fixture.query)).toBe(fixture.query_digest);
  });

  it('normalizes missing optional fields to null and ignores caller property insertion order', async () => {
    const query = { limit: 20, canonical_digest: digest('b'), raw_digest: digest('a'), project_id: 'my-project', schema_version: 1 };
    expect(await duplicateQueryDigest(query)).toBe(golden[0].query_digest);
    const unicode = golden[1].query;
    const reordered = { ...unicode, origin: { revision: unicode.origin!.revision, record_id: unicode.origin!.record_id, system: unicode.origin!.system } };
    expect(await duplicateQueryDigest(reordered)).toBe(golden[1].query_digest);
  });

  it('binds every query identity field, including cursor and limit', async () => {
    const query = buildDuplicateQuery(preview());
    const changes: Partial<DuplicateQuery>[] = [
      { project_id: 'other-project' }, { raw_digest: digest('c') }, { canonical_digest: digest('d') },
      { origin: { system: 'tracker', record_id: 'record-1', revision: 'revision-1' } },
      { command_id: 'duplicate-command-1' }, { cursor: 'page-2' }, { limit: 19 },
    ];
    for (const change of changes) expect(await duplicateQueryDigest({ ...query, ...change })).not.toBe(golden[0].query_digest);
  });

  it('rejects domain-invalid queries, trailing newlines, malformed Unicode and unsafe references', async () => {
    const query = buildDuplicateQuery(preview());
    const changes: Partial<DuplicateQuery>[] = [
      { schema_version: 2 }, { project_id: 'project\n' }, { project_id: '..' },
      { raw_digest: `${digest('a')}\n` }, { canonical_digest: digest('A') },
      { limit: 0 }, { limit: 101 }, { limit: 1.5 }, { limit: Number.MAX_SAFE_INTEGER + 1 },
      { command_id: 'command\n' }, { cursor: 'https:secret' }, { cursor: '../source' }, { cursor: '\u0085' },
      { cursor: 'has space' }, { cursor: '\ud800' }, { cursor: '雪'.repeat(342) },
      { origin: { system: 'tracker\n', record_id: 'record-1', revision: 'revision-1' } },
      { origin: { system: 'tracker', record_id: 'record-1', revision: 'file:local' } },
    ];
    for (const change of changes) await expect(duplicateQueryDigest({ ...query, ...change })).rejects.toThrow('query is invalid');
  });

  it('rejects accessors and cyclic inputs without evaluating caller accessors', async () => {
    const query = buildDuplicateQuery(preview());
    const getter = vi.fn(() => 'project-1');
    Object.defineProperty(query, 'project_id', { get: getter, enumerable: true });
    await expect(duplicateQueryDigest(query)).rejects.toThrow('query is invalid');
    expect(getter).not.toHaveBeenCalled();
    const cyclic = buildDuplicateQuery(preview()) as DuplicateQuery & { extra?: unknown };
    cyclic.extra = cyclic;
    await expect(duplicateQueryDigest(cyclic)).rejects.toThrow('query is invalid');
  });

  it('fails closed before fetching when secure SHA-256 is unavailable', async () => {
    vi.stubGlobal('crypto', undefined);
    const fetcher = returning({});
    await expect(queryIntakeDuplicates(request(), ownerToken, intakeToken, fetcher)).rejects.toThrow('Secure browser SHA-256');
    expect(fetcher).not.toHaveBeenCalled();
  });
});

describe('scoped duplicate lookup', () => {
  it('posts only the scoped query body with separate credentials in headers and forwards abort', async () => {
    const input = request();
    const resultPage = await page(input.query);
    const controller = new AbortController();
    const fetcher: typeof fetch = vi.fn(async (url, init) => {
      expect(url).toBe('/v2/intake/duplicates/query');
      expect(init?.method).toBe('POST');
      expect(init?.cache).toBe('no-store');
      expect(init?.redirect).toBe('error');
      expect(init?.signal).toBe(controller.signal);
      const headers = new Headers(init?.headers);
      expect(Object.fromEntries(headers)).toEqual({
        authorization: `Bearer ${ownerToken}`,
        'content-type': 'application/json',
        'x-admiralty-intake-authorization': `Bearer ${intakeToken}`,
        'x-admiralty-requested-with': 'intake',
      });
      expect(headers.has('Origin')).toBe(false);
      expect(headers.has('Host')).toBe(false);
      expect(JSON.parse(String(init?.body))).toEqual(input);
      expect(String(init?.body)).not.toContain(ownerToken);
      expect(String(init?.body)).not.toContain(intakeToken);
      return Response.json(envelope(resultPage));
    });
    expect(await queryIntakeDuplicates(input, ownerToken, intakeToken, fetcher, controller.signal)).toEqual(resultPage);
    expect(fetcher).toHaveBeenCalledOnce();
  });

  it('snapshots query and outer scope before its asynchronous digest work', async () => {
    const input = request();
    const original = structuredClone(input);
    const resultPage = await page(input.query);
    const fetcher: typeof fetch = vi.fn(async (_url, init) => {
      expect(JSON.parse(String(init?.body))).toEqual(original);
      return Response.json(envelope(resultPage));
    });
    const pending = queryIntakeDuplicates(input, ownerToken, intakeToken, fetcher);
    input.root_id = 'changed-root';
    input.project_id = 'changed-project';
    input.query.cursor = 'changed-cursor';
    expect(await pending).toEqual(resultPage);
  });

  it('rejects scope mismatch, invalid root and extra outer fields before fetching', async () => {
    const fetcher = returning({});
    for (const input of [
      { ...request(), project_id: 'other-project' },
      { ...request(), root_id: 'root\n' },
      { ...request(), extra: 'not-an-authority' },
    ]) await expect(queryIntakeDuplicates(input, ownerToken, intakeToken, fetcher)).rejects.toThrow('query is invalid');
    expect(fetcher).not.toHaveBeenCalled();
  });

  it.each([
    ['', intakeToken], [ownerToken, ''], [ownerToken, ownerToken],
    [`${ownerToken}\n`, intakeToken], [ownerToken, 'has space'], ['a'.repeat(4097), intakeToken],
  ])('rejects absent, reused or malformed credentials before fetching', async (owner, intake) => {
    const fetcher = returning({});
    await expect(queryIntakeDuplicates(request(), owner, intake, fetcher)).rejects.toThrow('separate intake token');
    expect(fetcher).not.toHaveBeenCalled();
  });

  it.each(['partial', 'incomplete', 'complete'] as const)('accepts an empty %s page without inferring exhaustive coverage', async kind => {
    const resultPage = await page();
    resultPage.candidates = [];
    resultPage.coverage = kind === 'partial' ? { kind, continuation: 'page-2' }
      : kind === 'incomplete' ? { kind, reason: 'Older provenance may be absent.' } : { kind };
    expect(await queryIntakeDuplicates(request(), ownerToken, intakeToken, returning(envelope(resultPage)))).toEqual(resultPage);
  });

  it('rejects pages bound to a different cursor, project, raw or canonical digest', async () => {
    const original = buildDuplicateQuery(preview());
    const changes: Partial<DuplicateQuery>[] = [
      { cursor: 'page-2' }, { project_id: 'other-project' },
      { raw_digest: digest('c') }, { canonical_digest: digest('d') },
    ];
    for (const change of changes) {
      const resultPage = await page({ ...original, ...change });
      await expect(queryIntakeDuplicates(request(original), ownerToken, intakeToken, returning(envelope(resultPage)))).rejects.toThrow('response could not be confirmed');
    }
  });

  it('rejects authority flags, unknown versions and additional envelope fields', async () => {
    const valid = envelope(await page());
    for (const value of [
      { ...valid, execution_granted: true }, { ...valid, acceptance_granted: true },
      { ...valid, schema_version: 2 }, { ...valid, grant: true },
      { data: valid.data, schema_version: 1, execution_granted: false },
    ]) await expect(queryIntakeDuplicates(request(), ownerToken, intakeToken, returning(value))).rejects.toThrow('response could not be confirmed');
  });

  it('rejects malformed candidates and domain bounds even when the generated shape permits them', async () => {
    const valid = await page();
    const changes: Partial<DuplicateCandidate>[] = [
      { task_id: 'task\n' }, { task_id: '..' }, { contract_revision: 0 },
      { contract_revision: 0x1_0000_0000 }, { contract_revision: Number.MAX_SAFE_INTEGER + 1 },
      { contract_revision: 1.5 }, { contract_digest: `${digest('e')}\n` },
      { lifecycle: '' }, { freshness: ' ' }, { freshness: '\u0085' }, { lifecycle: '雪'.repeat(43) },
      { freshness: '\ud800' }, { evidence_refs: Array(65).fill('lineage:one') },
      { evidence_refs: ['https:untrusted'] }, { evidence_refs: ['lineage/one'] },
      { evidence_refs: ['雪'.repeat(342)] },
    ];
    for (const change of changes) {
      const value = { ...valid, candidates: [{ ...candidate(), ...change }] };
      await expect(queryIntakeDuplicates(request(), ownerToken, intakeToken, returning(envelope(value)))).rejects.toThrow('response could not be confirmed');
    }
    const unknownReason = { ...valid, candidates: [{ ...candidate(), reason: 'invented_match' }] };
    await expect(queryIntakeDuplicates(request(), ownerToken, intakeToken, returning({ ...envelope(valid), data: unknownReason }))).rejects.toThrow('response could not be confirmed');
  });

  it('enforces query limit and core maximum page count', async () => {
    const query = buildDuplicateQuery(preview(), null, 1);
    const overLimit = await page(query);
    overLimit.candidates.push(candidate());
    await expect(queryIntakeDuplicates(request(query), ownerToken, intakeToken, returning(envelope(overLimit)))).rejects.toThrow('response could not be confirmed');
    const maxQuery = buildDuplicateQuery(preview(), null, 100);
    const overMaximum = await page(maxQuery);
    overMaximum.candidates = Array.from({ length: 101 }, candidate);
    await expect(queryIntakeDuplicates(request(maxQuery), ownerToken, intakeToken, returning(envelope(overMaximum)))).rejects.toThrow('response could not be confirmed');
  });

  it('rejects invalid partial cursors and incomplete explanations', async () => {
    const valid = await page();
    for (const coverage of [
      { kind: 'partial', continuation: 'page with space' }, { kind: 'partial', continuation: 'file:local' },
      { kind: 'incomplete', reason: '' }, { kind: 'incomplete', reason: '雪'.repeat(1366) },
      { kind: 'incomplete', reason: '\0' }, { kind: 'unknown' },
    ]) await expect(queryIntakeDuplicates(request(), ownerToken, intakeToken, returning({ ...envelope(valid), data: { ...valid, coverage } }))).rejects.toThrow('response could not be confirmed');
  });

  it('keeps server errors and transport details out of user-facing messages', async () => {
    const secret = `${ownerToken} ${intakeToken} <script>server detail</script>`;
    for (const status of [400, 401, 403, 413, 422, 429, 500, 503]) {
      const response = new Response(secret, { status });
      const error = await queryIntakeDuplicates(request(), ownerToken, intakeToken, vi.fn(async () => response)).catch((error: unknown) => error);
      if (!(error instanceof Error)) throw new Error('Expected duplicate lookup rejection');
      expect(error).toMatchObject({ status });
      expect(error.message).not.toContain(secret);
      expect(error.message).not.toContain(ownerToken);
      expect(error.message).not.toContain(intakeToken);
      expect(response.bodyUsed).toBe(false);
    }
    await expect(queryIntakeDuplicates(request(), ownerToken, intakeToken, vi.fn(async () => { throw new Error(secret); }))).rejects.toThrow('response could not be confirmed');
  });

  it('rejects invalid JSON and responses exceeding the shared bounded reader', async () => {
    for (const response of [
      new Response('not JSON'),
      new Response('{}', { headers: { 'Content-Length': String(8 * 1024 * 1024 + 1) } }),
      new Response(new Uint8Array([0xff])),
    ]) await expect(queryIntakeDuplicates(request(), ownerToken, intakeToken, vi.fn(async () => response))).rejects.toThrow('response could not be confirmed');
  });

  it('rejects pre-aborted requests and replies from fetchers that ignore cancellation', async () => {
    const controller = new AbortController();
    const fetcher = returning({});
    controller.abort();
    await expect(queryIntakeDuplicates(request(), ownerToken, intakeToken, fetcher, controller.signal)).rejects.toMatchObject({ name: 'AbortError' });
    expect(fetcher).not.toHaveBeenCalled();
    const active = new AbortController();
    const valid = envelope(await page());
    const ignoresAbort: typeof fetch = vi.fn(async () => {
      active.abort();
      return Response.json(valid);
    });
    await expect(queryIntakeDuplicates(request(), ownerToken, intakeToken, ignoresAbort, active.signal)).rejects.toMatchObject({ name: 'AbortError' });
  });
});
