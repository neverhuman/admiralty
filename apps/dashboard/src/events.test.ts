import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createSseParser, cursorFromId, retryDelayMs, watchFleet, type StreamFrame } from './events';
import { parseDomainRecord, parseFleet } from './validation';

const fixture = () => ({ schema_version: 1, simulated: true, cursor: { recovery_generation: 'generation-1', event_seq: '0' },
  projects: [], tasks: [], coordination: { queued_messages: 0, acknowledged_messages: 0, routine_model_turns: 0, measured_tokens: null },
  sqlite_version: '3.51.3', native_execution_qualified: false });
const domain = (seq: string, generation = 'generation-1') => `event: domain\nid: ${generation}:${seq}\ndata: ${JSON.stringify({
  seq, observed_utc: '2026-10-18T12:00:00Z', event: { kind: 'task_started', task_id: 'task', attempt_id: 'attempt' },
})}\n\n`;

describe('bounded SSE framing and API envelopes', () => {
  it('parses split CRLF, comments, multiline data and consecutive frames', () => {
    const frames: StreamFrame[] = [];
    const parser = createSseParser(frame => frames.push(frame));
    parser.push(': heartbeat\r\nid: generation:1\r');
    parser.push('\nevent: domain\r\ndata: {"seq":\r\ndata: "1"}\r\n\r');
    expect(frames).toHaveLength(0);
    parser.push('\ndata: next\n\n');
    expect(frames).toEqual([{ event: 'domain', id: 'generation:1', data: '{"seq":\n"1"}' }, { event: 'message', id: undefined, data: 'next' }]);
  });
  it('bounds both unfinished lines and multi-line frames', () => {
    expect(() => createSseParser(() => {}).push('x'.repeat(2 * 1024 * 1024 + 1))).toThrow('size limit');
    const parser = createSseParser(() => {});
    expect(() => parser.push('data: text\n'.repeat(220_000))).toThrow('size limit');
  });
  it('keeps integer cursors exact and rejects ambiguous encodings', () => {
    expect(cursorFromId('generation:9007199254740993').event_seq).toBe('9007199254740993');
    for (const id of [undefined, 'bad', 'generation:01', 'generation:-1', 'generation:9223372036854775808']) expect(() => cursorFromId(id)).toThrow();
  });
  it('rejects unsupported envelopes, negative counters and unknown domain variants', () => {
    expect(parseFleet(fixture()).cursor.event_seq).toBe('0');
    for (const patch of [{ schema_version: 2 }, { native_execution_qualified: true }, { tasks: null },
      { projects: [{ id: 'x', name: 'x', total: -1, running: 0, blocked: 0, accepted: 0, awaiting_verification: 0 }] },
      { cursor: { recovery_generation: 'generation', event_seq: 9007199254740992 } }]) {
      expect(() => parseFleet({ ...fixture(), ...patch })).toThrow('Unsupported API response');
    }
    expect(() => parseDomainRecord({ seq: '1', observed_utc: 'now', event: { kind: 'self_accepted' } })).toThrow('unknown domain event');
  });
});

describe('event driven fleet subscription', () => {
  const stops: Array<() => void> = [];
  beforeEach(() => vi.useFakeTimers());
  afterEach(async () => { for (const stop of stops.splice(0)) stop(); await vi.advanceTimersByTimeAsync(0); vi.useRealTimers(); });
  function service() {
    let snapshot = fixture(), active = 0, maximum = 0;
    const channels: ReadableStreamDefaultController<Uint8Array>[] = [];
    const fetcher = vi.fn<typeof fetch>(async (url, init) => {
      if (String(url) === '/v1/fleet') return Response.json(snapshot);
      let ended = false;
      const end = () => { if (!ended) { ended = true; active--; } };
      active++; maximum = Math.max(maximum, active);
      const body = new ReadableStream<Uint8Array>({ start(controller) {
        channels.push(controller);
        init?.signal?.addEventListener('abort', () => { end(); controller.error(new DOMException('Aborted', 'AbortError')); }, { once: true });
      }, cancel: end });
      return new Response(body, { headers: { 'Content-Type': 'text/event-stream' } });
    });
    const onFleet = vi.fn(), onState = vi.fn(), onError = vi.fn();
    const start = () => { const stop = watchFleet({ token: 'private-token', fetcher, onFleet, onState, onError }); stops.push(stop); return stop; };
    return { fetcher, onFleet, onState, onError, start, send: (data: string) => channels.at(-1)!.enqueue(new TextEncoder().encode(data)),
      setSnapshot: (next: ReturnType<typeof fixture>) => { snapshot = next; }, maximum: () => maximum,
      fleetCalls: () => fetcher.mock.calls.filter(([url]) => url === '/v1/fleet').length,
      streamCalls: () => fetcher.mock.calls.filter(([url]) => String(url).startsWith('/v1/events')).length };
  }
  it('uses one authenticated stream and makes no idle snapshot polls', async () => {
    const server = service(); server.start(); await vi.advanceTimersByTimeAsync(0);
    for (let i = 0; i < 5; i++) { server.send(': keepalive\n\n'); await vi.advanceTimersByTimeAsync(14_000); }
    expect(server.fleetCalls()).toBe(1); expect(server.streamCalls()).toBe(1); expect(server.maximum()).toBe(1);
    for (const [url, init] of server.fetcher.mock.calls) {
      expect(String(url)).not.toContain('private-token');
      expect(new Headers(init?.headers).get('Authorization')).toBe('Bearer private-token');
    }
  });
  it('coalesces bursts, preserves exact cursors, and ignores already covered events', async () => {
    const server = service(); server.start(); await vi.advanceTimersByTimeAsync(0);
    const next = fixture(); next.cursor.event_seq = '3'; server.setSnapshot(next);
    server.send(domain('1') + domain('2') + domain('3'));
    await vi.advanceTimersByTimeAsync(119); expect(server.fleetCalls()).toBe(1);
    await vi.advanceTimersByTimeAsync(1); expect(server.fleetCalls()).toBe(2);
    server.send(domain('3')); await vi.advanceTimersByTimeAsync(500);
    expect(server.fleetCalls()).toBe(2); expect(server.streamCalls()).toBe(1);
  });
  it('invalidates generation on recovery, takes a snapshot, and reconnects without overlap', async () => {
    const server = service(); server.start(); await vi.advanceTimersByTimeAsync(0);
    const next = fixture(); next.cursor = { recovery_generation: 'generation-2', event_seq: '7' }; server.setSnapshot(next);
    server.send('event: snapshot_required\ndata: recovery changed\n\n');
    await vi.advanceTimersByTimeAsync(500);
    expect(server.fleetCalls()).toBe(2); expect(server.streamCalls()).toBe(2); expect(server.maximum()).toBe(1);
    expect(String(server.fetcher.mock.calls.at(-1)?.[0])).toContain('generation=generation-2');
    expect(String(server.fetcher.mock.calls.at(-1)?.[0])).toContain('after=7');
  });
  it('cancels pending debounced snapshots on unmount', async () => {
    const server = service(); const stop = server.start(); await vi.advanceTimersByTimeAsync(0);
    server.send(domain('1')); await vi.advanceTimersByTimeAsync(0); stop();
    await vi.advanceTimersByTimeAsync(1000);
    expect(server.fleetCalls()).toBe(1); expect(server.streamCalls()).toBe(1);
  });
  it('stops automatic retries after a bounded exponential backoff', async () => {
    const fetcher = vi.fn<typeof fetch>().mockRejectedValue(new Error('Unavailable'));
    const onState = vi.fn(), onError = vi.fn();
    stops.push(watchFleet({ fetcher, onFleet: vi.fn(), onState, onError }));
    await vi.advanceTimersByTimeAsync(0);
    for (let attempt = 0; attempt < 6; attempt++) await vi.advanceTimersByTimeAsync(retryDelayMs(attempt));
    expect(fetcher).toHaveBeenCalledTimes(7); expect(onState).toHaveBeenLastCalledWith('paused');
    expect(onError.mock.lastCall?.[0]).toContain('Automatic retries paused');
    await vi.advanceTimersByTimeAsync(60_000); expect(fetcher).toHaveBeenCalledTimes(7);
    expect(retryDelayMs(99)).toBe(30_000);
  });
  it('pauses on authentication failure until access is changed or user retries', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(new Response('', { status: 401 }));
    const onState = vi.fn(), onError = vi.fn();
    stops.push(watchFleet({ fetcher, onFleet: vi.fn(), onState, onError }));
    await vi.advanceTimersByTimeAsync(10_000);
    expect(fetcher).toHaveBeenCalledTimes(1); expect(onState).toHaveBeenLastCalledWith('paused');
    expect(onError.mock.lastCall?.[0]).toContain('Update session access');
  });
  it('cancels and reconnects a stream missing its heartbeat', async () => {
    const server = service(); server.start(); await vi.advanceTimersByTimeAsync(45_500);
    expect(server.streamCalls()).toBe(2); expect(server.maximum()).toBe(1); expect(server.fleetCalls()).toBe(1);
  });
  it('refetches a snapshot after the server rejects a stale cursor with HTTP 409', async () => {
    let snapshots = 0, streams = 0;
    const fetcher = vi.fn<typeof fetch>(async (url, init) => {
      if (url === '/v1/fleet') {
        const snapshot = fixture(); snapshot.cursor.recovery_generation = `generation-${++snapshots}`;
        return Response.json(snapshot);
      }
      if (++streams === 1) return new Response('', { status: 409 });
      return new Response(new ReadableStream({ start(controller) {
        init?.signal?.addEventListener('abort', () => controller.error(new DOMException('Aborted', 'AbortError')), { once: true });
      } }), { headers: { 'Content-Type': 'text/event-stream' } });
    });
    stops.push(watchFleet({ fetcher, onFleet: vi.fn(), onState: vi.fn(), onError: vi.fn() }));
    await vi.advanceTimersByTimeAsync(500);
    expect(snapshots).toBe(2); expect(streams).toBe(2);
    expect(String(fetcher.mock.calls.at(-1)?.[0])).toContain('generation=generation-2');
  });
  it('handles live events above the JavaScript safe integer range without rounding', async () => {
    const server = service();
    const before = fixture(); before.cursor.event_seq = '9007199254740993'; server.setSnapshot(before);
    server.start(); await vi.advanceTimersByTimeAsync(0);
    const after = fixture(); after.cursor.event_seq = '9007199254740994'; server.setSnapshot(after);
    server.send(domain('9007199254740994')); await vi.advanceTimersByTimeAsync(120);
    expect(server.onFleet.mock.lastCall?.[0].cursor.event_seq).toBe('9007199254740994');
    expect(server.fleetCalls()).toBe(2);
  });
  it('bounds a stalled snapshot request and aborts it before retrying', async () => {
    const fetcher = vi.fn<typeof fetch>((_url, init) => new Promise((_resolve, reject) => {
      init?.signal?.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError')), { once: true });
    }));
    const onState = vi.fn();
    stops.push(watchFleet({ fetcher, onFleet: vi.fn(), onState, onError: vi.fn() }));
    await vi.advanceTimersByTimeAsync(15_500);
    expect(fetcher).toHaveBeenCalledTimes(2);
    expect(fetcher.mock.calls[0][1]?.signal?.aborted).toBe(true);
  });
});
