import { useEffect, useMemo, useRef, useState } from 'react';
import IntegrationQuality from '../features/integration/IntegrationQuality';
import {
  validateCoordinationMessage,
  validateDomainEventV2,
  validateTaskContract,
  type AcceptanceCriterion,
  type CandidateDecisionEvidence,
  type CheckSpec,
  type CoordinationMessage,
  type DomainEventV2,
  type LifecycleAction,
  type TaskContract,
} from '../../../../packages/contracts/index';
import {
  ApiError,
  fleetTotals,
  formatUnknown,
  getFleet,
  readBoundedJson,
  taskStateLabel,
  type Cursor,
  type Fleet,
} from '../api';
import { createSseParser, cursorFromId, type ConnectionState } from '../events';
import * as sharedValidation from '../validation';
import { assertBoundedJson, decimal, parseCursor, parseFleet, type DomainRecord } from '../validation';

export type CommandJourneyTab = 'intake' | 'detail' | 'attention' | 'actions' | 'integration';

export type AnyTaskState =
  | Fleet['tasks'][number]['state']
  | 'accepted'
  | 'failed'
  | 'cancelled';

export type TaskLifecycleView = {
  revision: number;
  attempts: Record<string, string>;
  closure_digest: string | null;
  decision: CandidateDecisionEvidence | null;
  reason: string | null;
};

export type AnyFleetTask = {
  contract: TaskContract;
  state: AnyTaskState;
  attempt_id: string | null;
  owner: string | null;
  candidate_ref: string | null;
  lifecycle?: TaskLifecycleView;
};

export type AnyFleet = Omit<Fleet, 'schema_version' | 'tasks'> & {
  schema_version: 1 | 2;
  tasks: AnyFleetTask[];
};

export type NativeFleet = Omit<Fleet, 'schema_version' | 'tasks'> & {
  schema_version: 2;
  tasks: AnyFleetTask[];
};

export type AnyDomainRecord = {
  seq: string;
  observed_utc: string;
  event: DomainEventV2;
};

export type IntakeDraft = {
  project_id: string;
  root_id: string;
  task_id: string;
  revision: string;
  title: string;
  objective: string;
  non_goals: string;
  owned_paths: string;
  forbidden_paths: string;
  read_first: string;
  dependencies: string;
  criterion_id: string;
  criterion_text: string;
  check_id: string;
  check_program: string;
  check_args: string;
  check_cwd: string;
  check_timeout_secs: string;
  authority_ref: string;
  max_attempts: string;
  simulated: boolean;
  full_non_goals?: string[];
  full_owned_paths?: string[];
  full_forbidden_paths?: string[];
  full_read_first?: string[];
  full_dependencies?: string[];
  full_criteria?: AcceptanceCriterion[];
  full_checks?: CheckSpec[];
};

export type DuplicateCandidate = {
  task_id: string;
  revision: number;
  title: string;
  state: AnyTaskState;
  reason: 'same_task_id' | 'owned_path_overlap' | 'title_similarity';
  overlapping_paths: string[];
};

export type DuplicateCoverage = {
  inspected: number;
  total: number;
  partial: boolean;
};

export type DuplicateCandidateList = DuplicateCandidate[] & DuplicateCoverage;

export type IntakeOperationKind = 'create' | 'revise_contract';

export type IntakePreview = {
  valid: boolean;
  operation: IntakeOperationKind;
  contract: TaskContract | null;
  expectedTaskDigest: string | null;
  errors: string[];
  warnings: string[];
  duplicates: DuplicateCandidateList;
  duplicateCoverage: DuplicateCoverage;
  cliCommand: string | null;
  nonDispatchNotice: string;
};

export type CommandReceipt = {
  event_seq: string;
  source_contiguous_seq: '1';
  duplicate: boolean;
  execution_granted: false;
  acceptance_granted: false;
};

export type SubmitTaskRequest = {
  request_id: string;
  contract: TaskContract;
};

export type PendingLifecycleAction =
  | { action: 'cancel_pending'; reason: string }
  | { action: 'revise_contract'; contract: TaskContract };

export type SubmitLifecycleRequest = {
  request_id: string;
  task_id: string;
  expected_task_digest: string;
  action: PendingLifecycleAction;
};

export type TaskEvidenceItem = {
  seq: string;
  observed_utc: string;
  kind: DomainEventV2['kind'];
  summary: string;
  detail: string;
  attempt_id: string | null;
  actor: string | null;
  candidate_ref: string | null;
  decision_evidence?: CandidateDecisionEvidence | null;
};

export type DependencyResolution = {
  task_id: string;
  found: boolean;
  state: AnyTaskState | 'missing';
  satisfied: boolean;
};

export type ScopedMessageLedgerEntry = {
  seq: string;
  observed_utc: string;
  message: CoordinationMessage;
  acknowledged: boolean;
  ack_seq: string | null;
  ack_observed_utc: string | null;
};

export type LatestDecisionSource = 'snapshot' | 'historical_incomplete' | 'none';

export type TaskDetailModel = {
  task: AnyFleetTask;
  projectName: string;
  taskDigest: string;
  canCancelPending: boolean;
  canReviseContract: boolean;
  dependencies: DependencyResolution[];
  criteriaWithChecks: Array<{
    criterion: AcceptanceCriterion;
    checks: CheckSpec[];
    missingCheckIds: string[];
  }>;
  timeline: TaskEvidenceItem[];
  relatedMessages: ScopedMessageLedgerEntry[];
  latestDecision: CandidateDecisionEvidence | null;
  latestDecisionSource: LatestDecisionSource;
};

export type EventReplayStatus = 'complete' | 'partial_cap' | 'partial_eof' | 'unavailable';

export type ReplayCoverageState = {
  status: EventReplayStatus;
  coveredCount: number;
  coveredThroughSeq: string;
  targetSeq: string;
  maxEvents: number;
  detail: string | null;
};

export type ReplayedDomainRecords = AnyDomainRecord[] & {
  partial: boolean;
  status: EventReplayStatus;
  coveredThroughSeq: string;
  targetSeq: string;
  maxEvents: number;
};

export class EventReplayError extends Error {
  constructor(
    message: string,
    public readonly status: EventReplayStatus,
    public readonly records: AnyDomainRecord[],
    public readonly coveredThroughSeq: string,
    public readonly targetSeq: string,
  ) {
    super(message);
    this.name = 'EventReplayError';
  }
}

export type AttentionSeverity = 'high' | 'medium' | 'low';
export type AttentionCategory =
  | 'failed_task'
  | 'blocked_task'
  | 'awaiting_verification'
  | 'unassigned_ready'
  | 'pending_question'
  | 'handoff_failed_check'
  | 'unacknowledged_message';

export type AttentionItem = {
  id: string;
  severity: AttentionSeverity;
  category: AttentionCategory;
  project_id: string;
  task_id: string | null;
  owner: string | null;
  title: string;
  detail: string;
  observed_utc: string | null;
  seq: string | null;
};

export type HealthStatus = {
  status: 'ok';
  version: string;
  native_execution_qualified: false;
};

export type FailureReason =
  | 'failure'
  | 'test_fail'
  | 'context_exhausted'
  | 'provider_unavailable'
  | 'setup_error'
  | 'blocked_input';

export type WaitReason =
  | 'capacity_or_unavailable'
  | 'attempt_limit'
  | 'no_capable_model'
  | 'escalation_not_authorized'
  | 'escalation_exhausted'
  | 'pinned_model_unavailable'
  | 'setup_required'
  | 'needs_input';

export type ModelSelectionRequest = {
  required_capabilities: string[];
  pinned_model: string | null;
  current_model: string | null;
  attempts_by_tier: Record<string, number>;
  last_failure: FailureReason | null;
  allow_escalation: boolean;
  allow_capacity_fallback: boolean;
};

export type ModelCapacitySnapshot = {
  in_use: Record<string, number>;
  unavailable: string[];
};

export type ModelSelectionOutcome =
  | { status: 'selected'; model_id: string; usage_cost: { status: 'unknown' } }
  | { status: 'waiting'; model_id: string | null; reason: WaitReason };

const NON_DISPATCH_NOTICE =
  'Intake preview validates schema, path boundaries, and duplicate candidates locally. Submitting a pending contract or lifecycle update persists to the durable journal only; it never dispatches workers or grants acceptance.';

/** Matches crates/admiralty-core/src/ids.rs: 1..128 ASCII alphanumeric, '_', '.', '-', excluding '.' and '..'. */
const RUST_ID_PATTERN = /^[A-Za-z0-9_.-]{1,128}$/;

export function isValidRustId(value: string): boolean {
  return RUST_ID_PATTERN.test(value) && value !== '.' && value !== '..';
}

export function splitList(raw: string): string[] {
  return [...new Set(raw.split(/[\n,]+/).map(item => item.trim()).filter(Boolean))];
}

/**
 * Parses check.args without Set deduplication, preserving exact argument order,
 * repeated flags/options, and empty arguments (via JSON string array or quoted "" tokens).
 */
export function parseCheckArgs(raw: string | readonly string[]): { args: string[]; error: string | null } {
  if (Array.isArray(raw)) {
    if (!raw.every(item => typeof item === 'string')) {
      return { args: [], error: 'check_args array must contain only strings.' };
    }
    return { args: [...raw], error: null };
  }
  const str = String(raw);
  const trimmed = str.trim();
  if (!trimmed) {
    return { args: [], error: null };
  }
  if (trimmed.startsWith('[')) {
    try {
      const parsed = JSON.parse(trimmed) as unknown;
      if (!Array.isArray(parsed) || !parsed.every(item => typeof item === 'string')) {
        return { args: [], error: 'check_args JSON array must be an array of strings.' };
      }
      return { args: parsed as string[], error: null };
    } catch (error) {
      return {
        args: [],
        error: error instanceof Error ? `Invalid check_args JSON array: ${error.message}` : 'Invalid check_args JSON array.',
      };
    }
  }
  const tokens = str.split(/[\n,]/).map(token => {
    const t = token.trim();
    if (t === '""' || t === "''") return '';
    return t;
  });
  return { args: tokens, error: null };
}

export function splitArgv(raw: string | readonly string[]): string[] {
  return parseCheckArgs(raw).args;
}

/** Matches crates/admiralty-core/src/contracts.rs safe_relative_path. */
export function isSafeRelativePath(path: string): boolean {
  if (
    !path ||
    path.length > 512 ||
    path.startsWith('/') ||
    path.includes('\\') ||
    path.includes('\0') ||
    /\p{Cc}/u.test(path)
  ) {
    return false;
  }
  const segments = path.split('/');
  if (segments.some(segment => segment === '..')) {
    return false;
  }
  const nonEmpty = segments.filter(segment => segment.length > 0);
  return nonEmpty.length > 0 && nonEmpty.every(segment => segment !== '..');
}

function pathsOverlap(left: string, right: string): boolean {
  const cleanLeft = left.replace(/\/+\*?$/, '');
  const cleanRight = right.replace(/\/+\*?$/, '');
  return (
    cleanLeft === cleanRight ||
    cleanLeft.startsWith(`${cleanRight}/`) ||
    cleanRight.startsWith(`${cleanLeft}/`)
  );
}

function titleTokens(title: string): Set<string> {
  return new Set(
    title
      .toLowerCase()
      .split(/[^a-z0-9]+/)
      .filter(token => token.length >= 4),
  );
}

const SHA256_K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);

function rotr(value: number, bits: number): number {
  return (value >>> bits) | (value << (32 - bits));
}

export function sha256Hex(input: string): string {
  const bytes = new TextEncoder().encode(input);
  const bitLen = bytes.length * 8;
  const totalBytes = Math.ceil((bytes.length + 9) / 64) * 64;
  const padded = new Uint8Array(totalBytes);
  padded.set(bytes);
  padded[bytes.length] = 0x80;
  const view = new DataView(padded.buffer);
  view.setUint32(totalBytes - 8, Math.floor(bitLen / 0x1_0000_0000), false);
  view.setUint32(totalBytes - 4, bitLen >>> 0, false);

  let h0 = 0x6a09e667;
  let h1 = 0xbb67ae85;
  let h2 = 0x3c6ef372;
  let h3 = 0xa54ff53a;
  let h4 = 0x510e527f;
  let h5 = 0x9b05688c;
  let h6 = 0x1f83d9ab;
  let h7 = 0x5be0cd19;

  const w = new Uint32Array(64);
  for (let offset = 0; offset < totalBytes; offset += 64) {
    for (let i = 0; i < 16; i++) {
      w[i] = view.getUint32(offset + i * 4, false);
    }
    for (let i = 16; i < 64; i++) {
      const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3);
      const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) >>> 0;
    }

    let a = h0;
    let b = h1;
    let c = h2;
    let d = h3;
    let e = h4;
    let f = h5;
    let g = h6;
    let h = h7;

    for (let i = 0; i < 64; i++) {
      const s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
      const ch = (e & f) ^ (~e & g);
      const temp1 = (h + s1 + ch + SHA256_K[i] + w[i]) >>> 0;
      const s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const temp2 = (s0 + maj) >>> 0;

      h = g;
      g = f;
      f = e;
      e = (d + temp1) >>> 0;
      d = c;
      c = b;
      b = a;
      a = (temp1 + temp2) >>> 0;
    }

    h0 = (h0 + a) >>> 0;
    h1 = (h1 + b) >>> 0;
    h2 = (h2 + c) >>> 0;
    h3 = (h3 + d) >>> 0;
    h4 = (h4 + e) >>> 0;
    h5 = (h5 + f) >>> 0;
    h6 = (h6 + g) >>> 0;
    h7 = (h7 + h) >>> 0;
  }

  return [h0, h1, h2, h3, h4, h5, h6, h7]
    .map(word => word.toString(16).padStart(8, '0'))
    .join('');
}

function encodeJsonObject(entries: ReadonlyArray<readonly [string, string]>): string {
  return `{${entries.map(([key, rawJson]) => `${JSON.stringify(key)}:${rawJson}`).join(',')}}`;
}

function encodeStringArray(items: readonly string[]): string {
  return `[${items.map(item => JSON.stringify(item)).join(',')}]`;
}

function serializeContractFragment(contract: TaskContract): string {
  const criteriaJson = `[${contract.criteria
    .map(c =>
      encodeJsonObject([
        ['id', JSON.stringify(c.id)],
        ['text', JSON.stringify(c.text)],
        ['check_ids', encodeStringArray(c.check_ids)],
      ]),
    )
    .join(',')}]`;
  const checksJson = `[${contract.checks
    .map(chk =>
      encodeJsonObject([
        ['id', JSON.stringify(chk.id)],
        ['program', JSON.stringify(chk.program)],
        ['args', encodeStringArray(chk.args)],
        ['cwd', JSON.stringify(chk.cwd)],
        ['timeout_secs', JSON.stringify(chk.timeout_secs)],
      ]),
    )
    .join(',')}]`;
  return encodeJsonObject([
    ['schema_version', JSON.stringify(contract.schema_version)],
    ['project_id', JSON.stringify(contract.project_id)],
    ['root_id', JSON.stringify(contract.root_id)],
    ['task_id', JSON.stringify(contract.task_id)],
    ['revision', JSON.stringify(contract.revision)],
    ['title', JSON.stringify(contract.title)],
    ['objective', JSON.stringify(contract.objective)],
    ['non_goals', encodeStringArray(contract.non_goals)],
    ['owned_paths', encodeStringArray(contract.owned_paths)],
    ['forbidden_paths', encodeStringArray(contract.forbidden_paths)],
    ['read_first', encodeStringArray(contract.read_first)],
    ['dependencies', encodeStringArray(contract.dependencies)],
    ['criteria', criteriaJson],
    ['checks', checksJson],
    ['authority_ref', JSON.stringify(contract.authority_ref)],
    ['max_attempts', JSON.stringify(contract.max_attempts)],
    ['simulated', JSON.stringify(contract.simulated)],
  ]);
}

/**
 * Serializes Rust BTreeMap<AttemptId, PrincipalId> directly as lexically sorted
 * key/value JSON fragments without any intermediate JS object round-trip.
 * Preserves lexical byte order ("10" before "2") and preserves own "__proto__" keys.
 */
export function serializeAttemptsBTreeMap(
  attempts: Record<string, string> | ReadonlyMap<string, string>,
): string {
  const entries: Array<[string, string]> = [];
  if (attempts instanceof Map) {
    for (const [k, v] of attempts.entries()) {
      if (typeof k === 'string' && typeof v === 'string') {
        entries.push([k, v]);
      }
    }
  } else {
    for (const key of Object.getOwnPropertyNames(attempts)) {
      const desc = Object.getOwnPropertyDescriptor(attempts, key);
      if (desc && desc.enumerable && typeof desc.value === 'string') {
        entries.push([key, desc.value]);
      }
    }
  }
  entries.sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0));
  return `{${entries.map(([k, v]) => `${JSON.stringify(k)}:${JSON.stringify(v)}`).join(',')}}`;
}

function serializeLifecycleFragment(lifecycle: TaskLifecycleView): string {
  const decisionJson = lifecycle.decision
    ? encodeJsonObject([
        ['contract_digest', JSON.stringify(lifecycle.decision.contract_digest)],
        ['verification_policy_digest', JSON.stringify(lifecycle.decision.verification_policy_digest)],
        ['receipt_digest', JSON.stringify(lifecycle.decision.receipt_digest)],
        ['custody_digest', JSON.stringify(lifecycle.decision.custody_digest)],
        ['closure_digest', JSON.stringify(lifecycle.decision.closure_digest)],
        ['source_commit_sha', JSON.stringify(lifecycle.decision.source_commit_sha)],
        ['source_tree_sha', JSON.stringify(lifecycle.decision.source_tree_sha)],
        ['verifier', JSON.stringify(lifecycle.decision.verifier)],
      ])
    : 'null';
  return encodeJsonObject([
    ['revision', JSON.stringify(lifecycle.revision)],
    ['attempts', serializeAttemptsBTreeMap(lifecycle.attempts)],
    ['closure_digest', JSON.stringify(lifecycle.closure_digest)],
    ['decision', decisionJson],
    ['reason', JSON.stringify(lifecycle.reason)],
  ]);
}

/** Matches Rust admiralty_core::content_digest(&task_view) serde_json field and BTreeMap order. */
export function serializeTaskViewForDigest(task: AnyFleetTask): string {
  const fields: Array<[string, string]> = [
    ['contract', serializeContractFragment(task.contract)],
    ['state', JSON.stringify(task.state)],
    ['attempt_id', JSON.stringify(task.attempt_id)],
    ['owner', JSON.stringify(task.owner)],
    ['candidate_ref', JSON.stringify(task.candidate_ref)],
  ];
  if (task.lifecycle !== undefined && task.lifecycle !== null) {
    fields.push(['lifecycle', serializeLifecycleFragment(task.lifecycle)]);
  }
  return encodeJsonObject(fields);
}

export function computeTaskViewDigest(task: AnyFleetTask): string {
  return `sha256:${sha256Hex(serializeTaskViewForDigest(task))}`;
}

const CANONICAL_UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const SHA256_DIGEST_PATTERN = /^sha256:[0-9a-f]{64}$/;

export function isCanonicalRequestId(value: string): boolean {
  return CANONICAL_UUID_PATTERN.test(value);
}

export function generateCanonicalRequestId(): string {
  if (typeof globalThis.crypto?.randomUUID === 'function') {
    return globalThis.crypto.randomUUID().toLowerCase();
  }
  const bytes = new Uint8Array(16);
  if (typeof globalThis.crypto?.getRandomValues === 'function') {
    globalThis.crypto.getRandomValues(bytes);
  } else {
    for (let i = 0; i < 16; i++) {
      bytes[i] = Math.floor(Math.random() * 256);
    }
  }
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, b => b.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20, 32)}`;
}

export function parseDomainRecordV2(value: unknown): AnyDomainRecord {
  const shared = (sharedValidation as { parseDomainRecordV2?: (input: unknown) => AnyDomainRecord }).parseDomainRecordV2;
  if (typeof shared === 'function') {
    return shared(value);
  }
  assertBoundedJson(value);
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('Unsupported API response: expected object');
  }
  const record = value as Record<string, unknown>;
  if (!decimal(record.seq) || record.seq === '0' || typeof record.observed_utc !== 'string') {
    throw new Error('Unsupported API response: invalid domain record');
  }
  if (!validateDomainEventV2(record.event)) {
    throw new Error('Unsupported API response: invalid or unknown domain event v2 schema');
  }
  return record as AnyDomainRecord;
}

function assertObject(value: unknown): Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('Unsupported API response: expected object');
  }
  return value as Record<string, unknown>;
}

function assertEnvelope(condition: unknown, label: string): asserts condition {
  if (!condition) {
    throw new Error(`Unsupported API response: ${label}`);
  }
}

const isSafeCount = (value: unknown): value is number =>
  Number.isSafeInteger(value) && Number(value) >= 0;
const isNullableText = (value: unknown): value is string | null =>
  value === null || typeof value === 'string';

function parseStrictLifecycle(value: unknown): TaskLifecycleView {
  const lifecycle = assertObject(value);
  const lifecycleKeys = ['revision', 'attempts', 'closure_digest', 'decision', 'reason'];
  assertEnvelope(
    Object.keys(lifecycle).length === lifecycleKeys.length &&
      lifecycleKeys.every(key => Object.hasOwn(lifecycle, key)),
    'invalid lifecycle projection fields',
  );
  assertEnvelope(
    Number.isSafeInteger(lifecycle.revision) &&
      Number(lifecycle.revision) >= 0 &&
      Number(lifecycle.revision) <= 0xffff_ffff,
    'invalid lifecycle revision',
  );
  const attempts = assertObject(lifecycle.attempts);
  assertEnvelope(
    Object.values(attempts).every(entry => typeof entry === 'string') &&
      isNullableText(lifecycle.closure_digest) &&
      isNullableText(lifecycle.reason),
    'invalid lifecycle projection',
  );
  if (lifecycle.decision !== null) {
    const decision = assertObject(lifecycle.decision);
    const keys = [
      'closure_digest',
      'contract_digest',
      'custody_digest',
      'receipt_digest',
      'source_commit_sha',
      'source_tree_sha',
      'verification_policy_digest',
      'verifier',
    ];
    assertEnvelope(
      Object.keys(decision).length === keys.length &&
        keys.every(key => typeof decision[key] === 'string'),
      'invalid lifecycle decision',
    );
  }
  return lifecycle as unknown as TaskLifecycleView;
}

export function parseNativeFleet(value: unknown): NativeFleet {
  const shared = (sharedValidation as { parseNativeFleet?: (input: unknown) => NativeFleet }).parseNativeFleet;
  if (typeof shared === 'function') {
    return shared(value);
  }
  assertBoundedJson(value);
  const fleet = assertObject(value);
  assertEnvelope(
    fleet.schema_version === 2 &&
      typeof fleet.simulated === 'boolean' &&
      fleet.native_execution_qualified === false,
    'unqualified fleet envelope',
  );
  parseCursor(fleet.cursor);
  assertEnvelope(Array.isArray(fleet.projects) && Array.isArray(fleet.tasks), 'invalid fleet collections');
  for (const item of fleet.projects) {
    const project = assertObject(item);
    assertEnvelope(typeof project.id === 'string' && typeof project.name === 'string', 'invalid project identity');
    for (const key of ['total', 'running', 'awaiting_verification', 'blocked', 'accepted']) {
      assertEnvelope(isSafeCount(project[key]), `invalid project ${key}`);
    }
  }
  const states = [
    'ready',
    'blocked',
    'claimed',
    'running',
    'awaiting_verification',
    'accepted',
    'failed',
    'cancelled',
  ];
  for (const item of fleet.tasks) {
    const task = assertObject(item);
    assertEnvelope(validateTaskContract(task.contract), 'invalid task contract schema or version');
    assertEnvelope(
      states.includes(String(task.state)) &&
        isNullableText(task.attempt_id) &&
        isNullableText(task.owner) &&
        isNullableText(task.candidate_ref),
      'invalid task projection',
    );
    if (Object.hasOwn(task, 'lifecycle')) {
      parseStrictLifecycle(task.lifecycle);
    }
  }
  const coordination = assertObject(fleet.coordination);
  assertEnvelope(
    isSafeCount(coordination.queued_messages) &&
      isSafeCount(coordination.acknowledged_messages) &&
      coordination.routine_model_turns === 0 &&
      (coordination.measured_tokens === null || decimal(coordination.measured_tokens)),
    'invalid coordination metrics',
  );
  assertEnvelope(typeof fleet.sqlite_version === 'string', 'missing SQLite version');
  return fleet as unknown as NativeFleet;
}

export function parseAnyFleet(value: unknown): AnyFleet {
  assertBoundedJson(value);
  const fleet = assertObject(value);
  if (fleet.schema_version === 1) {
    if (Array.isArray(fleet.tasks)) {
      for (const item of fleet.tasks) {
        const task = assertObject(item);
        assertEnvelope(!Object.hasOwn(task, 'lifecycle'), 'v1 task projection cannot contain lifecycle metadata');
      }
    }
    return parseFleet(value) as unknown as AnyFleet;
  }
  if (fleet.schema_version === 2) {
    return parseNativeFleet(value);
  }
  throw new Error('Unsupported API response: unqualified fleet envelope');
}

export async function getCommandFleet(
  token?: string,
  signal?: AbortSignal,
  fetcher: typeof fetch = fetch,
): Promise<AnyFleet> {
  const response = await fetcher('/v2/fleet', {
    headers: token ? { Authorization: `Bearer ${token}` } : undefined,
    signal,
    cache: 'no-store',
  });
  if (response.status === 404) {
    return getFleet(token, signal, fetcher);
  }
  if (!response.ok) {
    throw new ApiError(`Fleet request failed (${response.status})`, response.status);
  }
  return parseNativeFleet(await readBoundedJson(response));
}

export function parseCommandReceipt(value: unknown): CommandReceipt {
  assertBoundedJson(value);
  const receipt = assertObject(value);
  assertEnvelope(
    decimal(receipt.event_seq) &&
      receipt.event_seq !== '0' &&
      receipt.source_contiguous_seq === '1' &&
      typeof receipt.duplicate === 'boolean' &&
      receipt.execution_granted === false &&
      receipt.acceptance_granted === false,
    'invalid command receipt envelope',
  );
  return {
    event_seq: receipt.event_seq,
    source_contiguous_seq: '1',
    duplicate: receipt.duplicate,
    execution_granted: false,
    acceptance_granted: false,
  };
}

async function readCommandErrorDetail(response: Response, fallbackPrefix: string): Promise<ApiError> {
  let detail = '';
  try {
    const parsed = (await readBoundedJson(response)) as { error?: unknown; detail?: unknown };
    if (typeof parsed.error === 'string' && parsed.error.trim()) {
      detail = `: ${parsed.error}`;
    } else if (typeof parsed.detail === 'string' && parsed.detail.trim()) {
      detail = `: ${parsed.detail}`;
    }
  } catch {
    /* bounded fallback */
  }
  return new ApiError(`${fallbackPrefix} (${response.status})${detail}`, response.status);
}

export async function submitNativeTask(
  request: SubmitTaskRequest,
  token?: string,
  signal?: AbortSignal,
  fetcher: typeof fetch = fetch,
): Promise<CommandReceipt> {
  if (!isCanonicalRequestId(request.request_id)) {
    throw new Error('request_id must be a canonical lowercase UUID');
  }
  if (!validateTaskContract(request.contract)) {
    throw new Error('Task contract does not match TaskContract schema v1');
  }
  const body = JSON.stringify({
    request_id: request.request_id,
    contract: request.contract,
  });
  const response = await fetcher('/v2/tasks', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
    body,
    signal,
  });
  if (!response.ok) {
    throw await readCommandErrorDetail(response, 'Task submission failed');
  }
  return parseCommandReceipt(await readBoundedJson(response));
}

export async function submitNativeLifecycle(
  request: SubmitLifecycleRequest,
  token?: string,
  signal?: AbortSignal,
  fetcher: typeof fetch = fetch,
): Promise<CommandReceipt> {
  if (!isCanonicalRequestId(request.request_id)) {
    throw new Error('request_id must be a canonical lowercase UUID');
  }
  if (!isValidRustId(request.task_id)) {
    throw new Error('task_id must be a valid Admiralty identifier');
  }
  if (!SHA256_DIGEST_PATTERN.test(request.expected_task_digest)) {
    throw new Error('expected_task_digest must be a lowercase sha256:<64-hex> digest');
  }
  if (request.action.action === 'cancel_pending') {
    const reason = request.action.reason;
    if (!reason || !reason.trim() || reason.length > 512 || /\p{Cc}/u.test(reason)) {
      throw new Error('Pending cancellation reason must be 1–512 printable characters without control characters');
    }
  } else if (request.action.action === 'revise_contract') {
    if (request.action.contract.task_id !== request.task_id || !validateTaskContract(request.action.contract)) {
      throw new Error('Contract revision must match target task_id and TaskContract schema v1');
    }
  }
  const body = JSON.stringify({
    request_id: request.request_id,
    task_id: request.task_id,
    expected_task_digest: request.expected_task_digest,
    action: request.action,
  });
  const response = await fetcher('/v2/tasks/lifecycle', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
    body,
    signal,
  });
  if (!response.ok) {
    throw await readCommandErrorDetail(response, 'Lifecycle command failed');
  }
  return parseCommandReceipt(await readBoundedJson(response));
}

export function canCancelPendingTask(task: AnyFleetTask): boolean {
  return (task.state === 'ready' || task.state === 'blocked') && task.attempt_id === null;
}

export function canReviseTaskContract(
  task: AnyFleetTask,
  allTasks: readonly AnyFleetTask[] = [],
): boolean {
  const attemptsCount = task.lifecycle ? Object.keys(task.lifecycle.attempts).length : 0;
  const referencedByDependent = allTasks.some(other =>
    other.contract.dependencies.includes(task.contract.task_id),
  );
  return (
    (task.state === 'ready' || task.state === 'blocked') &&
    task.attempt_id === null &&
    attemptsCount === 0 &&
    !referencedByDependent
  );
}

export function emptyIntakeDraft(projectId = 'admiralty'): IntakeDraft {
  return {
    project_id: projectId,
    root_id: 'root-main',
    task_id: 'task-intake-1',
    revision: '1',
    title: 'Verify bounded task intake contract',
    objective: 'Validate contract fields and inspect duplicate candidates before persistence.',
    non_goals: 'Do not silently launch workers or bypass verification gates',
    owned_paths: 'apps/dashboard/src/ui/CommandJourneys.tsx',
    forbidden_paths: 'crates/admiralty-core/src/events.rs, packages/contracts/index.ts',
    read_first: 'docs/plans/admiralty-v2/REMAINING_AUDIT_2026-10-18.md',
    dependencies: '',
    criterion_id: 'crit-1',
    criterion_text: 'Vitest suite and TypeScript build succeed locally',
    check_id: 'chk-vitest',
    check_program: 'pnpm',
    check_args: '--dir, apps/dashboard, test, --, --run',
    check_cwd: '.',
    check_timeout_secs: '120',
    authority_ref: 'operator:intake-preview',
    max_attempts: '2',
    simulated: false,
  };
}

function formatListFieldForDraft(items: readonly string[]): string {
  if (items.some(item => item.includes(',') || item.includes('\n'))) {
    return JSON.stringify(items);
  }
  return items.join(', ');
}

function parseListFieldFromDraft(
  raw: string,
  fullBackup: readonly string[] | undefined,
  deduplicate: boolean,
): string[] {
  const trimmed = raw.trim();
  if (fullBackup && formatListFieldForDraft(fullBackup) === trimmed) {
    return [...fullBackup];
  }
  if (trimmed.startsWith('[')) {
    try {
      const parsed = JSON.parse(trimmed) as unknown;
      if (Array.isArray(parsed) && parsed.every(item => typeof item === 'string')) {
        return deduplicate ? [...new Set(parsed as string[])] : (parsed as string[]);
      }
    } catch {
      /* fall through to split */
    }
  }
  if (deduplicate) {
    return splitList(raw);
  }
  return raw
    .split(/[\n,]+/)
    .map(item => item.trim())
    .filter(Boolean);
}

export function contractFromTaskForRevision(
  task: AnyFleetTask,
  patch?: Partial<Pick<TaskContract, 'title' | 'objective'>>,
): TaskContract {
  return {
    schema_version: task.contract.schema_version,
    project_id: task.contract.project_id,
    root_id: task.contract.root_id,
    task_id: task.contract.task_id,
    revision: task.contract.revision + 1,
    title: patch?.title ?? task.contract.title,
    objective: patch?.objective ?? task.contract.objective,
    non_goals: [...task.contract.non_goals],
    owned_paths: [...task.contract.owned_paths],
    forbidden_paths: [...task.contract.forbidden_paths],
    read_first: [...task.contract.read_first],
    dependencies: [...task.contract.dependencies],
    criteria: task.contract.criteria.map(c => ({
      id: c.id,
      text: c.text,
      check_ids: [...c.check_ids],
    })),
    checks: task.contract.checks.map(chk => ({
      id: chk.id,
      program: chk.program,
      args: [...chk.args],
      cwd: chk.cwd,
      timeout_secs: chk.timeout_secs,
    })),
    authority_ref: task.contract.authority_ref,
    max_attempts: task.contract.max_attempts,
    simulated: task.contract.simulated,
  };
}

export function draftFromTaskForRevision(task: AnyFleetTask): IntakeDraft {
  const firstCriterion = task.contract.criteria[0];
  const firstCheck = task.contract.checks[0];
  return {
    project_id: task.contract.project_id,
    root_id: task.contract.root_id,
    task_id: task.contract.task_id,
    revision: String(task.contract.revision + 1),
    title: task.contract.title,
    objective: task.contract.objective,
    non_goals: formatListFieldForDraft(task.contract.non_goals),
    owned_paths: formatListFieldForDraft(task.contract.owned_paths),
    forbidden_paths: formatListFieldForDraft(task.contract.forbidden_paths),
    read_first: formatListFieldForDraft(task.contract.read_first),
    dependencies: formatListFieldForDraft(task.contract.dependencies),
    criterion_id: firstCriterion?.id ?? 'crit-1',
    criterion_text: firstCriterion?.text ?? '',
    check_id: firstCheck?.id ?? 'chk-1',
    check_program: firstCheck?.program ?? 'cargo',
    check_args: JSON.stringify(firstCheck?.args ?? []),
    check_cwd: firstCheck?.cwd ?? '.',
    check_timeout_secs: String(firstCheck?.timeout_secs ?? 60),
    authority_ref: task.contract.authority_ref,
    max_attempts: String(task.contract.max_attempts),
    simulated: task.contract.simulated,
    full_non_goals: [...task.contract.non_goals],
    full_owned_paths: [...task.contract.owned_paths],
    full_forbidden_paths: [...task.contract.forbidden_paths],
    full_read_first: [...task.contract.read_first],
    full_dependencies: [...task.contract.dependencies],
    full_criteria: task.contract.criteria.map(c => ({
      id: c.id,
      text: c.text,
      check_ids: [...c.check_ids],
    })),
    full_checks: task.contract.checks.map(chk => ({
      id: chk.id,
      program: chk.program,
      args: [...chk.args],
      cwd: chk.cwd,
      timeout_secs: chk.timeout_secs,
    })),
  };
}

export function draftToContractCandidate(draft: IntakeDraft): unknown {
  const revision = /^\d+$/.test(draft.revision.trim()) ? Number(draft.revision.trim()) : NaN;
  const maxAttempts = /^\d+$/.test(draft.max_attempts.trim()) ? Number(draft.max_attempts.trim()) : NaN;
  const timeoutSecs = /^\d+$/.test(draft.check_timeout_secs.trim()) ? Number(draft.check_timeout_secs.trim()) : NaN;
  const checkId = draft.check_id.trim();
  const parsedArgs = parseCheckArgs(draft.check_args);

  const originalFirstCheckId = draft.full_checks?.[0]?.id;
  const renamedFirstCheck =
    Boolean(originalFirstCheckId && checkId && checkId !== originalFirstCheckId);
  const remapCriterionCheckIds = (ids: readonly string[]): string[] =>
    renamedFirstCheck
      ? ids.map(id => (id === originalFirstCheckId ? checkId : id))
      : [...ids];

  const checks: CheckSpec[] =
    draft.full_checks && draft.full_checks.length > 0
      ? draft.full_checks.map((chk, idx) =>
          idx === 0 && checkId
            ? {
                id: checkId,
                program: draft.check_program.trim(),
                args: parsedArgs.args,
                cwd: draft.check_cwd.trim(),
                timeout_secs: timeoutSecs,
              }
            : { ...chk, args: [...chk.args] },
        )
      : checkId
        ? [
            {
              id: checkId,
              program: draft.check_program.trim(),
              args: parsedArgs.args,
              cwd: draft.check_cwd.trim(),
              timeout_secs: timeoutSecs,
            },
          ]
        : [];

  const criteria: AcceptanceCriterion[] =
    draft.full_criteria && draft.full_criteria.length > 0
      ? draft.full_criteria.map((crit, idx) =>
          idx === 0 && (draft.criterion_id.trim() || draft.criterion_text.trim())
            ? {
                id: draft.criterion_id.trim(),
                text: draft.criterion_text.trim(),
                check_ids:
                  crit.check_ids.length > 0
                    ? remapCriterionCheckIds(crit.check_ids)
                    : checkId
                      ? [checkId]
                      : [],
              }
            : { ...crit, check_ids: remapCriterionCheckIds(crit.check_ids) },
        )
      : draft.criterion_id.trim() || draft.criterion_text.trim()
        ? [
            {
              id: draft.criterion_id.trim(),
              text: draft.criterion_text.trim(),
              check_ids: checkId ? [checkId] : [],
            },
          ]
        : [];

  return {
    schema_version: 1,
    project_id: draft.project_id.trim(),
    root_id: draft.root_id.trim(),
    task_id: draft.task_id.trim(),
    revision,
    title: draft.title.trim(),
    objective: draft.objective.trim(),
    non_goals: parseListFieldFromDraft(draft.non_goals, draft.full_non_goals, true),
    owned_paths: parseListFieldFromDraft(draft.owned_paths, draft.full_owned_paths, true),
    forbidden_paths: parseListFieldFromDraft(draft.forbidden_paths, draft.full_forbidden_paths, true),
    read_first: parseListFieldFromDraft(draft.read_first, draft.full_read_first, true),
    dependencies: parseListFieldFromDraft(draft.dependencies, draft.full_dependencies, false),
    criteria,
    checks,
    authority_ref: draft.authority_ref.trim(),
    max_attempts: maxAttempts,
    simulated: draft.simulated,
  };
}

function attachDuplicateCoverage(
  items: DuplicateCandidate[],
  coverage: DuplicateCoverage,
): DuplicateCandidateList {
  return Object.assign(items, coverage);
}

export function findDuplicateCandidates(
  contract: Pick<TaskContract, 'project_id' | 'task_id' | 'revision' | 'title' | 'owned_paths'>,
  existingTasks: readonly AnyFleetTask[],
  maxInspect = 500,
): DuplicateCandidateList {
  const duplicates: DuplicateCandidate[] = [];
  const candidateTokens = titleTokens(contract.title);
  const boundedMax = Math.max(0, maxInspect);
  const inspected = Math.min(existingTasks.length, boundedMax);
  const total = existingTasks.length;
  const partial = total > inspected;
  const seenSameId = new Set<string>();

  // Exact task_id matches are checked across all snapshot tasks so IDs beyond index 500 are never missed.
  for (const existing of existingTasks) {
    const other = existing.contract;
    if (other.task_id === contract.task_id && !seenSameId.has(other.task_id)) {
      seenSameId.add(other.task_id);
      duplicates.push({
        task_id: other.task_id,
        revision: other.revision,
        title: other.title,
        state: existing.state,
        reason: 'same_task_id',
        overlapping_paths: other.owned_paths.filter(left =>
          contract.owned_paths.some(right => pathsOverlap(left, right)),
        ),
      });
    }
  }

  // Heuristic owned-path overlap and title similarity scan is bounded to `inspected` tasks.
  for (let index = 0; index < inspected; index++) {
    const existing = existingTasks[index];
    const other = existing.contract;
    if (other.task_id === contract.task_id) continue;
    if (other.project_id !== contract.project_id) continue;

    const overlapping = contract.owned_paths.filter(left =>
      other.owned_paths.some(right => pathsOverlap(left, right)),
    );
    if (overlapping.length > 0 && existing.state !== 'accepted' && existing.state !== 'cancelled') {
      duplicates.push({
        task_id: other.task_id,
        revision: other.revision,
        title: other.title,
        state: existing.state,
        reason: 'owned_path_overlap',
        overlapping_paths: overlapping,
      });
      continue;
    }

    if (candidateTokens.size >= 2) {
      const otherTokens = titleTokens(other.title);
      let shared = 0;
      for (const token of candidateTokens) {
        if (otherTokens.has(token)) shared++;
      }
      if (shared >= 2 && shared / Math.max(candidateTokens.size, otherTokens.size) >= 0.6) {
        duplicates.push({
          task_id: other.task_id,
          revision: other.revision,
          title: other.title,
          state: existing.state,
          reason: 'title_similarity',
          overlapping_paths: [],
        });
      }
    }
  }

  return attachDuplicateCoverage(duplicates, { inspected, total, partial });
}

function isIntakeDraft(input: unknown): input is IntakeDraft {
  return (
    typeof input === 'object' &&
    input !== null &&
    'criterion_id' in input &&
    'check_program' in input &&
    typeof (input as IntakeDraft).revision === 'string'
  );
}

export function buildIntakePreview(
  input: IntakeDraft | unknown,
  existingTasks: readonly AnyFleetTask[],
  options?: { mode?: 'auto' | 'create' | 'revise'; maxDuplicateInspect?: number },
): IntakePreview {
  const maxInspect = options?.maxDuplicateInspect ?? 500;
  const emptyCoverage: DuplicateCoverage = {
    inspected: Math.min(existingTasks.length, Math.max(0, maxInspect)),
    total: existingTasks.length,
    partial: existingTasks.length > Math.max(0, maxInspect),
  };
  const draftArgvError = isIntakeDraft(input) ? parseCheckArgs(input.check_args).error : null;
  const rawCandidate = isIntakeDraft(input) ? draftToContractCandidate(input) : input;
  const errors: string[] = [];
  const warnings: string[] = [];
  if (draftArgvError) {
    errors.push(draftArgvError);
  }

  try {
    assertBoundedJson(rawCandidate);
  } catch (error) {
    return {
      valid: false,
      operation: 'create',
      contract: null,
      expectedTaskDigest: null,
      errors: [error instanceof Error ? error.message : 'Input exceeds validation bounds'],
      warnings: [],
      duplicates: attachDuplicateCoverage([], emptyCoverage),
      duplicateCoverage: emptyCoverage,
      cliCommand: null,
      nonDispatchNotice: NON_DISPATCH_NOTICE,
    };
  }

  // Inspect raw candidate before/alongside generated schema validation so specific Rust contract errors surface clearly.
  if (!rawCandidate || typeof rawCandidate !== 'object' || Array.isArray(rawCandidate)) {
    return {
      valid: false,
      operation: 'create',
      contract: null,
      expectedTaskDigest: null,
      errors: ['Contract candidate must be a JSON object.'],
      warnings: [],
      duplicates: attachDuplicateCoverage([], emptyCoverage),
      duplicateCoverage: emptyCoverage,
      cliCommand: null,
      nonDispatchNotice: NON_DISPATCH_NOTICE,
    };
  }

  const candidateObj = rawCandidate as Record<string, unknown>;
  if (typeof candidateObj.project_id === 'string' && !isValidRustId(candidateObj.project_id)) {
    errors.push(
      `Invalid project_id "${candidateObj.project_id}": identifier must contain 1..128 ASCII letters, digits, '_', '-', or '.', and cannot be '.' or '..' (':' and '/' are forbidden).`,
    );
  }
  if (typeof candidateObj.root_id === 'string' && !isValidRustId(candidateObj.root_id)) {
    errors.push(
      `Invalid root_id "${candidateObj.root_id}": identifier must contain 1..128 ASCII letters, digits, '_', '-', or '.', and cannot be '.' or '..' (':' and '/' are forbidden).`,
    );
  }
  if (typeof candidateObj.task_id === 'string' && !isValidRustId(candidateObj.task_id)) {
    errors.push(
      `Invalid task_id "${candidateObj.task_id}": identifier must contain 1..128 ASCII letters, digits, '_', '-', or '.', and cannot be '.' or '..' (':' and '/' are forbidden).`,
    );
  }
  if (Array.isArray(candidateObj.checks)) {
    for (const rawCheck of candidateObj.checks) {
      if (rawCheck && typeof rawCheck === 'object') {
        const chk = rawCheck as Record<string, unknown>;
        const chkId = typeof chk.id === 'string' ? chk.id : '<unnamed>';
        if (typeof chk.cwd === 'string' && !isSafeRelativePath(chk.cwd)) {
          errors.push(`Invalid check cwd "${chk.cwd}" on check "${chkId}": must be a safe relative path without ".." or leading "/".`);
        }
        if (typeof chk.program === 'string' && chk.program.includes('\0')) {
          errors.push(`Check "${chkId}" program must not contain NUL bytes.`);
        }
        if (Array.isArray(chk.args) && chk.args.some(arg => typeof arg === 'string' && arg.includes('\0'))) {
          errors.push(`Check "${chkId}" args must not contain NUL bytes.`);
        }
      }
    }
  }
  if (Array.isArray(candidateObj.criteria)) {
    const seenCrit = new Set<string>();
    for (const rawCrit of candidateObj.criteria) {
      if (rawCrit && typeof rawCrit === 'object') {
        const critId = (rawCrit as Record<string, unknown>).id;
        if (typeof critId === 'string') {
          if (seenCrit.has(critId)) {
            errors.push(`Duplicate criterion ID "${critId}".`);
          }
          seenCrit.add(critId);
        }
      }
    }
  }

  if (!validateTaskContract(rawCandidate)) {
    if (errors.length === 0) {
      errors.push('Contract does not match TaskContract schema v1 (check required fields and integer bounds).');
    }
    return {
      valid: false,
      operation: 'create',
      contract: null,
      expectedTaskDigest: null,
      errors,
      warnings: [],
      duplicates: attachDuplicateCoverage([], emptyCoverage),
      duplicateCoverage: emptyCoverage,
      cliCommand: null,
      nonDispatchNotice: NON_DISPATCH_NOTICE,
    };
  }

  const contract = rawCandidate as TaskContract;
  if (contract.schema_version !== 1) {
    errors.push('schema_version must equal 1.');
  }
  if (!isValidRustId(contract.project_id)) {
    errors.push('project_id must be 1–128 ASCII letters, digits, underscores, hyphens, or dots (not "." or "..").');
  }
  if (!isValidRustId(contract.root_id)) {
    errors.push('root_id must be 1–128 ASCII letters, digits, underscores, hyphens, or dots (not "." or "..").');
  }
  if (!isValidRustId(contract.task_id)) {
    errors.push('task_id must be 1–128 ASCII letters, digits, underscores, hyphens, or dots (not "." or "..").');
  }
  if (!contract.title.trim() || /\p{Cc}/u.test(contract.title)) {
    errors.push('title must be non-empty readable text without control characters.');
  }
  if (!contract.objective.trim() || /\p{Cc}/u.test(contract.objective)) {
    errors.push('objective must be non-empty readable text without control characters.');
  }
  if (!contract.authority_ref.trim() || /\p{Cc}/u.test(contract.authority_ref)) {
    errors.push('authority_ref is required to establish work-order provenance.');
  }
  if (contract.revision < 1) {
    errors.push('revision must be at least 1.');
  }
  if (contract.max_attempts < 1 || contract.max_attempts > 20) {
    errors.push('max_attempts must be between 1 and 20.');
  }
  if (contract.owned_paths.length === 0) {
    errors.push('At least one owned_paths entry is required for write-scope isolation.');
  }
  for (const path of contract.owned_paths) {
    if (!isSafeRelativePath(path)) {
      errors.push(`Invalid owned path "${path}": must be a safe relative path without ".." or leading "/".`);
    }
  }
  for (const path of contract.forbidden_paths) {
    if (!isSafeRelativePath(path)) {
      errors.push(`Invalid forbidden path "${path}": must be a safe relative path without ".." or leading "/".`);
    }
  }
  for (const path of contract.read_first) {
    if (!isSafeRelativePath(path)) {
      errors.push(`Invalid read_first path "${path}": must be a safe relative path.`);
    }
  }
  for (const owned of contract.owned_paths) {
    for (const forbidden of contract.forbidden_paths) {
      if (pathsOverlap(owned, forbidden)) {
        errors.push(`Write scope conflict: owned path "${owned}" overlaps forbidden path "${forbidden}".`);
      }
    }
  }
  const seenDeps = new Set<string>();
  for (const dep of contract.dependencies) {
    if (!isValidRustId(dep)) {
      errors.push(`Dependency ID "${dep}" must be a valid Admiralty TaskId.`);
    }
    if (seenDeps.has(dep)) {
      errors.push(`Duplicate dependency "${dep}".`);
    }
    seenDeps.add(dep);
  }
  if (contract.dependencies.includes(contract.task_id)) {
    errors.push(`Task "${contract.task_id}" cannot depend on itself.`);
  }
  if (contract.criteria.length === 0) {
    errors.push('At least one acceptance criterion is required.');
  }
  if (contract.checks.length === 0) {
    errors.push('At least one executable verification check is required.');
  }

  const checkIds = new Set<string>();
  for (const check of contract.checks) {
    if (!check.id.trim() || /\p{Cc}/u.test(check.id)) {
      errors.push(`Check ID "${check.id}" must be non-empty without control characters.`);
    }
    if (checkIds.has(check.id)) {
      errors.push(`Duplicate check ID "${check.id}".`);
    }
    checkIds.add(check.id);
    if (!check.program.trim() || check.program.includes('\0') || /\s/.test(check.program) || /[;|&$`<>]/.test(check.program)) {
      errors.push(`Check "${check.id}" program must be an argv executable name without shell metacharacters or NUL bytes.`);
    }
    if (!isSafeRelativePath(check.cwd)) {
      errors.push(`Invalid check cwd "${check.cwd}" on check "${check.id}": must be a safe relative path without ".." or leading "/".`);
    }
    if (check.args.some(arg => arg.includes('\0'))) {
      errors.push(`Check "${check.id}" args must not contain NUL bytes.`);
    }
    if (check.timeout_secs < 1 || check.timeout_secs > 86_400) {
      errors.push(`Check "${check.id}" timeout_secs must be between 1 and 86400 seconds.`);
    }
  }

  const criterionIds = new Set<string>();
  for (const criterion of contract.criteria) {
    if (!criterion.id.trim() || !criterion.text.trim()) {
      errors.push('Every acceptance criterion requires a non-empty id and text.');
    }
    if (criterionIds.has(criterion.id)) {
      errors.push(`Duplicate criterion ID "${criterion.id}".`);
    }
    criterionIds.add(criterion.id);
    if (criterion.check_ids.length === 0) {
      errors.push(`Criterion "${criterion.id}" must bind at least one verification check ID.`);
    }
    for (const refId of criterion.check_ids) {
      if (!checkIds.has(refId)) {
        errors.push(`Criterion "${criterion.id}" references undeclared check "${refId}".`);
      }
    }
  }

  const duplicates = findDuplicateCandidates(contract, existingTasks, maxInspect);
  if (duplicates.partial) {
    warnings.push(
      `Duplicate scan inspected ${duplicates.inspected} of ${duplicates.total} tasks (partial heuristic coverage).`,
    );
  }

  let operation: IntakeOperationKind = 'create';
  let expectedTaskDigest: string | null = null;
  const existingSameTask = existingTasks.find(t => t.contract.task_id === contract.task_id);

  if (!existingSameTask && options?.mode === 'revise') {
    errors.push(`Task "${contract.task_id}" does not exist in the current snapshot; cannot revise non-existent task.`);
  }

  for (const dup of duplicates) {
    if (dup.reason === 'same_task_id') {
      const existing = existingSameTask!;
      if (options?.mode === 'create') {
        errors.push(
          `Task "${dup.task_id}" already exists at revision ${dup.revision}; POST /v2/tasks cannot overwrite an existing task. Switch to CAS contract revision (POST /v2/tasks/lifecycle).`,
        );
      } else if (contract.revision <= dup.revision) {
        errors.push(
          `Task "${dup.task_id}" already exists at revision ${dup.revision} (${taskStateLabel(dup.state as Fleet['tasks'][number]['state'])}); revision must be ${dup.revision + 1}.`,
        );
      } else if (contract.revision !== dup.revision + 1) {
        errors.push(
          `Task "${dup.task_id}" is at revision ${dup.revision}; contract revision must increment by exactly +1 (expected revision ${dup.revision + 1}, got ${contract.revision}).`,
        );
      } else if (!canReviseTaskContract(existing, existingTasks)) {
        errors.push(
          `Task "${dup.task_id}" (${taskStateLabel(existing.state as Fleet['tasks'][number]['state'])}) is not eligible for contract revision; only unstarted, unreferenced ready/blocked tasks without attempts can be revised via CAS lifecycle.`,
        );
      } else if (
        contract.project_id !== existing.contract.project_id ||
        contract.root_id !== existing.contract.root_id ||
        contract.authority_ref !== existing.contract.authority_ref ||
        contract.simulated !== existing.contract.simulated
      ) {
        errors.push(
          `Contract revision for "${dup.task_id}" must preserve immutable project_id, root_id, authority_ref, and simulated fields.`,
        );
      } else {
        operation = 'revise_contract';
        expectedTaskDigest = computeTaskViewDigest(existing);
        warnings.push(
          `Supersedes existing unstarted task "${dup.task_id}" rev ${dup.revision} (${taskStateLabel(dup.state as Fleet['tasks'][number]['state'])}) via POST /v2/tasks/lifecycle (revise_contract, CAS ${expectedTaskDigest.slice(0, 19)}…); POST /v2/tasks cannot supersede existing task IDs.`,
        );
      }
    } else if (dup.reason === 'owned_path_overlap') {
      warnings.push(
        `Owned paths (${dup.overlapping_paths.join(', ')}) overlap active task "${dup.task_id}" (${taskStateLabel(dup.state as Fleet['tasks'][number]['state'])}).`,
      );
    } else {
      warnings.push(`Similar title to existing task "${dup.task_id}" ("${dup.title}").`);
    }
  }

  const valid = errors.length === 0;
  const safeTaskFile = `${contract.task_id}.rev${contract.revision}.json`;
  const cliCommand =
    valid && operation === 'create'
      ? `admiralty work submit --database <admiralty.sqlite> --file workplans/${safeTaskFile}`
      : null;

  return {
    valid,
    operation,
    contract: valid ? contract : contract,
    expectedTaskDigest,
    errors,
    warnings,
    duplicates,
    duplicateCoverage: {
      inspected: duplicates.inspected,
      total: duplicates.total,
      partial: duplicates.partial,
    },
    cliCommand,
    nonDispatchNotice: NON_DISPATCH_NOTICE,
  };
}

export function deriveCoordinationLedger(
  records: readonly (DomainRecord | AnyDomainRecord)[],
): ScopedMessageLedgerEntry[] {
  const queued = new Map<string, ScopedMessageLedgerEntry>();
  const acks = new Map<string, { seq: string; observed_utc: string }>();

  for (const record of records) {
    const event = record.event;
    if (!validateDomainEventV2(event)) continue;
    if (event.kind === 'message_queued' && validateCoordinationMessage(event.message)) {
      const key = `${event.message.message_id}\0${event.message.recipient}\0${event.message.context_epoch}`;
      const existingAck = acks.get(key);
      queued.set(key, {
        seq: record.seq,
        observed_utc: record.observed_utc,
        message: event.message,
        acknowledged: Boolean(existingAck),
        ack_seq: existingAck?.seq ?? null,
        ack_observed_utc: existingAck?.observed_utc ?? null,
      });
    } else if (event.kind === 'message_acknowledged') {
      const key = `${event.message_id}\0${event.recipient}\0${event.context_epoch}`;
      acks.set(key, { seq: record.seq, observed_utc: record.observed_utc });
      const entry = queued.get(key);
      if (entry && !entry.acknowledged) {
        entry.acknowledged = true;
        entry.ack_seq = record.seq;
        entry.ack_observed_utc = record.observed_utc;
      }
    }
  }

  return [...queued.values()].sort((left, right) => {
    const a = BigInt(left.seq);
    const b = BigInt(right.seq);
    return a < b ? -1 : a > b ? 1 : 0;
  });
}

function messageTaskId(message: CoordinationMessage): string | null {
  switch (message.payload.kind) {
    case 'task_assigned':
    case 'question':
    case 'artifact_ready':
    case 'handoff':
      return message.payload.task_id;
    case 'answer':
      return null;
  }
}

function describeLifecycleAction(action: LifecycleAction): {
  summary: string;
  detail: string;
  attempt_id: string | null;
  candidate_ref: string | null;
  decision_evidence: CandidateDecisionEvidence | null;
} {
  switch (action.action) {
    case 'decide_candidate':
      return {
        summary: `Candidate ${action.accepted ? 'accepted' : 'rejected'} by verifier ${action.evidence.verifier}`,
        detail: `Receipt: ${action.evidence.receipt_digest} · Commit: ${action.evidence.source_commit_sha} · Custody: ${action.evidence.custody_digest}`,
        attempt_id: action.attempt_id,
        candidate_ref: action.candidate_ref,
        decision_evidence: action.evidence,
      };
    case 'close_attempt':
      return {
        summary: `Attempt ${action.attempt_id} ${action.cancelled ? 'cancelled' : 'closed'}`,
        detail: `Reason: ${action.reason} · Closure: ${action.closure_digest}`,
        attempt_id: action.attempt_id,
        candidate_ref: null,
        decision_evidence: null,
      };
    case 'retry':
      return {
        summary: `Task retry authorized after attempt ${action.attempt_id}`,
        detail: `Closure digest: ${action.closure_digest}`,
        attempt_id: action.attempt_id,
        candidate_ref: null,
        decision_evidence: null,
      };
    case 'cancel_pending':
      return {
        summary: `Pending task cancelled`,
        detail: `Reason: ${action.reason}`,
        attempt_id: null,
        candidate_ref: null,
        decision_evidence: null,
      };
    case 'revise_contract':
      return {
        summary: `Contract revised to rev ${action.contract.revision}`,
        detail: `Authority: ${action.contract.authority_ref} · Title: ${action.contract.title}`,
        attempt_id: null,
        candidate_ref: null,
        decision_evidence: null,
      };
  }
}

export function deriveTaskDetail(
  task: AnyFleetTask,
  fleet: { projects: Fleet['projects']; tasks: readonly AnyFleetTask[] },
  records: readonly (DomainRecord | AnyDomainRecord)[],
): TaskDetailModel {
  const projectMap = new Map(fleet.projects.map(project => [project.id, project.name]));
  const tasksById = new Map(fleet.tasks.map(item => [item.contract.task_id, item]));

  const dependencies: DependencyResolution[] = task.contract.dependencies.map(depId => {
    const dep = tasksById.get(depId);
    if (!dep) {
      return { task_id: depId, found: false, state: 'missing', satisfied: false };
    }
    return {
      task_id: depId,
      found: true,
      state: dep.state,
      satisfied: dep.state === 'accepted',
    };
  });

  const checksById = new Map(task.contract.checks.map(check => [check.id, check]));
  const criteriaWithChecks = task.contract.criteria.map(criterion => {
    const checks: CheckSpec[] = [];
    const missingCheckIds: string[] = [];
    for (const checkId of criterion.check_ids) {
      const found = checksById.get(checkId);
      if (found) checks.push(found);
      else missingCheckIds.push(checkId);
    }
    return { criterion, checks, missingCheckIds };
  });

  const timeline: TaskEvidenceItem[] = [];
  const hasSnapshotLifecycle = task.lifecycle !== undefined && task.lifecycle !== null;
  let latestDecision: CandidateDecisionEvidence | null = hasSnapshotLifecycle
    ? task.lifecycle!.decision
    : null;
  let latestDecisionSource: LatestDecisionSource =
    hasSnapshotLifecycle && task.lifecycle!.decision ? 'snapshot' : 'none';

  for (const record of records) {
    const event = record.event;
    if (!validateDomainEventV2(event)) continue;
    if (event.kind === 'task_submitted' && event.contract.task_id === task.contract.task_id) {
      timeline.push({
        seq: record.seq,
        observed_utc: record.observed_utc,
        kind: event.kind,
        summary: `Task submitted (rev ${event.contract.revision})`,
        detail: `Authority: ${event.contract.authority_ref} · ${event.contract.checks.length} check(s)`,
        attempt_id: null,
        actor: event.contract.authority_ref,
        candidate_ref: null,
      });
    } else if (event.kind === 'task_claimed' && event.task_id === task.contract.task_id) {
      timeline.push({
        seq: record.seq,
        observed_utc: record.observed_utc,
        kind: event.kind,
        summary: `Task claimed by ${event.owner} (rev ${event.revision})`,
        detail: `Attempt ${event.attempt_id}`,
        attempt_id: event.attempt_id,
        actor: event.owner,
        candidate_ref: null,
      });
    } else if (event.kind === 'task_started' && event.task_id === task.contract.task_id) {
      timeline.push({
        seq: record.seq,
        observed_utc: record.observed_utc,
        kind: event.kind,
        summary: `Attempt ${event.attempt_id} started`,
        detail: `Execution observed for ${event.task_id}`,
        attempt_id: event.attempt_id,
        actor: task.owner,
        candidate_ref: null,
      });
    } else if (event.kind === 'candidate_submitted' && event.task_id === task.contract.task_id) {
      timeline.push({
        seq: record.seq,
        observed_utc: record.observed_utc,
        kind: event.kind,
        summary: `Candidate submitted for verification`,
        detail: `Attempt ${event.attempt_id} · Candidate ${event.candidate_ref}`,
        attempt_id: event.attempt_id,
        actor: task.owner,
        candidate_ref: event.candidate_ref,
      });
    } else if (event.kind === 'task_lifecycle' && event.command.task_id === task.contract.task_id) {
      const described = describeLifecycleAction(event.command.action);
      // Authoritative v2 snapshot lifecycle.decision always takes precedence over bounded replay history.
      // Only when snapshot lifecycle is absent (v1 projection) and the replayed decision binds the current
      // attempt_id + candidate_ref do we allow an explicitly marked incomplete historical fallback.
      if (
        !hasSnapshotLifecycle &&
        described.decision_evidence &&
        task.attempt_id !== null &&
        task.candidate_ref !== null &&
        described.attempt_id === task.attempt_id &&
        described.candidate_ref === task.candidate_ref
      ) {
        latestDecision = described.decision_evidence;
        latestDecisionSource = 'historical_incomplete';
      }
      timeline.push({
        seq: record.seq,
        observed_utc: record.observed_utc,
        kind: event.kind,
        summary: described.summary,
        detail: `${described.detail} · CAS ${event.command.expected_task_digest}`,
        attempt_id: described.attempt_id,
        actor: event.command.actor,
        candidate_ref: described.candidate_ref,
        decision_evidence: described.decision_evidence,
      });
    } else if (event.kind === 'message_queued' && messageTaskId(event.message) === task.contract.task_id) {
      timeline.push({
        seq: record.seq,
        observed_utc: record.observed_utc,
        kind: event.kind,
        summary: `Coordination ${event.message.payload.kind.replaceAll('_', ' ')} (${event.message.message_id})`,
        detail: `${event.message.sender} → ${event.message.recipient} · epoch ${event.message.context_epoch}`,
        attempt_id: null,
        actor: event.message.sender,
        candidate_ref: event.message.payload.kind === 'handoff' ? event.message.payload.candidate_ref : null,
      });
    }
  }

  const ledger = deriveCoordinationLedger(records);
  const questionIdsForTask = new Set(
    ledger
      .filter(entry => entry.message.payload.kind === 'question' && entry.message.payload.task_id === task.contract.task_id)
      .map(entry => entry.message.message_id),
  );
  const relatedMessages = ledger.filter(entry => {
    const directTask = messageTaskId(entry.message);
    if (directTask === task.contract.task_id) return true;
    if (entry.message.payload.kind === 'answer' && questionIdsForTask.has(entry.message.payload.question_id)) {
      return true;
    }
    return false;
  });

  return {
    task,
    projectName: projectMap.get(task.contract.project_id) ?? task.contract.project_id,
    taskDigest: computeTaskViewDigest(task),
    canCancelPending: canCancelPendingTask(task),
    canReviseContract: canReviseTaskContract(task, fleet.tasks),
    dependencies,
    criteriaWithChecks,
    timeline,
    relatedMessages,
    latestDecision,
    latestDecisionSource,
  };
}

export function deriveAttentionItems(
  fleet: { projects: Fleet['projects']; tasks: readonly AnyFleetTask[] },
  records: readonly (DomainRecord | AnyDomainRecord)[],
): AttentionItem[] {
  const items: AttentionItem[] = [];
  const tasksById = new Map(fleet.tasks.map(task => [task.contract.task_id, task]));

  for (const task of fleet.tasks) {
    if (task.state === 'failed') {
      items.push({
        id: `failed:${task.contract.task_id}:${task.contract.revision}`,
        severity: 'high',
        category: 'failed_task',
        project_id: task.contract.project_id,
        task_id: task.contract.task_id,
        owner: task.owner,
        title: `Failed task: ${task.contract.title}`,
        detail: task.lifecycle?.reason
          ? `Failure reason: ${task.lifecycle.reason}`
          : `Task ${task.contract.task_id} (rev ${task.contract.revision}) is in failed state.`,
        observed_utc: null,
        seq: null,
      });
    } else if (task.state === 'blocked') {
      const unmetDeps = task.contract.dependencies.filter(depId => tasksById.get(depId)?.state !== 'accepted');
      items.push({
        id: `blocked:${task.contract.task_id}:${task.contract.revision}`,
        severity: 'high',
        category: 'blocked_task',
        project_id: task.contract.project_id,
        task_id: task.contract.task_id,
        owner: task.owner,
        title: `Blocked task: ${task.contract.title}`,
        detail: unmetDeps.length
          ? `Waiting on dependencies: ${unmetDeps.join(', ')}`
          : `Task ${task.contract.task_id} (rev ${task.contract.revision}) is marked blocked.`,
        observed_utc: null,
        seq: null,
      });
    } else if (task.state === 'awaiting_verification') {
      items.push({
        id: `verify:${task.contract.task_id}:${task.contract.revision}`,
        severity: 'medium',
        category: 'awaiting_verification',
        project_id: task.contract.project_id,
        task_id: task.contract.task_id,
        owner: task.owner,
        title: `Awaiting independent verification: ${task.contract.title}`,
        detail: `Candidate ${task.candidate_ref ?? 'unrecorded'} (attempt ${task.attempt_id ?? 'unrecorded'}) requires clean-runner verification.`,
        observed_utc: null,
        seq: null,
      });
    } else if (task.state === 'ready' && !task.owner) {
      items.push({
        id: `ready:${task.contract.task_id}:${task.contract.revision}`,
        severity: 'low',
        category: 'unassigned_ready',
        project_id: task.contract.project_id,
        task_id: task.contract.task_id,
        owner: null,
        title: `Ready for claim: ${task.contract.title}`,
        detail: `Task ${task.contract.task_id} (rev ${task.contract.revision}) has no active owner.`,
        observed_utc: null,
        seq: null,
      });
    }
  }

  const ledger = deriveCoordinationLedger(records);
  const answeredQuestions = new Set<string>();
  for (const entry of ledger) {
    if (entry.message.payload.kind === 'answer') {
      answeredQuestions.add(entry.message.payload.question_id);
    }
  }

  for (const entry of ledger) {
    const { message } = entry;
    const payload = message.payload;
    if (payload.kind === 'question' && !answeredQuestions.has(message.message_id)) {
      items.push({
        id: `question:${message.message_id}`,
        severity: 'high',
        category: 'pending_question',
        project_id: message.project_id,
        task_id: payload.task_id,
        owner: message.recipient,
        title: `Unanswered question on ${payload.task_id}: ${payload.question}`,
        detail: `Requirement: ${payload.requirement} · Options: ${payload.options.join(' | ') || 'none'} · From ${message.sender} to ${message.recipient}`,
        observed_utc: entry.observed_utc,
        seq: entry.seq,
      });
    } else if (payload.kind === 'handoff' && payload.failed_check) {
      items.push({
        id: `handoff-fail:${message.message_id}`,
        severity: 'high',
        category: 'handoff_failed_check',
        project_id: message.project_id,
        task_id: payload.task_id,
        owner: message.recipient,
        title: `Handoff reported failed check "${payload.failed_check}" on ${payload.task_id}`,
        detail: `Candidate ${payload.candidate_ref} · Next action: ${payload.next_action} (${message.sender} → ${message.recipient})`,
        observed_utc: entry.observed_utc,
        seq: entry.seq,
      });
    } else if (!entry.acknowledged) {
      items.push({
        id: `unacked:${message.message_id}:${message.recipient}:${message.context_epoch}`,
        severity: 'medium',
        category: 'unacknowledged_message',
        project_id: message.project_id,
        task_id: messageTaskId(message),
        owner: message.recipient,
        title: `Unacknowledged ${payload.kind.replaceAll('_', ' ')} message (${message.message_id})`,
        detail: `${message.sender} → ${message.recipient} · epoch ${message.context_epoch}`,
        observed_utc: entry.observed_utc,
        seq: entry.seq,
      });
    }
  }

  const severityOrder: Record<AttentionSeverity, number> = { high: 0, medium: 1, low: 2 };
  return items.sort((left, right) => {
    const sev = severityOrder[left.severity] - severityOrder[right.severity];
    if (sev !== 0) return sev;
    return left.id.localeCompare(right.id);
  });
}

export function parseHealthStatus(value: unknown): HealthStatus {
  assertBoundedJson(value);
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('Unsupported health response');
  }
  const record = value as Record<string, unknown>;
  if (
    record.status !== 'ok' ||
    typeof record.version !== 'string' ||
    !record.version.trim() ||
    record.native_execution_qualified !== false
  ) {
    throw new Error('Unsupported health envelope');
  }
  return {
    status: 'ok',
    version: record.version,
    native_execution_qualified: false,
  };
}

export async function getHealthStatus(
  signal?: AbortSignal,
  fetcher: typeof fetch = fetch,
): Promise<HealthStatus> {
  const response = await fetcher('/healthz', { signal, cache: 'no-store' });
  if (!response.ok) {
    throw new ApiError(`Health check failed (${response.status})`, response.status);
  }
  return parseHealthStatus(await readBoundedJson(response));
}

const VALID_WAIT_REASONS = new Set<WaitReason>([
  'capacity_or_unavailable',
  'attempt_limit',
  'no_capable_model',
  'escalation_not_authorized',
  'escalation_exhausted',
  'pinned_model_unavailable',
  'setup_required',
  'needs_input',
]);

export function parseModelSelectionOutcome(value: unknown): ModelSelectionOutcome {
  assertBoundedJson(value);
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('Unsupported model selection outcome');
  }
  const outcome = value as Record<string, unknown>;
  if (outcome.status === 'selected') {
    const usage = outcome.usage_cost as { status?: unknown } | null;
    if (
      typeof outcome.model_id !== 'string' ||
      !outcome.model_id.trim() ||
      usage?.status !== 'unknown'
    ) {
      throw new Error('Invalid selected model outcome');
    }
    return {
      status: 'selected',
      model_id: outcome.model_id,
      usage_cost: { status: 'unknown' },
    };
  }
  if (outcome.status === 'waiting') {
    if (
      !(outcome.model_id === null || typeof outcome.model_id === 'string') ||
      !VALID_WAIT_REASONS.has(outcome.reason as WaitReason)
    ) {
      throw new Error('Invalid waiting model outcome');
    }
    return {
      status: 'waiting',
      model_id: outcome.model_id as string | null,
      reason: outcome.reason as WaitReason,
    };
  }
  throw new Error('Unknown model selection outcome status');
}

export async function selectModelDryRun(
  request: ModelSelectionRequest,
  capacity: ModelCapacitySnapshot,
  token?: string,
  signal?: AbortSignal,
  fetcher: typeof fetch = fetch,
): Promise<ModelSelectionOutcome> {
  const body = JSON.stringify({ request, capacity });
  const response = await fetcher('/v1/models/select', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
    body,
    signal,
  });
  if (!response.ok) {
    let detail = '';
    try {
      const parsed = (await readBoundedJson(response)) as { detail?: unknown; error?: unknown };
      if (typeof parsed.detail === 'string') detail = `: ${parsed.detail}`;
      else if (typeof parsed.error === 'string') detail = `: ${parsed.error}`;
    } catch {
      /* bounded fallback */
    }
    throw new ApiError(`Model dry-run selection failed (${response.status})${detail}`, response.status);
  }
  return parseModelSelectionOutcome(await readBoundedJson(response));
}

function attachReplayMetadata(
  records: AnyDomainRecord[],
  meta: {
    partial: boolean;
    status: EventReplayStatus;
    coveredThroughSeq: string;
    targetSeq: string;
    maxEvents: number;
  },
): ReplayedDomainRecords {
  return Object.assign(records, meta);
}

export async function replayEventsToCursor(
  cursor: Cursor,
  token?: string,
  signal?: AbortSignal,
  fetcher: typeof fetch = fetch,
  maxEvents = 200,
  endpoint: '/v1/events' | '/v2/events' = '/v1/events',
  timeoutMs = 5_000,
): Promise<ReplayedDomainRecords> {
  if (signal?.aborted) {
    throw new Error('Event replay aborted');
  }
  if (!decimal(cursor.event_seq) || BigInt(cursor.event_seq) === 0n || maxEvents <= 0) {
    return attachReplayMetadata([], {
      partial: false,
      status: 'complete',
      coveredThroughSeq: '0',
      targetSeq: decimal(cursor.event_seq) ? cursor.event_seq : '0',
      maxEvents,
    });
  }
  const targetSeq = BigInt(cursor.event_seq);
  const controller = new AbortController();
  const onAbort = () => controller.abort();
  signal?.addEventListener('abort', onAbort, { once: true });

  const records: AnyDomainRecord[] = [];
  let lastSeq = '0';
  let reachedTarget = false;
  let truncatedByCap = false;
  let stopped = false;
  let timedOut = false;

  const timer = setTimeout(() => {
    timedOut = true;
    stopped = true;
    controller.abort();
  }, Math.max(1, timeoutMs));

  function raceDeadline<T>(promise: Promise<T>): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const onSignalAbort = () => {
        if (timedOut) {
          reject(
            new EventReplayError(
              `Event replay timed out after ${timeoutMs}ms before reaching cursor #${cursor.event_seq}`,
              'unavailable',
              records,
              lastSeq,
              cursor.event_seq,
            ),
          );
        } else {
          reject(new Error('Event replay aborted'));
        }
      };
      if (controller.signal.aborted) {
        onSignalAbort();
        return;
      }
      controller.signal.addEventListener('abort', onSignalAbort, { once: true });
      promise.then(
        value => {
          controller.signal.removeEventListener('abort', onSignalAbort);
          resolve(value);
        },
        err => {
          controller.signal.removeEventListener('abort', onSignalAbort);
          reject(err);
        },
      );
    });
  }

  try {
    const query = new URLSearchParams({
      after: '0',
      generation: cursor.recovery_generation,
    });
    const response = await raceDeadline(
      fetcher(`${endpoint}?${query}`, {
        headers: {
          Accept: 'text/event-stream',
          ...(token ? { Authorization: `Bearer ${token}` } : {}),
        },
        signal: controller.signal,
        cache: 'no-store',
      }),
    );
    if (!response.ok) {
      throw new ApiError(`Event replay failed (${response.status})`, response.status);
    }
    if (!response.headers.get('content-type')?.includes('text/event-stream') || !response.body) {
      throw new Error('Unsupported event stream response');
    }

    const parser = createSseParser(frame => {
      if (stopped) return;
      if (frame.event === 'snapshot_required') {
        throw new Error('Snapshot required during event replay');
      }
      if (frame.event === 'error') {
        throw new Error('Event stream reported an error');
      }
      if (frame.event !== 'domain') return;
      const incoming = cursorFromId(frame.id);
      const record = parseDomainRecordV2(JSON.parse(frame.data));
      if (
        incoming.event_seq !== record.seq ||
        incoming.recovery_generation !== cursor.recovery_generation
      ) {
        throw new Error('Event replay generation or sequence mismatch');
      }
      const seqNum = BigInt(record.seq);
      // Never include records after targetSeq.
      if (seqNum > targetSeq) {
        reachedTarget = true;
        stopped = true;
        controller.abort();
        return;
      }
      // Enforce maxEvents cap strictly inside the frame callback.
      if (records.length >= maxEvents) {
        truncatedByCap = true;
        stopped = true;
        controller.abort();
        return;
      }
      records.push(record);
      lastSeq = record.seq;
      if (seqNum === targetSeq) {
        reachedTarget = true;
        stopped = true;
        controller.abort();
      } else if (records.length >= maxEvents) {
        truncatedByCap = true;
        stopped = true;
        controller.abort();
      }
    });

    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    try {
      while (!stopped && !controller.signal.aborted) {
        const chunk = await raceDeadline(reader.read());
        if (chunk.done) {
          if (!stopped) {
            parser.push(decoder.decode());
          }
          break;
        }
        parser.push(decoder.decode(chunk.value, { stream: true }));
      }
    } finally {
      await reader.cancel().catch(() => undefined);
      reader.releaseLock();
    }

    if (timedOut) {
      throw new EventReplayError(
        `Event replay timed out after ${timeoutMs}ms before reaching cursor #${cursor.event_seq}`,
        'unavailable',
        records,
        lastSeq,
        cursor.event_seq,
      );
    }
    if (signal?.aborted) {
      throw new Error('Event replay aborted');
    }
    if (!reachedTarget && !truncatedByCap) {
      throw new EventReplayError(
        `Event replay truncated before snapshot cursor (early EOF at seq #${lastSeq} of #${cursor.event_seq})`,
        'partial_eof',
        records,
        lastSeq,
        cursor.event_seq,
      );
    }

    return attachReplayMetadata(records, {
      partial: truncatedByCap,
      status: truncatedByCap ? 'partial_cap' : 'complete',
      coveredThroughSeq: lastSeq,
      targetSeq: cursor.event_seq,
      maxEvents,
    });
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', onAbort);
    controller.abort();
  }
}

export type RetainedCommandEnvelope =
  | {
      kind: 'create';
      fingerprint: string;
      request_id: string;
      request: SubmitTaskRequest;
    }
  | {
      kind: 'cancel_pending' | 'revise_contract';
      fingerprint: string;
      request_id: string;
      request: SubmitLifecycleRequest;
    };

export async function executeIntakeCommand(params: {
  retainedRef: { current: RetainedCommandEnvelope | null };
  preview: IntakePreview;
  effectiveCasDigest: string | null;
  onPinCasDigest?: (digest: string) => void;
  token?: string;
  fetcher?: typeof fetch;
  onSubmitTask?: (request: SubmitTaskRequest) => Promise<CommandReceipt>;
  onReviseContract?: (request: SubmitLifecycleRequest) => Promise<CommandReceipt>;
  onRefreshSnapshot?: () => void | Promise<void>;
}): Promise<CommandReceipt> {
  let envelope = params.retainedRef.current;
  if (!envelope) {
    if (!params.preview.valid || !params.preview.contract) {
      throw new Error(params.preview.errors[0] ?? 'Contract preview is invalid');
    }
    if (params.preview.operation === 'revise_contract') {
      const casDigest = params.effectiveCasDigest ?? params.preview.expectedTaskDigest;
      if (!casDigest) {
        throw new Error('Missing expected_task_digest for CAS contract revision');
      }
      params.onPinCasDigest?.(casDigest);
      const req: SubmitLifecycleRequest = {
        request_id: generateCanonicalRequestId(),
        task_id: params.preview.contract.task_id,
        expected_task_digest: casDigest,
        action: { action: 'revise_contract', contract: params.preview.contract },
      };
      const fingerprint = JSON.stringify({
        op: 'revise_contract',
        task_id: req.task_id,
        expected_task_digest: casDigest,
        contract: serializeContractFragment(params.preview.contract),
      });
      envelope = {
        kind: 'revise_contract',
        fingerprint,
        request_id: req.request_id,
        request: req,
      };
    } else {
      const req: SubmitTaskRequest = {
        request_id: generateCanonicalRequestId(),
        contract: params.preview.contract,
      };
      const fingerprint = JSON.stringify({
        op: 'create',
        contract: serializeContractFragment(params.preview.contract),
      });
      envelope = {
        kind: 'create',
        fingerprint,
        request_id: req.request_id,
        request: req,
      };
    }
    params.retainedRef.current = envelope;
  }

  // Dispatch the retained envelope directly without re-evaluating current snapshot eligibility or CAS.
  const receipt =
    envelope.kind === 'create'
      ? params.onSubmitTask
        ? await params.onSubmitTask(envelope.request)
        : await submitNativeTask(envelope.request, params.token, undefined, params.fetcher)
      : params.onReviseContract
        ? await params.onReviseContract(envelope.request)
        : await submitNativeLifecycle(envelope.request, params.token, undefined, params.fetcher);

  params.retainedRef.current = null;
  try {
    await params.onRefreshSnapshot?.();
  } catch {
    /* post-commit refresh failure must not present a persisted command as unpersisted */
  }
  return receipt;
}

export async function executeTaskDetailLifecycleCommand(params: {
  kind: 'cancel_pending' | 'revise_contract';
  retainedRef: { current: RetainedCommandEnvelope | null };
  detail: TaskDetailModel | null;
  boundDigest: string | null;
  cancelReason?: string;
  reviseTitle?: string;
  reviseObjective?: string;
  token?: string;
  fetcher?: typeof fetch;
  onCancelPendingTask?: (request: SubmitLifecycleRequest) => Promise<CommandReceipt>;
  onReviseContract?: (request: SubmitLifecycleRequest) => Promise<CommandReceipt>;
  onRefreshSnapshot?: () => void | Promise<void>;
}): Promise<CommandReceipt> {
  let envelope = params.retainedRef.current;
  if (!envelope || envelope.kind !== params.kind) {
    if (!params.detail) {
      throw new Error('Task detail is unavailable');
    }
    const { task } = params.detail;
    const expectedDigest = params.boundDigest ?? params.detail.taskDigest;
    if (params.kind === 'cancel_pending') {
      if (!params.detail.canCancelPending) {
        throw new Error(`Task "${task.contract.task_id}" is not eligible for pending cancellation`);
      }
      const reason = (params.cancelReason ?? '').trim();
      if (!reason) {
        throw new Error('Pending cancellation reason is required');
      }
      const req: SubmitLifecycleRequest = {
        request_id: generateCanonicalRequestId(),
        task_id: task.contract.task_id,
        expected_task_digest: expectedDigest,
        action: { action: 'cancel_pending', reason },
      };
      const fingerprint = JSON.stringify({
        op: 'cancel_pending',
        task_id: req.task_id,
        expected_task_digest: expectedDigest,
        reason,
      });
      envelope = {
        kind: 'cancel_pending',
        fingerprint,
        request_id: req.request_id,
        request: req,
      };
    } else {
      if (!params.detail.canReviseContract) {
        throw new Error(`Task "${task.contract.task_id}" is not eligible for contract revision`);
      }
      const nextContract: TaskContract = {
        ...task.contract,
        revision: task.contract.revision + 1,
        title: (params.reviseTitle ?? '').trim() || task.contract.title,
        objective: (params.reviseObjective ?? '').trim() || task.contract.objective,
      };
      const req: SubmitLifecycleRequest = {
        request_id: generateCanonicalRequestId(),
        task_id: task.contract.task_id,
        expected_task_digest: expectedDigest,
        action: { action: 'revise_contract', contract: nextContract },
      };
      const fingerprint = JSON.stringify({
        op: 'revise_contract',
        task_id: req.task_id,
        expected_task_digest: expectedDigest,
        contract: serializeContractFragment(nextContract),
      });
      envelope = {
        kind: 'revise_contract',
        fingerprint,
        request_id: req.request_id,
        request: req,
      };
    }
    params.retainedRef.current = envelope;
  }

  // Dispatch the retained lifecycle envelope directly without re-checking new snapshot eligibility or recomputing revision/CAS.
  const request = envelope.request as SubmitLifecycleRequest;
  const receipt =
    envelope.kind === 'cancel_pending'
      ? params.onCancelPendingTask
        ? await params.onCancelPendingTask(request)
        : await submitNativeLifecycle(request, params.token, undefined, params.fetcher)
      : params.onReviseContract
        ? await params.onReviseContract(request)
        : await submitNativeLifecycle(request, params.token, undefined, params.fetcher);

  params.retainedRef.current = null;
  try {
    await params.onRefreshSnapshot?.();
  } catch {
    /* post-commit refresh failure must not present a persisted command as unpersisted */
  }
  return receipt;
}

export function IntakeJourney({
  fleet,
  initialDraft,
  initialRetainedEnvelope = null,
  token,
  fetcher = fetch,
  onSubmitTask,
  onReviseContract,
  onRefreshSnapshot,
  onAccess,
}: {
  fleet: AnyFleet | Fleet | null;
  initialDraft?: IntakeDraft;
  initialRetainedEnvelope?: RetainedCommandEnvelope | null;
  token?: string;
  fetcher?: typeof fetch;
  onSubmitTask?: (request: SubmitTaskRequest) => Promise<CommandReceipt>;
  onReviseContract?: (request: SubmitLifecycleRequest) => Promise<CommandReceipt>;
  onRefreshSnapshot?: () => void | Promise<void>;
  onAccess?: () => void;
}) {
  const defaultProject = fleet?.projects[0]?.id ?? 'admiralty';
  const [draft, setDraft] = useState<IntakeDraft>(() => initialDraft ?? emptyIntakeDraft(defaultProject));
  const [jsonMode, setJsonMode] = useState(false);
  const [rawJson, setRawJson] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState('');
  const [submitReceipt, setSubmitReceipt] = useState<CommandReceipt | null>(null);
  const [pinnedCasDigest, setPinnedCasDigest] = useState<string | null>(null);
  const retainedSubmitRef = useRef<RetainedCommandEnvelope | null>(initialRetainedEnvelope);
  const [retainedSubmit, setRetainedSubmit] = useState<RetainedCommandEnvelope | null>(initialRetainedEnvelope);

  const preview = useMemo(() => {
    if (jsonMode) {
      try {
        return buildIntakePreview(JSON.parse(rawJson), fleet?.tasks ?? []);
      } catch (error) {
        const emptyCoverage: DuplicateCoverage = {
          inspected: Math.min(fleet?.tasks.length ?? 0, 500),
          total: fleet?.tasks.length ?? 0,
          partial: (fleet?.tasks.length ?? 0) > 500,
        };
        return {
          valid: false,
          operation: 'create',
          contract: null,
          expectedTaskDigest: null,
          errors: [error instanceof Error ? `Invalid JSON: ${error.message}` : 'Invalid JSON'],
          warnings: [],
          duplicates: attachDuplicateCoverage([], emptyCoverage),
          duplicateCoverage: emptyCoverage,
          cliCommand: null,
          nonDispatchNotice: NON_DISPATCH_NOTICE,
        } satisfies IntakePreview;
      }
    }
    return buildIntakePreview(draft, fleet?.tasks ?? []);
  }, [draft, jsonMode, rawJson, fleet]);

  // Keep pinned CAS digest stable once bound unless the user changes target task_id or explicitly rebinds.
  const effectiveCasDigest = pinnedCasDigest ?? preview.expectedTaskDigest;

  function clearRetainedSubmit() {
    retainedSubmitRef.current = null;
    setRetainedSubmit(null);
  }

  function updateField<K extends keyof IntakeDraft>(key: K, value: IntakeDraft[K]) {
    if (key === 'task_id') {
      setPinnedCasDigest(null);
    }
    clearRetainedSubmit();
    setSubmitError('');
    setSubmitReceipt(null);
    setDraft(current => ({ ...current, [key]: value }));
  }

  async function handleLiveSubmit() {
    if (submitting) return;
    if (!retainedSubmitRef.current && (!preview.valid || !preview.contract)) return;
    setSubmitting(true);
    setSubmitError('');

    try {
      const receipt = await executeIntakeCommand({
        retainedRef: retainedSubmitRef,
        preview,
        effectiveCasDigest,
        onPinCasDigest: setPinnedCasDigest,
        token,
        fetcher,
        onSubmitTask,
        onReviseContract,
        onRefreshSnapshot,
      });
      setRetainedSubmit(null);
      setSubmitReceipt(receipt);
    } catch (error) {
      setRetainedSubmit(retainedSubmitRef.current);
      setSubmitReceipt(null);
      setSubmitError(error instanceof Error ? error.message : 'Operator command failed');
    } finally {
      setSubmitting(false);
    }
  }

  const revisableTasks = useMemo(
    () => (fleet?.tasks ?? []).filter(t => canReviseTaskContract(t, fleet?.tasks ?? [])),
    [fleet],
  );

  return (
    <div className="models-settings" data-journey="intake">
      <div className="models-notice">
        <b>Live work intake & CAS contract revision · non-dispatching admission boundary</b>
        <p>{preview.nonDispatchNotice}</p>
      </div>

      <section className="panel models-panel" aria-labelledby="intake-form-heading">
        <div className="panel-head">
          <div>
            <div className="eyebrow">Work order intake and contract validation</div>
            <h2 id="intake-form-heading">Author or import TaskContract</h2>
          </div>
          <div className="heading-meta">
            {revisableTasks.length > 0 && (
              <label>
                Load unstarted task for +1 revision{' '}
                <select
                  aria-label="Load unstarted task for +1 revision"
                  value=""
                  onChange={event => {
                    const found = revisableTasks.find(t => t.contract.task_id === event.target.value);
                    if (found) {
                      clearRetainedSubmit();
                      setPinnedCasDigest(computeTaskViewDigest(found));
                      setDraft(draftFromTaskForRevision(found));
                      setJsonMode(false);
                      setSubmitError('');
                      setSubmitReceipt(null);
                    }
                  }}
                >
                  <option value="">Select ready/blocked task…</option>
                  {revisableTasks.map(item => (
                    <option key={item.contract.task_id} value={item.contract.task_id}>
                      {item.contract.task_id} (rev {item.contract.revision} → {item.contract.revision + 1})
                    </option>
                  ))}
                </select>
              </label>
            )}
            <button
              type="button"
              className="secondary-button"
              onClick={() => {
                if (!jsonMode && preview.contract) {
                  setRawJson(JSON.stringify(preview.contract, null, 2));
                }
                setJsonMode(current => !current);
              }}
            >
              {jsonMode ? 'Switch to structured fields' : 'Switch to JSON import'}
            </button>
          </div>
        </div>

        {jsonMode ? (
          <div className="models-form">
            <label style={{ gridColumn: '1 / -1' }}>
              TaskContract JSON (schema_version 1)
              <textarea
                aria-label="TaskContract JSON"
                rows={10}
                value={rawJson}
                onChange={event => {
                  clearRetainedSubmit();
                  setSubmitError('');
                  setSubmitReceipt(null);
                  setRawJson(event.target.value);
                }}
              />
              <small>Validated with bounded JSON limits, Rust ID rules, and generated TaskContract schema guards.</small>
            </label>
          </div>
        ) : (
          <fieldset className="models-form">
            <label>
              Project ID
              <input
                value={draft.project_id}
                onChange={event => updateField('project_id', event.target.value)}
                placeholder="admiralty"
              />
            </label>
            <label>
              Root ID
              <input
                value={draft.root_id}
                onChange={event => updateField('root_id', event.target.value)}
                placeholder="root-main"
              />
            </label>
            <label>
              Task ID
              <input
                value={draft.task_id}
                onChange={event => updateField('task_id', event.target.value)}
                placeholder="task-1"
              />
            </label>
            <label>
              Revision
              <input
                type="number"
                min="1"
                step="1"
                value={draft.revision}
                onChange={event => updateField('revision', event.target.value)}
              />
            </label>
            <label>
              Title
              <input
                value={draft.title}
                onChange={event => updateField('title', event.target.value)}
                placeholder="Task title"
              />
            </label>
            <label>
              Authority reference
              <input
                value={draft.authority_ref}
                onChange={event => updateField('authority_ref', event.target.value)}
                placeholder="operator:intake"
              />
            </label>
            <label>
              Objective
              <input
                value={draft.objective}
                onChange={event => updateField('objective', event.target.value)}
                placeholder="Measurable task objective"
              />
            </label>
            <label>
              Owned paths (comma-separated)
              <input
                value={draft.owned_paths}
                onChange={event => updateField('owned_paths', event.target.value)}
                placeholder="apps/dashboard/src/ui/CommandJourneys.tsx"
              />
            </label>
            <label>
              Forbidden paths (comma-separated)
              <input
                value={draft.forbidden_paths}
                onChange={event => updateField('forbidden_paths', event.target.value)}
                placeholder="crates/admiralty-core/src/events.rs"
              />
            </label>
            <label>
              Read first (comma-separated)
              <input
                value={draft.read_first}
                onChange={event => updateField('read_first', event.target.value)}
              />
            </label>
            <label>
              Dependencies (comma-separated Task IDs)
              <input
                value={draft.dependencies}
                onChange={event => updateField('dependencies', event.target.value)}
                placeholder="task-0"
              />
            </label>
            <label>
              Max attempts (1–20)
              <input
                type="number"
                min="1"
                max="20"
                step="1"
                value={draft.max_attempts}
                onChange={event => updateField('max_attempts', event.target.value)}
              />
            </label>
            <label>
              Criterion ID
              <input
                value={draft.criterion_id}
                onChange={event => updateField('criterion_id', event.target.value)}
              />
            </label>
            <label>
              Criterion text
              <input
                value={draft.criterion_text}
                onChange={event => updateField('criterion_text', event.target.value)}
              />
            </label>
            <label>
              Verification check ID
              <input
                value={draft.check_id}
                onChange={event => updateField('check_id', event.target.value)}
              />
            </label>
            <label>
              Check program (argv[0])
              <input
                value={draft.check_program}
                onChange={event => updateField('check_program', event.target.value)}
              />
            </label>
            <label>
              Check args (JSON string array or comma-separated, order & duplicates preserved)
              <input
                value={draft.check_args}
                onChange={event => updateField('check_args', event.target.value)}
              />
            </label>
            <label>
              Check working directory (relative path)
              <input
                value={draft.check_cwd}
                onChange={event => updateField('check_cwd', event.target.value)}
                placeholder="."
              />
            </label>
            <label>
              Check timeout (seconds)
              <input
                type="number"
                min="1"
                max="86400"
                value={draft.check_timeout_secs}
                onChange={event => updateField('check_timeout_secs', event.target.value)}
              />
            </label>
          </fieldset>
        )}

        <div className="models-form-actions" style={{ marginTop: 12 }}>
          <button
            type="button"
            className="primary-button"
            disabled={submitting || (!retainedSubmit && !preview.valid)}
            onClick={() => void handleLiveSubmit()}
          >
            {submitting
              ? 'Submitting…'
              : retainedSubmit
                ? `Retry retained ${retainedSubmit.kind === 'revise_contract' ? '+1 contract revision' : 'task submission'} (${retainedSubmit.request_id.slice(0, 8)}…)`
                : preview.operation === 'revise_contract'
                  ? `Submit +1 contract revision (POST /v2/tasks/lifecycle)`
                  : `Submit pending task (POST /v2/tasks)`}
          </button>
          {retainedSubmit && (
            <button
              type="button"
              className="secondary-button"
              disabled={submitting}
              onClick={() => {
                clearRetainedSubmit();
                setSubmitError('');
              }}
            >
              Discard retained request
            </button>
          )}
          {(preview.operation === 'revise_contract' || retainedSubmit?.kind === 'revise_contract') && effectiveCasDigest && (
            <span>
              CAS digest: <code>{effectiveCasDigest}</code>
              {preview.expectedTaskDigest && preview.expectedTaskDigest !== effectiveCasDigest && (
                <button
                  type="button"
                  className="text-button"
                  onClick={() => {
                    setPinnedCasDigest(preview.expectedTaskDigest);
                    clearRetainedSubmit();
                    setSubmitError('');
                  }}
                >
                  Rebind latest CAS
                </button>
              )}
            </span>
          )}
          {retainedSubmit && (
            <span>
              Idempotency request_id: <code>{retainedSubmit.request_id}</code>
            </span>
          )}
        </div>

        {submitError && (
          <div className="error-banner" role="alert" style={{ marginTop: 12 }}>
            <span>Command rejected (draft & request_id preserved)</span>
            <span>{submitError}</span>
            {onAccess && (
              <button type="button" className="text-button" onClick={onAccess}>
                Update bearer token
              </button>
            )}
            {onRefreshSnapshot && (
              <button
                type="button"
                className="text-button"
                onClick={() => void onRefreshSnapshot()}
              >
                Refresh snapshot
              </button>
            )}
          </div>
        )}

        {submitReceipt && (
          <div className="models-status" role="status" style={{ marginTop: 12 }}>
            <strong>
              {submitReceipt.duplicate
                ? 'Idempotent replay acknowledged (duplicate: true)'
                : 'Durable operator command persisted'}:
            </strong>{' '}
            event_seq <code>#{submitReceipt.event_seq}</code> · source_contiguous_seq{' '}
            <code>{submitReceipt.source_contiguous_seq}</code> · execution_granted:{' '}
            <code>false</code> · acceptance_granted: <code>false</code>
          </div>
        )}
      </section>

      <section className="panel models-panel" aria-labelledby="intake-preview-heading">
        <div className="panel-head">
          <div>
            <div className="eyebrow">Admission preview and duplicate check</div>
            <h2 id="intake-preview-heading">
              {!preview.valid
                ? 'Admission blocked by validation errors'
                : preview.operation === 'revise_contract'
                  ? 'Ready for CAS contract revision (POST /v2/tasks/lifecycle)'
                  : 'Ready for pending task submission (POST /v2/tasks)'}
            </h2>
          </div>
          <span className={`state-pill ${preview.valid ? 'ready' : 'blocked'}`}>
            {preview.valid
              ? preview.operation === 'revise_contract'
                ? 'valid +1 revision'
                : 'valid new contract'
              : `${preview.errors.length} error(s)`}
          </span>
        </div>

        {preview.errors.length > 0 && (
          <div className="error-banner" role="alert">
            <span>Validation errors</span>
            <span>{preview.errors.join(' · ')}</span>
          </div>
        )}

        {preview.warnings.length > 0 && (
          <div className="simulation-note" role="status">
            <strong>Admission warnings:</strong> {preview.warnings.join(' · ')}
          </div>
        )}

        <div className="table-scroll">
          <table>
            <thead>
              <tr>
                <th>Duplicate / overlap candidate</th>
                <th>Revision</th>
                <th>Current state</th>
                <th>Reason</th>
                <th>Overlapping owned paths</th>
              </tr>
            </thead>
            <tbody>
              {preview.duplicates.length === 0 && (
                <tr>
                  <td colSpan={5} className="muted">
                    No duplicate or overlapping candidates detected.
                  </td>
                </tr>
              )}
              {preview.duplicates.map(dup => (
                <tr key={`${dup.task_id}:${dup.revision}:${dup.reason}`}>
                  <td>
                    <strong>{dup.title}</strong>
                    <small>{dup.task_id}</small>
                  </td>
                  <td>rev {dup.revision}</td>
                  <td>
                    <span className={`state-pill ${dup.state}`}>
                      {taskStateLabel(dup.state as Fleet['tasks'][number]['state'])}
                    </span>
                  </td>
                  <td>{dup.reason.replaceAll('_', ' ')}</td>
                  <td>{dup.overlapping_paths.join(', ') || 'None'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        {preview.duplicates.length === 0 && (
          <p>
            {preview.duplicateCoverage.partial ? (
              <>
                Partial duplicate scan: no conflicts in the first{' '}
                <strong>{preview.duplicateCoverage.inspected}</strong> of{' '}
                <strong>{preview.duplicateCoverage.total}</strong> snapshot task(s) (heuristic scan truncated at{' '}
                {preview.duplicateCoverage.inspected}; full duplicate coverage not declared).
              </>
            ) : (
              <>
                No duplicate task IDs, overlapping active owned paths, or similar titles detected across{' '}
                {preview.duplicateCoverage.inspected} of {preview.duplicateCoverage.total} snapshot task(s) (partial:{' '}
                <code>false</code>).
              </>
            )}
          </p>
        )}

        {preview.cliCommand && (
          <p>
            Offline CLI registration command: <code>{preview.cliCommand}</code>
          </p>
        )}
      </section>
    </div>
  );
}

export function TaskDetailEvidenceJourney({
  fleet,
  records,
  replayCoverage,
  selectedTaskId,
  initialRetainedEnvelope = null,
  onSelectTask,
  token,
  fetcher = fetch,
  onCancelPendingTask,
  onReviseContract,
  onRefreshSnapshot,
  onOpenReviseInIntake,
  onAccess,
}: {
  fleet: AnyFleet | Fleet | null;
  records: readonly (DomainRecord | AnyDomainRecord)[];
  replayCoverage?: ReplayCoverageState;
  selectedTaskId?: string;
  initialRetainedEnvelope?: RetainedCommandEnvelope | null;
  onSelectTask?: (taskId: string) => void;
  token?: string;
  fetcher?: typeof fetch;
  onCancelPendingTask?: (request: SubmitLifecycleRequest) => Promise<CommandReceipt>;
  onReviseContract?: (request: SubmitLifecycleRequest) => Promise<CommandReceipt>;
  onRefreshSnapshot?: () => void | Promise<void>;
  onOpenReviseInIntake?: (task: AnyFleetTask) => void;
  onAccess?: () => void;
}) {
  const [internalTaskId, setInternalTaskId] = useState<string>('');
  const [cancelReason, setCancelReason] = useState('Operator cancelled pending work order before claim.');
  const [reviseTitle, setReviseTitle] = useState('');
  const [reviseObjective, setReviseObjective] = useState('');
  const [pinnedTaskDigest, setPinnedTaskDigest] = useState<string | null>(null);
  const [lifecycleBusy, setLifecycleBusy] = useState(false);
  const [lifecycleError, setLifecycleError] = useState('');
  const [lifecycleReceipt, setLifecycleReceipt] = useState<CommandReceipt | null>(null);
  const retainedLifecycleRef = useRef<RetainedCommandEnvelope | null>(initialRetainedEnvelope);
  const [retainedLifecycle, setRetainedLifecycle] = useState<RetainedCommandEnvelope | null>(initialRetainedEnvelope);
  const lastTaskIdRef = useRef<string | undefined>(undefined);

  const activeTaskId = selectedTaskId ?? internalTaskId;

  const currentTask = useMemo(() => {
    if (!fleet?.tasks.length) return null;
    return fleet.tasks.find(item => item.contract.task_id === activeTaskId) ?? fleet.tasks[0];
  }, [fleet, activeTaskId]);

  const detail = useMemo(() => {
    if (!fleet || !currentTask) return null;
    return deriveTaskDetail(currentTask, fleet, records);
  }, [fleet, currentTask, records]);

  function clearRetainedLifecycle() {
    retainedLifecycleRef.current = null;
    setRetainedLifecycle(null);
  }

  useEffect(() => {
    if (currentTask && currentTask.contract.task_id !== lastTaskIdRef.current) {
      const switchedTask = lastTaskIdRef.current !== undefined;
      lastTaskIdRef.current = currentTask.contract.task_id;
      setReviseTitle(currentTask.contract.title);
      setReviseObjective(currentTask.contract.objective);
      setPinnedTaskDigest(computeTaskViewDigest(currentTask));
      if (switchedTask) {
        clearRetainedLifecycle();
        setLifecycleError('');
        setLifecycleReceipt(null);
      }
    }
  }, [currentTask?.contract.task_id]);

  if (!fleet) {
    return <div className="loading-card">Awaiting fleet snapshot to inspect task details and evidence…</div>;
  }

  if (!detail) {
    return (
      <section className="panel workplans-panel" data-journey="detail">
        <div className="panel-head">
          <div>
            <div className="eyebrow">Task detail and evidence</div>
            <h2>No tasks in current snapshot</h2>
          </div>
          <span className="tiny-badge">0 tasks</span>
        </div>
        <div className="empty-state">
          <span>⌁</span>The current fleet snapshot contains no task contracts.
        </div>
      </section>
    );
  }

  const { task } = detail;
  const boundDigest = pinnedTaskDigest ?? detail.taskDigest;

  async function handleCancelPending() {
    if (lifecycleBusy) return;
    if (retainedLifecycleRef.current?.kind !== 'cancel_pending' && (!detail || !detail.canCancelPending)) {
      return;
    }
    setLifecycleBusy(true);
    setLifecycleError('');
    try {
      const receipt = await executeTaskDetailLifecycleCommand({
        kind: 'cancel_pending',
        retainedRef: retainedLifecycleRef,
        detail,
        boundDigest,
        cancelReason,
        token,
        fetcher,
        onCancelPendingTask,
        onReviseContract,
        onRefreshSnapshot,
      });
      setRetainedLifecycle(null);
      setLifecycleReceipt(receipt);
    } catch (error) {
      setRetainedLifecycle(retainedLifecycleRef.current);
      setLifecycleReceipt(null);
      setLifecycleError(error instanceof Error ? error.message : 'Pending cancellation failed');
    } finally {
      setLifecycleBusy(false);
    }
  }

  async function handleQuickRevise() {
    if (lifecycleBusy) return;
    if (retainedLifecycleRef.current?.kind !== 'revise_contract' && (!detail || !detail.canReviseContract)) {
      return;
    }
    setLifecycleBusy(true);
    setLifecycleError('');
    try {
      const receipt = await executeTaskDetailLifecycleCommand({
        kind: 'revise_contract',
        retainedRef: retainedLifecycleRef,
        detail,
        boundDigest,
        reviseTitle,
        reviseObjective,
        token,
        fetcher,
        onCancelPendingTask,
        onReviseContract,
        onRefreshSnapshot,
      });
      setRetainedLifecycle(null);
      setLifecycleReceipt(receipt);
    } catch (error) {
      setRetainedLifecycle(retainedLifecycleRef.current);
      setLifecycleReceipt(null);
      setLifecycleError(error instanceof Error ? error.message : 'Contract revision failed');
    } finally {
      setLifecycleBusy(false);
    }
  }

  return (
    <div className="models-settings" data-journey="detail">
      {replayCoverage && replayCoverage.status !== 'complete' && (
        <div className="error-banner" role="alert">
          <span>
            {replayCoverage.status === 'unavailable'
              ? 'Event history unavailable'
              : 'Partial event replay coverage'}
          </span>
          <span>
            {replayCoverage.detail ??
              `Covered ${replayCoverage.coveredCount} event(s) through seq #${replayCoverage.coveredThroughSeq} of snapshot cursor #${replayCoverage.targetSeq}.`}
          </span>
        </div>
      )}

      <section className="panel models-panel" aria-labelledby="task-detail-heading">
        <div className="panel-head">
          <div>
            <div className="eyebrow">
              TASK DETAIL / {detail.projectName.toUpperCase()} · {task.contract.task_id} (REV {task.contract.revision})
            </div>
            <h2 id="task-detail-heading">{task.contract.title}</h2>
          </div>
          <span className={`state-pill ${task.state}`}>
            {taskStateLabel(task.state as Fleet['tasks'][number]['state'])}
          </span>
        </div>

        <div className="table-toolbar">
          <label>
            Select task{' '}
            <select
              aria-label="Select task to inspect"
              value={task.contract.task_id}
              onChange={event => {
                setInternalTaskId(event.target.value);
                onSelectTask?.(event.target.value);
              }}
            >
              {fleet.tasks.map(row => (
                <option key={`${row.contract.task_id}:${row.contract.revision}`} value={row.contract.task_id}>
                  {row.contract.task_id} (rev {row.contract.revision}) — {row.contract.title}
                </option>
              ))}
            </select>
          </label>
          <span>
            Authority: <code>{task.contract.authority_ref}</code> · Root: <code>{task.contract.root_id}</code> · Max attempts:{' '}
            <strong>{task.contract.max_attempts}</strong> · CAS: <code>{detail.taskDigest.slice(0, 23)}…</code>
          </span>
        </div>

        <p>{task.contract.objective}</p>

        <div className="coord-stats">
          <div>
            <span className="stat-label">Current owner</span>
            <strong>{task.owner ?? 'Unassigned'}</strong>
            <small>Principal ID in snapshot</small>
          </div>
          <div>
            <span className="stat-label">Active attempt</span>
            <strong>{task.attempt_id ?? 'None'}</strong>
            <small>Attempt lease identifier</small>
          </div>
          <div>
            <span className="stat-label">Candidate reference</span>
            <strong>{task.candidate_ref ?? 'None'}</strong>
            <small>Immutable candidate digest/ref</small>
          </div>
          <div>
            <span className="stat-label">Native execution</span>
            <strong>{fleet.native_execution_qualified ? 'Qualified' : 'Not qualified'}</strong>
            <small>{task.contract.simulated ? 'Synthetic task contract' : 'Production task contract'}</small>
          </div>
        </div>

        {detail.latestDecision && (
          <div className="models-status" role="status">
            <strong>
              {detail.latestDecisionSource === 'snapshot'
                ? 'Trusted verifier decision evidence (authoritative snapshot):'
                : 'Historical replay decision (incomplete fallback tied to current attempt/candidate):'}
            </strong>{' '}
            Verifier <code>{detail.latestDecision.verifier}</code> · Receipt{' '}
            <code>{detail.latestDecision.receipt_digest}</code> · Custody{' '}
            <code>{detail.latestDecision.custody_digest}</code> · Commit{' '}
            <code>{detail.latestDecision.source_commit_sha}</code>
          </div>
        )}

        <div className="models-chain" aria-label="Task scope paths">
          {task.contract.owned_paths.map(path => (
            <span key={`own:${path}`}>
              <b>OWNED</b> {path}
            </span>
          ))}
          {task.contract.forbidden_paths.map(path => (
            <span key={`forbid:${path}`}>
              <b>FORBIDDEN</b> {path}
            </span>
          ))}
          {task.contract.read_first.map(path => (
            <span key={`read:${path}`}>
              <b>READ FIRST</b> {path}
            </span>
          ))}
        </div>

        {detail.dependencies.length > 0 && (
          <p>
            Dependencies:{' '}
            {detail.dependencies
              .map(dep => `${dep.task_id} (${dep.found ? taskStateLabel(dep.state as Fleet['tasks'][number]['state']) : 'missing'})`)
              .join(', ')}
          </p>
        )}
      </section>

      <section className="panel models-panel" aria-labelledby="task-lifecycle-controls-heading">
        <div className="panel-head">
          <div>
            <div className="eyebrow">Pending task lifecycle commands (POST /v2/tasks/lifecycle)</div>
            <h2 id="task-lifecycle-controls-heading">CAS-guarded pending cancellation & +1 contract revision</h2>
          </div>
          <span className={`state-pill ${detail.canCancelPending ? 'ready' : 'blocked'}`}>
            {detail.canCancelPending ? 'pending eligible' : 'pending lifecycle locked'}
          </span>
        </div>

        <p>
          Bound source TaskView CAS digest: <code>{boundDigest}</code>
          {boundDigest !== detail.taskDigest && (
            <>
              {' '}
              (Snapshot updated to <code>{detail.taskDigest}</code> ·{' '}
              <button
                type="button"
                className="text-button"
                onClick={() => {
                  setPinnedTaskDigest(detail.taskDigest);
                  clearRetainedLifecycle();
                  setLifecycleError('');
                }}
              >
                Rebind CAS to latest snapshot
              </button>
              )
            </>
          )}
        </p>

        {retainedLifecycle && (
          <div className="models-form-actions" style={{ marginBottom: 12 }}>
            <span>
              Retained idempotency request_id: <code>{retainedLifecycle.request_id}</code>
            </span>
            <button
              type="button"
              className="primary-button"
              disabled={lifecycleBusy}
              onClick={() =>
                void (retainedLifecycle.kind === 'cancel_pending'
                  ? handleCancelPending()
                  : handleQuickRevise())
              }
            >
              {lifecycleBusy
                ? 'Submitting…'
                : `Retry retained ${retainedLifecycle.kind === 'cancel_pending' ? 'pending cancellation' : 'contract revision'} (${retainedLifecycle.request_id.slice(0, 8)}…)`}
            </button>
            <button
              type="button"
              className="secondary-button"
              disabled={lifecycleBusy}
              onClick={() => {
                clearRetainedLifecycle();
                setLifecycleError('');
              }}
            >
              Discard retained request
            </button>
          </div>
        )}

        {lifecycleError && (
          <div className="error-banner" role="alert">
            <span>Lifecycle command rejected (draft & CAS preserved)</span>
            <span>{lifecycleError}</span>
            {onAccess && (
              <button type="button" className="text-button" onClick={onAccess}>
                Update bearer token
              </button>
            )}
            {onRefreshSnapshot && (
              <button
                type="button"
                className="text-button"
                onClick={() => void onRefreshSnapshot()}
              >
                Refresh snapshot
              </button>
            )}
          </div>
        )}

        {lifecycleReceipt && (
          <div className="models-status" role="status">
            <strong>
              {lifecycleReceipt.duplicate
                ? 'Idempotent lifecycle replay acknowledged (duplicate: true)'
                : 'Lifecycle transition persisted'}:
            </strong>{' '}
            event_seq <code>#{lifecycleReceipt.event_seq}</code> · execution_granted:{' '}
            <code>false</code> · acceptance_granted: <code>false</code>
          </div>
        )}

        {detail.canCancelPending || retainedLifecycle ? (
          <div className="models-form">
            <label>
              Pending cancellation reason (1–512 printable chars)
              <input
                aria-label="Pending cancellation reason"
                value={cancelReason}
                onChange={event => {
                  clearRetainedLifecycle();
                  setLifecycleError('');
                  setCancelReason(event.target.value);
                }}
              />
            </label>
            <div className="models-form-actions">
              <button
                type="button"
                className="secondary-button"
                disabled={
                  lifecycleBusy ||
                  (retainedLifecycle?.kind !== 'cancel_pending' &&
                    (!detail.canCancelPending || !cancelReason.trim()))
                }
                onClick={() => void handleCancelPending()}
              >
                {lifecycleBusy ? 'Submitting…' : 'Cancel pending task (POST /v2/tasks/lifecycle)'}
              </button>
            </div>
            {detail.canReviseContract || retainedLifecycle?.kind === 'revise_contract' ? (
              <>
                <label>
                  Revise title (rev {task.contract.revision} → rev {task.contract.revision + 1})
                  <input
                    aria-label="Revise contract title"
                    value={reviseTitle}
                    onChange={event => {
                      clearRetainedLifecycle();
                      setLifecycleError('');
                      setReviseTitle(event.target.value);
                    }}
                  />
                </label>
                <label>
                  Revise objective (rev {task.contract.revision} → rev {task.contract.revision + 1})
                  <input
                    aria-label="Revise contract objective"
                    value={reviseObjective}
                    onChange={event => {
                      clearRetainedLifecycle();
                      setLifecycleError('');
                      setReviseObjective(event.target.value);
                    }}
                  />
                </label>
                <div className="models-form-actions">
                  <button
                    type="button"
                    className="primary-button"
                    disabled={
                      lifecycleBusy ||
                      (retainedLifecycle?.kind !== 'revise_contract' &&
                        (!detail.canReviseContract || !reviseTitle.trim() || !reviseObjective.trim()))
                    }
                    onClick={() => void handleQuickRevise()}
                  >
                    {lifecycleBusy
                      ? 'Submitting…'
                      : `Submit rev ${task.contract.revision + 1} contract revision`}
                  </button>
                  {onOpenReviseInIntake && (
                    <button
                      type="button"
                      className="secondary-button"
                      onClick={() => onOpenReviseInIntake(task)}
                    >
                      Edit full rev {task.contract.revision + 1} contract in Intake
                    </button>
                  )}
                </div>
              </>
            ) : (
              <p>
                Contract revision unavailable: task has recorded attempt history or dependent task references.
              </p>
            )}
          </div>
        ) : (
          <p>
            Pending cancellation and contract revision are disabled for{' '}
            <strong>{taskStateLabel(task.state as Fleet['tasks'][number]['state'])}</strong> tasks
            {task.attempt_id ? ` (active attempt ${task.attempt_id})` : ''}. Operator HTTP endpoints never expose fake accept, retry, or worker launch controls.
          </p>
        )}
      </section>

      <section className="panel models-panel" aria-labelledby="task-criteria-heading">
        <div className="panel-head">
          <div>
            <div className="eyebrow">Acceptance criteria and verification checks</div>
            <h2 id="task-criteria-heading">Contract verification bindings</h2>
          </div>
          <span className="tiny-badge">{detail.criteriaWithChecks.length} criteria</span>
        </div>
        <div className="table-scroll">
          <table>
            <thead>
              <tr>
                <th>Criterion</th>
                <th>Bound check ID</th>
                <th>Executable command (argv)</th>
                <th>Working dir</th>
                <th>Timeout</th>
              </tr>
            </thead>
            <tbody>
              {detail.criteriaWithChecks.map(({ criterion, checks, missingCheckIds }) =>
                checks.length > 0 ? (
                  checks.map(check => (
                    <tr key={`${criterion.id}:${check.id}`}>
                      <td>
                        <strong>{criterion.id}</strong>
                        <small>{criterion.text}</small>
                      </td>
                      <td>
                        <code>{check.id}</code>
                      </td>
                      <td>
                        <code>{[check.program, ...check.args].join(' ')}</code>
                      </td>
                      <td>
                        <code>{check.cwd}</code>
                      </td>
                      <td>{check.timeout_secs}s</td>
                    </tr>
                  ))
                ) : (
                  <tr key={criterion.id}>
                    <td>
                      <strong>{criterion.id}</strong>
                      <small>{criterion.text}</small>
                    </td>
                    <td colSpan={4}>Missing check definition(s): {missingCheckIds.join(', ') || 'none'}</td>
                  </tr>
                ),
              )}
            </tbody>
          </table>
        </div>
      </section>

      <section className="panel models-panel" aria-labelledby="task-evidence-heading">
        <div className="panel-head">
          <div>
            <div className="eyebrow">Durable event replay and evidence timeline</div>
            <h2 id="task-evidence-heading">Observed task lifecycle & coordination events</h2>
          </div>
          <span className="tiny-badge">{detail.timeline.length} events</span>
        </div>
        {detail.timeline.length > 0 ? (
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>Seq</th>
                  <th>Observed UTC</th>
                  <th>Event kind</th>
                  <th>Summary</th>
                  <th>Attempt / Candidate</th>
                </tr>
              </thead>
              <tbody>
                {detail.timeline.map(item => (
                  <tr key={`${item.seq}:${item.kind}`}>
                    <td>
                      <code>#{item.seq}</code>
                    </td>
                    <td>{item.observed_utc}</td>
                    <td>
                      <span className="state-pill">{item.kind.replaceAll('_', ' ')}</span>
                    </td>
                    <td>
                      <strong>{item.summary}</strong>
                      <small>{item.detail}</small>
                    </td>
                    <td>
                      {item.candidate_ref ?? item.attempt_id ?? '—'}
                      {item.actor && <small>Actor: {item.actor}</small>}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <div className="empty-state">
            <span>⌁</span>
            {replayCoverage?.status === 'unavailable'
              ? `Event history unavailable (${replayCoverage.detail ?? 'replay failed'}); timeline cannot be shown.`
              : `No replayed domain events matched task ${task.contract.task_id} in the current stream window.`}
          </div>
        )}
      </section>
    </div>
  );
}

export function AttentionCoordinationJourney({
  fleet,
  records,
  replayCoverage,
  onSelectTask,
}: {
  fleet: AnyFleet | Fleet | null;
  records: readonly (DomainRecord | AnyDomainRecord)[];
  replayCoverage?: ReplayCoverageState;
  onSelectTask?: (taskId: string) => void;
}) {
  const [severityFilter, setSeverityFilter] = useState<AttentionSeverity | ''>('');
  const [projectFilter, setProjectFilter] = useState<string>('');

  const attentionItems = useMemo(() => {
    if (!fleet) return [];
    return deriveAttentionItems(fleet, records);
  }, [fleet, records]);

  const filteredAttention = useMemo(
    () =>
      attentionItems.filter(
        item =>
          (!severityFilter || item.severity === severityFilter) &&
          (!projectFilter || item.project_id === projectFilter),
      ),
    [attentionItems, severityFilter, projectFilter],
  );

  const ledger = useMemo(() => deriveCoordinationLedger(records), [records]);

  if (!fleet) {
    return <div className="loading-card">Awaiting fleet snapshot for attention and coordination triage…</div>;
  }

  return (
    <div className="models-settings" data-journey="attention">
      {replayCoverage && replayCoverage.status !== 'complete' && (
        <div className="error-banner" role="alert">
          <span>
            {replayCoverage.status === 'unavailable'
              ? 'Event history unavailable'
              : 'Partial event replay coverage'}
          </span>
          <span>
            {replayCoverage.detail ??
              `Covered ${replayCoverage.coveredCount} event(s) through seq #${replayCoverage.coveredThroughSeq} of snapshot cursor #${replayCoverage.targetSeq}.`}
          </span>
        </div>
      )}

      <section className="panel activity-panel" aria-labelledby="attention-pulse-heading">
        <div className="panel-head">
          <div>
            <div className="eyebrow">Coordination and attention pulse</div>
            <h2 id="attention-pulse-heading">Durable coordination summary</h2>
          </div>
          <span className="tiny-badge">{attentionItems.length} attention items</span>
        </div>
        <div className="coord-stats">
          <div>
            <span className="stat-label">Needs attention</span>
            <strong>{attentionItems.length}</strong>
            <small>Blocked, verifying, questions & unacked</small>
          </div>
          <div>
            <span className="stat-label">Queued messages</span>
            <strong>{fleet.coordination.queued_messages}</strong>
            <small>Recorded in fleet snapshot</small>
          </div>
          <div>
            <span className="stat-label">Acknowledged</span>
            <strong>{fleet.coordination.acknowledged_messages}</strong>
            <small>Application ACKs recorded</small>
          </div>
          <div>
            <span className="stat-label">Measured tokens</span>
            <strong>{formatUnknown(fleet.coordination.measured_tokens)}</strong>
            <small>Routine turns: {fleet.coordination.routine_model_turns}</small>
          </div>
        </div>
      </section>

      <section className="panel models-panel" aria-labelledby="attention-queue-heading">
        <div className="panel-head">
          <div>
            <div className="eyebrow">Needs attention: operator triage</div>
            <h2 id="attention-queue-heading">Prioritized blockers, verification gates & pending questions</h2>
          </div>
          <div className="heading-meta">
            <label>
              Severity{' '}
              <select
                aria-label="Filter attention by severity"
                value={severityFilter}
                onChange={event => setSeverityFilter(event.target.value as AttentionSeverity | '')}
              >
                <option value="">All severities</option>
                <option value="high">High</option>
                <option value="medium">Medium</option>
                <option value="low">Low</option>
              </select>
            </label>
            <label>
              Project{' '}
              <select
                aria-label="Filter attention by project"
                value={projectFilter}
                onChange={event => setProjectFilter(event.target.value)}
              >
                <option value="">All projects</option>
                {fleet.projects.map(project => (
                  <option key={project.id} value={project.id}>
                    {project.name}
                  </option>
                ))}
              </select>
            </label>
          </div>
        </div>

        {filteredAttention.length > 0 ? (
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>Severity</th>
                  <th>Category</th>
                  <th>Project / Task</th>
                  <th>Owner / Recipient</th>
                  <th>Cause & detail</th>
                </tr>
              </thead>
              <tbody>
                {filteredAttention.map(item => (
                  <tr key={item.id}>
                    <td>
                      <span
                        className={`state-pill ${
                          item.severity === 'high'
                            ? 'blocked'
                            : item.severity === 'medium'
                              ? 'awaiting_verification'
                              : 'ready'
                        }`}
                      >
                        {item.severity}
                      </span>
                    </td>
                    <td>{item.category.replaceAll('_', ' ')}</td>
                    <td>
                      <strong>{item.project_id}</strong>
                      {item.task_id && (
                        <small>
                          {onSelectTask ? (
                            <button
                              type="button"
                              className="text-button"
                              onClick={() => onSelectTask(item.task_id!)}
                            >
                              Inspect {item.task_id}
                            </button>
                          ) : (
                            item.task_id
                          )}
                        </small>
                      )}
                    </td>
                    <td>{item.owner ?? <span className="muted">Unassigned</span>}</td>
                    <td>
                      <strong>{item.title}</strong>
                      <small>
                        {item.detail}
                        {item.seq ? ` · seq #${item.seq}` : ''}
                      </small>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <div className="empty-state">
            <span>⌁</span>No attention items match the selected filters.
          </div>
        )}
      </section>

      <section className="panel models-panel" aria-labelledby="coord-ledger-heading">
        <div className="panel-head">
          <div>
            <div className="eyebrow">Scoped coordination ledger</div>
            <h2 id="coord-ledger-heading">Replayed messages & application acknowledgements</h2>
          </div>
          <span className="tiny-badge">{ledger.length} messages</span>
        </div>
        {ledger.length > 0 ? (
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>Seq</th>
                  <th>Message ID</th>
                  <th>Project / Epoch</th>
                  <th>Sender → Recipient</th>
                  <th>Payload</th>
                  <th>Application ACK</th>
                </tr>
              </thead>
              <tbody>
                {ledger.map(entry => (
                  <tr key={`${entry.seq}:${entry.message.message_id}`}>
                    <td>
                      <code>#{entry.seq}</code>
                    </td>
                    <td>
                      <strong>{entry.message.message_id}</strong>
                      <small>{entry.observed_utc}</small>
                    </td>
                    <td>
                      {entry.message.project_id}
                      <small>epoch: {entry.message.context_epoch}</small>
                    </td>
                    <td>
                      {entry.message.sender} → <strong>{entry.message.recipient}</strong>
                    </td>
                    <td>
                      <span className="state-pill">{entry.message.payload.kind.replaceAll('_', ' ')}</span>
                      <small>Authority: {entry.message.authority_ref}</small>
                    </td>
                    <td>
                      {entry.acknowledged ? (
                        <span className="state-pill ready">ACK #{entry.ack_seq}</span>
                      ) : (
                        <span className="state-pill blocked">Pending ACK</span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <div className="empty-state">
            <span>⌁</span>
            {replayCoverage?.status === 'unavailable'
              ? `Event history unavailable (${replayCoverage.detail ?? 'replay failed'}); coordination ledger cannot be shown.`
              : 'No coordination messages replayed in the current event window.'}
          </div>
        )}
      </section>
    </div>
  );
}

export function OperatorActionsJourney({
  fleet,
  health,
  token,
  fetcher = fetch,
  onRefreshSnapshot,
  onAccess,
}: {
  fleet: AnyFleet | Fleet | null;
  health: HealthStatus | null;
  token?: string;
  fetcher?: typeof fetch;
  onRefreshSnapshot?: () => void;
  onAccess?: () => void;
}) {
  const [capability, setCapability] = useState('coding');
  const [currentModel, setCurrentModel] = useState('');
  const [pinnedModel, setPinnedModel] = useState('');
  const [lastFailure, setLastFailure] = useState<FailureReason | ''>('');
  const [allowEscalation, setAllowEscalation] = useState(true);
  const [allowCapacityFallback, setAllowCapacityFallback] = useState(true);
  const [outcome, setOutcome] = useState<ModelSelectionOutcome | null>(null);
  const [probeError, setProbeError] = useState('');
  const [probing, setProbing] = useState(false);

  const totals = useMemo(() => (fleet ? fleetTotals(fleet as Fleet) : null), [fleet]);

  async function runDrySelect() {
    if (probing) return;
    setProbing(true);
    setProbeError('');
    setOutcome(null);
    try {
      const result = await selectModelDryRun(
        {
          required_capabilities: splitList(capability),
          pinned_model: pinnedModel.trim() || null,
          current_model: currentModel.trim() || null,
          attempts_by_tier: {},
          last_failure: lastFailure || null,
          allow_escalation: allowEscalation,
          allow_capacity_fallback: allowCapacityFallback,
        },
        { in_use: {}, unavailable: [] },
        token,
        undefined,
        fetcher,
      );
      setOutcome(result);
    } catch (error) {
      setProbeError(error instanceof Error ? error.message : 'Model selection dry-run failed');
    } finally {
      setProbing(false);
    }
  }

  return (
    <div className="models-settings" data-journey="actions">
      <div className="models-notice">
        <b>Operator actions & dry-run diagnostics · read-only routing probe</b>
        <p>
          Server endpoints inspect durable projections and evaluate model routing policy without starting native workers or fabricating acceptance.
        </p>
      </div>

      <section className="panel models-panel" aria-labelledby="actions-health-heading">
        <div className="panel-head">
          <div>
            <div className="eyebrow">Live server health and snapshot cursor</div>
            <h2 id="actions-health-heading">Control plane status</h2>
          </div>
          {onRefreshSnapshot && (
            <button type="button" className="secondary-button" onClick={onRefreshSnapshot}>
              Refresh snapshot & replay
            </button>
          )}
        </div>
        <div className="coord-stats">
          <div>
            <span className="stat-label">/healthz status</span>
            <strong>{health ? `${health.status} (v${health.version})` : 'Unknown'}</strong>
            <small>Native execution: {health?.native_execution_qualified ? 'qualified' : 'false'}</small>
          </div>
          <div>
            <span className="stat-label">Recovery generation</span>
            <strong>{fleet?.cursor.recovery_generation ?? 'Unknown'}</strong>
            <small>SQLite {fleet?.sqlite_version ?? 'unknown'} · Schema v{fleet?.schema_version ?? 1}</small>
          </div>
          <div>
            <span className="stat-label">Durable event seq</span>
            <strong>{fleet?.cursor.event_seq ?? '0'}</strong>
            <small>{fleet?.simulated ? 'Simulated mode' : 'Authenticated snapshot'}</small>
          </div>
          <div>
            <span className="stat-label">Tracked tasks</span>
            <strong>{totals?.tasks ?? 0}</strong>
            <small>{totals?.running ?? 0} running · {totals?.blocked ?? 0} blocked · {totals?.accepted ?? 0} accepted</small>
          </div>
        </div>
      </section>

      <section className="panel models-panel" aria-labelledby="actions-select-heading">
        <div className="panel-head">
          <div>
            <div className="eyebrow">Dry-run routing probe (POST /v1/models/select)</div>
            <h2 id="actions-select-heading">Evaluate model selection & fallback policy</h2>
          </div>
          <span className="tiny-badge">Pure policy check</span>
        </div>

        {probeError && (
          <div className="error-banner" role="alert">
            <span>Dry-run probe error</span>
            <span>{probeError}</span>
            {onAccess && (
              <button type="button" className="text-button" onClick={onAccess}>
                Update session access
              </button>
            )}
          </div>
        )}

        {outcome && (
          <div className="models-status" role="status">
            {outcome.status === 'selected' ? (
              <>
                Selected model: <strong>{outcome.model_id}</strong> · Usage cost: {outcome.usage_cost.status} (no provider launched)
              </>
            ) : (
              <>
                Waiting: reason <strong>{outcome.reason.replaceAll('_', ' ')}</strong>
                {outcome.model_id ? ` (model ${outcome.model_id})` : ''}
              </>
            )}
          </div>
        )}

        <form
          onSubmit={event => {
            event.preventDefault();
            void runDrySelect();
          }}
        >
          <fieldset disabled={probing} className="models-form">
            <label>
              Required capabilities
              <input
                value={capability}
                onChange={event => setCapability(event.target.value)}
                placeholder="coding"
              />
            </label>
            <label>
              Current model (optional)
              <input
                value={currentModel}
                onChange={event => setCurrentModel(event.target.value)}
                placeholder="ornith"
              />
            </label>
            <label>
              Pinned model (optional)
              <input
                value={pinnedModel}
                onChange={event => setPinnedModel(event.target.value)}
                placeholder="Leave blank for tier routing"
              />
            </label>
            <label>
              Simulated failure reason
              <select
                aria-label="Simulated failure reason"
                value={lastFailure}
                onChange={event => setLastFailure(event.target.value as FailureReason | '')}
              >
                <option value="">None (initial selection)</option>
                <option value="failure">Generic failure</option>
                <option value="test_fail">Verification test failure</option>
                <option value="context_exhausted">Context exhausted</option>
                <option value="provider_unavailable">Provider unavailable</option>
                <option value="setup_error">Setup error (blocks escalation)</option>
                <option value="blocked_input">Blocked on operator input</option>
              </select>
            </label>
            <label className="models-enabled">
              <input
                type="checkbox"
                checked={allowEscalation}
                onChange={event => setAllowEscalation(event.target.checked)}
              />
              Allow tier escalation
            </label>
            <label className="models-enabled">
              <input
                type="checkbox"
                checked={allowCapacityFallback}
                onChange={event => setAllowCapacityFallback(event.target.checked)}
              />
              Allow capacity fallback
            </label>
            <div className="models-form-actions">
              <button type="submit" className="primary-button">
                {probing ? 'Evaluating…' : 'Run dry-run selection'}
              </button>
              <span>Calls POST /v1/models/select without reserving slots or starting workers.</span>
            </div>
          </fieldset>
        </form>
      </section>
    </div>
  );
}

export function CommandJourneys({
  fleet: externalFleet,
  records: externalRecords,
  replayCoverage: externalReplayCoverage,
  health: externalHealth,
  initialTab = 'intake',
  initialTaskId,
  token,
  fetcher = fetch,
  onSubmitTask,
  onCancelPendingTask,
  onReviseContract,
  onRefreshSnapshot,
  onAccess,
}: {
  fleet?: AnyFleet | Fleet | null;
  records?: readonly (DomainRecord | AnyDomainRecord)[];
  replayCoverage?: ReplayCoverageState;
  health?: HealthStatus | null;
  initialTab?: CommandJourneyTab;
  initialTaskId?: string;
  token?: string;
  fetcher?: typeof fetch;
  onSubmitTask?: (request: SubmitTaskRequest) => Promise<CommandReceipt>;
  onCancelPendingTask?: (request: SubmitLifecycleRequest) => Promise<CommandReceipt>;
  onReviseContract?: (request: SubmitLifecycleRequest) => Promise<CommandReceipt>;
  onRefreshSnapshot?: () => void | Promise<void>;
  onAccess?: () => void;
}) {
  const [tab, setTab] = useState<CommandJourneyTab>(initialTab);
  const [selectedTaskId, setSelectedTaskId] = useState<string | undefined>(initialTaskId);
  const [intakeInitialDraft, setIntakeInitialDraft] = useState<IntakeDraft | undefined>(undefined);
  const [fetchedFleet, setFetchedFleet] = useState<AnyFleet | null>(null);
  const [fetchedRecords, setFetchedRecords] = useState<AnyDomainRecord[]>([]);
  const [fetchedReplayCoverage, setFetchedReplayCoverage] = useState<ReplayCoverageState | undefined>(undefined);
  const [fetchedHealth, setFetchedHealth] = useState<HealthStatus | null>(null);
  const [error, setError] = useState('');
  const [connection, setConnection] = useState<ConnectionState>('connecting');
  const [reloadCount, setReloadCount] = useState(0);

  const fleet = externalFleet !== undefined ? externalFleet : fetchedFleet;
  const records = externalRecords !== undefined ? externalRecords : fetchedRecords;
  const replayCoverage = externalReplayCoverage !== undefined ? externalReplayCoverage : fetchedReplayCoverage;
  const health = externalHealth !== undefined ? externalHealth : fetchedHealth;

  const triggerRefresh = async () => {
    await onRefreshSnapshot?.();
    setReloadCount(count => count + 1);
  };

  useEffect(() => {
    if (externalFleet !== undefined && externalRecords !== undefined && externalHealth !== undefined) {
      setConnection('connected');
      return;
    }
    const controller = new AbortController();
    setConnection('connecting');
    setError('');

    void (async () => {
      try {
        const [nextFleet, nextHealth] = await Promise.all([
          externalFleet !== undefined
            ? Promise.resolve(externalFleet)
            : getCommandFleet(token, controller.signal, fetcher),
          externalHealth !== undefined
            ? Promise.resolve(externalHealth)
            : getHealthStatus(controller.signal, fetcher).catch(() => null),
        ]);
        if (controller.signal.aborted) return;
        if (externalFleet === undefined) setFetchedFleet(nextFleet);
        if (externalHealth === undefined) setFetchedHealth(nextHealth);
        if (externalRecords === undefined && nextFleet) {
          const eventEndpoint = nextFleet.schema_version === 2 ? '/v2/events' : '/v1/events';
          try {
            const replayed = await replayEventsToCursor(
              nextFleet.cursor,
              token,
              controller.signal,
              fetcher,
              200,
              eventEndpoint,
            );
            if (controller.signal.aborted) return;
            setFetchedRecords(replayed);
            setFetchedReplayCoverage({
              status: replayed.status,
              coveredCount: replayed.length,
              coveredThroughSeq: replayed.coveredThroughSeq,
              targetSeq: replayed.targetSeq,
              maxEvents: replayed.maxEvents,
              detail: replayed.partial
                ? `Partial event history: bounded replay stopped at ${replayed.length} events (seq #${replayed.coveredThroughSeq} of #${replayed.targetSeq}).`
                : null,
            });
            setConnection(replayed.partial ? 'reconnecting' : 'connected');
          } catch (replayErr) {
            if (controller.signal.aborted) return;
            if (replayErr instanceof EventReplayError) {
              setFetchedRecords(replayErr.records);
              setFetchedReplayCoverage({
                status: replayErr.status,
                coveredCount: replayErr.records.length,
                coveredThroughSeq: replayErr.coveredThroughSeq,
                targetSeq: replayErr.targetSeq,
                maxEvents: 200,
                detail: replayErr.message,
              });
              setError(replayErr.message);
              setConnection(replayErr.status === 'partial_eof' ? 'reconnecting' : 'paused');
            } else {
              const msg = replayErr instanceof Error ? replayErr.message : 'Event replay unavailable';
              setFetchedRecords([]);
              setFetchedReplayCoverage({
                status: 'unavailable',
                coveredCount: 0,
                coveredThroughSeq: '0',
                targetSeq: nextFleet.cursor.event_seq,
                maxEvents: 200,
                detail: msg,
              });
              setError(`Event history unavailable: ${msg}`);
              setConnection('paused');
            }
          }
        } else if (!controller.signal.aborted) {
          setConnection('connected');
        }
      } catch (reason) {
        if (controller.signal.aborted) return;
        setConnection('paused');
        setError(reason instanceof Error ? reason.message : 'Command journey snapshot unavailable');
      }
    })();

    return () => controller.abort();
  }, [externalFleet, externalRecords, externalHealth, token, fetcher, reloadCount]);

  const attentionCount = useMemo(
    () => (fleet ? deriveAttentionItems(fleet, records).length : 0),
    [fleet, records],
  );

  const tabs: Array<{ id: CommandJourneyTab; label: string; badge?: number }> = [
    { id: 'intake', label: 'Work intake' },
    { id: 'detail', label: 'Task detail & evidence', badge: fleet?.tasks.length },
    { id: 'attention', label: 'Attention & coordination', badge: attentionCount },
    { id: 'actions', label: 'Operator actions' },
    { id: 'integration', label: 'Integration & quality' },
  ];

  return (
    <div className="command-journeys" data-connection={connection}>
      <div className="legacy-project-tabs" role="tablist" aria-label="Command UI journeys" style={{ marginBottom: 16 }}>
        {tabs.map(item => (
          <button
            key={item.id}
            type="button"
            role="tab"
            aria-selected={tab === item.id}
            onClick={() => setTab(item.id)}
          >
            {item.label}
            {item.badge !== undefined ? ` (${item.badge})` : ''}
          </button>
        ))}
      </div>

      {error && (
        <div className="error-banner" role="alert">
          <span>Command UI connection issue</span>
          <span>{error}</span>
          <button
            type="button"
            className="text-button"
            onClick={() => void triggerRefresh()}
          >
            Retry
          </button>
        </div>
      )}

      {tab === 'intake' && (
        <IntakeJourney
          key={intakeInitialDraft ? `${intakeInitialDraft.task_id}:${intakeInitialDraft.revision}` : 'default'}
          fleet={fleet}
          initialDraft={intakeInitialDraft}
          token={token}
          fetcher={fetcher}
          onSubmitTask={onSubmitTask}
          onReviseContract={onReviseContract}
          onRefreshSnapshot={triggerRefresh}
          onAccess={onAccess}
        />
      )}
      {tab === 'detail' && (
        <TaskDetailEvidenceJourney
          fleet={fleet}
          records={records}
          replayCoverage={replayCoverage}
          selectedTaskId={selectedTaskId}
          onSelectTask={taskId => setSelectedTaskId(taskId)}
          token={token}
          fetcher={fetcher}
          onCancelPendingTask={onCancelPendingTask}
          onReviseContract={onReviseContract}
          onRefreshSnapshot={triggerRefresh}
          onOpenReviseInIntake={task => {
            setIntakeInitialDraft(draftFromTaskForRevision(task));
            setTab('intake');
          }}
          onAccess={onAccess}
        />
      )}
      {tab === 'attention' && (
        <AttentionCoordinationJourney
          fleet={fleet}
          records={records}
          replayCoverage={replayCoverage}
          onSelectTask={taskId => {
            setSelectedTaskId(taskId);
            setTab('detail');
          }}
        />
      )}
      {tab === 'actions' && (
        <OperatorActionsJourney
          fleet={fleet}
          health={health}
          token={token}
          fetcher={fetcher}
          onRefreshSnapshot={() => void triggerRefresh()}
          onAccess={onAccess}
        />
      )}
      {tab === 'integration' && (
        <IntegrationQuality fleet={fleet} onSelectTask={taskId => {
          setSelectedTaskId(taskId);
          setTab('detail');
        }} />
      )}
    </div>
  );
}

export default CommandJourneys;
