/**
 * Dependency-free JSON guards shared by every eagerly loaded module.
 *
 * Kept apart from `validation.ts`, which imports the generated contract
 * validators (~280 KB of standalone schema code), so the first paint of the
 * console does not wait for schema guards it only needs once native fleet
 * data arrives. Behaviour is unchanged: `validation.ts` re-exports these.
 */
import type { Cursor } from './api';

export function object(value: unknown): Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) throw new Error('Unsupported API response: expected object');
  return value as Record<string, unknown>;
}
export function assert(condition: unknown, label: string): asserts condition {
  if (!condition) throw new Error(`Unsupported API response: ${label}`);
}
export const text = (value: unknown): value is string => typeof value === 'string';
export const count = (value: unknown): value is number => Number.isSafeInteger(value) && Number(value) >= 0;
export const nullableText = (value: unknown): value is string | null => value === null || text(value);
export const API_VALIDATION_LIMITS = Object.freeze({ depth: 32, nodes: 100_000, collection: 10_000, text: 8 * 1024 * 1024, string: 1024 * 1024 });

/** Bound schema work before generated guards. Inputs must be finite JSON trees. */
export function assertBoundedJson(value: unknown, limits: Readonly<Record<keyof typeof API_VALIDATION_LIMITS, number>> = API_VALIDATION_LIMITS): void {
  const pending = [{ value, depth: 0 }];
  const seen = new WeakSet<object>();
  let nodes = 0, characters = 0;
  while (pending.length) {
    const next = pending.pop()!;
    assert(++nodes <= limits.nodes && next.depth <= limits.depth, 'input exceeds validation bounds');
    if (typeof next.value === 'string') {
      characters += next.value.length;
      assert(next.value.length <= limits.string && characters <= limits.text, 'input exceeds text bounds');
    } else if (next.value !== null && typeof next.value === 'object') {
      const entry = next.value;
      assert(!seen.has(entry), 'input must be an acyclic JSON tree'); seen.add(entry);
      const prototype = Object.getPrototypeOf(entry);
      assert(Array.isArray(entry) || prototype === Object.prototype || prototype === null, 'input must contain JSON objects');
      if (Array.isArray(entry)) assert(entry.length <= limits.collection, 'input exceeds collection bounds');
      const keys = Object.keys(entry);
      assert(keys.length <= limits.collection && nodes + pending.length + keys.length <= limits.nodes, 'input exceeds collection bounds');
      for (const key of keys) {
        characters += key.length;
        assert(characters <= limits.text, 'input exceeds text bounds');
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
