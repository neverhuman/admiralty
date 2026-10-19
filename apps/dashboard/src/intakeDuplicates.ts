import {
  validateDuplicatePage,
  validateDuplicateQuery,
  validateIntakePreview,
  type DuplicatePage,
  type DuplicateQuery,
  type IntakePreview,
} from '../../../packages/contracts/index';
import { readBoundedJson } from './api';
import { assertBoundedJson } from './validation';

const encoder = new TextEncoder();
const DIGEST = /^sha256:[0-9a-f]{64}$/;
const MAX_QUERY_BYTES = 16 * 1024;
const MAX_PAGE_BYTES = 8 * 1024 * 1024;
const QUERY_ERROR = 'Related-work query is invalid or exceeds its bounds.';
const RESPONSE_ERROR = 'Related-work response could not be confirmed or validated.';
const CRYPTO_ERROR = 'Secure browser SHA-256 support is required for related-work search.';

export type IntakeDuplicateRequest = {
  project_id: string;
  root_id: string;
  query: DuplicateQuery;
};

type NormalizedQuery = DuplicateQuery & {
  origin: NonNullable<DuplicateQuery['origin']> | null;
  command_id: string | null;
  cursor: string | null;
};

function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}
function exactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  return Object.keys(value).length === keys.length && keys.every(key => Object.hasOwn(value, key));
}
function wellFormed(value: string): boolean {
  for (let index = 0; index < value.length; index++) {
    const unit = value.charCodeAt(index);
    if (unit >= 0xd800 && unit <= 0xdbff) {
      const next = value.charCodeAt(++index);
      if (!(next >= 0xdc00 && next <= 0xdfff)) return false;
    } else if (unit >= 0xdc00 && unit <= 0xdfff) return false;
  }
  return true;
}
function text(value: unknown, maxBytes: number): value is string {
  return typeof value === 'string' && value.length <= maxBytes
    && /[^\p{White_Space}]/u.test(value) && !value.includes('\0') && wellFormed(value)
    && encoder.encode(value).byteLength <= maxBytes;
}
function reference(value: unknown): value is string {
  return text(value, 1024) && !/[\x00-\x20\x7f/\\]/.test(value)
    && !value.startsWith('.') && !value.includes('..')
    && !/^(javascript|data|file|http|https|ssh):/i.test(value);
}
function digest(value: unknown): value is string {
  return typeof value === 'string' && value.length === 71 && DIGEST.test(value);
}
function identifier(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0 && value.length <= 128
    && value !== '.' && value !== '..' && !/[^A-Za-z0-9_.-]/.test(value);
}

/** Normalize to the typed Rust struct's serialization order, never sorted keys. */
function normalizedQuery(query: DuplicateQuery): NormalizedQuery {
  try {
    assertBoundedJson(query);
    if (!validateDuplicateQuery(query) || query.schema_version !== 1 || !identifier(query.project_id)
      || !digest(query.raw_digest) || !digest(query.canonical_digest)
      || !Number.isSafeInteger(query.limit) || query.limit < 1 || query.limit > 100
      || (query.cursor != null && !reference(query.cursor))
      || (query.command_id != null && !identifier(query.command_id))) throw new Error(QUERY_ERROR);
    const origin = query.origin ?? null;
    if (origin !== null && (!identifier(origin.system) || !reference(origin.record_id) || !reference(origin.revision))) throw new Error(QUERY_ERROR);
    const normalized: NormalizedQuery = {
      schema_version: 1,
      project_id: query.project_id,
      raw_digest: query.raw_digest,
      canonical_digest: query.canonical_digest,
      origin: origin === null ? null : { system: origin.system, record_id: origin.record_id, revision: origin.revision },
      command_id: query.command_id ?? null,
      cursor: query.cursor ?? null,
      limit: query.limit,
    };
    if (encoder.encode(JSON.stringify(normalized)).byteLength > MAX_QUERY_BYTES) throw new Error(QUERY_ERROR);
    return normalized;
  } catch {
    throw new Error(QUERY_ERROR);
  }
}

/** Current plain-text UI searches content only; source/command identity is not invented. */
export function buildDuplicateQuery(preview: IntakePreview, cursor: string | null = null, limit = 20): DuplicateQuery {
  try {
    assertBoundedJson(preview);
    if (!validateIntakePreview(preview) || !preview.authorized_scope.project_id
      || !preview.authorized_scope.root_id || !identifier(preview.authorized_scope.root_id)) throw new Error(QUERY_ERROR);
    return normalizedQuery({
      schema_version: 1,
      project_id: preview.authorized_scope.project_id,
      raw_digest: preview.raw_digest,
      canonical_digest: preview.canonical_digest,
      origin: null,
      command_id: null,
      cursor,
      limit,
    });
  } catch {
    throw new Error(QUERY_ERROR);
  }
}

async function sha256(bytes: Uint8Array<ArrayBuffer>): Promise<string> {
  try {
    if (!globalThis.crypto?.subtle) throw new Error(CRYPTO_ERROR);
    const result = await globalThis.crypto.subtle.digest('SHA-256', bytes);
    return `sha256:${Array.from(new Uint8Array(result), byte => byte.toString(16).padStart(2, '0')).join('')}`;
  } catch {
    throw new Error(CRYPTO_ERROR);
  }
}

/** Rust canonical_hash hashes the domain plus the UTF-8 TEXT of content_digest. */
export async function duplicateQueryDigest(query: DuplicateQuery): Promise<string> {
  const normalized = normalizedQuery(query);
  const contentDigest = await sha256(encoder.encode(JSON.stringify(normalized)));
  return sha256(encoder.encode(`admiralty.intake.duplicate-query/v1\0${contentDigest}`));
}

function validPage(value: unknown, query: NormalizedQuery, expectedDigest: string): value is DuplicatePage {
  if (!validateDuplicatePage(value)) return false;
  const page = value as DuplicatePage;
  if (page.schema_version !== 1 || page.query_digest !== expectedDigest
    || !digest(page.query_digest) || page.candidates.length > query.limit || page.candidates.length > 100) return false;
  for (const candidate of page.candidates) {
    if (!identifier(candidate.task_id) || !Number.isSafeInteger(candidate.contract_revision)
      || candidate.contract_revision < 1 || candidate.contract_revision > 0xffff_ffff
      || !digest(candidate.contract_digest) || !text(candidate.lifecycle, 128) || !text(candidate.freshness, 128)
      || candidate.evidence_refs.length > 64 || !candidate.evidence_refs.every(reference)) return false;
  }
  if (page.coverage.kind === 'partial' && !reference(page.coverage.continuation)) return false;
  if (page.coverage.kind === 'incomplete' && !text(page.coverage.reason, 4096)) return false;
  return encoder.encode(JSON.stringify(page)).byteLength <= MAX_PAGE_BYTES;
}

function statusMessage(status: number): string {
  if (status === 401) return 'Related-work search credentials were rejected.';
  if (status === 403) return 'The current intake scope denied related-work search.';
  if (status === 429) return 'Related-work search capacity is currently full.';
  if (status === 503) return 'Related-work search is unavailable.';
  if (status === 413) return 'Related-work search exceeds the server body limit.';
  if (status === 400 || status === 422) return 'The server rejected this related-work query.';
  return 'Related-work search returned an unsupported response.';
}
function aborted(): Error {
  const error = new Error('Related-work search was canceled.');
  error.name = 'AbortError';
  return error;
}
function validToken(token: string): boolean {
  return token.length > 0 && token.length <= 4096 && !/[^!-~]/.test(token);
}

export async function queryIntakeDuplicates(
  request: IntakeDuplicateRequest,
  ownerToken: string,
  intakeToken: string,
  fetcher: typeof fetch = fetch,
  signal?: AbortSignal,
): Promise<DuplicatePage> {
  let query: NormalizedQuery;
  let body: string;
  try {
    assertBoundedJson(request);
    if (!object(request) || !exactKeys(request, ['project_id', 'root_id', 'query'])
      || !identifier(request.project_id) || !identifier(request.root_id)) throw new Error(QUERY_ERROR);
    query = normalizedQuery(request.query);
    if (query.project_id !== request.project_id) throw new Error(QUERY_ERROR);
    body = JSON.stringify({ project_id: request.project_id, root_id: request.root_id, query });
    if (encoder.encode(body).byteLength > MAX_QUERY_BYTES) throw new Error(QUERY_ERROR);
  } catch {
    throw new Error(QUERY_ERROR);
  }
  if (!validToken(ownerToken) || !validToken(intakeToken) || ownerToken === intakeToken) {
    throw new Error('Supply the owner access token and a separate intake token for related-work search.');
  }
  if (signal?.aborted) throw aborted();
  const expectedDigest = await duplicateQueryDigest(query);
  if (signal?.aborted) throw aborted();
  let response: Response;
  try {
    response = await fetcher('/v2/intake/duplicates/query', {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${ownerToken}`,
        'X-Admiralty-Intake-Authorization': `Bearer ${intakeToken}`,
        'X-Admiralty-Requested-With': 'intake',
        'Content-Type': 'application/json',
      },
      body,
      cache: 'no-store',
      redirect: 'error',
      signal,
    });
  } catch {
    if (signal?.aborted) throw aborted();
    throw new Error(RESPONSE_ERROR);
  }
  if (signal?.aborted) throw aborted();
  if (!response.ok) throw Object.assign(new Error(statusMessage(response.status)), { status: response.status });
  try {
    const value = await readBoundedJson(response);
    if (signal?.aborted) throw aborted();
    assertBoundedJson(value);
    if (!object(value) || !exactKeys(value, ['schema_version', 'data', 'execution_granted', 'acceptance_granted'])
      || value.schema_version !== 1 || value.execution_granted !== false || value.acceptance_granted !== false
      || !validPage(value.data, query, expectedDigest)) throw new Error(RESPONSE_ERROR);
    return value.data;
  } catch {
    if (signal?.aborted) throw aborted();
    throw new Error(RESPONSE_ERROR);
  }
}
