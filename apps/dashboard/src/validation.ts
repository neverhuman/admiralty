import { validateDomainEvent, validateDomainEventV2, validateTaskContract, type DomainEvent, type DomainEventV2 } from '../../../packages/contracts/index';
import type { Fleet, NativeFleet, NativeTaskLifecycle } from './api';
import { API_VALIDATION_LIMITS, assert, assertBoundedJson, count, decimal, nullableText, object, parseCursor, text } from './jsonBounds';

export { API_VALIDATION_LIMITS, assertBoundedJson, decimal, parseCursor };

export type DomainRecord = { seq: string; observed_utc: string; event: DomainEvent };
export type DomainRecordV2 = { seq: string; observed_utc: string; event: DomainEventV2 };
function parseFleetEnvelope(value: unknown, version: 1 | 2): Record<string, unknown> {
  assertBoundedJson(value);
  const fleet = object(value);
  assert(fleet.schema_version === version && typeof fleet.simulated === 'boolean' && fleet.native_execution_qualified === false, 'unqualified fleet envelope');
  parseCursor(fleet.cursor);
  assert(Array.isArray(fleet.projects) && Array.isArray(fleet.tasks), 'invalid fleet collections');
  for (const value of fleet.projects) {
    const project = object(value);
    assert(text(project.id) && text(project.name), 'invalid project identity');
    for (const key of ['total', 'running', 'awaiting_verification', 'blocked', 'accepted']) assert(count(project[key]), `invalid project ${key}`);
  }
  for (const value of fleet.tasks) {
    const task = object(value);
    assert(validateTaskContract(task.contract), 'invalid task contract schema or version');
    const states = version === 1
      ? ['ready', 'blocked', 'claimed', 'running', 'awaiting_verification']
      : ['ready', 'blocked', 'claimed', 'running', 'awaiting_verification', 'accepted', 'failed', 'cancelled'];
    assert(states.includes(String(task.state))
      && nullableText(task.attempt_id) && nullableText(task.owner) && nullableText(task.candidate_ref), 'invalid task projection');
    if (version === 1) assert(!Object.hasOwn(task, 'lifecycle'), 'v1 task projection cannot contain lifecycle metadata');
    if (version === 2 && Object.hasOwn(task, 'lifecycle')) parseLifecycle(task.lifecycle);
  }
  const coordination = object(fleet.coordination);
  assert(count(coordination.queued_messages) && count(coordination.acknowledged_messages) && coordination.routine_model_turns === 0
    && (coordination.measured_tokens === null || decimal(coordination.measured_tokens)), 'invalid coordination metrics');
  assert(text(fleet.sqlite_version), 'missing SQLite version');
  return fleet;
}
export function parseFleet(value: unknown): Fleet {
  return parseFleetEnvelope(value, 1) as unknown as Fleet;
}
export function parseNativeFleet(value: unknown): NativeFleet {
  return parseFleetEnvelope(value, 2) as unknown as NativeFleet;
}
function parseLifecycle(value: unknown): NativeTaskLifecycle {
  const lifecycle = object(value);
  const lifecycleKeys = ['revision', 'attempts', 'closure_digest', 'decision', 'reason'];
  assert(Object.keys(lifecycle).length === lifecycleKeys.length && lifecycleKeys.every(key => Object.hasOwn(lifecycle, key)), 'invalid lifecycle projection fields');
  assert(Number.isSafeInteger(lifecycle.revision) && Number(lifecycle.revision) >= 0 && Number(lifecycle.revision) <= 0xffff_ffff, 'invalid lifecycle revision');
  const attempts = object(lifecycle.attempts);
  assert(Object.values(attempts).every(text) && nullableText(lifecycle.closure_digest) && nullableText(lifecycle.reason), 'invalid lifecycle projection');
  if (lifecycle.decision !== null) {
    const decision = object(lifecycle.decision);
    const keys = ['closure_digest', 'contract_digest', 'custody_digest', 'receipt_digest', 'source_commit_sha', 'source_tree_sha', 'verification_policy_digest', 'verifier'];
    assert(Object.keys(decision).length === keys.length && keys.every(key => text(decision[key])), 'invalid lifecycle decision');
  }
  return lifecycle as unknown as NativeTaskLifecycle;
}
export function parseDomainRecord(value: unknown): DomainRecord {
  assertBoundedJson(value);
  const record = object(value);
  assert(decimal(record.seq) && record.seq !== '0' && text(record.observed_utc), 'invalid domain record');
  assert(validateDomainEvent(record.event), 'invalid or unknown domain event schema or version');
  return record as DomainRecord;
}
export function parseDomainRecordV2(value: unknown): DomainRecordV2 {
  assertBoundedJson(value);
  const record = object(value);
  assert(decimal(record.seq) && record.seq !== '0' && text(record.observed_utc), 'invalid domain record');
  assert(validateDomainEventV2(record.event), 'invalid or unknown domain event schema or version');
  return record as DomainRecordV2;
}
