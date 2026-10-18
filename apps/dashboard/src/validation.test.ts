import { describe, expect, it } from 'vitest';
import { assertBoundedJson, parseDomainRecord, parseFleet, API_VALIDATION_LIMITS } from './validation';
import { readBoundedJson, MAX_FLEET_RESPONSE_BYTES } from './api';

const record = (event: unknown) => ({ seq: '1', observed_utc: '2026-10-18T12:00:00Z', event });
const valid = () => ({ kind: 'task_started', task_id: 'task', attempt_id: 'attempt' });

describe('generated domain guards at the API boundary', () => {
  it('uses generated additionalProperties and union validation', () => {
    expect(parseDomainRecord(record(valid())).event.kind).toBe('task_started');
    expect(() => parseDomainRecord(record({ ...valid(), self_accepted: true }))).toThrow('schema or version');
    expect(() => parseDomainRecord(record({ kind: 'task_started', task_id: 'task' }))).toThrow('schema or version');
    expect(() => parseDomainRecord(record({ kind: 'task_claimed', task_id: 'task', attempt_id: 'attempt', owner: 'worker', revision: 4294967296 }))).toThrow('schema or version');
  });
  it('bounds nesting, collection length, strings and cyclic inputs before schema work', () => {
    let nested: unknown = 'leaf';
    for (let i = 0; i < API_VALIDATION_LIMITS.depth + 1; i++) nested = { next: nested };
    expect(() => assertBoundedJson(nested)).toThrow('bounds');
    expect(() => assertBoundedJson(Array(API_VALIDATION_LIMITS.collection + 1))).toThrow('bounds');
    expect(() => assertBoundedJson('x'.repeat(API_VALIDATION_LIMITS.string + 1))).toThrow('bounds');
    const cyclic: Record<string, unknown> = {}; cyclic.next = cyclic;
    expect(() => assertBoundedJson(cyclic)).toThrow('acyclic');
    expect(() => parseFleet(cyclic)).toThrow('acyclic');
  });
  it('never executes accessors and rejects non-JSON values', () => {
    let touched = false;
    const value = Object.defineProperty({}, 'payload', { enumerable: true, get() { touched = true; return {}; } });
    expect(() => assertBoundedJson(value)).toThrow('accessors'); expect(touched).toBe(false);
    for (const value of [NaN, Infinity, 1n, undefined, new Date()]) expect(() => assertBoundedJson(value)).toThrow();
  });
});

describe('bounded snapshot transport', () => {
  it('counts actual bytes, cancels oversize streams even with a false Content-Length', async () => {
    let cancelled = false;
    const response = new Response(new ReadableStream({ start(controller) {
      controller.enqueue(new Uint8Array(MAX_FLEET_RESPONSE_BYTES)); controller.enqueue(new Uint8Array(1));
    }, cancel() { cancelled = true; } }), { headers: { 'Content-Length': '1' } });
    await expect(readBoundedJson(response)).rejects.toThrow('eight MiB'); expect(cancelled).toBe(true);
  });
  it('refuses a declared oversized body before consuming it', async () => {
    let cancelled = false;
    const response = new Response(new ReadableStream({ cancel() { cancelled = true; } }), { headers: { 'Content-Length': String(MAX_FLEET_RESPONSE_BYTES + 1) } });
    await expect(readBoundedJson(response)).rejects.toThrow('eight MiB'); expect(cancelled).toBe(true);
  });
  it('decodes split UTF-8 safely and rejects malformed JSON or encoding', async () => {
    const bytes = new TextEncoder().encode('{"name":"⚓"}');
    const response = new Response(new ReadableStream({ start(controller) {
      for (const byte of bytes) controller.enqueue(new Uint8Array([byte])); controller.close();
    } }));
    await expect(readBoundedJson(response)).resolves.toEqual({ name: '⚓' });
    await expect(readBoundedJson(new Response('{broken'))).rejects.toThrow();
    await expect(readBoundedJson(new Response(new Uint8Array([255])))).rejects.toThrow();
  });
});
