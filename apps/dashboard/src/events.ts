import { ApiError, getNativeFleet, type Cursor, type NativeFleet } from './api';
import { parseCursor, parseDomainRecordV2 } from './validation';

export type StreamFrame = { event: string; id?: string; data: string };
const MAX_FRAME_CHARACTERS = 2 * 1024 * 1024;

/** Incremental SSE framing, including split CRLF and multiline data. */
export function createSseParser(onFrame: (frame: StreamFrame) => void) {
  let buffer = '', event = '', id: string | undefined, data: string[] = [], size = 0;
  function line(value: string) {
    size += value.length;
    if (size > MAX_FRAME_CHARACTERS) throw new Error('Event stream frame exceeds size limit');
    if (!value) {
      if (data.length) onFrame({ event: event || 'message', id, data: data.join('\n') });
      event = ''; id = undefined; data = []; size = 0;
      return;
    }
    if (value.startsWith(':')) return;
    const colon = value.indexOf(':');
    const field = colon < 0 ? value : value.slice(0, colon);
    let contents = colon < 0 ? '' : value.slice(colon + 1);
    if (contents.startsWith(' ')) contents = contents.slice(1);
    if (field === 'event') event = contents;
    if (field === 'id' && !contents.includes('\0')) id = contents;
    if (field === 'data') data.push(contents);
  }
  return { push(chunk: string) {
    buffer += chunk;
    for (;;) {
      const end = buffer.search(/[\r\n]/);
      if (end < 0 || (buffer[end] === '\r' && end === buffer.length - 1)) break;
      const width = buffer[end] === '\r' && buffer[end + 1] === '\n' ? 2 : 1;
      line(buffer.slice(0, end)); buffer = buffer.slice(end + width);
    }
    if (buffer.length + size > MAX_FRAME_CHARACTERS) throw new Error('Event stream frame exceeds size limit');
  } };
}
export function cursorFromId(id: string | undefined): Cursor {
  const colon = id?.lastIndexOf(':') ?? -1;
  if (!id || colon < 1) throw new Error('Event stream is missing a durable cursor');
  return parseCursor({ recovery_generation: id.slice(0, colon), event_seq: id.slice(colon + 1) });
}
export function retryDelayMs(attempt: number): number { return Math.min(30_000, 500 * 2 ** Math.min(Math.max(attempt, 0), 6)); }
export type ConnectionState = 'connecting' | 'connected' | 'reconnecting' | 'paused';
type WatchOptions = { token?: string; onFleet: (fleet: NativeFleet) => void; onState: (state: ConnectionState) => void;
  onError: (message: string) => void; fetcher?: typeof fetch };

/** One sequential fetch stream; idle fleets trigger no additional snapshot reads. */
export function watchFleet(options: WatchOptions): () => void {
  const fetcher = options.fetcher ?? fetch;
  const lifetime = new AbortController();
  let stream: AbortController | null = null, cursor: Cursor | null = null;
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;
  let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  let wakeReconnect: (() => void) | undefined;
  let refreshInFlight: Promise<void> | null = null, dirty = false;
  async function refresh() {
    if (refreshInFlight) { dirty = true; return refreshInFlight; }
    refreshInFlight = (async () => {
      const snapshotRequest = new AbortController();
      const abort = () => snapshotRequest.abort();
      lifetime.signal.addEventListener('abort', abort, { once: true });
      const deadline = setTimeout(abort, 15_000);
      let result: NativeFleet;
      try { result = await getNativeFleet(options.token, snapshotRequest.signal, fetcher); }
      finally { clearTimeout(deadline); lifetime.signal.removeEventListener('abort', abort); }
      if (lifetime.signal.aborted) return;
      if (!cursor || cursor.recovery_generation !== result.cursor.recovery_generation) {
        if (cursor) stream?.abort();
        cursor = result.cursor;
      } else if (BigInt(result.cursor.event_seq) > BigInt(cursor.event_seq)) cursor = result.cursor;
      options.onFleet(result); options.onError('');
    })();
    try { await refreshInFlight; } finally {
      refreshInFlight = null;
      if (dirty && !lifetime.signal.aborted) { dirty = false; scheduleRefresh(); }
    }
  }
  function scheduleRefresh() {
    if (refreshTimer || lifetime.signal.aborted) return;
    refreshTimer = setTimeout(() => {
      refreshTimer = undefined;
      void refresh().catch(error => { if (!lifetime.signal.aborted) { cursor = null; options.onError(error instanceof Error ? error.message : 'Snapshot refresh failed'); stream?.abort(); } });
    }, 120);
  }
  function pause(delay: number) {
    return new Promise<void>(resolve => {
      wakeReconnect = resolve;
      reconnectTimer = setTimeout(() => { reconnectTimer = undefined; wakeReconnect = undefined; resolve(); }, delay);
    });
  }
  async function run() {
    let failures = 0;
    while (!lifetime.signal.aborted) {
      let opened = 0, needsSnapshot = false;
      try {
        options.onState(failures ? 'reconnecting' : 'connecting');
        if (!cursor) await refresh();
        if (lifetime.signal.aborted || !cursor) return;
        const currentCursor: Cursor = cursor;
        stream = new AbortController();
        const query = new URLSearchParams({ after: currentCursor.event_seq, generation: currentCursor.recovery_generation });
        const handshake = setTimeout(() => stream?.abort(), 15_000);
        let response: Response;
        try {
          response = await fetcher(`/v2/events?${query}`, {
            headers: { Accept: 'text/event-stream', ...(options.token ? { Authorization: `Bearer ${options.token}` } : {}) },
            signal: stream.signal, cache: 'no-store',
          });
        } finally { clearTimeout(handshake); }
        if (lifetime.signal.aborted) { await response.body?.cancel(); return; }
        if (!response.ok) throw new ApiError(`Event stream failed (${response.status})`, response.status);
        if (!response.headers.get('content-type')?.includes('text/event-stream') || !response.body) throw new Error('Unsupported event stream response');
        options.onState('connected'); options.onError(''); opened = Date.now();
        const parser = createSseParser(frame => {
          if (frame.event === 'snapshot_required') { needsSnapshot = true; throw new Error('Snapshot required after recovery'); }
          if (frame.event === 'error') throw new Error('Event stream reported an error');
          if (frame.event !== 'domain') return;
          const incoming = cursorFromId(frame.id);
          const record = parseDomainRecordV2(JSON.parse(frame.data));
          if (incoming.event_seq !== record.seq || incoming.recovery_generation !== cursor?.recovery_generation) {
            needsSnapshot = true; throw new Error('Event stream generation changed');
          }
          if (BigInt(incoming.event_seq) <= BigInt(cursor.event_seq)) return;
          cursor = incoming; failures = 0; scheduleRefresh();
        });
        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        let heartbeat: ReturnType<typeof setTimeout> | undefined;
        try {
          while (!lifetime.signal.aborted) {
            heartbeat = setTimeout(() => stream?.abort(), 45_000);
            const result = await reader.read();
            clearTimeout(heartbeat);
            if (lifetime.signal.aborted) return;
            if (result.done) throw new Error('Event stream ended');
            parser.push(decoder.decode(result.value, { stream: true }));
          }
        } finally {
          clearTimeout(heartbeat);
          await reader.cancel().catch(() => undefined); reader.releaseLock();
        }
      } catch (error) {
        stream?.abort(); stream = null;
        if (lifetime.signal.aborted) return;
        if (error instanceof ApiError && error.status === 409) needsSnapshot = true;
        if (needsSnapshot) cursor = null;
        if (opened && Date.now() - opened >= 30_000) failures = 0;
        const message = error instanceof Error ? error.message : 'Fleet connection unavailable';
        const unauthorized = error instanceof ApiError && [401, 403].includes(error.status);
        if (unauthorized || failures >= 6) {
          options.onState('paused');
          options.onError(unauthorized ? `${message}. Update session access, then retry.` : `${message}. Automatic retries paused; retry when service is ready.`);
          return;
        }
        options.onState('reconnecting'); options.onError(message);
        await pause(retryDelayMs(failures++));
      }
    }
  }
  void run();
  return () => { lifetime.abort(); stream?.abort(); clearTimeout(refreshTimer); clearTimeout(reconnectTimer); wakeReconnect?.(); };
}
