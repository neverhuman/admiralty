import { validateDomainEvent, validateTaskContract, type DomainEvent } from '../../../packages/contracts/index';
import type { Cursor, Fleet } from './api';

export type DomainRecord = { seq: string; observed_utc: string; event: DomainEvent };
function object(value: unknown): Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) throw new Error('Unsupported API response: expected object');
  return value as Record<string, unknown>;
}
function assert(condition: unknown, label: string): asserts condition {
  if (!condition) throw new Error(`Unsupported API response: ${label}`);
}
const text = (value: unknown): value is string => typeof value === 'string';
const count = (value: unknown): value is number => Number.isSafeInteger(value) && Number(value) >= 0;
const nullableText = (value: unknown): value is string | null => value === null || text(value);
export const API_VALIDATION_LIMITS = Object.freeze({ depth: 32, nodes: 100_000, collection: 10_000, text: 8 * 1024 * 1024, string: 1024 * 1024 });

/** Bound schema work before generated guards. Inputs must be finite JSON trees. */
export function assertBoundedJson(value: unknown): void {
  const pending = [{ value, depth: 0 }];
  const seen = new WeakSet<object>();
  let nodes = 0, characters = 0;
  while (pending.length) {
    const next = pending.pop()!;
    assert(++nodes <= API_VALIDATION_LIMITS.nodes && next.depth <= API_VALIDATION_LIMITS.depth, 'input exceeds validation bounds');
    if (typeof next.value === 'string') {
      characters += next.value.length;
      assert(next.value.length <= API_VALIDATION_LIMITS.string && characters <= API_VALIDATION_LIMITS.text, 'input exceeds text bounds');
    } else if (next.value !== null && typeof next.value === 'object') {
      const entry = next.value;
      assert(!seen.has(entry), 'input must be an acyclic JSON tree'); seen.add(entry);
      const prototype = Object.getPrototypeOf(entry);
      assert(Array.isArray(entry) || prototype === Object.prototype || prototype === null, 'input must contain JSON objects');
      if (Array.isArray(entry)) assert(entry.length <= API_VALIDATION_LIMITS.collection, 'input exceeds collection bounds');
      const keys = Object.keys(entry);
      assert(keys.length <= API_VALIDATION_LIMITS.collection && nodes + pending.length + keys.length <= API_VALIDATION_LIMITS.nodes, 'input exceeds collection bounds');
      for (const key of keys) {
        characters += key.length;
        assert(characters <= API_VALIDATION_LIMITS.text, 'input exceeds text bounds');
        const descriptor = Object.getOwnPropertyDescriptor(entry, key)!;
        assert(Object.hasOwn(descriptor, 'value'), 'input must not contain accessors');
        pending.push({ value: descriptor.value, depth: next.depth + 1 });
      }
    } else assert(next.value === null || typeof next.value === 'boolean' || (typeof next.value === 'number' && Number.isFinite(next.value)), 'input must contain finite JSON values');
  }
}
export function decimal(value: unknown): value is string {
  return text(value) && /^(0|[1-9][0-9]{0,18})$/.test(value) && BigInt(value) <= 9223372036854775807n;
}
export function parseCursor(value: unknown): Cursor {
  const cursor = object(value);
  assert(text(cursor.recovery_generation) && cursor.recovery_generation.length > 0 && cursor.recovery_generation.length <= 128
    && !/[\r\n\0:]/.test(cursor.recovery_generation) && decimal(cursor.event_seq), 'invalid event cursor');
  return { recovery_generation: cursor.recovery_generation, event_seq: cursor.event_seq };
}
export function parseFleet(value: unknown): Fleet {
  assertBoundedJson(value);
  const fleet = object(value);
  assert(fleet.schema_version === 1 && typeof fleet.simulated === 'boolean' && fleet.native_execution_qualified === false, 'unqualified fleet envelope');
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
    assert(['ready', 'blocked', 'claimed', 'running', 'awaiting_verification'].includes(String(task.state))
      && nullableText(task.attempt_id) && nullableText(task.owner) && nullableText(task.candidate_ref), 'invalid task projection');
  }
  const coordination = object(fleet.coordination);
  assert(count(coordination.queued_messages) && count(coordination.acknowledged_messages) && coordination.routine_model_turns === 0
    && (coordination.measured_tokens === null || decimal(coordination.measured_tokens)), 'invalid coordination metrics');
  assert(text(fleet.sqlite_version), 'missing SQLite version');
  return fleet as Fleet;
}
export function parseDomainRecord(value: unknown): DomainRecord {
  assertBoundedJson(value);
  const record = object(value);
  assert(decimal(record.seq) && record.seq !== '0' && text(record.observed_utc), 'invalid domain record');
  assert(validateDomainEvent(record.event), 'invalid or unknown domain event schema or version');
  return record as DomainRecord;
}
