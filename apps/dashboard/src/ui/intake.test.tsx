import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { validateIntakePreview, type IntakePreview } from '../../../../packages/contracts/index';
import Intake, {
  buildPreviewEnvelope, captureRawIntake, previewRawIntake, readRawCapture,
  validateIntakeEnvelope, invalidateIntakeDraft,
} from './Intake';

const digest = (digit: string) => `sha256:${digit.repeat(64)}`;
const source = { projectId: 'project-a', rootId: 'root-a', sourceId: 'mail-17', revision: 'original/v3', capturedAt: '2026-10-19T08:30', text: 'First paragraph.\n\nSecond paragraph β.' };
function preview(): IntakePreview {
  return {
    schema_version: 1, parser_revision: 'plain-text/v1', policy_revision: 'intake-policy/v1', policy_digest: digest('a'),
    format: 'plain_text', raw_digest: digest('b'), canonical_digest: digest('c'), raw_byte_length: new TextEncoder().encode('First paragraph.\n\nSecond paragraph β.').byteLength,
    provenance: { schema_version: 1, source_id: source.sourceId, revision: source.revision, captured_at: new Date(source.capturedAt).toISOString(), requester: { kind: 'unknown' }, origin: null },
    authorized_scope: { principal: 'operator-a', authority_ref: 'grant:intake', authority_generation: '7', project_id: source.projectId, root_id: source.rootId },
    requests: [{ index: 0, text: 'First paragraph.\n\nSecond paragraph β.', constraints: [], outcomes: [], unresolved: ['outcome_unknown', 'task_contract_missing', 'criteria_unknown'], raw_span: { start: 0, end: new TextEncoder().encode('First paragraph.\n\nSecond paragraph β.').byteLength } }],
    manifest_digest: digest('d'),
  };
}
function envelope(data: unknown, flags = { execution_granted: false, acceptance_granted: false }) {
  return { schema_version: 1, data, ...flags };
}
function response(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
}
function captureRequest() {
  const p = preview(), input = buildPreviewEnvelope(source);
  return {
    schema_version: 1 as const, command_id: '8b3f8e16-08ce-4abd-a6ad-fcaaed0f3912', capture_id: 'capture-a',
    revision: 1 as const, expected_revision: 0 as const, parent_receipt_digest: null,
    scope: p.authorized_scope, format: 'plain_text' as const, provenance: input.provenance,
    source_payload: input.source_payload, preview_manifest_digest: p.manifest_digest,
    policy_revision: p.policy_revision, policy_digest: p.policy_digest,
  };
}
function receipt(request = captureRequest()) {
  return { schema_version: 1 as const, command_id: request.command_id, capture_id: request.capture_id, revision: 1,
    request_digest: digest('1'), raw_digest: digest('2'), canonical_digest: digest('3'),
    preview_manifest_digest: request.preview_manifest_digest, parent_receipt_digest: null };
}
function captureResult(request = captureRequest()) { return { receipt: receipt(request), receipt_digest: digest('4'), replayed: false }; }

describe('raw request intake', () => {
  it('sends one exact plain-text preview envelope with UTF-8 paragraphs and both current credentials', async () => {
    const calls: Array<{ path: string; init: RequestInit }> = [];
    const fetcher: typeof fetch = async (input, init) => {
      calls.push({ path: String(input), init: init! }); return response(envelope(preview()));
    };
    const body = buildPreviewEnvelope(source);
    const result = await previewRawIntake(body, 'owner-token', 'separate-intake-token', fetcher);
    expect(result.requests).toHaveLength(1);
    expect(result.requests[0].text).toBe(source.text);
    expect(calls).toHaveLength(1);
    expect(calls[0].path).toBe('/v2/intake/preview');
    expect(calls[0].init).toMatchObject({ method: 'POST', cache: 'no-store' });
    expect(JSON.parse(String(calls[0].init.body))).toEqual({
      project_id: 'project-a', root_id: 'root-a', format: 'plain_text',
      provenance: { schema_version: 1, source_id: 'mail-17', revision: 'original/v3', captured_at: new Date(source.capturedAt).toISOString(), requester: { kind: 'unknown' }, origin: null },
      source_payload: { kind: 'inline_utf8', text: source.text },
    });
    const headers = new Headers(calls[0].init.headers);
    expect(headers.get('Authorization')).toBe('Bearer owner-token');
    expect(headers.get('X-Admiralty-Intake-Authorization')).toBe('Bearer separate-intake-token');
    expect(headers.get('X-Admiralty-Requested-With')).toBe('intake');
    expect(headers.get('Content-Type')).toBe('application/json');
    expect(headers.has('Origin')).toBe(false);
    expect(headers.has('Host')).toBe(false);
  });

  it('rejects UTF-8 overflow before fetch, including multi-byte characters', async () => {
    let calls = 0;
    const fetcher: typeof fetch = async () => { calls++; return response(envelope(preview())); };
    expect(() => previewRawIntake(buildPreviewEnvelope({ ...source, text: '😀'.repeat(262_145) }), 'owner', 'intake', fetcher)).toThrow('one MiB');
    expect(calls).toBe(0);
  });

  it('allows a preview body at one MiB but rejects the larger capture envelope before sending', async () => {
    const encoder = new TextEncoder();
    const emptyBody = buildPreviewEnvelope({ ...source, text: '' });
    const fixedPreviewBytes = encoder.encode(JSON.stringify(emptyBody)).byteLength;
    const text = 'x'.repeat(1_048_576 - fixedPreviewBytes);
    const input = buildPreviewEnvelope({ ...source, text });
    const previewBody = JSON.stringify(input);
    expect(encoder.encode(text).byteLength).toBeLessThanOrEqual(1_048_576);
    expect(encoder.encode(previewBody).byteLength).toBe(1_048_576);
    const p = { ...preview(), provenance: input.provenance, raw_byte_length: encoder.encode(text).byteLength,
      requests: [{ ...preview().requests[0], text, raw_span: { start: 0, end: encoder.encode(text).byteLength } }] };
    let previewCalls = 0;
    const previewFetcher: typeof fetch = async (_url, init) => {
      previewCalls++;
      expect(encoder.encode(String(init?.body)).byteLength).toBe(1_048_576);
      return response(envelope(p));
    };
    await previewRawIntake(input, 'owner', 'intake', previewFetcher);
    expect(previewCalls).toBe(1);
    const base = captureRequest();
    const request = { ...base, provenance: input.provenance, source_payload: input.source_payload,
      preview_manifest_digest: p.manifest_digest };
    const captureBody = JSON.stringify(request);
    expect(encoder.encode(captureBody).byteLength).toBeGreaterThan(1_048_576);
    let captureCalls = 0;
    const captureFetcher: typeof fetch = async () => { captureCalls++; return response(envelope(captureResult(request))); };
    expect(() => captureRawIntake(request, captureBody, 'owner', 'intake', captureFetcher)).toThrow('one MiB');
    expect(captureCalls).toBe(0);
  });

  it('invalidates preview and receipt readback when any source draft field changes', () => {
    const current = { preview: preview(), previewBody: '{"original":true}', capture: captureResult(),
      stored: { request: captureRequest(), preview: preview(), receipt: receipt(), receipt_digest: digest('4') } };
    expect(invalidateIntakeDraft(current)).toEqual({ preview: null, previewBody: null, capture: null, stored: null });
    expect(buildPreviewEnvelope({ ...source, text: `${source.text} changed` }).source_payload.text).not.toBe(source.text);
  });

  it('fails closed on true authority flags and invalid generated schemas', () => {
    expect(() => validateIntakeEnvelope(envelope(preview(), { execution_granted: true, acceptance_granted: false }), (value): value is IntakePreview => Boolean(value))).toThrow();
    expect(() => validateIntakeEnvelope(envelope({ ...preview(), injected: true }), (value): value is IntakePreview => validateIntakePreview(value))).toThrow();
  });

  it('maps unavailable, authentication, denied scope, and conflicts without exposing response bodies', async () => {
    for (const [status, expected] of [[503, 'unavailable'], [401, 'credentials'], [403, 'scope denied'], [409, 'conflicted']] as const) {
      const fetcher: typeof fetch = async () => response({ error: 'private raw server detail must not escape' }, status);
      let failure: unknown;
      try { await previewRawIntake(buildPreviewEnvelope(source), 'owner', 'intake', fetcher); }
      catch (error) { failure = error; }
      expect(failure).toBeInstanceOf(Error);
      expect((failure as Error).message).toContain(expected);
      expect((failure as Error).message).not.toContain('private raw server detail');
    }
  });

  it('retries the exact frozen capture body and command ID after a lost response, without task/provider calls', async () => {
    const request = captureRequest(), body = JSON.stringify(request), bodies: string[] = [], paths: string[] = [];
    let first = true;
    const fetcher: typeof fetch = async (input, init) => {
      paths.push(String(input)); bodies.push(String(init?.body));
      if (first) { first = false; throw new TypeError('connection dropped'); }
      return response(envelope(captureResult(request)));
    };
    await expect(captureRawIntake(request, body, 'owner', 'intake', fetcher)).rejects.toThrow();
    const result = await captureRawIntake(request, body, 'owner', 'intake', fetcher);
    expect(result.receipt.command_id).toBe(request.command_id);
    expect(bodies).toEqual([body, body]);
    expect(paths).toEqual(['/v2/intake/captures', '/v2/intake/captures']);
    expect(paths.some(path => path.includes('/v2/tasks') || path.includes('provider'))).toBe(false);
  });

  it('reads the exact recorded revision by POST and checks the response against its receipt', async () => {
    const request = captureRequest(), p = preview();
    const result = { ...captureResult(request), receipt: { ...receipt(request), raw_digest: p.raw_digest, canonical_digest: p.canonical_digest } };
    const stored = { request, preview: p, receipt: result.receipt, receipt_digest: result.receipt_digest };
    const calls: Array<{ path: string; init: RequestInit }> = [];
    const fetcher: typeof fetch = async (input, init) => { calls.push({ path: String(input), init: init! }); return response(envelope(stored)); };
    const original = await readRawCapture(request, result, 'owner', 'intake', fetcher);
    expect(original.request.source_payload.text).toBe(source.text);
    expect(calls).toHaveLength(1);
    expect(calls[0].path).toBe('/v2/intake/captures/capture-a');
    expect(calls[0].init.method).toBe('POST');
    expect(JSON.parse(String(calls[0].init.body))).toEqual({ project_id: 'project-a', root_id: 'root-a', revision: 1 });
    expect(calls[0].path.includes('?')).toBe(false);
    const sameLengthChangedSource = { ...stored, request: { ...request, source_payload: { kind: 'inline_utf8' as const, text: 'First paragraph.\n\nSecond paragraph α.' } } };
    expect(new TextEncoder().encode(sameLengthChangedSource.request.source_payload.text).byteLength).toBe(new TextEncoder().encode(request.source_payload.text).byteLength);
    const changedFetcher: typeof fetch = async () => response(envelope(sameLengthChangedSource));
    await expect(readRawCapture(request, result, 'owner', 'intake', changedFetcher)).rejects.toThrow('does not match');
  });

  it('shows required scope and provenance fields and keeps credentials out of initial markup', () => {
    const html = renderToStaticMarkup(<Intake token="owner-secret" />);
    for (const label of ['Project ID', 'Root ID', 'Source ID', 'Source revision', 'Observed draft time', 'Original raw request', 'Separate intake token']) expect(html).toContain(label);
    expect(html).not.toContain('owner-secret');
    expect(html).toContain('type="password"');
    expect(html).not.toMatch(/type="password"[^>]*\svalue=/);
    expect(html).toContain('Requester attribution is unknown');
    expect(html).toContain('Enriched-record, duplicate-query, and delivery views are not part');
  });
});
