import { expect, it } from 'vitest';
import { spawn, type ChildProcess } from 'node:child_process';
import { createHash } from 'node:crypto';
import * as nodeFs from 'node:fs';
import { chmodSync, mkdtempSync, readFileSync, realpathSync } from 'node:fs';
import { createServer } from 'node:http';
import { join, resolve } from 'node:path';
import {
  DIST_DIR, REPO_ROOT, ensureBuiltDashboard, launchBrowserHarness,
  type BrowserHarness,
} from './browser-harness';

// The existing E2E ambient shim deliberately exposes a small subset of Node.
// Keep additional descriptor operations local to this fixture; no harness,
// dependency or shared ambient edits are needed.
interface FileStat {
  dev: number; ino: number; size: number; uid: number; mode: number;
  nlink: number; mtimeMs: number; ctimeMs: number;
  isFile(): boolean;
}
const fdFs = nodeFs as unknown as {
  constants: { O_RDONLY: number; O_NOFOLLOW: number; O_NONBLOCK: number };
  openSync(path: string, flags: string | number, mode?: number): number;
  closeSync(fd: number): void;
  fstatSync(fd: number): FileStat;
  lstatSync(path: string): FileStat;
  readSync(fd: number, buffer: Uint8Array, offset: number, length: number, position: number | null): number;
  writeSync(fd: number, value: string): number;
  fsyncSync(fd: number): void;
};
const runtime = process as typeof process & { getuid(): number };
const MAX_RESPONSE_BYTES = 256 * 1024;
const READY_MS = 25_000;
const RAW = 'Preserve this entire original request as one.\n\n第二段 — keep Unicode 🧭 and e\u0301.\nNo task contract is inferred.';
const SOURCE_ID = 'browser-unicode-source';
const SOURCE_REVISION = 'draft-one';
const CAPTURE_ID = 'browser-unicode-capture';
const OBSERVED_LOCAL_TIME = '2026-10-19T07:25';
type Json = Record<string, unknown>;
const sleep = (ms: number) => new Promise<void>(done => setTimeout(done, ms));

function writePrivate(path: string, value: unknown): void {
  const fd = fdFs.openSync(path, 'wx', 0o600);
  try {
    const text = typeof value === 'string' ? value : JSON.stringify(value, null, 2) + '\n';
    if (fdFs.writeSync(fd, text) !== new TextEncoder().encode(text).byteLength) {
      throw new Error('Private fixture write was incomplete');
    }
    fdFs.fsyncSync(fd);
    if ((fdFs.fstatSync(fd).mode & 0o7777) !== 0o600) throw new Error('Fixture file is not mode0600');
  } finally { fdFs.closeSync(fd); }
}
function fileIdentity(stat: FileStat): string {
  return [stat.dev, stat.ino, stat.size, stat.uid, stat.mode, stat.nlink, stat.mtimeMs, stat.ctimeMs].join(':');
}
function pinNativeBinary(): { path: string; sha256: string; fd: number; identity: string } {
  const supplied = process.env.ADMIRALTY_E2E_ADMIRALTYD_BIN;
  const pin = process.env.ADMIRALTY_E2E_ADMIRALTYD_SHA256;
  if (!supplied || !supplied.startsWith('/') || !pin || !/^[0-9a-f]{64}$/.test(pin)) {
    throw new Error('Intake E2E requires absolute ADMIRALTY_E2E_ADMIRALTYD_BIN and exact ADMIRALTY_E2E_ADMIRALTYD_SHA256; no build/fallback/skip is permitted');
  }
  const path = realpathSync(supplied);
  if (path !== resolve(supplied)) throw new Error('Use the canonical native binary path');
  const fd = fdFs.openSync(path, fdFs.constants.O_RDONLY | fdFs.constants.O_NOFOLLOW | fdFs.constants.O_NONBLOCK);
  try {
    const before = fdFs.fstatSync(fd);
    if (!before.isFile() || before.uid !== runtime.getuid() || before.size <= 0 || before.size > 512 * 1024 * 1024) {
      throw new Error('Native binary must be an owned bounded regular file');
    }
    const hash = createHash('sha256'), buffer = new Uint8Array(64 * 1024);
    let bytes = 0;
    for (;;) {
      const n = fdFs.readSync(fd, buffer, 0, buffer.length, null);
      if (!n) break;
      bytes += n;
      if (bytes > before.size) throw new Error('Native binary changed while hashing');
      hash.update(buffer.subarray(0, n) as Buffer);
    }
    if (bytes !== before.size || hash.digest('hex') !== pin
      || fileIdentity(fdFs.fstatSync(fd)) !== fileIdentity(before)
      || fileIdentity(fdFs.lstatSync(path)) !== fileIdentity(before)) throw new Error('Native binary pin/identity mismatch');
    return { path, sha256: pin, fd, identity: fileIdentity(before) };
  } catch (error) { fdFs.closeSync(fd); throw error; }
}
async function boundedJson(response: Response): Promise<Json> {
  const reader = response.body?.getReader();
  if (!reader) throw new Error('Native JSON response has no body');
  let bytes = 0, text = '';
  const decoder = new TextDecoder();
  try {
    for (;;) {
      const chunk = await reader.read();
      if (chunk.done) break;
      bytes += chunk.value.byteLength;
      if (bytes > MAX_RESPONSE_BYTES) throw new Error('Native response exceeds fixture bound');
      text += decoder.decode(chunk.value, { stream: true });
    }
    text += decoder.decode();
    const value: unknown = JSON.parse(text);
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Expected native JSON object');
    return value as Json;
  } finally { void reader.cancel().catch(() => undefined); }
}

interface NativeFixture {
  root: string; baseUrl: string; ownerToken: string; intakeToken: string;
  binarySha256: string; build: Awaited<ReturnType<typeof ensureBuiltDashboard>>;
  fleet(timeoutMs?: number): Promise<Json>;
  close(): Promise<void>;
}
async function startPrivateNativeFixture(): Promise<NativeFixture> {
  const binary = pinNativeBinary();
  const root = mkdtempSync(join(realpathSync('/tmp'), 'adm-intake-browser-'));
  chmodSync(root, 0o700);
  const ownerToken = `adm-owner-${crypto.randomUUID()}-${crypto.randomUUID()}`;
  const intakeToken = `adm-intake-${crypto.randomUUID()}-${crypto.randomUUID()}`;
  const redact = (text: string) => text.replaceAll(ownerToken, '[owner credential]').replaceAll(intakeToken, '[intake credential]').replace(/[\u0000-\u0008\u000b-\u001f\u007f]/g, '');
  let child: ChildProcess | undefined, exited = false, stderr = '', stderrOverflow = false;
  let exitState: { code?: number | null; signal?: string | null; spawnError?: boolean } = {};
  let resolveExit: () => void = () => undefined;
  const exit = new Promise<void>(done => { resolveExit = done; });
  let closed = false;
  async function close(): Promise<void> {
    if (closed) return;
    closed = true;
    let confirmed = !child || exited;
    if (child && !exited) {
      child.kill('SIGTERM'); // Only this fixture's direct child, never a named service/session.
      await Promise.race([exit, sleep(1_000)]);
      if (!exited) child.kill('SIGKILL');
      await Promise.race([exit, sleep(1_000)]);
      confirmed = exited;
    }
    try {
      writePrivate(join(root, 'native-exit.json'), {
        ...exitState, owned_child_pid: (child as (ChildProcess & { pid?: number }) | undefined)?.pid ?? null,
        confirmed, stderrOverflow, stderr: redact(stderr).slice(-16 * 1024),
      });
    } finally { fdFs.closeSync(binary.fd); }
    if (!confirmed) throw new Error(`Owned native child termination is unconfirmed; preserve ${root}`);
    // Preserve this private fixture on success AND failure for independent proof.
    // Token/config files stay0600; no production credential was used or copied.
  }
  try {
    const build = await ensureBuiltDashboard(); // The existing harness owns the single source-bound Vite build.
    const tokenFile = join(root, 'owner-token'), intakeTokenFile = join(root, 'intake-token');
    writePrivate(tokenFile, ownerToken); writePrivate(intakeTokenFile, intakeToken);
    const nativeConfig = join(root, 'native.json');
    writePrivate(nativeConfig, { schema_version: 1, operator: { principal: 'owner', projects: ['my-project'] }, agents: [] });
    const reservation = createServer((_request, response) => response.end());
    await new Promise<void>((done, reject) => {
      reservation.once('error', reject);
      reservation.listen(0, '127.0.0.1', done);
    });
    const address = reservation.address();
    if (!address || typeof address === 'string') throw new Error('No ephemeral loopback address');
    const baseUrl = `http://127.0.0.1:${address.port}`;
    (reservation as typeof reservation & { closeAllConnections?(): void }).closeAllConnections?.();
    await Promise.race([
      new Promise<void>((done, reject) => reservation.close(error => error ? reject(error) : done())),
      sleep(1_000).then(() => { throw new Error('Owned port reservation did not close'); }),
    ]);
    const intake = JSON.parse(readFileSync(join(REPO_ROOT, 'configs/examples/intake.json'), 'utf8')) as {
      origins: string[]; credentials: Array<{ token_file: string }>;
    };
    intake.origins = [baseUrl]; intake.credentials[0].token_file = intakeTokenFile;
    const intakeConfig = join(root, 'intake.json'); writePrivate(intakeConfig, intake);
    if (fileIdentity(fdFs.fstatSync(binary.fd)) !== binary.identity || fileIdentity(fdFs.lstatSync(binary.path)) !== binary.identity) {
      throw new Error('Pinned native executable changed before spawn');
    }
    child = spawn(binary.path, ['--assets', DIST_DIR, '--database', join(root, 'native.sqlite'), '--listen', `127.0.0.1:${address.port}`,
      '--token-file', tokenFile, '--native-config', nativeConfig, '--intake-config', intakeConfig],
    { stdio: ['ignore', 'ignore', 'pipe'], env: { PATH: '/usr/bin:/bin', TMPDIR: root } });
    child.once('exit', (code, signal) => { exited = true; exitState = { code, signal }; resolveExit(); });
    child.once('error', () => { exited = true; exitState = { spawnError: true }; resolveExit(); });
    child.stderr?.on('data', (chunk: Buffer) => {
      stderr = redact(stderr + chunk.toString('utf8'));
      if (new TextEncoder().encode(stderr).byteLength > 16 * 1024) {
        stderrOverflow = true; stderr = stderr.slice(-8 * 1024); child?.kill('SIGKILL');
      }
    });
    const fleet = async (timeoutMs = 2_000) => {
      const response = await fetch(`${baseUrl}/v2/fleet`, { headers: { Authorization: `Bearer ${ownerToken}` }, signal: AbortSignal.timeout(timeoutMs) });
      if (response.status !== 200) throw new Error(`Owned native fleet status${response.status}`);
      return boundedJson(response);
    };
    const deadline = Date.now() + READY_MS;
    let ready = false, lastReadyReason = 'listener has not responded';
    while (Date.now() < deadline) {
      if (exited || stderrOverflow) throw new Error(`Owned child exited during startup (${JSON.stringify(exitState)}); ${redact(stderr)}`);
      try { await fleet(Math.max(1, Math.min(300, deadline - Date.now()))); ready = true; break; }
      catch (error) {
        // Every fetch AND response body is bounded by the remaining outer8s budget.
        lastReadyReason = error instanceof Error ? redact(error.message).slice(0, 200) : 'readiness response failed';
      }
      await sleep(Math.min(35, Math.max(0, deadline - Date.now())));
    }
    if (!ready) throw new Error(`Owned native readiness exceeded8s (${lastReadyReason}); ${redact(stderr)}`);
    return { root, baseUrl, ownerToken, intakeToken, binarySha256: binary.sha256, build, fleet, close };
  } catch (error) {
    await close();
    const detail = error instanceof Error ? redact(error.message).slice(0, 2_000) : 'fixture setup failed';
    throw new Error(`Intake native fixture failed; preserve ${root}: ${detail}`);
  }
}

interface TransportProof {
  requests: Array<{ path: string; body: string }>;
  responses: Array<{ path: string; body: string }>;
  dropped: number;
}
async function installOneLostReply(browser: BrowserHarness, secrets: string[]): Promise<void> {
  await browser.evaluate(`(() => {
    const original = window.fetch.bind(window), secrets = ${JSON.stringify(secrets)};
    const proof = { requests: [], responses: [], dropped: 0 };
    window.__intakeProof = proof;
    const safe = text => { if (secrets.some(secret => text.includes(secret))) throw new Error('Credential exposed outside authorization headers'); return text; };
    const body = async response => {
      const reader = response.body.getReader(), decoder = new TextDecoder(); let size = 0, text = '';
      try { for (;;) { const chunk = await reader.read(); if (chunk.done) break; size += chunk.value.byteLength;
        if (size > 65536) throw new Error('Intake response capture bound'); text += decoder.decode(chunk.value, {stream:true}); }
        return safe(text + decoder.decode());
      } finally { void reader.cancel().catch(() => {}); }
    };
    window.fetch = async (input, init) => {
      const url = new URL(typeof input === 'string' ? input : input instanceof URL ? input.href : input.url, location.href);
      safe(url.href);
      const method = (init?.method || (input instanceof Request ? input.method : 'GET')).toUpperCase();
      const record = url.origin === location.origin && url.pathname.startsWith('/v2/') && method === 'POST';
      if (!record) return original(input, init);
      if (proof.requests.length >= 16 || typeof init?.body !== 'string' || new TextEncoder().encode(init.body).byteLength > 16384) throw new Error('Intake request capture bound');
      proof.requests.push({path:url.pathname, body:safe(init.body)});
      const hold = url.pathname === '/v2/intake/duplicates/query' && window.__holdNextDuplicate === true;
      if (hold) window.__holdNextDuplicate = false;
      const response = await original(input, {...init,signal:hold ? AbortSignal.timeout(5000) : init.signal || AbortSignal.timeout(5000)});
      proof.responses.push({path:url.pathname,body:await body(response.clone())});
      if (hold) {
        window.__duplicateHeld = true;
        await new Promise(resolve => { window.__releaseDuplicate = resolve; });
        window.__duplicateReleased = true;
      }
      if (url.pathname === '/v2/intake/captures' && response.ok && proof.dropped === 0) {
        proof.dropped++;
        throw new TypeError('Controlled browser transport loss AFTER actual native capture response');
      }
      return response;
    };
  })()`, 2_500);
}
function assertNoGrant(envelope: Json): void {
  expect(envelope.schema_version).toBe(1);
  expect(envelope.execution_granted).toBe(false);
  expect(envelope.acceptance_granted).toBe(false);
}
function assertEmptyNativeFleet(snapshot: Json): void {
  expect(snapshot.tasks).toEqual([]);
  expect(snapshot.simulated).toBe(false);
  expect(snapshot.native_execution_qualified).toBe(false);
  const coordination = snapshot.coordination as Json;
  expect(coordination.queued_messages).toBe(0);
  expect(coordination.acknowledged_messages).toBe(0);
}
async function waitForConfirmedReplyLoss(browser: BrowserHarness): Promise<void> {
  const deadline = Date.now() + 8_000;
  while (Date.now() < deadline) {
    const confirmed = await browser.evaluate<boolean>(`Boolean(window.__intakeProof?.dropped === 1 && [...document.querySelectorAll('button')].some(button => button.textContent.includes('Retry same capture request') && !button.disabled))`, Math.min(1_000, Math.max(1, deadline - Date.now())));
    if (confirmed) return;
    await sleep(Math.min(25, Math.max(0, deadline - Date.now())));
  }
  throw new Error('The real committed reply was not dropped and retained as an actionable pending request');
}

it('captures a real native Unicode source once and replays an identical browser request after a lost reply', async () => {
  const native = await startPrivateNativeFixture();
  let browser: BrowserHarness | undefined;
  let failure: unknown;
  try {
    const before = await native.fleet(); assertEmptyNativeFleet(before);
    browser = await launchBrowserHarness(); // Existing sandbox/startup/buffer bounds stay intact.
    await browser.goto(`${native.baseUrl}/#/intake`);
    await browser.waitForText('Raw request intake', 8_000);
    await installOneLostReply(browser, [native.ownerToken, native.intakeToken]);
    await browser.clickButtonByText('Owner access');
    await browser.waitForText('Session access');
    await browser.fillInputByLabel('Bearer token', native.ownerToken);
    await browser.clickButtonByText('Apply token');
    for (const [label, value] of [
      ['Project ID', 'my-project'], ['Root ID', 'my-root'], ['Source ID', SOURCE_ID],
      ['Source revision', SOURCE_REVISION], ['Observed draft time', OBSERVED_LOCAL_TIME],
      ['Original raw request', RAW], ['Capture ID', CAPTURE_ID], ['Separate intake token', native.intakeToken],
    ]) await browser.fillInputByLabel(label, value);
    const observedUtc = await browser.evaluate<string>(`new Date(${JSON.stringify(OBSERVED_LOCAL_TIME)}).toISOString()`, 2_500);
    await browser.clickButtonByText('Preview exact source');
    await browser.waitForSelector('article[aria-label="Intake preview"]', 8_000);
    await browser.waitForText('1 request', 5_000);
    expect(await browser.evaluate<number>(`window.__intakeProof.requests.filter(r => r.path === '/v2/intake/duplicates/query').length`, 2_500)).toBe(0);
    await browser.clickButtonByText('Find related work');
    await browser.waitForText('Coverage incomplete:', 8_000);
    await browser.waitForText('No related work returned on this page.', 5_000);
    await browser.clickButtonByText('Capture unresolved request');
    await browser.waitForText('No receipt is confirmed for this command.', 8_000);
    await waitForConfirmedReplyLoss(browser);
    // A real SPA navigation must retain the unknown command, not mint a new one.
    await browser.clickNav('Overview');
    await browser.waitForText('Native task projections; separate from the detailed producer fleet.', 5_000);
    expect(await browser.evaluate<boolean>(`Boolean(document.querySelector('[aria-labelledby="intake-title"]')?.closest('[hidden]'))`, 2_500)).toBe(true);
    await browser.clickNav('Intake');
    await browser.waitForText('Preserve an original request, inspect unresolved facts', 5_000);
    await browser.waitForText('No receipt is confirmed for this command.', 5_000);
    expect(await browser.evaluate<boolean>(`Boolean(document.querySelector('[aria-labelledby="intake-title"]')?.closest('[hidden]'))`, 2_500)).toBe(false);
    await browser.clickButtonByText('Retry same capture request');
    await browser.waitForText('Original immutable capture receipt replayed.', 8_000);
    await browser.waitForSelector('article[aria-label="Recorded original source"]', 8_000);
    expect(await browser.evaluate<string>(`document.querySelector('article[aria-label="Recorded original source"] pre')?.textContent || ''`, 2_500)).toBe(RAW);
    const proof = await browser.evaluate<TransportProof>('window.__intakeProof', 2_500);
    expect(proof.dropped).toBe(1);
    expect(proof.requests.map(request => request.path)).toEqual([
      '/v2/intake/preview', '/v2/intake/duplicates/query', '/v2/intake/captures', '/v2/intake/captures', `/v2/intake/captures/${CAPTURE_ID}`,
    ]);
    expect(proof.requests[2].body).toBe(proof.requests[3].body);
    const previewEnvelope = JSON.parse(proof.responses[0].body) as Json; assertNoGrant(previewEnvelope);
    const preview = previewEnvelope.data as { requests: Array<{ text: string; unresolved: unknown[]; outcomes: unknown[] }>; raw_byte_length: number };
    expect(preview.requests).toHaveLength(1); expect(preview.requests[0].text).toBe(RAW);
    expect(preview.requests[0].unresolved.length).toBeGreaterThan(0); expect(preview.requests[0].outcomes).toEqual([]);
    expect(preview.raw_byte_length).toBe(new TextEncoder().encode(RAW).byteLength);
    const duplicateRequest = JSON.parse(proof.requests[1].body) as Json;
    expect(duplicateRequest).toEqual({ project_id: 'my-project', root_id: 'my-root', query: {
      schema_version: 1, project_id: 'my-project', raw_digest: (previewEnvelope.data as Json).raw_digest,
      canonical_digest: (previewEnvelope.data as Json).canonical_digest, origin: null, command_id: null, cursor: null, limit: 20,
    } });
    const duplicates = JSON.parse(proof.responses[1].body) as Json; assertNoGrant(duplicates);
    expect((duplicates.data as Json).candidates).toEqual([]);
    expect(((duplicates.data as Json).coverage as Json).kind).toBe('incomplete');
    const capture = JSON.parse(proof.requests[2].body) as Json;
    expect(capture.source_payload).toEqual({ kind: 'inline_utf8', text: RAW });
    expect(capture.format).toBe('plain_text'); expect(capture.revision).toBe(1); expect(capture.expected_revision).toBe(0);
    expect(capture.provenance).toEqual({ schema_version: 1, source_id: SOURCE_ID, revision: SOURCE_REVISION, captured_at: observedUtc, requester: { kind: 'unknown' }, origin: null });
    expect(capture.scope).toMatchObject({ principal: 'operator', project_id: 'my-project', root_id: 'my-root', authority_generation: '1' });
    expect(capture.command_id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    const first = JSON.parse(proof.responses[2].body) as Json, replay = JSON.parse(proof.responses[3].body) as Json;
    assertNoGrant(first); assertNoGrant(replay);
    expect((first.data as Json).replayed).toBe(false); expect((replay.data as Json).replayed).toBe(true);
    expect((first.data as Json).receipt).toEqual((replay.data as Json).receipt);
    expect((first.data as Json).receipt_digest).toBe((replay.data as Json).receipt_digest);
    expect(JSON.parse(proof.requests[4].body)).toEqual({ project_id: 'my-project', root_id: 'my-root', revision: 1 });
    const read = JSON.parse(proof.responses[4].body) as Json; assertNoGrant(read);
    expect((read.data as Json).request).toEqual(capture);
    expect((read.data as Json).receipt_digest).toBe((first.data as Json).receipt_digest);
    // Hold an ACTUAL native query reply after transport receipt. Credential change
    // must invalidate results even when this fetch wrapper ignores AbortSignal.
    await browser.evaluate(`window.__holdNextDuplicate = true; window.__duplicateHeld = false; window.__duplicateReleased = false`, 2_500);
    await browser.clickButtonByText('Find related work');
    const heldDeadline = Date.now() + 5_000;
    while (!await browser.evaluate<boolean>('window.__duplicateHeld', 500)) {
      if (Date.now() >= heldDeadline) throw new Error('Actual duplicate reply was not held');
      await sleep(20);
    }
    expect(await browser.evaluate<boolean>(`[...document.querySelectorAll('button')].some(button => button.textContent === 'Searching…' && button.disabled)`, 2_500)).toBe(true);
    await browser.waitForText('Searching related work…', 2_500);
    await browser.fillInputByLabel('Separate intake token', 'changed-private-fixture-credential');
    expect(await browser.evaluate<boolean>(`Boolean(document.querySelector('section[aria-label="Related work results"]'))`, 2_500)).toBe(false);
    await browser.evaluate(`window.__releaseDuplicate(); new Promise(resolve => setTimeout(resolve, 75))`, 2_500);
    expect(await browser.evaluate<boolean>('window.__duplicateReleased', 2_500)).toBe(true);
    expect(await browser.evaluate<boolean>(`Boolean(document.querySelector('section[aria-label="Related work results"]'))`, 2_500)).toBe(false);
    await browser.fillInputByLabel('Separate intake token', native.intakeToken);
    const finalProof = await browser.evaluate<TransportProof>('window.__intakeProof', 2_500);
    expect(finalProof.requests).toHaveLength(6);
    expect(finalProof.requests[5].path).toBe('/v2/intake/duplicates/query');
    expect(finalProof.requests[5].body).toBe(proof.requests[1].body);
    assertNoGrant(JSON.parse(finalProof.responses[5].body) as Json);
    const leak = await browser.evaluate<{ markup: string; storage: string; urls: string }>(`({markup:document.documentElement.outerHTML,storage:JSON.stringify({local:Object.entries(localStorage),session:Object.entries(sessionStorage)}),urls:JSON.stringify([location.href,...[...document.querySelectorAll('[href],[src],[action]')].flatMap(e=>['href','src','action'].map(k=>e.getAttribute(k)||''))])})`, 2_500);
    for (const secret of [native.ownerToken, native.intakeToken]) {
      expect(leak.markup.includes(secret)).toBe(false); expect(leak.storage.includes(secret)).toBe(false); expect(leak.urls.includes(secret)).toBe(false);
      expect(JSON.stringify(finalProof).includes(secret)).toBe(false);
    }
    const after = await native.fleet(); assertEmptyNativeFleet(after);
    const accessibility = await browser.runAxeAudit();
    expect(accessibility.violations, JSON.stringify(accessibility.violations)).toEqual([]);
    expect(accessibility.incomplete, JSON.stringify(accessibility.incomplete)).toEqual([]);
    writePrivate(join(native.root, 'browser-proof.json'), {
      binary_sha256: native.binarySha256, production_build: native.build,
      accessibility,
      transport: finalProof, duplicate_query_scope: {actual_native_queries: 2, empty_page_coverage: 'incomplete', stale_reply_after_credential_change_discarded: true, provider_or_acceptance: false}, native_tasks_before: before.tasks, native_tasks_after: after.tasks,
      native_execution_qualified: false, provider_executed: false, acceptance_granted: false,
      outbox_scope: 'No public outbox inspection API; assert zero native tasks/messages and only preview/duplicate-query/capture/read POSTs, not general delivery qualification',
      abandoned_reload_scope: 'SPA navigation/back retains pending envelope; reload/close discards page memory and is not exercised',
    });
  } catch (error) {
    failure = error;
    // Preserve the exact request/reply trace before closing this owned browser.
    // Never persist markup, headers or credential values even on a failed check.
    if (browser) {
      try {
        const transport = await browser.evaluate<TransportProof | null>('window.__intakeProof || null', 1_000);
        const data = { stage: 'browser-assertion-failed', transport, native_execution_qualified: false };
        const serialized = JSON.stringify(data);
        if (new TextEncoder().encode(serialized).byteLength > 2 * 1024 * 1024
          || serialized.includes(native.ownerToken) || serialized.includes(native.intakeToken)) {
          throw new Error('Failure trace is unsafe or exceeds its bound');
        }
        writePrivate(join(native.root, 'browser-failure-proof.json'), data);
      } catch {
        try { writePrivate(join(native.root, 'browser-failure-metadata.json'), { transport_available: false, diagnostic: 'Browser trace unavailable, unsafe or over bound; private native fixture retained' }); }
        catch { /* Preserve the original failure; no unsafe cleanup or overwrite. */ }
      }
    }
  }
  finally {
    try { await browser?.close(); } catch (error) { failure ??= error; }
    try { await native.close(); } catch (error) { failure ??= error; }
  }
  if (failure) throw new Error(`Native intake browser proof failed; private evidence preserved at ${native.root}: ${failure instanceof Error ? failure.message.replaceAll(native.ownerToken, '[owner credential]').replaceAll(native.intakeToken, '[intake credential]') : 'unknown failure'}`);
  // Emit only bounded non-secret metadata, after confirmed direct-child teardown.
  console.info(JSON.stringify({ kind: 'native-intake-browser-proof', root: native.root, binary_sha256: native.binarySha256 }));
}, 120_000);
