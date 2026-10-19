import { useRef, useState, type CSSProperties, type FormEvent } from 'react';
import {
  validateIntakePreview,
  type IntakePreview,
  type SourceProvenance,
  type SourcePayload,
  type ScopeSnapshot,
} from '../../../../packages/contracts/index';
import { readBoundedJson } from '../api';
import { assertBoundedJson } from '../validation';

const MAX_SOURCE_BYTES = 1024 * 1024;
const DIGEST = /^sha256:[0-9a-f]{64}$/;
const surface: CSSProperties = { background: '#050505', color: '#ddd', border: '1px solid #292929', padding: '1rem', fontFamily: 'inherit' };
const field: CSSProperties = { display: 'grid', gap: '.25rem', minWidth: 0 };
const control: CSSProperties = { background: '#0b0b0b', color: '#eee', border: '1px solid #444', padding: '.45rem', font: 'inherit', width: '100%', boxSizing: 'border-box' };

type Draft = { projectId: string; rootId: string; sourceId: string; revision: string; capturedAt: string; text: string; captureId: string };
type IntakeResult<T> = { schema_version: 1; data: T; execution_granted: false; acceptance_granted: false };
type CaptureReceipt = {
  schema_version: 1; command_id: string; capture_id: string; revision: number;
  request_digest: string; raw_digest: string; canonical_digest: string;
  preview_manifest_digest: string; parent_receipt_digest: string | null;
};
type CaptureResult = { receipt: CaptureReceipt; receipt_digest: string; replayed: boolean };
type InlineSourcePayload = Extract<SourcePayload, { kind: 'inline_utf8' }>;
type CaptureRequest = {
  schema_version: 1; command_id: string; capture_id: string; revision: 1; expected_revision: 0;
  parent_receipt_digest: null; scope: ScopeSnapshot; format: 'plain_text'; provenance: SourceProvenance;
  source_payload: InlineSourcePayload; preview_manifest_digest: string; policy_revision: string; policy_digest: string;
};
type StoredCapture = { request: CaptureRequest; preview: IntakePreview; receipt: CaptureReceipt; receipt_digest: string };
type PendingCapture = { request: CaptureRequest; body: string; rawDigest: string; canonicalDigest: string };
type ReadTarget = { request: CaptureRequest; result: CaptureResult };
type RequestSummary = { commandId: string; captureId: string; sourceId: string; sourceRevision: string; manifestDigest: string; bodyBytes: number; outcome: string };
export type IntakeDerivedState = { preview: IntakePreview | null; previewBody: string | null; capture: CaptureResult | null; stored: StoredCapture | null };
export function invalidateIntakeDraft(_state: IntakeDerivedState): IntakeDerivedState {
  return { preview: null, previewBody: null, capture: null, stored: null };
}

function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}
function exactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  return Object.keys(value).length === keys.length && keys.every(key => Object.hasOwn(value, key));
}
function stableJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(',')}]`;
  if (object(value)) return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${stableJson(value[key])}`).join(',')}}`;
  return JSON.stringify(value);
}
function digest(value: unknown): value is string { return typeof value === 'string' && DIGEST.test(value); }
function isScope(value: unknown): value is ScopeSnapshot {
  if (!object(value) || !exactKeys(value, ['principal', 'authority_ref', 'authority_generation', ...(Object.hasOwn(value, 'project_id') ? ['project_id'] : []), ...(Object.hasOwn(value, 'root_id') ? ['root_id'] : [])])) return false;
  return typeof value.principal === 'string' && typeof value.authority_ref === 'string'
    && typeof value.authority_generation === 'string' && /^[1-9][0-9]{0,19}$/.test(value.authority_generation)
    && (!Object.hasOwn(value, 'project_id') || value.project_id === null || typeof value.project_id === 'string')
    && (!Object.hasOwn(value, 'root_id') || value.root_id === null || typeof value.root_id === 'string');
}
function isProvenance(value: unknown): value is SourceProvenance {
  if (!object(value) || !exactKeys(value, ['schema_version', 'source_id', 'revision', 'captured_at', 'requester', 'origin'])) return false;
  return value.schema_version === 1 && typeof value.source_id === 'string' && typeof value.revision === 'string'
    && typeof value.captured_at === 'string' && object(value.requester) && exactKeys(value.requester, ['kind'])
    && value.requester.kind === 'unknown' && value.origin === null;
}
function isSourcePayload(value: unknown): value is InlineSourcePayload {
  return object(value) && exactKeys(value, ['kind', 'text']) && value.kind === 'inline_utf8' && typeof value.text === 'string';
}
function isCaptureRequest(value: unknown): value is CaptureRequest {
  if (!object(value) || !exactKeys(value, ['schema_version', 'command_id', 'capture_id', 'revision', 'expected_revision', 'parent_receipt_digest', 'scope', 'format', 'provenance', 'source_payload', 'preview_manifest_digest', 'policy_revision', 'policy_digest'])) return false;
  return value.schema_version === 1 && typeof value.command_id === 'string' && typeof value.capture_id === 'string'
    && value.revision === 1 && value.expected_revision === 0 && value.parent_receipt_digest === null
    && isScope(value.scope) && value.format === 'plain_text' && isProvenance(value.provenance)
    && isSourcePayload(value.source_payload) && digest(value.preview_manifest_digest)
    && typeof value.policy_revision === 'string' && digest(value.policy_digest);
}
function isReceipt(value: unknown): value is CaptureReceipt {
  if (!object(value) || !exactKeys(value, ['schema_version', 'command_id', 'capture_id', 'revision', 'request_digest', 'raw_digest', 'canonical_digest', 'preview_manifest_digest', 'parent_receipt_digest'])) return false;
  return value.schema_version === 1 && typeof value.command_id === 'string' && typeof value.capture_id === 'string'
    && value.revision === 1 && digest(value.request_digest) && digest(value.raw_digest)
    && digest(value.canonical_digest) && digest(value.preview_manifest_digest) && value.parent_receipt_digest === null;
}
export function validateIntakeEnvelope<T>(value: unknown, isData: (data: unknown) => data is T): IntakeResult<T> {
  assertBoundedJson(value);
  if (!object(value) || !exactKeys(value, ['schema_version', 'data', 'execution_granted', 'acceptance_granted'])
    || value.schema_version !== 1 || value.execution_granted !== false || value.acceptance_granted !== false
    || !isData(value.data)) throw new Error('Unsupported intake response');
  return value as IntakeResult<T>;
}
function isCaptureResult(value: unknown): value is CaptureResult {
  if (!object(value) || !exactKeys(value, ['receipt', 'receipt_digest', 'replayed']) || !isReceipt(value.receipt)
    || !digest(value.receipt_digest) || typeof value.replayed !== 'boolean') return false;
  return true;
}
function isStoredCapture(value: unknown): value is StoredCapture {
  if (!object(value) || !exactKeys(value, ['request', 'preview', 'receipt', 'receipt_digest'])
    || !isCaptureRequest(value.request) || !validateIntakePreview(value.preview) || !isReceipt(value.receipt) || !digest(value.receipt_digest)) return false;
  const request = value.request, savedPreview = value.preview, receipt = value.receipt;
  return savedPreview.format === request.format && savedPreview.manifest_digest === request.preview_manifest_digest
    && savedPreview.policy_revision === request.policy_revision && savedPreview.policy_digest === request.policy_digest
    && savedPreview.provenance.source_id === request.provenance.source_id && savedPreview.provenance.revision === request.provenance.revision
    && savedPreview.raw_byte_length === new TextEncoder().encode(request.source_payload.text).byteLength
    && JSON.stringify(savedPreview.authorized_scope) === JSON.stringify(request.scope)
    && receipt.command_id === request.command_id && receipt.capture_id === request.capture_id && receipt.revision === request.revision
    && receipt.preview_manifest_digest === request.preview_manifest_digest && receipt.raw_digest === savedPreview.raw_digest
    && receipt.canonical_digest === savedPreview.canonical_digest && receipt.parent_receipt_digest === request.parent_receipt_digest;
}
function uuid(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') return crypto.randomUUID();
  throw new Error('Secure browser UUID support is required for capture.');
}
function generatedCaptureId(): string { try { return `intake-${uuid()}`; } catch { return ''; } }
function provenance(draft: Pick<Draft, 'sourceId' | 'revision' | 'capturedAt'>): SourceProvenance {
  const at = new Date(draft.capturedAt);
  if (!Number.isFinite(at.getTime())) throw new Error('Enter the observed draft time.');
  return { schema_version: 1, source_id: draft.sourceId.trim(), revision: draft.revision.trim(), captured_at: at.toISOString(), requester: { kind: 'unknown' }, origin: null };
}
function sourcePayload(text: string): InlineSourcePayload { return { kind: 'inline_utf8', text }; }
export function buildPreviewEnvelope(draft: Pick<Draft, 'projectId' | 'rootId' | 'sourceId' | 'revision' | 'capturedAt' | 'text'>) {
  const bytes = new TextEncoder().encode(draft.text);
  if (bytes.byteLength > MAX_SOURCE_BYTES) throw new Error('Original source exceeds one MiB in UTF-8.');
  if (!draft.projectId.trim() || !draft.rootId.trim() || !draft.sourceId.trim() || !draft.revision.trim()) throw new Error('Project, root, source ID, and source revision are required.');
  return { project_id: draft.projectId.trim(), root_id: draft.rootId.trim(), format: 'plain_text' as const,
    provenance: provenance(draft), source_payload: sourcePayload(draft.text) };
}
function requestHeaders(ownerToken: string, intakeToken: string): HeadersInit {
  return { Authorization: `Bearer ${ownerToken}`, 'X-Admiralty-Intake-Authorization': `Bearer ${intakeToken}`,
    'X-Admiralty-Requested-With': 'intake', 'Content-Type': 'application/json' };
}
async function post<T>(fetcher: typeof fetch, path: string, body: string, ownerToken: string, intakeToken: string, isData: (value: unknown) => value is T): Promise<T> {
  const response = await fetcher(path, { method: 'POST', headers: requestHeaders(ownerToken, intakeToken), body, cache: 'no-store' });
  if (!response.ok) throw Object.assign(new Error(statusMessage(response.status)), { status: response.status });
  return validateIntakeEnvelope(await readBoundedJson(response), isData).data;
}
export function previewRawIntake(envelope: ReturnType<typeof buildPreviewEnvelope>, ownerToken: string, intakeToken: string, fetcher: typeof fetch = fetch): Promise<IntakePreview> {
  const body = JSON.stringify(envelope);
  if (new TextEncoder().encode(body).byteLength > MAX_SOURCE_BYTES) throw new Error('Request envelope exceeds one MiB UTF-8 body limit.');
  return post(fetcher, '/v2/intake/preview', body, ownerToken, intakeToken,
    (value): value is IntakePreview => validateIntakePreview(value) && value.format === 'plain_text'
      && value.provenance.source_id === envelope.provenance.source_id && value.provenance.revision === envelope.provenance.revision
      && value.provenance.captured_at === envelope.provenance.captured_at
      && value.raw_byte_length === new TextEncoder().encode(envelope.source_payload.text).byteLength
      && value.authorized_scope.project_id === envelope.project_id && value.authorized_scope.root_id === envelope.root_id);
}
export function captureRawIntake(request: CaptureRequest, serializedBody: string, ownerToken: string, intakeToken: string, fetcher: typeof fetch = fetch): Promise<CaptureResult> {
  if (!isCaptureRequest(request) || JSON.stringify(request) !== serializedBody) throw new Error('Capture retry envelope is not the frozen request.');
  if (new TextEncoder().encode(serializedBody).byteLength > MAX_SOURCE_BYTES) throw new Error('Capture request exceeds one MiB UTF-8 body limit.');
  return post(fetcher, '/v2/intake/captures', serializedBody, ownerToken, intakeToken, isCaptureResult).then(result => {
    if (result.receipt.command_id !== request.command_id || result.receipt.capture_id !== request.capture_id
      || result.receipt.revision !== request.revision || result.receipt.preview_manifest_digest !== request.preview_manifest_digest) throw new Error('Capture receipt does not bind the submitted command.');
    return result;
  });
}
export async function readRawCapture(request: CaptureRequest, expected: CaptureResult,
  ownerToken: string, intakeToken: string, fetcher: typeof fetch = fetch): Promise<StoredCapture> {
  const { project_id: projectId, root_id: rootId } = request.scope;
  if (!projectId || !rootId) throw new Error('Saved capture scope is incomplete.');
  const body = JSON.stringify({ project_id: projectId, root_id: rootId, revision: request.revision });
  const record = await post(fetcher, `/v2/intake/captures/${encodeURIComponent(request.capture_id)}`, body, ownerToken, intakeToken, isStoredCapture);
  if (stableJson(record.request) !== stableJson(request) || stableJson(record.receipt) !== stableJson(expected.receipt)
    || record.receipt_digest !== expected.receipt_digest) throw new Error('Readback does not match the saved capture request and receipt.');
  return record;
}
function statusMessage(status: number): string {
  if (status === 400 || status === 422) return 'The server rejected this intake request. Review the source and capture fields before starting a new command.';
  if (status === 503) return 'Intake is unavailable.';
  if (status === 401) return 'Intake credentials were rejected. Check both access tokens.';
  if (status === 403) return 'The current intake scope denied this request.';
  if (status === 409) return 'The capture conflicted with current stored state. The exact request is retained for a safe retry.';
  if (status === 413) return 'The request exceeds the server body limit.';
  if (status === 404) return 'The requested intake record was not found.';
  if (status === 415) return 'The intake endpoint requires JSON.';
  if (status === 429) return 'Intake request capacity is currently full.';
  return 'The intake endpoint returned an unsupported response.';
}
function safeError(error: unknown): string {
  if (object(error) && typeof error.status === 'number') return statusMessage(error.status);
  const known = new Set([
    'Secure browser UUID support is required for capture.', 'Enter a unique capture ID; secure browser UUID support is unavailable.',
    'Capture request exceeds one MiB UTF-8 body limit.', 'Request envelope exceeds one MiB UTF-8 body limit.',
    'Original source exceeds one MiB in UTF-8.', 'Project, root, source ID, and source revision are required.',
    'Enter the observed draft time.', 'Capture retry envelope is not the frozen request.',
    'Capture receipt does not bind the submitted command.', 'Capture receipt digests do not match the preview.',
    'Readback does not match the saved capture request and receipt.', 'Saved capture scope is incomplete.',
  ]);
  if (error instanceof Error && known.has(error.message)) return error.message;
  return 'The response could not be confirmed or validated. No server details were shown.';
}

export type IntakeProps = { token?: string; onAccess?: () => void; fetcher?: typeof fetch };
export default function Intake({ token = '', onAccess, fetcher = fetch }: IntakeProps) {
  const [draft, setDraft] = useState<Draft>(() => ({ projectId: '', rootId: '', sourceId: '', revision: '', capturedAt: '', text: '', captureId: generatedCaptureId() }));
  const [intakeToken, setIntakeToken] = useState('');
  const [preview, setPreview] = useState<IntakePreview | null>(null);
  const [previewBody, setPreviewBody] = useState<string | null>(null);
  const [pending, setPending] = useState<PendingCapture | null>(null);
  const [capture, setCapture] = useState<CaptureResult | null>(null);
  const [readTarget, setReadTarget] = useState<ReadTarget | null>(null);
  const [receiptHistory, setReceiptHistory] = useState<CaptureResult[]>([]);
  const [requestHistory, setRequestHistory] = useState<RequestSummary[]>([]);
  const [stored, setStored] = useState<StoredCapture | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');
  const busyRef = useRef(false);
  const mutate = (key: keyof Draft, value: string) => {
    setDraft(current => ({ ...current, [key]: value,
      ...(key !== 'captureId' && capture?.receipt.capture_id === draft.captureId ? { captureId: generatedCaptureId() } : {}) }));
    const invalidated = invalidateIntakeDraft({ preview, previewBody, capture, stored });
    setPreview(invalidated.preview); setPreviewBody(invalidated.previewBody); setCapture(invalidated.capture); setStored(invalidated.stored);
    if (!pending) setMessage('Draft changed; preview again before capture.');
  };
  const authorized = Boolean(token.trim() && intakeToken.trim());
  async function run(action: () => Promise<void>) {
    if (!authorized || busyRef.current) { if (!authorized) setMessage('Enter the owner access token and separate intake token.'); return; }
    busyRef.current = true; setBusy(true); setMessage('');
    try { await action(); }
    catch (error) { setMessage(safeError(error)); }
    finally { busyRef.current = false; setBusy(false); }
  }
  async function previewDraft(event?: FormEvent) {
    event?.preventDefault();
    await run(async () => {
      const envelope = buildPreviewEnvelope(draft); const body = JSON.stringify(envelope);
      const result = await previewRawIntake(envelope, token, intakeToken, fetcher);
      setPreview(result); setPreviewBody(body); setCapture(null); setStored(null); setMessage('Preview is ready. One plain-text source remains one unresolved request until explicit enrichment.');
    });
  }
  function makeCapture(): PendingCapture {
    if (!preview || !previewBody) throw new Error('Preview this exact draft before capture.');
    const source = buildPreviewEnvelope(draft);
    const captureId = draft.captureId.trim();
    if (!captureId) throw new Error('Enter a unique capture ID; secure browser UUID support is unavailable.');
    const unsigned = {
      schema_version: 1 as const, command_id: '00000000-0000-4000-8000-000000000000', capture_id: captureId, revision: 1 as const, expected_revision: 0 as const,
      parent_receipt_digest: null, scope: preview.authorized_scope, format: 'plain_text' as const, provenance: source.provenance,
      source_payload: source.source_payload, preview_manifest_digest: preview.manifest_digest,
      policy_revision: preview.policy_revision, policy_digest: preview.policy_digest,
    };
    if (new TextEncoder().encode(JSON.stringify(unsigned)).byteLength > MAX_SOURCE_BYTES) throw new Error('Capture request exceeds one MiB UTF-8 body limit.');
    const request: CaptureRequest = { ...unsigned, command_id: uuid() };
    const body = JSON.stringify(request);
    if (new TextEncoder().encode(body).byteLength > MAX_SOURCE_BYTES) throw new Error('Capture request exceeds one MiB UTF-8 body limit.');
    return { request, body, rawDigest: preview.raw_digest, canonicalDigest: preview.canonical_digest };
  }
  function rememberRequest(frozen: PendingCapture, outcome: string, result?: CaptureResult) {
    const summary: RequestSummary = { commandId: frozen.request.command_id, captureId: frozen.request.capture_id,
      sourceId: frozen.request.provenance.source_id, sourceRevision: frozen.request.provenance.revision,
      manifestDigest: frozen.request.preview_manifest_digest, bodyBytes: new TextEncoder().encode(frozen.body).byteLength,
      outcome: result ? `receipt confirmed · ${result.receipt_digest}` : outcome };
    setRequestHistory(current => [summary, ...current.filter(item => item.commandId !== summary.commandId)].slice(0, 5));
  }
  async function readConfirmedOriginal(target: ReadTarget) {
    const record = await readRawCapture(target.request, target.result, token, intakeToken, fetcher);
    setStored(record);
  }
  async function retryReadback() {
    if (!readTarget) return;
    const frozen = readTarget;
    await run(async () => {
      await readConfirmedOriginal(frozen);
      setMessage('Original bytes and receipt were read again at the recorded revision.');
    });
  }
  async function captureDraft() {
    if (pending) { setMessage('Retry or resolve the retained capture before starting a new one.'); return; }
    if (!authorized) { setMessage('Enter the owner access token and separate intake token.'); return; }
    await run(async () => {
      const frozen = makeCapture();
      setPending(frozen);
      rememberRequest(frozen, 'response unresolved; exact request retained for retry');
      let result: CaptureResult;
      try { result = await captureRawIntake(frozen.request, frozen.body, token, intakeToken, fetcher); }
      catch (error) {
        const status = object(error) && typeof error.status === 'number' ? error.status : null;
        rememberRequest(frozen, status === null ? 'response unresolved; server may have captured it'
          : status === 400 || status === 413 || status === 422 ? `server refusal (${status}); exact request retained`
          : status === 409 ? 'conflict response; existing capture remains possible; exact request retained'
          : `HTTP ${status}; no receipt confirmed; exact request retained`);
        throw error;
      }
      if (result.receipt.raw_digest !== frozen.rawDigest || result.receipt.canonical_digest !== frozen.canonicalDigest) throw new Error('Capture receipt digests do not match the preview.');
      rememberRequest(frozen, 'receipt confirmed', result);
      const target = { request: frozen.request, result };
      setCapture(result); setReceiptHistory(current => [result, ...current.filter(item => item.receipt_digest !== result.receipt_digest)].slice(0, 5));
      setReadTarget(target); setPending(null); setDraft(current => ({ ...current, captureId: generatedCaptureId() }));
      setMessage(result.replayed ? 'Original immutable capture receipt replayed.' : 'Original request captured with unresolved facts; no execution or acceptance was granted.');
      await readConfirmedOriginal(target);
    });
  }
  async function retryCapture() {
    if (!pending) return;
    const frozen = pending;
    await run(async () => {
      rememberRequest(frozen, 'retry in progress; exact command and body retained');
      let result: CaptureResult;
      try { result = await captureRawIntake(frozen.request, frozen.body, token, intakeToken, fetcher); }
      catch (error) {
        const status = object(error) && typeof error.status === 'number' ? error.status : null;
        rememberRequest(frozen, status === null ? 'response unresolved; server may have captured it'
          : status === 400 || status === 413 || status === 422 ? `server refusal (${status}); exact request retained`
          : status === 409 ? 'conflict response; existing capture remains possible; exact request retained'
          : `HTTP ${status}; no receipt confirmed; exact request retained`);
        throw error;
      }
      if (result.receipt.raw_digest !== frozen.rawDigest || result.receipt.canonical_digest !== frozen.canonicalDigest) throw new Error('Capture receipt digests do not match the preview.');
      rememberRequest(frozen, 'receipt confirmed', result);
      const target = { request: frozen.request, result };
      setCapture(result); setReceiptHistory(current => [result, ...current.filter(item => item.receipt_digest !== result.receipt_digest)].slice(0, 5));
      setReadTarget(target); setPending(null); setDraft(current => ({ ...current, captureId: generatedCaptureId() }));
      setMessage(result.replayed ? 'Original immutable capture receipt replayed.' : 'Capture receipt recorded.');
      await readConfirmedOriginal(target);
    });
  }
  function abandonPendingCapture() {
    if (!pending || busyRef.current) return;
    rememberRequest(pending, 'locally abandoned; server may already have captured it');
    setPending(null); setPreview(null); setPreviewBody(null); setCapture(null); setStored(null); setReadTarget(null);
    setDraft(current => ({ ...current, sourceId: '', revision: '', capturedAt: '', text: '', captureId: generatedCaptureId() }));
    setMessage('Local retry envelope discarded. This does not undo or verify any server-side capture; keep the retained request summary for reference. Enter a new source identity and capture ID before proceeding.');
  }
  const unresolvedCount = preview?.requests.reduce((n, request) => n + request.unresolved.length, 0) ?? 0;
  return <section style={surface} aria-labelledby="intake-title">
    <h2 id="intake-title" style={{ marginTop: 0 }}>Raw request intake</h2>
    <p>Plain text is preserved as one request across paragraphs. Preview and capture do not create tasks, plans, outbox work, or acceptance.</p>
    {!token && <p role="status">Owner access must be supplied by the application.</p>}
    <form onSubmit={previewDraft}>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(180px, 1fr))', gap: '.65rem' }}>
        <label style={field}>Project ID<input aria-label="Project ID" value={draft.projectId} disabled={busy || Boolean(pending)} style={control} onChange={e => mutate('projectId', e.target.value)} /></label>
        <label style={field}>Root ID<input aria-label="Root ID" value={draft.rootId} disabled={busy || Boolean(pending)} style={control} onChange={e => mutate('rootId', e.target.value)} /></label>
        <label style={field}>Source ID<input aria-label="Source ID" value={draft.sourceId} disabled={busy || Boolean(pending)} style={control} onChange={e => mutate('sourceId', e.target.value)} /></label>
        <label style={field}>Source revision<input aria-label="Source revision" value={draft.revision} disabled={busy || Boolean(pending)} style={control} onChange={e => mutate('revision', e.target.value)} /></label>
        <label style={field}>Observed draft time<input aria-label="Observed draft time" type="datetime-local" value={draft.capturedAt} disabled={busy || Boolean(pending)} style={control} onChange={e => mutate('capturedAt', e.target.value)} /></label>
        <label style={field}>Capture ID<input aria-label="Capture ID" value={draft.captureId} disabled={busy || Boolean(pending)} style={control} onChange={e => mutate('captureId', e.target.value)} /></label>
      </div>
      <label style={{ ...field, marginTop: '.65rem' }}>Original raw request<textarea aria-label="Original raw request" value={draft.text} disabled={busy || Boolean(pending)} rows={10} style={{ ...control, resize: 'vertical' }} onChange={e => mutate('text', e.target.value)} /></label>
      <p>{new TextEncoder().encode(draft.text).byteLength.toLocaleString()} / 1,048,576 UTF-8 bytes</p>
      <label style={{ ...field, maxWidth: '32rem' }}>Separate intake token<input aria-label="Separate intake token" type="password" autoComplete="new-password" disabled={busy} style={control} onChange={e => setIntakeToken(e.target.value)} /></label>
      <p>Requester attribution is unknown. A profile name does not authenticate or authorize intake.</p>
      {onAccess && <button type="button" disabled={busy} onClick={onAccess}>Owner access</button>}{' '}
      <button type="submit" disabled={busy || Boolean(pending) || !authorized}>Preview exact source</button>{' '}
      <button type="button" disabled={busy || !authorized || !preview || Boolean(pending) || capture?.receipt.preview_manifest_digest === preview?.manifest_digest} onClick={() => void captureDraft()}>Capture unresolved request</button>
    </form>
    {pending && <aside style={{ marginTop: '1rem', borderTop: '1px solid #444', paddingTop: '.7rem' }}>
      <p>No receipt is confirmed for this command. Retry will resend the identical command ID and serialized request.</p>
      <button type="button" disabled={busy || !authorized} onClick={() => void retryCapture()}>Retry same capture request</button>
      {' '}<button type="button" disabled={busy} onClick={abandonPendingCapture}>Abandon local retry envelope and reset draft</button>
    </aside>}
    {message && <p role="status" aria-live="polite">{message}</p>}
    {preview && <article aria-label="Intake preview" style={{ marginTop: '1rem', borderTop: '1px solid #444', paddingTop: '.7rem' }}>
      <h3>Preview · {preview.requests.length} request · {unresolvedCount} unresolved facts</h3>
      <p>Scope: principal <strong>{preview.authorized_scope.principal}</strong>; authority {preview.authorized_scope.authority_ref} generation {preview.authorized_scope.authority_generation}; project {preview.authorized_scope.project_id ?? 'unknown'}; root {preview.authorized_scope.root_id ?? 'unknown'}.</p>
      <p>Format: {preview.format}. Raw bytes: {preview.raw_byte_length}. Raw digest: <code>{preview.raw_digest}</code>. Canonical digest: <code>{preview.canonical_digest}</code>. Manifest digest: <code>{preview.manifest_digest}</code>.</p>
      {preview.requests.map(request => <div key={request.index} style={{ borderLeft: '2px solid #555', paddingLeft: '.7rem', margin: '.6rem 0' }}>
        <h4>Request {request.index + 1}</h4><pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{request.text}</pre>
        <p>Unresolved: {request.unresolved.length ? request.unresolved.join(', ') : 'none reported'}; outcomes: {request.outcomes.length}.</p>
      </div>)}
      <details><summary>Original raw source</summary><pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{draft.text}</pre></details>
    </article>}
    {capture && <article aria-label="Capture receipt" style={{ marginTop: '1rem', borderTop: '1px solid #444', paddingTop: '.7rem' }}>
      <h3>Immutable capture receipt</h3><p>Receipt recorded; execution and acceptance remain false. Replayed: {String(capture.replayed)}.</p>
      <dl><dt>Capture ID / revision</dt><dd>{capture.receipt.capture_id} / {capture.receipt.revision}</dd><dt>Command ID</dt><dd>{capture.receipt.command_id}</dd><dt>Receipt digest</dt><dd><code>{capture.receipt_digest}</code></dd><dt>Request digest</dt><dd><code>{capture.receipt.request_digest}</code></dd></dl>
    </article>}
    {readTarget && capture && !stored && <button type="button" disabled={busy || !authorized} onClick={() => void retryReadback()}>Read captured original again</button>}
    {receiptHistory.length > 0 && <details><summary>Recent immutable receipt summaries ({receiptHistory.length}, kept in page memory)</summary>
      <ul>{receiptHistory.map(item => <li key={item.receipt_digest}>{item.receipt.capture_id} revision {item.receipt.revision} · command {item.receipt.command_id} · <code>{item.receipt_digest}</code></li>)}</ul>
    </details>}
    {requestHistory.length > 0 && <details><summary>Recent capture request summaries ({requestHistory.length}, kept in page memory)</summary>
      <ul>{requestHistory.map(item => <li key={item.commandId}>{item.sourceId}@{item.sourceRevision} · capture {item.captureId} · command {item.commandId} · {item.bodyBytes} UTF-8 bytes · {item.outcome} · manifest <code>{item.manifestDigest}</code></li>)}</ul>
    </details>}
    {stored && <article aria-label="Recorded original source"><h3>Original bytes as recorded</h3><p>Capture {stored.request.capture_id}, revision {stored.receipt.revision}, receipt <code>{stored.receipt_digest}</code>.</p><pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{stored.request.source_payload.text}</pre></article>}
    <p>Enriched-record, duplicate-query, and delivery views are not part of this intake panel.</p>
  </section>;
}
