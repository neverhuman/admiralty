import { useEffect, useMemo, useState } from 'react';
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
import { assertBoundedJson, decimal, parseCursor, type DomainRecord } from '../validation';

export type CommandJourneyTab = 'intake' | 'detail' | 'attention' | 'actions';

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
  lifecycle?: TaskLifecycleView | null;
};

export type AnyFleet = Omit<Fleet, 'schema_version' | 'tasks'> & {
  schema_version: 1 | 2;
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
};

export type DuplicateCandidate = {
  task_id: string;
  revision: number;
  title: string;
  state: AnyTaskState;
  reason: 'same_task_id' | 'owned_path_overlap' | 'title_similarity';
  overlapping_paths: string[];
};

export type IntakePreview = {
  valid: boolean;
  contract: TaskContract | null;
  errors: string[];
  warnings: string[];
  duplicates: DuplicateCandidate[];
  cliCommand: string;
  nonDispatchNotice: string;
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

export type TaskDetailModel = {
  task: AnyFleetTask;
  projectName: string;
  dependencies: DependencyResolution[];
  criteriaWithChecks: Array<{
    criterion: AcceptanceCriterion;
    checks: CheckSpec[];
    missingCheckIds: string[];
  }>;
  timeline: TaskEvidenceItem[];
  relatedMessages: ScopedMessageLedgerEntry[];
  latestDecision: CandidateDecisionEvidence | null;
};

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
  'Intake preview validates schema, path boundaries, and duplicate candidates locally. It never dispatches workers or enables native execution.';

const SAFE_ID_PATTERN = /^[A-Za-z0-9_.:/-]{1,128}$/;

function splitList(raw: string): string[] {
  return [...new Set(raw.split(/[\n,]+/).map(item => item.trim()).filter(Boolean))];
}

function isSafeRelativePath(path: string): boolean {
  if (!path || path.length > 512 || path.startsWith('/') || path.includes('\\') || /\p{Cc}/u.test(path)) {
    return false;
  }
  const segments = path.split('/');
  return segments.every(segment => segment.length > 0 && segment !== '.' && segment !== '..');
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

export function parseDomainRecordV2(value: unknown): AnyDomainRecord {
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

export function parseAnyFleet(value: unknown): AnyFleet {
  assertBoundedJson(value);
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('Unsupported API response: expected object');
  }
  const fleet = value as Record<string, unknown>;
  if (
    (fleet.schema_version !== 1 && fleet.schema_version !== 2) ||
    typeof fleet.simulated !== 'boolean' ||
    fleet.native_execution_qualified !== false
  ) {
    throw new Error('Unsupported API response: unqualified fleet envelope');
  }
  parseCursor(fleet.cursor);
  if (!Array.isArray(fleet.projects) || !Array.isArray(fleet.tasks)) {
    throw new Error('Unsupported API response: invalid fleet collections');
  }
  const validStates =
    fleet.schema_version === 1
      ? ['ready', 'blocked', 'claimed', 'running', 'awaiting_verification']
      : ['ready', 'blocked', 'claimed', 'running', 'awaiting_verification', 'accepted', 'failed', 'cancelled'];
  for (const item of fleet.tasks) {
    if (!item || typeof item !== 'object' || Array.isArray(item)) {
      throw new Error('Unsupported API response: invalid task projection');
    }
    const task = item as Record<string, unknown>;
    if (
      !validateTaskContract(task.contract) ||
      !validStates.includes(String(task.state)) ||
      !(task.attempt_id === null || typeof task.attempt_id === 'string') ||
      !(task.owner === null || typeof task.owner === 'string') ||
      !(task.candidate_ref === null || typeof task.candidate_ref === 'string')
    ) {
      throw new Error('Unsupported API response: invalid task projection');
    }
  }
  return fleet as unknown as AnyFleet;
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
  return parseAnyFleet(await readBoundedJson(response));
}

export function emptyIntakeDraft(projectId = 'admiralty'): IntakeDraft {
  return {
    project_id: projectId,
    root_id: 'root-main',
    task_id: 'task-intake-1',
    revision: '1',
    title: 'Verify bounded task intake contract',
    objective: 'Validate contract fields and inspect duplicate candidates before offline CLI registration.',
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

export function draftToContractCandidate(draft: IntakeDraft): unknown {
  const revision = /^\d+$/.test(draft.revision.trim()) ? Number(draft.revision.trim()) : NaN;
  const maxAttempts = /^\d+$/.test(draft.max_attempts.trim()) ? Number(draft.max_attempts.trim()) : NaN;
  const timeoutSecs = /^\d+$/.test(draft.check_timeout_secs.trim()) ? Number(draft.check_timeout_secs.trim()) : NaN;
  const checkId = draft.check_id.trim();
  return {
    schema_version: 1,
    project_id: draft.project_id.trim(),
    root_id: draft.root_id.trim(),
    task_id: draft.task_id.trim(),
    revision,
    title: draft.title.trim(),
    objective: draft.objective.trim(),
    non_goals: splitList(draft.non_goals),
    owned_paths: splitList(draft.owned_paths),
    forbidden_paths: splitList(draft.forbidden_paths),
    read_first: splitList(draft.read_first),
    dependencies: splitList(draft.dependencies),
    criteria: draft.criterion_id.trim() || draft.criterion_text.trim()
      ? [
          {
            id: draft.criterion_id.trim(),
            text: draft.criterion_text.trim(),
            check_ids: checkId ? [checkId] : [],
          },
        ]
      : [],
    checks: checkId
      ? [
          {
            id: checkId,
            program: draft.check_program.trim(),
            args: splitList(draft.check_args),
            cwd: draft.check_cwd.trim(),
            timeout_secs: timeoutSecs,
          },
        ]
      : [],
    authority_ref: draft.authority_ref.trim(),
    max_attempts: maxAttempts,
    simulated: draft.simulated,
  };
}

export function findDuplicateCandidates(
  contract: Pick<TaskContract, 'project_id' | 'task_id' | 'revision' | 'title' | 'owned_paths'>,
  existingTasks: readonly AnyFleetTask[],
): DuplicateCandidate[] {
  const duplicates: DuplicateCandidate[] = [];
  const candidateTokens = titleTokens(contract.title);

  for (const existing of existingTasks.slice(0, 500)) {
    const other = existing.contract;
    if (other.task_id === contract.task_id) {
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
      continue;
    }

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

  return duplicates;
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
): IntakePreview {
  const rawCandidate = isIntakeDraft(input) ? draftToContractCandidate(input) : input;
  const errors: string[] = [];
  const warnings: string[] = [];

  try {
    assertBoundedJson(rawCandidate);
  } catch (error) {
    return {
      valid: false,
      contract: null,
      errors: [error instanceof Error ? error.message : 'Input exceeds validation bounds'],
      warnings: [],
      duplicates: [],
      cliCommand: 'admiralty work submit --contract <invalid-contract.json>',
      nonDispatchNotice: NON_DISPATCH_NOTICE,
    };
  }

  if (!validateTaskContract(rawCandidate)) {
    return {
      valid: false,
      contract: null,
      errors: ['Contract does not match TaskContract schema v1 (check required fields and integer bounds).'],
      warnings: [],
      duplicates: [],
      cliCommand: 'admiralty work submit --contract <invalid-contract.json>',
      nonDispatchNotice: NON_DISPATCH_NOTICE,
    };
  }

  const contract = rawCandidate as TaskContract;
  if (contract.schema_version !== 1) {
    errors.push('schema_version must equal 1.');
  }
  if (!SAFE_ID_PATTERN.test(contract.project_id)) {
    errors.push('project_id must be 1–128 safe ASCII identifier characters.');
  }
  if (!SAFE_ID_PATTERN.test(contract.root_id)) {
    errors.push('root_id must be 1–128 safe ASCII identifier characters.');
  }
  if (!SAFE_ID_PATTERN.test(contract.task_id)) {
    errors.push('task_id must be 1–128 safe ASCII identifier characters.');
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
    if (!SAFE_ID_PATTERN.test(check.id)) {
      errors.push(`Check ID "${check.id}" must use safe identifier characters.`);
    }
    if (checkIds.has(check.id)) {
      errors.push(`Duplicate check ID "${check.id}".`);
    }
    checkIds.add(check.id);
    if (!check.program.trim() || /\s/.test(check.program) || /[;|&$`<>]/.test(check.program)) {
      errors.push(`Check "${check.id}" program must be an argv executable name without shell metacharacters.`);
    }
    if (check.timeout_secs < 1 || check.timeout_secs > 86_400) {
      errors.push(`Check "${check.id}" timeout_secs must be between 1 and 86400 seconds.`);
    }
  }

  for (const criterion of contract.criteria) {
    if (!criterion.id.trim() || !criterion.text.trim()) {
      errors.push('Every acceptance criterion requires a non-empty id and text.');
    }
    if (criterion.check_ids.length === 0) {
      errors.push(`Criterion "${criterion.id}" must bind at least one verification check ID.`);
    }
    for (const refId of criterion.check_ids) {
      if (!checkIds.has(refId)) {
        errors.push(`Criterion "${criterion.id}" references undeclared check "${refId}".`);
      }
    }
  }

  const duplicates = findDuplicateCandidates(contract, existingTasks);
  for (const dup of duplicates) {
    if (dup.reason === 'same_task_id') {
      if (dup.revision >= contract.revision) {
        errors.push(
          `Task "${dup.task_id}" already exists at revision ${dup.revision} (${taskStateLabel(dup.state as Fleet['tasks'][number]['state'])}); new revision must exceed ${dup.revision}.`,
        );
      } else {
        warnings.push(
          `Supersedes existing task "${dup.task_id}" rev ${dup.revision} (${taskStateLabel(dup.state as Fleet['tasks'][number]['state'])}).`,
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

  const safeTaskFile = SAFE_ID_PATTERN.test(contract.task_id) ? `${contract.task_id}.rev${contract.revision}.json` : 'contract.json';
  return {
    valid: errors.length === 0,
    contract,
    errors,
    warnings,
    duplicates,
    cliCommand: `admiralty work submit --contract workplans/${safeTaskFile}`,
    nonDispatchNotice: NON_DISPATCH_NOTICE,
  };
}

export function deriveCoordinationLedger(records: readonly (DomainRecord | AnyDomainRecord)[]): ScopedMessageLedgerEntry[] {
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
  let latestDecision: CandidateDecisionEvidence | null = task.lifecycle?.decision ?? null;

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
      if (described.decision_evidence) {
        latestDecision = described.decision_evidence;
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
    dependencies,
    criteriaWithChecks,
    timeline,
    relatedMessages,
    latestDecision,
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

export async function replayEventsToCursor(
  cursor: Cursor,
  token?: string,
  signal?: AbortSignal,
  fetcher: typeof fetch = fetch,
  maxEvents = 200,
  endpoint: '/v1/events' | '/v2/events' = '/v1/events',
): Promise<AnyDomainRecord[]> {
  if (!decimal(cursor.event_seq) || BigInt(cursor.event_seq) === 0n || maxEvents <= 0) {
    return [];
  }
  const targetSeq = BigInt(cursor.event_seq);
  const controller = new AbortController();
  const onAbort = () => controller.abort();
  signal?.addEventListener('abort', onAbort, { once: true });

  const records: AnyDomainRecord[] = [];
  let reachedTarget = false;

  try {
    const query = new URLSearchParams({
      after: '0',
      generation: cursor.recovery_generation,
    });
    const response = await fetcher(`${endpoint}?${query}`, {
      headers: {
        Accept: 'text/event-stream',
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
      },
      signal: controller.signal,
      cache: 'no-store',
    });
    if (!response.ok) {
      throw new ApiError(`Event replay failed (${response.status})`, response.status);
    }
    if (!response.headers.get('content-type')?.includes('text/event-stream') || !response.body) {
      throw new Error('Unsupported event stream response');
    }

    const parser = createSseParser(frame => {
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
      records.push(record);
      if (BigInt(record.seq) >= targetSeq || records.length >= maxEvents) {
        reachedTarget = true;
      }
    });

    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    try {
      while (!reachedTarget && !controller.signal.aborted) {
        const chunk = await reader.read();
        if (chunk.done) break;
        parser.push(decoder.decode(chunk.value, { stream: true }));
      }
    } finally {
      await reader.cancel().catch(() => undefined);
      reader.releaseLock();
    }
    return records;
  } finally {
    signal?.removeEventListener('abort', onAbort);
    controller.abort();
  }
}

export function IntakeJourney({
  fleet,
  initialDraft,
}: {
  fleet: AnyFleet | Fleet | null;
  initialDraft?: IntakeDraft;
}) {
  const defaultProject = fleet?.projects[0]?.id ?? 'admiralty';
  const [draft, setDraft] = useState<IntakeDraft>(() => initialDraft ?? emptyIntakeDraft(defaultProject));
  const [jsonMode, setJsonMode] = useState(false);
  const [rawJson, setRawJson] = useState('');

  const preview = useMemo(() => {
    if (jsonMode) {
      try {
        return buildIntakePreview(JSON.parse(rawJson), fleet?.tasks ?? []);
      } catch (error) {
        return {
          valid: false,
          contract: null,
          errors: [error instanceof Error ? `Invalid JSON: ${error.message}` : 'Invalid JSON'],
          warnings: [],
          duplicates: [],
          cliCommand: 'admiralty work submit --contract <invalid-contract.json>',
          nonDispatchNotice: NON_DISPATCH_NOTICE,
        } satisfies IntakePreview;
      }
    }
    return buildIntakePreview(draft, fleet?.tasks ?? []);
  }, [draft, jsonMode, rawJson, fleet]);

  function updateField<K extends keyof IntakeDraft>(key: K, value: IntakeDraft[K]) {
    setDraft(current => ({ ...current, [key]: value }));
  }

  return (
    <div className="models-settings" data-journey="intake">
      <div className="models-notice">
        <b>Live work intake preview · non-dispatching admission lint</b>
        <p>{preview.nonDispatchNotice}</p>
      </div>

      <section className="panel models-panel" aria-labelledby="intake-form-heading">
        <div className="panel-head">
          <div>
            <div className="eyebrow">WORK ORDER INTAKE / CONTRACT VALIDATION</div>
            <h2 id="intake-form-heading">Author or import TaskContract</h2>
          </div>
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

        {jsonMode ? (
          <div className="models-form">
            <label style={{ gridColumn: '1 / -1' }}>
              TaskContract JSON (schema_version 1)
              <textarea
                aria-label="TaskContract JSON"
                rows={10}
                value={rawJson}
                onChange={event => setRawJson(event.target.value)}
              />
              <small>Validated with bounded JSON limits and generated TaskContract schema guards.</small>
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
              Check args (comma-separated)
              <input
                value={draft.check_args}
                onChange={event => updateField('check_args', event.target.value)}
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
      </section>

      <section className="panel models-panel" aria-labelledby="intake-preview-heading">
        <div className="panel-head">
          <div>
            <div className="eyebrow">ADMISSION PREVIEW & DUPLICATE CHECK</div>
            <h2 id="intake-preview-heading">
              {preview.valid ? 'Ready for offline CLI registration' : 'Admission blocked by validation errors'}
            </h2>
          </div>
          <span className={`state-pill ${preview.valid ? 'ready' : 'blocked'}`}>
            {preview.valid ? 'valid contract' : `${preview.errors.length} error(s)`}
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
            No duplicate task IDs, overlapping active owned paths, or similar titles detected across{' '}
            {fleet?.tasks.length ?? 0} snapshot task(s).
          </p>
        )}

        <p>
          Offline CLI registration command: <code>{preview.cliCommand}</code>
        </p>
      </section>
    </div>
  );
}

export function TaskDetailEvidenceJourney({
  fleet,
  records,
  selectedTaskId,
  onSelectTask,
}: {
  fleet: AnyFleet | Fleet | null;
  records: readonly (DomainRecord | AnyDomainRecord)[];
  selectedTaskId?: string;
  onSelectTask?: (taskId: string) => void;
}) {
  const [internalTaskId, setInternalTaskId] = useState<string>('');
  const activeTaskId = selectedTaskId ?? internalTaskId;

  const currentTask = useMemo(() => {
    if (!fleet?.tasks.length) return null;
    return fleet.tasks.find(item => item.contract.task_id === activeTaskId) ?? fleet.tasks[0];
  }, [fleet, activeTaskId]);

  const detail = useMemo(() => {
    if (!fleet || !currentTask) return null;
    return deriveTaskDetail(currentTask, fleet, records);
  }, [fleet, currentTask, records]);

  if (!fleet) {
    return <div className="loading-card">Awaiting fleet snapshot to inspect task details and evidence…</div>;
  }

  if (!detail) {
    return (
      <section className="panel workplans-panel" data-journey="detail">
        <div className="panel-head">
          <div>
            <div className="eyebrow">TASK DETAIL & EVIDENCE</div>
            <h2>No tasks in current snapshot</h2>
          </div>
          <span className="tiny-badge">0 TASKS</span>
        </div>
        <div className="empty-state">
          <span>⌁</span>The current fleet snapshot contains no task contracts.
        </div>
      </section>
    );
  }

  const { task } = detail;
  return (
    <div className="models-settings" data-journey="detail">
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
            <strong>{task.contract.max_attempts}</strong>
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
            <strong>Trusted verifier decision evidence:</strong> Verifier{' '}
            <code>{detail.latestDecision.verifier}</code> · Receipt{' '}
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

      <section className="panel models-panel" aria-labelledby="task-criteria-heading">
        <div className="panel-head">
          <div>
            <div className="eyebrow">ACCEPTANCE CRITERIA & VERIFICATION CHECKS</div>
            <h2 id="task-criteria-heading">Contract verification bindings</h2>
          </div>
          <span className="tiny-badge">{detail.criteriaWithChecks.length} CRITERIA</span>
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
            <div className="eyebrow">DURABLE EVENT REPLAY / EVIDENCE TIMELINE</div>
            <h2 id="task-evidence-heading">Observed task lifecycle & coordination events</h2>
          </div>
          <span className="tiny-badge">{detail.timeline.length} EVENTS</span>
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
            <span>⌁</span>No replayed domain events matched task {task.contract.task_id} in the current stream window.
          </div>
        )}
      </section>
    </div>
  );
}

export function AttentionCoordinationJourney({
  fleet,
  records,
  onSelectTask,
}: {
  fleet: AnyFleet | Fleet | null;
  records: readonly (DomainRecord | AnyDomainRecord)[];
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
      <section className="panel activity-panel" aria-labelledby="attention-pulse-heading">
        <div className="panel-head">
          <div>
            <div className="eyebrow">COORDINATION & ATTENTION PULSE</div>
            <h2 id="attention-pulse-heading">Durable coordination summary</h2>
          </div>
          <span className="tiny-badge">{attentionItems.length} ATTENTION ITEMS</span>
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
            <div className="eyebrow">NEEDS ATTENTION / OPERATOR TRIAGE</div>
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
            <div className="eyebrow">SCOPED COORDINATION LEDGER</div>
            <h2 id="coord-ledger-heading">Replayed messages & application acknowledgements</h2>
          </div>
          <span className="tiny-badge">{ledger.length} MESSAGES</span>
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
            <span>⌁</span>No coordination messages replayed in the current event window.
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
        <b>Operator actions & dry-run diagnostics · read-only server boundary</b>
        <p>
          Server endpoints inspect durable projections and evaluate model routing policy without starting native workers or mutating task state.
        </p>
      </div>

      <section className="panel models-panel" aria-labelledby="actions-health-heading">
        <div className="panel-head">
          <div>
            <div className="eyebrow">LIVE SERVER HEALTH & SNAPSHOT CURSOR</div>
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
            <div className="eyebrow">POST /v1/models/select · DRY-RUN ROUTING PROBE</div>
            <h2 id="actions-select-heading">Evaluate model selection & fallback policy</h2>
          </div>
          <span className="tiny-badge">PURE POLICY CHECK</span>
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
  health: externalHealth,
  initialTab = 'intake',
  initialTaskId,
  token,
  fetcher = fetch,
  onAccess,
}: {
  fleet?: AnyFleet | Fleet | null;
  records?: readonly (DomainRecord | AnyDomainRecord)[];
  health?: HealthStatus | null;
  initialTab?: CommandJourneyTab;
  initialTaskId?: string;
  token?: string;
  fetcher?: typeof fetch;
  onAccess?: () => void;
}) {
  const [tab, setTab] = useState<CommandJourneyTab>(initialTab);
  const [selectedTaskId, setSelectedTaskId] = useState<string | undefined>(initialTaskId);
  const [fetchedFleet, setFetchedFleet] = useState<AnyFleet | null>(null);
  const [fetchedRecords, setFetchedRecords] = useState<AnyDomainRecord[]>([]);
  const [fetchedHealth, setFetchedHealth] = useState<HealthStatus | null>(null);
  const [error, setError] = useState('');
  const [connection, setConnection] = useState<ConnectionState>('connecting');
  const [reloadCount, setReloadCount] = useState(0);

  const fleet = externalFleet !== undefined ? externalFleet : fetchedFleet;
  const records = externalRecords !== undefined ? externalRecords : fetchedRecords;
  const health = externalHealth !== undefined ? externalHealth : fetchedHealth;

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
          const replayed = await replayEventsToCursor(
            nextFleet.cursor,
            token,
            controller.signal,
            fetcher,
            200,
            eventEndpoint,
          ).catch(() => []);
          if (!controller.signal.aborted) setFetchedRecords(replayed);
        }
        if (!controller.signal.aborted) setConnection('connected');
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
  ];

  return (
    <div className="command-journeys" data-connection={connection}>
      <nav className="legacy-project-tabs" aria-label="Command UI journeys" style={{ marginBottom: 16 }}>
        {tabs.map(item => (
          <button
            key={item.id}
            type="button"
            aria-selected={tab === item.id}
            onClick={() => setTab(item.id)}
          >
            {item.label}
            {item.badge !== undefined ? ` (${item.badge})` : ''}
          </button>
        ))}
      </nav>

      {error && (
        <div className="error-banner" role="alert">
          <span>Command UI connection issue</span>
          <span>{error}</span>
          <button
            type="button"
            className="text-button"
            onClick={() => setReloadCount(count => count + 1)}
          >
            Retry
          </button>
        </div>
      )}

      {tab === 'intake' && <IntakeJourney fleet={fleet} />}
      {tab === 'detail' && (
        <TaskDetailEvidenceJourney
          fleet={fleet}
          records={records}
          selectedTaskId={selectedTaskId}
          onSelectTask={taskId => setSelectedTaskId(taskId)}
        />
      )}
      {tab === 'attention' && (
        <AttentionCoordinationJourney
          fleet={fleet}
          records={records}
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
          onRefreshSnapshot={() => setReloadCount(count => count + 1)}
          onAccess={onAccess}
        />
      )}
    </div>
  );
}

export default CommandJourneys;
