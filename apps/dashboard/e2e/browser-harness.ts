import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { homedir, tmpdir } from 'node:os';
import { dirname, extname, join, normalize, resolve } from 'node:path';
import type { Duplex, Readable, Writable } from 'node:stream';
import { fileURLToPath } from 'node:url';
import { validateTaskContract } from '../../../packages/contracts/index';
import {
  buildAxeRunExpression,
  loadPinnedAxeCoreBundle,
  WCAG21_AA_TAGS,
  type AxeAuditReport,
  type CustomAxTreeSmokeSummary,
} from './axe-audit';
import {
  computeCanonicalTaskViewDigest,
  createFleetFixture,
  createLegacyFixture,
  createModelsFixture,
  createNativeFleetV2Fixture,
  createTaskContractFixture,
  projectLegacyDetailSnapshot,
  fleetV1ToV2,
  serializeCanonicalTaskContract,
  type FleetFixture,
  type ModelsFixture,
  type NativeFleetV2Fixture,
  type NativeTaskViewFixture,
  type TaskContractFixture,
} from './fixtures';

const E2E_DIR = fileURLToPath(new URL('.', import.meta.url));
export const DASHBOARD_ROOT = resolve(E2E_DIR, '..');
export const REPO_ROOT = resolve(DASHBOARD_ROOT, '../..');
export const DIST_DIR = resolve(DASHBOARD_ROOT, 'dist');
export const BUILD_IDENTITY_FILE = join(DIST_DIR, '.admiralty-build-identity.json');

const CDP_COMMAND_TIMEOUT_MS = 30_000;
const BROWSER_STARTUP_TIMEOUT_MS = 25_000;
const MAX_BROWSER_STARTUP_ATTEMPTS = 2;
const GOTO_NAVIGATE_TIMEOUT_MS = 20_000;
// A navigation command receipt precedes document/script loading. Give that
// separate stage an explicit bounded budget within the 90-second test budget.
const GOTO_DOCUMENT_TIMEOUT_MS = 30_000;
const STOP_LOADING_TIMEOUT_MS = 5_000;
export const FORM_CONTROL_STAGE_TIMEOUT_MS = 10_000;
export const MAX_STAGE_EVALUATE_BUDGET_MS = 5_000;

export const MAX_HTTP_BODY_BYTES = 256 * 1024;
export const MAX_RECORDED_REQUESTS = 512;
export const MAX_RETAINED_REQUEST_BODY_BYTES = 64 * 1024;
export const MAX_ACTIVE_SSE_STREAMS = 16;

export const MAX_CDP_FRAME_BYTES = 4 * 1024 * 1024;
export const MAX_CDP_BUFFER_BYTES = 8 * 1024 * 1024;
export const MAX_PENDING_CDP_COMMANDS = 64;

const CANONICAL_UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const SHA256_DIGEST_RE = /^sha256:[0-9a-f]{64}$/;

const MIME_TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'application/javascript; charset=utf-8',
  '.mjs': 'application/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.png': 'image/png',
  '.webp': 'image/webp',
  '.svg': 'image/svg+xml',
  '.ico': 'image/x-icon',
};

function hashTreeInto(hash: ReturnType<typeof createHash>, rootPath: string, relPrefix: string): void {
  if (!existsSync(rootPath)) return;
  const st = statSync(rootPath);
  if (st.isDirectory()) {
    for (const entry of readdirSync(rootPath).sort()) {
      hashTreeInto(hash, join(rootPath, entry), `${relPrefix}/${entry}`);
    }
    return;
  }
  hash.update(`file:${relPrefix}\0`);
  hash.update(readFileSync(rootPath));
  hash.update('\0');
}

export function computeDashboardSourceDigest(): string {
  const hash = createHash('sha256');
  hashTreeInto(hash, join(DASHBOARD_ROOT, 'index.html'), 'apps/dashboard/index.html');
  hashTreeInto(hash, join(DASHBOARD_ROOT, 'package.json'), 'apps/dashboard/package.json');
  hashTreeInto(hash, join(DASHBOARD_ROOT, 'vite.config.ts'), 'apps/dashboard/vite.config.ts');
  hashTreeInto(hash, join(DASHBOARD_ROOT, 'src'), 'apps/dashboard/src');
  hashTreeInto(hash, join(REPO_ROOT, 'packages', 'contracts'), 'packages/contracts');
  return `sha256:${hash.digest('hex')}`;
}

export function computeDistBundleDigest(): string {
  const hash = createHash('sha256');
  hashTreeInto(hash, join(DIST_DIR, 'index.html'), 'dist/index.html');
  hashTreeInto(hash, join(DIST_DIR, 'assets'), 'dist/assets');
  return `sha256:${hash.digest('hex')}`;
}

export async function ensureBuiltDashboard(): Promise<{
  indexHtml: string;
  assets: string[];
  sourceDigest: string;
  bundleDigest: string;
  mode: 'production';
  nodeEnv: 'production';
}> {
  const sourceDigest = computeDashboardSourceDigest();
  const indexPath = join(DIST_DIR, 'index.html');
  const assetsDir = join(DIST_DIR, 'assets');

  let stampMatches = false;
  if (existsSync(indexPath) && existsSync(assetsDir) && existsSync(BUILD_IDENTITY_FILE)) {
    try {
      const parsed = JSON.parse(readFileSync(BUILD_IDENTITY_FILE, 'utf8')) as {
        mode?: string;
        nodeEnv?: string;
        sourceDigest?: string;
        bundleDigest?: string;
      };
      const currentBundleDigest = computeDistBundleDigest();
      stampMatches =
        parsed.mode === 'production' &&
        parsed.nodeEnv === 'production' &&
        parsed.sourceDigest === sourceDigest &&
        parsed.bundleDigest === currentBundleDigest;
    } catch {
      stampMatches = false;
    }
  }

  if (!stampMatches) {
    rmSync(DIST_DIR, { recursive: true, force: true });
    const viteCliCandidates = [
      join(DASHBOARD_ROOT, 'node_modules', 'vite', 'bin', 'vite.js'),
      join(REPO_ROOT, 'node_modules', 'vite', 'bin', 'vite.js'),
    ];
    const viteCli = viteCliCandidates.find((c) => existsSync(c));
    const childEnv: Record<string, string | undefined> = {
      ...process.env,
      NODE_ENV: 'production',
    };
    const res = viteCli
      ? spawnSync(process.execPath, [viteCli, 'build', '--mode', 'production'], {
          cwd: DASHBOARD_ROOT,
          env: childEnv,
          encoding: 'utf8',
        })
      : spawnSync('pnpm', ['--dir', DASHBOARD_ROOT, 'build'], {
          cwd: REPO_ROOT,
          env: childEnv,
          encoding: 'utf8',
        });
    if (res.status !== 0) {
      throw new Error(
        `Isolated production dashboard build failed (status=${String(res.status)}): ${res.stderr || res.stdout}`,
      );
    }
    const bundleDigest = computeDistBundleDigest();
    writeFileSync(
      BUILD_IDENTITY_FILE,
      JSON.stringify(
        {
          mode: 'production',
          nodeEnv: 'production',
          sourceDigest,
          bundleDigest,
        },
        null,
        2,
      ),
      { encoding: 'utf8' },
    );
  }

  if (!existsSync(indexPath) || !existsSync(assetsDir)) {
    throw new Error(`Dashboard build did not produce expected dist artifacts at ${DIST_DIR}`);
  }
  const indexHtml = readFileSync(indexPath, 'utf8');
  const assets = readdirSync(assetsDir);
  const bundleDigest = computeDistBundleDigest();
  return {
    indexHtml,
    assets,
    sourceDigest,
    bundleDigest,
    mode: 'production',
    nodeEnv: 'production',
  };
}

export interface RecordedRequest {
  method: string;
  url: string;
  pathname: string;
  search: string;
  authorization: string | null;
  body: string;
  timestampMs: number;
}

export interface DashboardTestServerOptions {
  skipBuildCheck?: boolean;
  maxHttpBodyBytes?: number;
  maxRecordedRequests?: number;
  maxRetainedRequestBodyBytes?: number;
  maxActiveSseStreams?: number;
  nativeOwnerToken?: string;
  nativeAllowedProjects?: string[];
}

export interface DashboardTestServer {
  baseUrl: string;
  requests: RecordedRequest[];
  clearRequests(): void;
  assertNoOverflow(): void;
  setFleet(fleet: FleetFixture | NativeFleetV2Fixture, includeTerminalV2Tasks?: boolean): void;
  setFleetV2(fleetV2: NativeFleetV2Fixture | Record<string, unknown>): void;
  getFleetV2Snapshot(): NativeFleetV2Fixture;
  setFleetStatus(status: number, errorDetail?: string): void;
  setLegacy(legacy: Record<string, unknown> | null, status?: number): void;
  setLegacyDetailResponse(status: number, delayMs?: number): void;
  setModels(models: ModelsFixture, status?: number): void;
  setModelsPutConflict(conflict: boolean): void;
  setRequireBearerToken(token: string | null): void;
  configureNativeCommands(options: { ownerToken?: string; allowedProjects?: string[] }): void;
  dropNextCommandResponse(count?: number): void;
  sendSseFrame(rawFrame: string): void;
  closeActiveSseStreams(): void;
  activeSseCount(): number;
  close(): Promise<void>;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function hasExactKeys(obj: Record<string, unknown>, allowedKeys: readonly string[]): boolean {
  const keys = Object.keys(obj);
  return keys.length === allowedKeys.length && allowedKeys.every((k) => Object.prototype.hasOwnProperty.call(obj, k));
}

function countAuthorizationHeaders(req: IncomingMessage): number {
  if (Array.isArray(req.rawHeaders)) {
    let count = 0;
    for (let i = 0; i < req.rawHeaders.length; i += 2) {
      if (req.rawHeaders[i]?.toLowerCase() === 'authorization') count++;
    }
    return count;
  }
  return req.headers.authorization !== undefined ? 1 : 0;
}

function hasControlChar(value: string): boolean {
  for (let i = 0; i < value.length; i++) {
    const code = value.charCodeAt(i);
    if (code <= 0x1f || (code >= 0x7f && code <= 0x9f)) return true;
  }
  return false;
}

function recomputeProjectSummaries(fleetV2: NativeFleetV2Fixture): void {
  const counts = new Map<
    string,
    { id: string; name: string; total: number; running: number; awaiting_verification: number; blocked: number; accepted: number }
  >();
  for (const existing of fleetV2.projects) {
    counts.set(existing.id, {
      id: existing.id,
      name: existing.name,
      total: 0,
      running: 0,
      awaiting_verification: 0,
      blocked: 0,
      accepted: 0,
    });
  }
  for (const task of fleetV2.tasks) {
    const pid = task.contract.project_id;
    let summary = counts.get(pid);
    if (!summary) {
      summary = { id: pid, name: pid, total: 0, running: 0, awaiting_verification: 0, blocked: 0, accepted: 0 };
      counts.set(pid, summary);
    }
    summary.total += 1;
    if (task.state === 'running' || task.state === 'claimed') summary.running += 1;
    else if (task.state === 'awaiting_verification') summary.awaiting_verification += 1;
    else if (task.state === 'blocked') summary.blocked += 1;
    else if (task.state === 'accepted') summary.accepted += 1;
  }
  fleetV2.projects = [...counts.values()];
}

export async function startDashboardTestServer(
  options: DashboardTestServerOptions = {},
): Promise<DashboardTestServer> {
  if (!options.skipBuildCheck) {
    await ensureBuiltDashboard();
  }

  const maxHttpBodyBytes = options.maxHttpBodyBytes ?? MAX_HTTP_BODY_BYTES;
  const maxRecordedRequests = options.maxRecordedRequests ?? MAX_RECORDED_REQUESTS;
  const maxRetainedRequestBodyBytes =
    options.maxRetainedRequestBodyBytes ?? MAX_RETAINED_REQUEST_BODY_BYTES;
  const maxActiveSseStreams = options.maxActiveSseStreams ?? MAX_ACTIVE_SSE_STREAMS;

  let fleet = createFleetFixture(4);
  let statefulFleetV2: NativeFleetV2Fixture = createNativeFleetV2Fixture(4, {
    includeTerminalLifecycleTasks: true,
  });
  let customFleetV2Payload: Record<string, unknown> | null = null;
  let fleetStatus = 200;
  let fleetErrorDetail = 'Service unavailable';
  let legacy: Record<string, unknown> | null = createLegacyFixture(4);
  let legacyStatus = 200;
  let legacyDetailStatus = 200;
  let legacyDetailDelayMs = 0;
  let models = createModelsFixture('1');
  let modelsStatus = 200;
  let forcePutConflict = false;
  let requiredToken: string | null = null;

  let nativeOwnerToken = options.nativeOwnerToken ?? 'adm-native-owner-token-0123456789abcdef';
  let nativeAllowedProjects = new Set<string>(
    options.nativeAllowedProjects ?? ['proj-001', 'proj-002', 'proj-003', 'proj-004'],
  );
  let dropNextCommandResponses = 0;

  const idempotencyRecords = new Map<
    string,
    {
      endpoint: '/v2/tasks' | '/v2/tasks/lifecycle';
      canonicalPayload: string;
      eventSeq: string;
    }
  >();

  const requests: RecordedRequest[] = [];
  const sseStreams = new Set<ServerResponse>();
  const domainEventHistory: Array<{ seq: string; generation: string; rawFrame: string }> = [];
  let overflowError: string | null = null;

  function sendJson(
    res: ServerResponse,
    status: number,
    payload: unknown,
    closeConnection = false,
  ): void {
    res.writeHead(status, {
      'Content-Type': 'application/json',
      'Cache-Control': 'no-store',
      ...(closeConnection ? { Connection: 'close' } : {}),
    });
    res.end(JSON.stringify(payload));
  }

  function dropCommandResponse(res: ServerResponse): void {
    const truncated = '{"event_seq":';
    res.writeHead(200, {
      'Content-Type': 'application/json',
      'Content-Length': String(Buffer.byteLength(truncated, 'utf8')),
      'Cache-Control': 'no-store',
      Connection: 'close',
    });
    res.end(truncated);
  }

  function checkAuthorized(req: IncomingMessage, res: ServerResponse): boolean {
    if (!requiredToken) return true;
    if (countAuthorizationHeaders(req) !== 1) {
      sendJson(res, 401, { error: 'Unauthorized', detail: 'Bearer token required' });
      return false;
    }
    const auth = req.headers.authorization ?? '';
    if (auth !== `Bearer ${requiredToken}`) {
      sendJson(res, 401, { error: 'Unauthorized', detail: 'Bearer token required' });
      return false;
    }
    return true;
  }

  function checkNativeOwnerAuthorized(req: IncomingMessage, res: ServerResponse): boolean {
    const expectedToken = requiredToken ?? nativeOwnerToken;
    if (countAuthorizationHeaders(req) !== 1) {
      sendJson(res, 401, { error: 'owner authentication required' }, true);
      return false;
    }
    const auth = req.headers.authorization ?? '';
    if (auth !== `Bearer ${expectedToken}`) {
      sendJson(res, 401, { error: 'owner authentication required' }, true);
      return false;
    }
    return true;
  }

  function nextEventSeq(): string {
    const next = String(BigInt(statefulFleetV2.cursor.event_seq) + 1n);
    statefulFleetV2.cursor = {
      ...statefulFleetV2.cursor,
      event_seq: next,
    };
    return next;
  }

  function handleSubmitTask(req: IncomingMessage, res: ServerResponse, body: string): void {
    if (!checkNativeOwnerAuthorized(req, res)) return;
    let parsed: unknown;
    try {
      parsed = JSON.parse(body);
    } catch {
      sendJson(res, 400, { error: 'command body is invalid' }, true);
      return;
    }
    if (!isPlainObject(parsed) || !hasExactKeys(parsed, ['request_id', 'contract'])) {
      sendJson(res, 400, { error: 'command body is invalid' }, true);
      return;
    }
    const requestId = parsed.request_id;
    if (typeof requestId !== 'string' || !CANONICAL_UUID_RE.test(requestId)) {
      sendJson(res, 400, { error: 'request_id must be a canonical UUID' }, true);
      return;
    }
    if (!validateTaskContract(parsed.contract)) {
      if (isPlainObject(parsed.contract) && typeof parsed.contract.project_id === 'string' && !nativeAllowedProjects.has(parsed.contract.project_id)) {
        sendJson(res, 403, { error: 'task contract is outside the owner grant' }, true);
        return;
      }
      sendJson(res, 403, { error: 'task contract is outside the owner grant' }, true);
      return;
    }
    const contract = parsed.contract as unknown as TaskContractFixture;
    if (!nativeAllowedProjects.has(contract.project_id)) {
      sendJson(res, 403, { error: 'task contract is outside the owner grant' }, true);
      return;
    }

    const canonicalPayload = JSON.stringify({
      request_id: requestId,
      contract: JSON.parse(serializeCanonicalTaskContract(contract)),
    });

    const existingRecord = idempotencyRecords.get(requestId);
    if (existingRecord) {
      if (existingRecord.endpoint === '/v2/tasks' && existingRecord.canonicalPayload === canonicalPayload) {
        sendJson(
          res,
          200,
          {
            event_seq: existingRecord.eventSeq,
            source_contiguous_seq: '1',
            duplicate: true,
            execution_granted: false,
            acceptance_granted: false,
          },
          true,
        );
        return;
      }
      sendJson(res, 409, { error: 'task command was not applied' }, true);
      return;
    }

    if (statefulFleetV2.tasks.some((t) => t.contract.task_id === contract.task_id)) {
      sendJson(res, 409, { error: 'task command was not applied' }, true);
      return;
    }
    for (const depId of contract.dependencies) {
      const depTask = statefulFleetV2.tasks.find((t) => t.contract.task_id === depId);
      if (!depTask || depTask.contract.project_id !== contract.project_id) {
        sendJson(res, 409, { error: 'task command was not applied' }, true);
        return;
      }
    }
    const allDepsAccepted = contract.dependencies.every((depId) => {
      const depTask = statefulFleetV2.tasks.find((t) => t.contract.task_id === depId);
      return depTask?.state === 'accepted';
    });

    const newTask: NativeTaskViewFixture = {
      contract: JSON.parse(serializeCanonicalTaskContract(contract)) as TaskContractFixture,
      state: allDepsAccepted ? 'ready' : 'blocked',
      attempt_id: null,
      owner: null,
      candidate_ref: null,
    };
    statefulFleetV2.tasks.push(newTask);
    recomputeProjectSummaries(statefulFleetV2);
    const eventSeq = nextEventSeq();
    idempotencyRecords.set(requestId, {
      endpoint: '/v2/tasks',
      canonicalPayload,
      eventSeq,
    });
    const createdFrame = `event: domain\nid: ${statefulFleetV2.cursor.recovery_generation}:${eventSeq}\ndata: ${JSON.stringify({
      seq: eventSeq,
      observed_utc: '2026-10-18T20:28:00Z',
      event: {
        kind: 'task_submitted',
        contract: newTask.contract,
      },
    })}\n\n`;
    domainEventHistory.push({
      seq: eventSeq,
      generation: statefulFleetV2.cursor.recovery_generation,
      rawFrame: createdFrame,
    });
    for (const stream of sseStreams) {
      stream.write(createdFrame);
    }

    if (dropNextCommandResponses > 0) {
      dropNextCommandResponses -= 1;
      dropCommandResponse(res);
      return;
    }

    sendJson(
      res,
      200,
      {
        event_seq: eventSeq,
        source_contiguous_seq: '1',
        duplicate: false,
        execution_granted: false,
        acceptance_granted: false,
      },
      true,
    );
  }

  function handleChangeLifecycle(req: IncomingMessage, res: ServerResponse, body: string): void {
    if (!checkNativeOwnerAuthorized(req, res)) return;
    let parsed: unknown;
    try {
      parsed = JSON.parse(body);
    } catch {
      sendJson(res, 400, { error: 'command body is invalid' }, true);
      return;
    }
    if (
      !isPlainObject(parsed) ||
      !hasExactKeys(parsed, ['request_id', 'task_id', 'expected_task_digest', 'action'])
    ) {
      sendJson(res, 400, { error: 'command body is invalid' }, true);
      return;
    }
    const requestId = parsed.request_id;
    const taskId = parsed.task_id;
    const expectedTaskDigest = parsed.expected_task_digest;
    const actionObj = parsed.action;

    if (typeof requestId !== 'string' || !CANONICAL_UUID_RE.test(requestId)) {
      sendJson(res, 400, { error: 'request_id must be a canonical UUID' }, true);
      return;
    }
    if (typeof taskId !== 'string' || taskId.length === 0 || taskId.length > 128) {
      sendJson(res, 400, { error: 'command body is invalid' }, true);
      return;
    }
    if (typeof expectedTaskDigest !== 'string' || !SHA256_DIGEST_RE.test(expectedTaskDigest)) {
      sendJson(res, 400, { error: 'expected_task_digest must be a SHA-256 digest' }, true);
      return;
    }
    if (!isPlainObject(actionObj) || typeof actionObj.action !== 'string') {
      sendJson(res, 400, { error: 'command body is invalid' }, true);
      return;
    }

    let canonicalAction: Record<string, unknown>;
    if (actionObj.action === 'cancel_pending') {
      if (!hasExactKeys(actionObj, ['action', 'reason']) || typeof actionObj.reason !== 'string') {
        sendJson(res, 400, { error: 'command body is invalid' }, true);
        return;
      }
      const reason = actionObj.reason;
      if (reason.trim().length === 0 || reason.length > 512 || hasControlChar(reason)) {
        sendJson(res, 400, { error: 'pending cancellation reason is invalid' }, true);
        return;
      }
      canonicalAction = { action: 'cancel_pending', reason };
    } else if (actionObj.action === 'revise_contract') {
      if (!hasExactKeys(actionObj, ['action', 'contract'])) {
        sendJson(res, 400, { error: 'command body is invalid' }, true);
        return;
      }
      if (!validateTaskContract(actionObj.contract)) {
        sendJson(res, 403, { error: 'contract revision is outside the owner grant' }, true);
        return;
      }
      const revisedContract = actionObj.contract as unknown as TaskContractFixture;
      if (
        revisedContract.task_id !== taskId ||
        !nativeAllowedProjects.has(revisedContract.project_id)
      ) {
        sendJson(res, 403, { error: 'contract revision is outside the owner grant' }, true);
        return;
      }
      canonicalAction = {
        action: 'revise_contract',
        contract: JSON.parse(serializeCanonicalTaskContract(revisedContract)),
      };
    } else {
      // Forged decide_candidate, close_attempt, retry, or unknown action rejected at HTTP boundary
      sendJson(res, 400, { error: 'command body is invalid' }, true);
      return;
    }

    const canonicalPayload = JSON.stringify({
      request_id: requestId,
      task_id: taskId,
      expected_task_digest: expectedTaskDigest,
      action: canonicalAction,
    });

    // Idempotent replay check happens BEFORE current task state/CAS transition checks
    // so retained receipts survive subsequent task revisions or cancellation!
    const existingRecord = idempotencyRecords.get(requestId);
    if (existingRecord) {
      if (
        existingRecord.endpoint === '/v2/tasks/lifecycle' &&
        existingRecord.canonicalPayload === canonicalPayload
      ) {
        sendJson(
          res,
          200,
          {
            event_seq: existingRecord.eventSeq,
            source_contiguous_seq: '1',
            duplicate: true,
            execution_granted: false,
            acceptance_granted: false,
          },
          true,
        );
        return;
      }
      sendJson(res, 409, { error: 'lifecycle command was not applied' }, true);
      return;
    }

    const task = statefulFleetV2.tasks.find((t) => t.contract.task_id === taskId);
    if (!task || !nativeAllowedProjects.has(task.contract.project_id)) {
      sendJson(res, 409, { error: 'lifecycle command was not applied' }, true);
      return;
    }

    const actualTaskDigest = computeCanonicalTaskViewDigest(task);
    if (actualTaskDigest !== expectedTaskDigest) {
      sendJson(res, 409, { error: 'lifecycle command was not applied' }, true);
      return;
    }

    if ((task.state !== 'ready' && task.state !== 'blocked') || task.attempt_id !== null) {
      sendJson(res, 409, { error: 'lifecycle command was not applied' }, true);
      return;
    }

    const currentLifecycle = task.lifecycle ?? {
      revision: 0,
      attempts: {},
      closure_digest: null,
      decision: null,
      reason: null,
    };

    if (actionObj.action === 'cancel_pending') {
      task.state = 'cancelled';
      task.lifecycle = {
        ...currentLifecycle,
        revision: currentLifecycle.revision + 1,
        reason: String(actionObj.reason),
      };
    } else {
      const revisedContract = canonicalAction.contract as TaskContractFixture;
      if (
        Object.keys(currentLifecycle.attempts).length > 0 ||
        revisedContract.project_id !== task.contract.project_id ||
        revisedContract.root_id !== task.contract.root_id ||
        revisedContract.authority_ref !== task.contract.authority_ref ||
        revisedContract.simulated !== task.contract.simulated ||
        revisedContract.revision !== task.contract.revision + 1
      ) {
        sendJson(res, 409, { error: 'lifecycle command was not applied' }, true);
        return;
      }
      task.contract = revisedContract;
      task.lifecycle = {
        ...currentLifecycle,
        revision: currentLifecycle.revision + 1,
      };
    }

    recomputeProjectSummaries(statefulFleetV2);
    const eventSeq = nextEventSeq();
    idempotencyRecords.set(requestId, {
      endpoint: '/v2/tasks/lifecycle',
      canonicalPayload,
      eventSeq,
    });
    const lifecycleFrame = `event: domain\nid: ${statefulFleetV2.cursor.recovery_generation}:${eventSeq}\ndata: ${JSON.stringify({
      seq: eventSeq,
      observed_utc: '2026-10-18T20:28:00Z',
      event: {
        kind: 'task_lifecycle',
        command: {
          protocol_version: 2,
          task_id: taskId,
          expected_task_digest: expectedTaskDigest,
          actor: 'owner',
          action: canonicalAction,
        },
      },
    })}\n\n`;
    domainEventHistory.push({
      seq: eventSeq,
      generation: statefulFleetV2.cursor.recovery_generation,
      rawFrame: lifecycleFrame,
    });
    for (const stream of sseStreams) {
      stream.write(lifecycleFrame);
    }

    if (dropNextCommandResponses > 0) {
      dropNextCommandResponses -= 1;
      dropCommandResponse(res);
      return;
    }

    sendJson(
      res,
      200,
      {
        event_seq: eventSeq,
        source_contiguous_seq: '1',
        duplicate: false,
        execution_granted: false,
        acceptance_granted: false,
      },
      true,
    );
  }

  const server = createServer((req, res) => {
    const parsedUrl = new URL(req.url ?? '/', 'http://127.0.0.1');
    const chunks: Buffer[] = [];
    let bodyBytes = 0;
    let abortedForBodyOverflow = false;

    req.on('data', (chunk: Buffer) => {
      if (abortedForBodyOverflow) return;
      bodyBytes += chunk.length;
      if (bodyBytes > maxHttpBodyBytes) {
        abortedForBodyOverflow = true;
        chunks.length = 0;
        sendJson(res, 413, { error: 'command body is invalid or too large' }, true);
        return;
      }
      chunks.push(chunk);
    });

    req.on('end', () => {
      if (abortedForBodyOverflow) return;
      const body = Buffer.concat(chunks).toString('utf8');
      if (Buffer.byteLength(body, 'utf8') > maxRetainedRequestBodyBytes) {
        overflowError = `HTTP request body (${Buffer.byteLength(body, 'utf8')} bytes) exceeded maxRetainedRequestBodyBytes (${maxRetainedRequestBodyBytes})`;
        sendJson(res, 413, { error: 'request body exceeds retained fixture bound' }, true);
        return;
      }
      if (requests.length >= maxRecordedRequests) {
        overflowError = `Recorded request log exceeded maxRecordedRequests (${maxRecordedRequests}); call clearRequests() or narrow test scope`;
        sendJson(res, 503, { error: 'fixture request log capacity exceeded' }, true);
        return;
      }

      requests.push({
        method: req.method ?? 'GET',
        url: req.url ?? '/',
        pathname: parsedUrl.pathname,
        search: parsedUrl.search,
        authorization: typeof req.headers.authorization === 'string' ? req.headers.authorization : null,
        body,
        timestampMs: Date.now(),
      });

      if (parsedUrl.pathname === '/healthz' && req.method === 'GET') {
        sendJson(res, 200, {
          status: 'ok',
          version: '0.1.0',
          native_execution_qualified: false,
        });
        return;
      }

      if (parsedUrl.pathname === '/v2/tasks' && req.method === 'POST') {
        handleSubmitTask(req, res, body);
        return;
      }

      if (parsedUrl.pathname === '/v2/tasks/lifecycle' && req.method === 'POST') {
        handleChangeLifecycle(req, res, body);
        return;
      }

      if (parsedUrl.pathname === '/v1/fleet' || parsedUrl.pathname === '/v2/fleet') {
        if (!checkAuthorized(req, res)) return;
        if (fleetStatus !== 200) {
          sendJson(res, fleetStatus, { detail: fleetErrorDetail });
          return;
        }
        const activeV2 = customFleetV2Payload ?? statefulFleetV2;
        sendJson(res, 200, parsedUrl.pathname === '/v2/fleet' ? activeV2 : fleet);
        return;
      }

      if (parsedUrl.pathname === '/v1/events' || parsedUrl.pathname === '/v2/events') {
        if (!checkAuthorized(req, res)) return;
        if (sseStreams.size >= maxActiveSseStreams) {
          overflowError = `Active SSE streams exceeded maxActiveSseStreams (${maxActiveSseStreams})`;
          sendJson(res, 503, { error: 'event stream capacity reached' });
          return;
        }
        res.writeHead(200, {
          'Content-Type': 'text/event-stream',
          'Cache-Control': 'no-store',
          Connection: 'keep-alive',
        });
        res.write(': connected\n\n');
        sseStreams.add(res);
        res.on('close', () => sseStreams.delete(res));

        // When CommandJourneys replayEventsToCursor requests historical catch-up (?after=0&generation=...),
        // replay recorded domain events up to the current snapshot cursor so replay finishes immediately.
        const afterParam = parsedUrl.searchParams.get('after');
        const genParam = parsedUrl.searchParams.get('generation');
        const currentGen = statefulFleetV2.cursor.recovery_generation;
        const currentSeqStr = statefulFleetV2.cursor.event_seq;
        if (
          afterParam !== null &&
          /^\d+$/.test(afterParam) &&
          /^\d+$/.test(currentSeqStr) &&
          (!genParam || genParam === currentGen)
        ) {
          const afterSeq = BigInt(afterParam);
          const targetSeq = BigInt(currentSeqStr);
          if (afterSeq < targetSeq) {
            let lastSent = afterSeq;
            for (const entry of domainEventHistory) {
              if (entry.generation === currentGen && /^\d+$/.test(entry.seq)) {
                const s = BigInt(entry.seq);
                if (s > afterSeq && s <= targetSeq) {
                  res.write(entry.rawFrame);
                  if (s > lastSent) lastSent = s;
                }
              }
            }
            if (lastSent < targetSeq) {
              const fallbackContract =
                statefulFleetV2.tasks[0]?.contract ?? createTaskContractFixture();
              const catchupFrame = `event: domain\nid: ${currentGen}:${currentSeqStr}\ndata: ${JSON.stringify(
                {
                  seq: currentSeqStr,
                  observed_utc: '2026-10-18T20:28:00Z',
                  event: {
                    kind: 'task_submitted',
                    contract: fallbackContract,
                  },
                },
              )}\n\n`;
              res.write(catchupFrame);
            }
          }
        }
        return;
      }

      if (
        parsedUrl.pathname === '/v1/legacy' ||
        parsedUrl.pathname === '/v1/legacy/detail' ||
        parsedUrl.pathname === '/v1/legacy/detail/initial'
      ) {
        if (!checkAuthorized(req, res)) return;
        const respond = () => {
          if (parsedUrl.pathname === '/v1/legacy/detail' && legacyDetailStatus !== 200) {
            sendJson(res, legacyDetailStatus, { detail: 'Controlled detail enrichment failure' });
          } else if (legacyStatus !== 200 || !legacy) {
            sendJson(res, legacyStatus === 200 ? 404 : legacyStatus, { detail: 'Legacy snapshot unavailable' });
          } else {
            sendJson(res, 200, parsedUrl.pathname === '/v1/legacy'
              ? legacy
              : projectLegacyDetailSnapshot(legacy, parsedUrl.pathname.endsWith('/initial')));
          }
        };
        if (parsedUrl.pathname === '/v1/legacy/detail' && legacyDetailDelayMs > 0) setTimeout(respond, legacyDetailDelayMs);
        else respond();
        return;
      }

      if (parsedUrl.pathname === '/v1/models') {
        if (!checkAuthorized(req, res)) return;
        if (modelsStatus !== 200) {
          sendJson(res, modelsStatus, { detail: 'Model registry unavailable' });
          return;
        }
        if (req.method === 'GET') {
          sendJson(res, 200, models);
          return;
        }
        if (req.method === 'PUT') {
          let payload: { expected_revision: string; models: Array<Record<string, unknown>> };
          try {
            payload = JSON.parse(body) as {
              expected_revision: string;
              models: Array<Record<string, unknown>>;
            };
          } catch {
            sendJson(res, 400, { detail: 'Invalid JSON body' });
            return;
          }
          if (forcePutConflict || payload.expected_revision !== models.revision) {
            forcePutConflict = false;
            models = { ...models, revision: String(BigInt(models.revision) + 1n) };
            sendJson(res, 409, { detail: 'Policy revision conflict' });
            return;
          }
          const nextRevision = String(BigInt(models.revision) + 1n);
          models = {
            schema_version: 1,
            revision: nextRevision,
            native_execution_qualified: false,
            models: payload.models.map((item) => ({
              id: String(item.id),
              label: String(item.label),
              provider: item.provider as ModelsFixture['models'][number]['provider'],
              provider_model: String(item.provider_model),
              tier: Number(item.tier),
              enabled: Boolean(item.enabled),
              node_id: item.node_id === null ? null : String(item.node_id),
              max_concurrency: Number(item.max_concurrency),
              capabilities: (item.capabilities as string[]).map(String),
              task_max_attempts: Number(item.task_max_attempts),
              escalate_to: item.escalate_to === null ? null : String(item.escalate_to),
              usage_cost: { status: 'unknown' as const },
            })),
          };
          sendJson(res, 200, models);
          return;
        }
      }

      // Serve built static files from dist/
      const safeRelPath = normalize(parsedUrl.pathname).replace(/^(\.\.(\/|\\|$))+/, '');
      let filePath = join(DIST_DIR, safeRelPath === '/' ? 'index.html' : safeRelPath);
      if (!existsSync(filePath) || statSync(filePath).isDirectory()) {
        if (parsedUrl.pathname.startsWith('/v1/') || parsedUrl.pathname.startsWith('/v2/')) {
          sendJson(res, 404, { error: 'Not found' });
          return;
        }
        filePath = join(DIST_DIR, 'index.html');
      }
      const ext = extname(filePath);
      const contentType = MIME_TYPES[ext] ?? 'application/octet-stream';
      res.writeHead(200, { 'Content-Type': contentType, 'Cache-Control': 'no-store' });
      res.end(readFileSync(filePath));
    });
  });

  await new Promise<void>((resolveListen, rejectListen) => {
    server.once('error', rejectListen);
    server.listen(0, '127.0.0.1', () => resolveListen());
  });

  const addr = server.address();
  if (!addr || typeof addr === 'string') throw new Error('Failed to bind ephemeral loopback server');
  const baseUrl = `http://127.0.0.1:${addr.port}`;

  return {
    baseUrl,
    requests,
    clearRequests() {
      requests.length = 0;
      overflowError = null;
    },
    assertNoOverflow() {
      if (overflowError) {
        throw new Error(overflowError);
      }
    },
    setFleet(next, includeTerminalV2Tasks = true) {
      if (next.schema_version === 2) {
        statefulFleetV2 = next;
        customFleetV2Payload = null;
        domainEventHistory.length = 0;
        fleetStatus = 200;
        return;
      }
      fleet = next;
      statefulFleetV2 = fleetV1ToV2(next, { includeTerminalLifecycleTasks: includeTerminalV2Tasks });
      customFleetV2Payload = null;
      domainEventHistory.length = 0;
      fleetStatus = 200;
    },
    setFleetV2(next) {
      if (
        isPlainObject(next) &&
        next.schema_version === 2 &&
        next.native_execution_qualified === false &&
        Array.isArray(next.tasks) &&
        Array.isArray(next.projects) &&
        isPlainObject(next.cursor)
      ) {
        statefulFleetV2 = next as unknown as NativeFleetV2Fixture;
        customFleetV2Payload = null;
        domainEventHistory.length = 0;
      } else {
        customFleetV2Payload = next as Record<string, unknown>;
      }
      fleetStatus = 200;
    },
    getFleetV2Snapshot() {
      return statefulFleetV2;
    },
    setFleetStatus(status, errorDetail = 'Service unavailable') {
      fleetStatus = status;
      fleetErrorDetail = errorDetail;
    },
    setLegacy(next, status = 200) {
      legacy = next;
      legacyStatus = status;
    },
    setLegacyDetailResponse(status, delayMs = 0) {
      if (!Number.isInteger(delayMs) || delayMs < 0 || delayMs > 5_000) throw new Error('Controlled detail delay must be within 0..5000ms');
      legacyDetailStatus = status;
      legacyDetailDelayMs = delayMs;
    },
    setModels(next, status = 200) {
      models = next;
      modelsStatus = status;
    },
    setModelsPutConflict(conflict) {
      forcePutConflict = conflict;
    },
    setRequireBearerToken(token) {
      requiredToken = token;
    },
    configureNativeCommands(cfg) {
      if (cfg.ownerToken !== undefined) nativeOwnerToken = cfg.ownerToken;
      if (cfg.allowedProjects !== undefined) {
        nativeAllowedProjects = new Set(cfg.allowedProjects);
      }
    },
    dropNextCommandResponse(count = 1) {
      dropNextCommandResponses = Math.max(0, count);
    },
    sendSseFrame(rawFrame) {
      for (const stream of sseStreams) {
        stream.write(rawFrame);
      }
    },
    closeActiveSseStreams() {
      for (const stream of sseStreams) {
        stream.end();
        stream.destroy();
      }
      sseStreams.clear();
    },
    activeSseCount() {
      return sseStreams.size;
    },
    async close() {
      for (const stream of sseStreams) {
        stream.destroy();
      }
      sseStreams.clear();
      await new Promise<void>((resolveClose) => server.close(() => resolveClose()));
    },
  };
}

export interface BoundedCdpFrameReaderOptions {
  maxFrameBytes?: number;
  maxBufferBytes?: number;
  onMessage: (msg: {
    id?: number;
    result?: Record<string, unknown>;
    error?: { message?: string };
  }) => void;
  onFatalError: (err: Error) => void;
}

export function createBoundedCdpFrameReader(options: BoundedCdpFrameReaderOptions): {
  push(chunk: Buffer | string): void;
  bufferedBytes(): number;
} {
  const maxFrameBytes = options.maxFrameBytes ?? MAX_CDP_FRAME_BYTES;
  const maxBufferBytes = options.maxBufferBytes ?? MAX_CDP_BUFFER_BYTES;
  let buffer = '';
  let failed = false;

  function fail(message: string): void {
    if (failed) return;
    failed = true;
    buffer = '';
    options.onFatalError(new Error(message));
  }

  return {
    push(chunk: Buffer | string) {
      if (failed) return;
      const text = typeof chunk === 'string' ? chunk : chunk.toString('utf8');
      const incomingBytes = Buffer.byteLength(text, 'utf8');
      const currentBytes = Buffer.byteLength(buffer, 'utf8');
      if (currentBytes + incomingBytes > maxBufferBytes) {
        fail(`CDP buffer exceeded maximum byte bound (${maxBufferBytes} bytes)`);
        return;
      }
      buffer += text;
      let nullIdx: number;
      while ((nullIdx = buffer.indexOf('\0')) !== -1) {
        const raw = buffer.slice(0, nullIdx);
        buffer = buffer.slice(nullIdx + 1);
        const frameBytes = Buffer.byteLength(raw, 'utf8');
        if (frameBytes > maxFrameBytes) {
          fail(`CDP frame exceeded maximum byte bound (${maxFrameBytes} bytes)`);
          return;
        }
        if (!raw.trim()) continue;
        let parsed: unknown;
        try {
          parsed = JSON.parse(raw);
        } catch (err) {
          fail(`Malformed CDP JSON frame: ${err instanceof Error ? err.message : String(err)}`);
          return;
        }
        if (!isPlainObject(parsed)) {
          fail('Malformed CDP JSON frame: expected JSON object');
          return;
        }
        options.onMessage(
          parsed as {
            id?: number;
            result?: Record<string, unknown>;
            error?: { message?: string };
          },
        );
        if (failed) return;
      }
      if (Buffer.byteLength(buffer, 'utf8') > maxFrameBytes) {
        fail(`CDP unterminated frame exceeded maximum byte bound (${maxFrameBytes} bytes)`);
      }
    },
    bufferedBytes() {
      return Buffer.byteLength(buffer, 'utf8');
    },
  };
}

export function discoverChromiumBinary(): string {
  if (process.env.ADMIRALTY_BROWSER_BINARY) {
    if (!existsSync(process.env.ADMIRALTY_BROWSER_BINARY)) {
      throw new Error(
        `Configured ADMIRALTY_BROWSER_BINARY does not exist: ${process.env.ADMIRALTY_BROWSER_BINARY}`,
      );
    }
    return process.env.ADMIRALTY_BROWSER_BINARY;
  }

  for (const candidate of [
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/opt/google/chrome/chrome',
    '/usr/bin/google-chrome-stable',
    '/usr/bin/google-chrome',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
  ]) {
    if (existsSync(candidate)) return candidate;
  }

  const playwrightCacheRoots = [
    process.env.PLAYWRIGHT_BROWSERS_PATH,
    join(homedir(), '.cache', 'ms-playwright'),
  ].filter((item): item is string => Boolean(item));

  for (const root of playwrightCacheRoots) {
    if (!existsSync(root)) continue;
    for (const entry of readdirSync(root).sort().reverse()) {
      if (entry.startsWith('chromium-')) {
        const candidate = join(root, entry, 'chrome-linux64', 'chrome');
        if (existsSync(candidate)) return candidate;
      }
      if (entry.startsWith('chromium_headless_shell-')) {
        const candidate = join(root, entry, 'chrome-headless-shell-linux64', 'chrome-headless-shell');
        if (existsSync(candidate)) return candidate;
      }
    }
  }

  throw new Error(
    'No qualified Chromium or Chrome binary found for dashboard E2E qualification. ' +
      'Set ADMIRALTY_BROWSER_BINARY or install Chromium before running browser qualification.',
  );
}

function discoverNormalUserSetuidSandbox(binaryPath: string): string | undefined {
  if (process.env.CHROME_DEVEL_SANDBOX && existsSync(process.env.CHROME_DEVEL_SANDBOX)) {
    return process.env.CHROME_DEVEL_SANDBOX;
  }
  const binDir = dirname(binaryPath);
  for (const candidate of [
    join(binDir, 'chrome_sandbox'),
    join(binDir, 'chrome-sandbox'),
    '/opt/google/chrome/chrome-sandbox',
  ]) {
    if (!existsSync(candidate)) continue;
    try {
      const st = statSync(candidate);
      if (st.uid === 0 && (st.mode & 0o4111) === 0o4111) {
        return candidate;
      }
    } catch {
      // Ignore unreadable candidate
    }
  }
  return undefined;
}

function readSysctlDiagnostic(path: string): string {
  try {
    return existsSync(path) ? readFileSync(path, 'utf8').trim() : 'unavailable';
  } catch {
    return 'unreadable';
  }
}

export type StageEvaluateFn = <T>(expression: string, timeoutMs: number) => Promise<T>;

function validateStageTimeoutMs(rawTimeoutMs: number, name: string): number {
  if (
    typeof rawTimeoutMs !== 'number' ||
    !Number.isFinite(rawTimeoutMs) ||
    !Number.isInteger(rawTimeoutMs) ||
    rawTimeoutMs < 1
  ) {
    throw new Error(`Invalid ${name}: expected positive finite integer, got ${String(rawTimeoutMs)}`);
  }
  return rawTimeoutMs;
}

export async function fillInputByLabelStage(
  evaluateFn: StageEvaluateFn,
  labelText: string,
  value: string,
  timeoutMs = FORM_CONTROL_STAGE_TIMEOUT_MS,
): Promise<void> {
  const stageTimeoutMs = validateStageTimeoutMs(timeoutMs, 'fillInputByLabel timeoutMs');
  const labelEsc = JSON.stringify(labelText);
  const valEsc = JSON.stringify(value);
  const deadline = Date.now() + stageTimeoutMs;
  let lastFailureDetail = 'matching <input> or <textarea> not mounted';
  let lastEvalError: string | null = null;

  while (Date.now() < deadline) {
    const remainingMs = deadline - Date.now();
    if (remainingMs <= 0) break;
    const evalTimeoutMs = Math.min(MAX_STAGE_EVALUATE_BUDGET_MS, remainingMs);
    try {
      const outcome = await evaluateFn<{ ok: boolean; reason?: string }>(
        `(() => {
          const byAria = [...document.querySelectorAll('input[aria-label], textarea[aria-label]')].find(
            el => el.getAttribute('aria-label') === ${labelEsc}
          );
          let input = byAria;
          if (!input) {
            const labels = [...document.querySelectorAll('label')];
            const match = labels.find(l => (l.textContent || '').includes(${labelEsc}));
            if (!match) {
              const knownLabels = labels
                .map(l => (l.textContent || '').replace(/\\s+/g, ' ').trim())
                .filter(Boolean)
                .slice(0, 8);
              return {
                ok: false,
                reason: \`label or aria-label \${${labelEsc}} not found (visible labels: [\${knownLabels.join(', ')}])\`
              };
            }
            const forId = match.getAttribute('for');
            input = forId ? document.getElementById(forId) : match.querySelector('input, textarea');
            if (!input) {
              return {
                ok: false,
                reason: \`label \${${labelEsc}} found but associated input/textarea is missing (for=\${JSON.stringify(forId)})\`
              };
            }
          }
          if (!(input instanceof HTMLInputElement) && !(input instanceof HTMLTextAreaElement)) {
            return {
              ok: false,
              reason: \`matched element for \${${labelEsc}} is <\${input.tagName.toLowerCase()}>, not input/textarea\`
            };
          }
          const proto = input instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
          const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set;
          setter?.call(input, ${valEsc});
          if (input.value !== ${valEsc}) {
            return {
              ok: false,
              reason: \`input.value mismatch after setter (expected \${${valEsc}}, actual \${JSON.stringify(input.value)})\`
            };
          }
          input.dispatchEvent(new Event('input', { bubbles: true }));
          input.dispatchEvent(new Event('change', { bubbles: true }));
          return { ok: true };
        })()`,
        evalTimeoutMs,
      );
      if (outcome?.ok) return;
      if (outcome?.reason) {
        lastFailureDetail = outcome.reason;
      }
    } catch (err) {
      lastEvalError = err instanceof Error ? err.message : String(err);
    }
    const sleepRemainingMs = deadline - Date.now();
    if (sleepRemainingMs <= 0) break;
    await new Promise((r) => setTimeout(r, Math.min(40, sleepRemainingMs)));
  }

  const details = [
    `reason: ${lastFailureDetail}`,
    ...(lastEvalError ? [`lastEvalError: ${lastEvalError}`] : []),
  ].join('; ');
  throw new Error(`Input for label not found: ${labelText} after ${stageTimeoutMs}ms (${details})`);
}

export async function selectByLabelStage(
  evaluateFn: StageEvaluateFn,
  labelText: string,
  value: string,
  timeoutMs = FORM_CONTROL_STAGE_TIMEOUT_MS,
): Promise<void> {
  const stageTimeoutMs = validateStageTimeoutMs(timeoutMs, 'selectByLabel timeoutMs');
  const labelEsc = JSON.stringify(labelText);
  const valEsc = JSON.stringify(value);
  const deadline = Date.now() + stageTimeoutMs;
  let lastFailureDetail = 'matching <select> or <option> not mounted';
  let lastEvalError: string | null = null;

  while (Date.now() < deadline) {
    const remainingMs = deadline - Date.now();
    if (remainingMs <= 0) break;
    const evalTimeoutMs = Math.min(MAX_STAGE_EVALUATE_BUDGET_MS, remainingMs);
    try {
      const outcome = await evaluateFn<{ ok: boolean; reason?: string }>(
        `(() => {
          const labels = [...document.querySelectorAll('label')];
          const match = labels.find(l => (l.textContent || '').includes(${labelEsc}));
          if (!match) {
            const knownLabels = labels
              .map(l => (l.textContent || '').replace(/\\s+/g, ' ').trim())
              .filter(Boolean)
              .slice(0, 8);
            return {
              ok: false,
              reason: \`label \${${labelEsc}} not found (visible labels: [\${knownLabels.join(', ')}])\`
            };
          }
          const forId = match.getAttribute('for');
          const candidate = forId ? document.getElementById(forId) : match.querySelector('select');
          const select = candidate instanceof HTMLSelectElement ? candidate : match.querySelector('select');
          if (!select) {
            return {
              ok: false,
              reason: \`label \${${labelEsc}} found but <select> element is missing\`
            };
          }
          const optionValues = Array.from(select.options).map(opt => opt.value);
          if (!optionValues.includes(${valEsc})) {
            return {
              ok: false,
              reason: \`option \${${valEsc}} not present in <select> (available options: [\${optionValues.slice(0, 12).join(', ')}])\`
            };
          }
          const setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value')?.set;
          setter?.call(select, ${valEsc});
          if (select.value !== ${valEsc}) {
            return {
              ok: false,
              reason: \`select.value mismatch after setter (expected \${${valEsc}}, actual \${JSON.stringify(select.value)})\`
            };
          }
          select.dispatchEvent(new Event('change', { bubbles: true }));
          return { ok: true };
        })()`,
        evalTimeoutMs,
      );
      if (outcome?.ok) return;
      if (outcome?.reason) {
        lastFailureDetail = outcome.reason;
      }
    } catch (err) {
      lastEvalError = err instanceof Error ? err.message : String(err);
    }
    const sleepRemainingMs = deadline - Date.now();
    if (sleepRemainingMs <= 0) break;
    await new Promise((r) => setTimeout(r, Math.min(40, sleepRemainingMs)));
  }

  const details = [
    `reason: ${lastFailureDetail}`,
    ...(lastEvalError ? [`lastEvalError: ${lastEvalError}`] : []),
  ].join('; ');
  throw new Error(
    `Select for label not found: ${labelText} (option ${value}) after ${stageTimeoutMs}ms (${details})`,
  );
}

export interface BrowserHarness {
  binaryPath: string;
  browserVersion: { product: string; revision: string; protocolVersion: string };
  resetPage(): Promise<void>;
  goto(url: string): Promise<void>;
  setViewportSize(width: number, height: number): Promise<void>;
  emulateReducedMotion(reduce: boolean): Promise<void>;
  evaluate<T>(expression: string, timeoutMs?: number): Promise<T>;
  captureScreenshot(path: string): Promise<void>;
  waitForText(expectedText: string, timeoutMs?: number): Promise<void>;
  waitForSelector(selector: string, timeoutMs?: number): Promise<void>;
  clickNav(viewLabel: string): Promise<void>;
  clickButtonByText(labelText: string): Promise<void>;
  clickButtonByAriaLabel(ariaLabel: string): Promise<void>;
  fillInputByLabel(labelText: string, value: string, timeoutMs?: number): Promise<void>;
  selectByLabel(labelText: string, value: string, timeoutMs?: number): Promise<void>;
  pressKey(key: string): Promise<void>;
  runAxeAudit(): Promise<AxeAuditReport>;
  runCustomAxTreeSmokeChecks(): Promise<CustomAxTreeSmokeSummary>;
  close(): Promise<void>;
}

export interface LaunchBrowserHarnessOptions {
  startupTimeoutMs?: number;
  maxStartupAttempts?: number;
  /** Test-only hook to simulate a stalled CDP pipe on specific 1-based attempt numbers */
  _testStallAttempts?: ReadonlyArray<number>;
  /** Test-only timeout override applied only to stalled attempts in _testStallAttempts */
  _testStallTimeoutMs?: number;
  /** Test-only hook to inject a fatal malformed CDP frame during startup on specific 1-based attempt numbers */
  _testFatalFrameAttempts?: ReadonlyArray<number>;
  /** Test-only callback invoked with each attempt's pid and profileDir */
  _testOnAttemptSpawn?: (attempt: number, info: { pid: number | undefined; profileDir: string }) => void;
}

export function captureOwnedProcessDiagnostics(pid: number | undefined, listenPort?: number): string {
  if (typeof pid !== 'number' || pid <= 0) return 'pid=none';
  if (process.platform !== 'linux') return `pid=${pid}`;
  let state = 'unknown';
  let startTicks = 'unknown';
  let exe = 'unavailable';
  let wchan = 'unavailable';
  try {
    const stat = readFileSync(`/proc/${pid}/stat`, 'utf8').trim();
    const closeParen = stat.lastIndexOf(')');
    if (closeParen !== -1) {
      const fields = stat.slice(closeParen + 2).split(/\s+/);
      state = fields[0] ?? 'unknown';
      startTicks = fields[19] ?? 'unknown';
    }
  } catch {
    state = 'unavailable';
  }
  try {
    exe = realpathSync(`/proc/${pid}/exe`);
  } catch {
    // Process exited or exe link unreadable
  }
  try {
    wchan = readFileSync(`/proc/${pid}/wchan`, 'utf8').trim() || 'none';
  } catch {
    // Ignore
  }
  let listener = 'n/a';
  if (typeof listenPort === 'number' && listenPort > 0) {
    try {
      const hexPort = listenPort.toString(16).toUpperCase().padStart(4, '0');
      const tcp = readFileSync('/proc/net/tcp', 'utf8');
      const listening = tcp
        .split('\n')
        .some((line) => line.includes(`0100007F:${hexPort}`) && line.trim().split(/\s+/)[3] === '0A');
      listener = listening ? `127.0.0.1:${listenPort}=LISTEN` : `127.0.0.1:${listenPort}=NOT_LISTENING`;
    } catch {
      listener = 'unavailable';
    }
  }
  return `pid=${pid} state=${state} start_ticks=${startTicks} wchan=${wchan} exe=${exe} listener=${listener}`;
}

export function isProcessAlive(pid: number, probe?: (pid: number) => void): boolean {
  const proc = process as unknown as { kill?: (targetPid: number, signal?: string | number) => boolean };
  if (!probe && typeof proc.kill !== 'function') return true;
  try {
    if (probe) probe(pid);
    else proc.kill!(pid, 0);
    return true;
  } catch (error) {
    // Permission, procfs and IO errors do not prove exit. Zombies remain in
    // custody until the direct ChildProcess reports that it has been reaped.
    return (error as { code?: string }).code !== 'ESRCH';
  }
}

function observedChildExit(child: ChildProcess): boolean {
  const status = child as unknown as { exitCode?: number | null; signalCode?: string | null };
  return typeof status.exitCode === 'number' || typeof status.signalCode === 'string';
}

function getChildPid(child: ChildProcess): number | undefined {
  return (child as unknown as { pid?: number }).pid;
}

export async function waitForOwnedProcessExit(child: ChildProcess, timeoutMs: number): Promise<void> {
  const pid = getChildPid(child);
  const deadline = Date.now() + timeoutMs;
  while (typeof pid === 'number' && !observedChildExit(child)) {
    if (Date.now() >= deadline) {
      const diag = captureOwnedProcessDiagnostics(pid);
      throw new Error(
        `Owned process ${pid} has no observed exit after ${timeoutMs}ms; retain its private files and do not retry startup [${diag}]`,
      );
    }
    await new Promise((resolve) => setTimeout(resolve, Math.min(20, Math.max(1, deadline - Date.now()))));
  }
}

function signalBrowserProcessGroup(child: ChildProcess, signal: 'SIGTERM' | 'SIGKILL'): void {
  // After reaping, the numeric PID/group may belong to unrelated new work.
  if (observedChildExit(child)) return;
  const pid = getChildPid(child);
  const proc = process as unknown as { kill?: (targetPid: number, sig?: string | number) => boolean };
  if (process.platform !== 'win32' && typeof pid === 'number' && pid > 0 && typeof proc.kill === 'function') {
    try {
      proc.kill(-pid, signal);
      return;
    } catch {
      // Fall back to direct child signal if process group already exited
    }
  }
  try {
    child.kill(signal);
  } catch {
    // Process already exited
  }
}

export async function launchBrowserHarness(
  options?: LaunchBrowserHarnessOptions,
): Promise<BrowserHarness> {
  const rawStartupTimeoutMs = options?.startupTimeoutMs ?? BROWSER_STARTUP_TIMEOUT_MS;
  if (
    typeof rawStartupTimeoutMs !== 'number' ||
    !Number.isFinite(rawStartupTimeoutMs) ||
    !Number.isInteger(rawStartupTimeoutMs) ||
    rawStartupTimeoutMs < 1
  ) {
    throw new Error(
      `Invalid startupTimeoutMs: expected positive finite integer, got ${String(rawStartupTimeoutMs)}`,
    );
  }
  const startupTimeoutMs = rawStartupTimeoutMs;

  const rawMaxStartupAttempts = options?.maxStartupAttempts ?? MAX_BROWSER_STARTUP_ATTEMPTS;
  if (
    typeof rawMaxStartupAttempts !== 'number' ||
    !Number.isFinite(rawMaxStartupAttempts) ||
    !Number.isInteger(rawMaxStartupAttempts) ||
    rawMaxStartupAttempts < 1 ||
    rawMaxStartupAttempts > MAX_BROWSER_STARTUP_ATTEMPTS
  ) {
    throw new Error(
      `Invalid maxStartupAttempts: expected finite integer in 1..${MAX_BROWSER_STARTUP_ATTEMPTS}, got ${String(rawMaxStartupAttempts)}`,
    );
  }
  const maxStartupAttempts = rawMaxStartupAttempts;
  const attemptDiagnostics: string[] = [];

  for (let attempt = 1; attempt <= maxStartupAttempts; attempt++) {
    try {
      return await launchBrowserHarnessAttempt(attempt, startupTimeoutMs, options);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      attemptDiagnostics.push(`attempt ${attempt}/${maxStartupAttempts}: ${msg}`);
      // Do not retry if the host explicitly lacks a usable normal-user sandbox configuration
      if (
        msg.includes('Unsupported runner browser sandbox configuration') ||
        attempt >= maxStartupAttempts
      ) {
        throw new Error(
          attemptDiagnostics.length === 1
            ? msg
            : `Browser startup failed after ${attemptDiagnostics.length} attempts (${attemptDiagnostics.join(' || ')})`,
        );
      }
    }
  }

  throw new Error(`Browser startup failed (${attemptDiagnostics.join(' || ')})`);
}

async function launchBrowserHarnessAttempt(
  attempt: number,
  startupTimeoutMs: number,
  options?: LaunchBrowserHarnessOptions,
): Promise<BrowserHarness> {
  const binaryPath = discoverChromiumBinary();
  const setuidSandboxPath = discoverNormalUserSetuidSandbox(binaryPath);
  const apparmorRestrictUserns = readSysctlDiagnostic(
    '/proc/sys/kernel/apparmor_restrict_unprivileged_userns',
  );
  const baseTmp = process.env.TMPDIR && existsSync(process.env.TMPDIR) ? process.env.TMPDIR : tmpdir();
  mkdirSync(baseTmp, { recursive: true, mode: 0o700 });
  const profileDir = mkdtempSync(join(baseTmp, 'adm-e2e-browser-'));

  const childEnv: Record<string, string | undefined> = {
    ...process.env,
    // Prevent headless Chromium from stalling on absent desktop/UPower D-Bus activation on Linux runners
    DBUS_SESSION_BUS_ADDRESS: 'unix:path=/dev/null',
    DBUS_SYSTEM_BUS_ADDRESS: 'unix:path=/dev/null',
    ...(setuidSandboxPath ? { CHROME_DEVEL_SANDBOX: setuidSandboxPath } : {}),
  };

  const browserArgs = [
    '--headless=new',
    '--disable-gpu',
    '--disable-dev-shm-usage',
    '--disable-background-networking',
    '--disable-component-update',
    '--disable-default-apps',
    '--disable-extensions',
    '--disable-sync',
    '--metrics-recording-only',
    '--no-default-browser-check',
    '--no-first-run',
    '--no-proxy-server',
    '--no-service-autorun',
    `--user-data-dir=${profileDir}`,
    '--window-size=1440,900',
    '--remote-debugging-pipe',
  ];

  // On Ubuntu 24.04 runners where apparmor_restrict_unprivileged_userns=1 and /opt/google/chrome/chrome
  // is not installed, Playwright's cached Chromium binary does not match the path-based /etc/apparmor.d/chrome
  // profile automatically. Transitioning into the system 'chrome' AppArmor profile via normal-user
  // '/usr/bin/aa-exec -p chrome -- <binaryPath>' enables Chromium's full normal-user userns+seccomp sandbox.
  let spawnCommand = binaryPath;
  let spawnArgs = browserArgs;
  let aaExecProfile: string | undefined;
  if (
    process.platform === 'linux' &&
    binaryPath !== '/opt/google/chrome/chrome' &&
    apparmorRestrictUserns === '1' &&
    !setuidSandboxPath &&
    existsSync('/usr/bin/aa-exec') &&
    existsSync('/etc/apparmor.d/chrome')
  ) {
    try {
      const probe = spawnSync('/usr/bin/aa-exec', ['-p', 'chrome', '--', '/bin/true'], {
        encoding: 'utf8',
      });
      if (probe.status === 0) {
        spawnCommand = '/usr/bin/aa-exec';
        spawnArgs = ['-p', 'chrome', '--', binaryPath, ...browserArgs];
        aaExecProfile = 'chrome';
      }
    } catch {
      // Fall back to direct binaryPath spawn and preserve truthful launch diagnostics if unsupported
    }
  }

  // Launch with OS sandbox intact (never pass --no-sandbox or --disable-setuid-sandbox).
  // Spawn detached on POSIX so the browser and its crashpad/zygote helpers form an isolated process group
  // that can be terminated atomically without leaking orphan chrome_crashpad_handler processes.
  const spawnOpts = {
    stdio: ['ignore', 'pipe', 'pipe', 'pipe', 'pipe'] as ('ignore' | 'pipe')[],
    env: childEnv,
    detached: process.platform !== 'win32',
  };
  const child: ChildProcess = spawn(spawnCommand, spawnArgs, spawnOpts);

  options?._testOnAttemptSpawn?.(attempt, { pid: getChildPid(child), profileDir });
  const stallThisAttempt = Boolean(options?._testStallAttempts?.includes(attempt));
  const fatalFrameThisAttempt = Boolean(options?._testFatalFrameAttempts?.includes(attempt));

  const cdpWrite = child.stdio[3] as Writable;
  const cdpRead = child.stdio[4] as Readable | Duplex;

  let nextId = 1;
  let closedReason: string | null = null;
  let stderrHead = '';
  let stderrTail = '';
  let exitCode: number | null | undefined = undefined;
  let exitSignal: string | null | undefined = undefined;

  child.stderr?.on('data', (chunk: Buffer) => {
    const text = chunk.toString('utf8');
    if (stderrHead.length < 2_000) {
      stderrHead = (stderrHead + text).slice(0, 2_000);
    }
    stderrTail = (stderrTail + text).slice(-2_000);
  });

  function formatStderrEvidence(): string {
    const head = stderrHead.trim();
    const tail = stderrTail.trim();
    if (!head && !tail) return 'none';
    if (head === tail || !tail || head.includes(tail)) return head;
    return `${head}\n...[truncated]...\n${tail}`;
  }

  function formatDiagnosticContext(): string {
    const apparmorUserns = readSysctlDiagnostic(
      '/proc/sys/kernel/apparmor_restrict_unprivileged_userns',
    );
    const unprivUsernsClone = readSysctlDiagnostic('/proc/sys/kernel/unprivileged_userns_clone');
    const stderrCombined = `${stderrHead}\n${stderrTail}`;
    const sandboxUnsupported =
      stderrCombined.includes('No usable sandbox!') ||
      stderrCombined.includes('zygote_host_impl_linux.cc') ||
      (exitSignal === 'SIGABRT' && apparmorUserns === '1' && !setuidSandboxPath);
    const prefix = sandboxUnsupported
      ? 'Unsupported runner browser sandbox configuration (normal-user Chromium sandbox failed closed without --no-sandbox bypass): '
      : '';
    return (
      `${prefix}binary=${binaryPath} aa_exec_profile=${aaExecProfile ?? 'none'} attempt=${attempt} exitCode=${String(exitCode)} signal=${String(exitSignal)} ` +
      `apparmor_restrict_unprivileged_userns=${apparmorUserns} ` +
      `unprivileged_userns_clone=${unprivUsernsClone} ` +
      `setuid_sandbox=${setuidSandboxPath ?? 'none'} ` +
      `stderr=${formatStderrEvidence()}`
    );
  }

  const pending = new Map<
    number,
    {
      resolve: (value: Record<string, unknown>) => void;
      reject: (err: Error) => void;
      timer: ReturnType<typeof setTimeout>;
    }
  >();

  function rejectAllPending(reason: string) {
    if (!closedReason) closedReason = reason;
    for (const [id, entry] of pending.entries()) {
      clearTimeout(entry.timer);
      pending.delete(id);
      entry.reject(new Error(`${reason} [${formatDiagnosticContext()}]`));
    }
  }

  let resolveExitDrain: (() => void) | undefined;
  const exitDrainPromise = new Promise<void>((resolve) => {
    resolveExitDrain = resolve;
  });

  child.once('error', (err) => {
    rejectAllPending(`Browser process error (${binaryPath}): ${err.message}`);
    resolveExitDrain?.();
  });
  child.once('exit', (code, signal) => {
    exitCode = code;
    exitSignal = signal;
    rejectAllPending(
      `Browser process exited unexpectedly (code=${String(code)}, signal=${String(signal)})`,
    );
    resolveExitDrain?.();
  });
  cdpRead.once('close', () => {
    rejectAllPending('CDP read pipe closed');
  });
  cdpRead.once('error', (err) => {
    rejectAllPending(`CDP read pipe error: ${err.message}`);
  });
  cdpWrite.once('error', (err) => {
    rejectAllPending(`CDP write pipe error: ${err.message}`);
  });

  let initialDiagnosticBeforeSignal: string | null = null;
  let terminatePromise: Promise<void> | null = null;
  function terminateBrowserProcessTree(reason: string): Promise<void> {
    if (terminatePromise) return terminatePromise;
    terminatePromise = (async () => {
      if (initialDiagnosticBeforeSignal === null) {
        initialDiagnosticBeforeSignal = formatDiagnosticContext();
      }
      rejectAllPending(reason);
      try {
        (cdpWrite as unknown as { destroy?: () => void }).destroy?.();
      } catch {
        // Ignore pipe destroy error
      }
      try {
        (cdpRead as unknown as { destroy?: () => void }).destroy?.();
      } catch {
        // Ignore pipe destroy error
      }
      if (exitCode === undefined && exitSignal === undefined) {
        signalBrowserProcessGroup(child, 'SIGTERM');
        await Promise.race([
          exitDrainPromise,
          new Promise<void>((resolve) => setTimeout(resolve, 400)),
        ]);
      }
      // Signal only while the original direct child remains unreaped.
      signalBrowserProcessGroup(child, 'SIGKILL');
      await Promise.race([
        exitDrainPromise,
        new Promise<void>((resolve) => setTimeout(resolve, 5_000)),
      ]);
      // An error/pipe event or a timeout is not proof that the PID was reaped.
      // Never remove custody files or spawn a retry while the owned child lives.
      await waitForOwnedProcessExit(child, 5_000);
      const groupPid = getChildPid(child);
      if (process.platform !== 'win32' && typeof groupPid === 'number') {
        const deadline = Date.now() + 5_000;
        while (isProcessAlive(-groupPid)) {
          if (Date.now() >= deadline) {
            throw new Error(`Owned browser group ${groupPid} has not disappeared; preserve its private profile and do not retry startup`);
          }
          await new Promise(resolve => setTimeout(resolve, 20));
        }
      }
      try {
        (child as unknown as { stdout?: { destroy?: () => void } }).stdout?.destroy?.();
        (child.stderr as unknown as { destroy?: () => void } | null)?.destroy?.();
      } catch {
        // Ignore stream destroy error
      }
      rmSync(profileDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    })();
    return terminatePromise;
  }

  const frameReader = createBoundedCdpFrameReader({
    maxFrameBytes: MAX_CDP_FRAME_BYTES,
    maxBufferBytes: MAX_CDP_BUFFER_BYTES,
    onMessage(msg) {
      if (msg.id !== undefined && pending.has(msg.id)) {
        const entry = pending.get(msg.id)!;
        clearTimeout(entry.timer);
        pending.delete(msg.id);
        if (msg.error) {
          entry.reject(new Error(msg.error.message ?? 'CDP command failed'));
        } else {
          entry.resolve(msg.result ?? {});
        }
      }
    },
    onFatalError(err) {
      void terminateBrowserProcessTree(err.message);
    },
  });

  cdpRead.on('data', (chunk: Buffer) => {
    if (!stallThisAttempt && !fatalFrameThisAttempt) {
      frameReader.push(chunk);
    }
  });

  function sendCdp(
    method: string,
    params: Record<string, unknown> = {},
    sessionId?: string,
    timeoutMs = CDP_COMMAND_TIMEOUT_MS,
  ): Promise<Record<string, unknown>> {
    if (closedReason) {
      return Promise.reject(
        new Error(`Cannot send CDP ${method}: ${closedReason} [${formatDiagnosticContext()}]`),
      );
    }
    if (pending.size >= MAX_PENDING_CDP_COMMANDS) {
      return Promise.reject(
        new Error(`CDP pending command limit exceeded (max ${MAX_PENDING_CDP_COMMANDS}): ${method}`),
      );
    }
    const id = nextId++;
    return new Promise((resolveCmd, rejectCmd) => {
      const timer = setTimeout(() => {
        pending.delete(id);
        rejectCmd(new Error(`CDP command timed out after ${timeoutMs}ms: ${method} [${formatDiagnosticContext()}]`));
      }, timeoutMs);
      pending.set(id, { resolve: resolveCmd, reject: rejectCmd, timer });
      const payload = sessionId ? { id, method, params, sessionId } : { id, method, params };
      if (fatalFrameThisAttempt) {
        setTimeout(() => {
          frameReader.push('{malformed-fatal-cdp-frame\0');
        }, 10);
      } else if (!stallThisAttempt) {
        cdpWrite.write(`${JSON.stringify(payload)}\0`);
      }
    });
  }

  const attemptTimeoutMs = stallThisAttempt
    ? (options?._testStallTimeoutMs ?? startupTimeoutMs)
    : startupTimeoutMs;

  let browserVersion = { product: '', revision: '', protocolVersion: '' };
  let sessionId = '';

  const startupPromise = (async () => {
    const versionInfo = await sendCdp('Browser.getVersion', {}, undefined, attemptTimeoutMs);
    browserVersion = {
      product: String(versionInfo.product ?? ''),
      revision: String(versionInfo.revision ?? ''),
      protocolVersion: String(versionInfo.protocolVersion ?? ''),
    };
    if (!browserVersion.product) {
      throw new Error(`Qualified browser check failed: empty Browser.getVersion product from ${binaryPath}`);
    }

    const existingTargets = await sendCdp('Target.getTargets', {}, undefined, attemptTimeoutMs);
    const initialTargetIds = Array.isArray(existingTargets.targetInfos)
      ? (existingTargets.targetInfos as Array<{ targetId?: string; type?: string }>)
          .filter((t) => t.type === 'page' && t.targetId)
          .map((t) => String(t.targetId))
      : [];

    const created = await sendCdp('Target.createTarget', { url: 'about:blank' }, undefined, attemptTimeoutMs);
    const targetId = String(created.targetId);
    const attached = await sendCdp(
      'Target.attachToTarget',
      { targetId, flatten: true },
      undefined,
      attemptTimeoutMs,
    );
    sessionId = String(attached.sessionId);

    // Close any inherited initial pages so only the isolated fixture page target remains
    for (const inheritedId of initialTargetIds) {
      if (inheritedId !== targetId) {
        await sendCdp('Target.closeTarget', { targetId: inheritedId }, undefined, attemptTimeoutMs).catch(
          () => undefined,
        );
      }
    }

    await Promise.all([
      sendCdp('Page.enable', {}, sessionId, attemptTimeoutMs),
      sendCdp('Runtime.enable', {}, sessionId, attemptTimeoutMs),
      sendCdp('DOM.enable', {}, sessionId, attemptTimeoutMs),
      sendCdp(
        'Emulation.setDeviceMetricsOverride',
        { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false },
        sessionId,
        attemptTimeoutMs,
      ),
    ]);

    // Verify the initial page target's renderer execution context is live before completing startup
    await sendCdp(
      'Runtime.evaluate',
      { expression: 'document.readyState', returnByValue: true },
      sessionId,
      attemptTimeoutMs,
    );
  })();
  // Prevent late unhandled rejection if startupTimer wins the race and rejectAllPending rejects startupPromise
  startupPromise.catch(() => undefined);

  let startupTimer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      startupPromise,
      new Promise<never>((_, rejectStartup) => {
        startupTimer = setTimeout(
          () => rejectStartup(new Error(`Browser startup timed out after ${attemptTimeoutMs}ms`)),
          attemptTimeoutMs,
        );
      }),
    ]);
  } catch (err) {
    // Allow brief drain if termination has not started yet so spontaneous child exit code/signal and stderr are captured
    if (!terminatePromise && exitCode === undefined && exitSignal === undefined) {
      await Promise.race([
        exitDrainPromise,
        new Promise<void>((resolve) => setTimeout(resolve, 150)),
      ]);
    }
    const baseMsg = err instanceof Error ? err.message : String(err);
    const diagBeforeKill = initialDiagnosticBeforeSignal ?? formatDiagnosticContext();
    await terminateBrowserProcessTree(baseMsg);
    throw new Error(baseMsg.includes(diagBeforeKill) ? baseMsg : `${baseMsg} | ${diagBeforeKill}`);
  } finally {
    clearTimeout(startupTimer);
  }

  async function evaluate<T>(expression: string, timeoutMs = CDP_COMMAND_TIMEOUT_MS): Promise<T> {
    const res = await sendCdp(
      'Runtime.evaluate',
      {
        expression,
        returnByValue: true,
        awaitPromise: true,
      },
      sessionId,
      timeoutMs,
    );
    const exceptionDetails = res.exceptionDetails as { text?: string; exception?: { description?: string } } | undefined;
    if (exceptionDetails) {
      throw new Error(
        `Browser evaluation failed: ${exceptionDetails.exception?.description ?? exceptionDetails.text ?? 'unknown error'}`,
      );
    }
    const resultObj = res.result as { value?: T } | undefined;
    return resultObj?.value as T;
  }

  async function waitForCondition(
    predicateExpr: string,
    description: string,
    timeoutMs = 10_000,
  ): Promise<void> {
    const stageTimeoutMs = validateStageTimeoutMs(timeoutMs, 'waitForCondition timeoutMs');
    const deadline = Date.now() + stageTimeoutMs;
    let lastEvalError: string | null = null;
    while (Date.now() < deadline) {
      const remainingMs = deadline - Date.now();
      if (remainingMs <= 0) break;
      const evalTimeoutMs = Math.min(MAX_STAGE_EVALUATE_BUDGET_MS, remainingMs);
      try {
        const ok = await evaluate<boolean>(`Boolean(${predicateExpr})`, evalTimeoutMs);
        if (ok) return;
      } catch (err) {
        lastEvalError = err instanceof Error ? err.message : String(err);
      }
      const sleepRemainingMs = deadline - Date.now();
      if (sleepRemainingMs <= 0) break;
      await new Promise((r) => setTimeout(r, Math.min(40, sleepRemainingMs)));
    }
    // Use textContent (not layout-reflowing innerText) with a strict 1,000ms bound so waitForCondition
    // never hangs an extra 20,000ms on CDP_COMMAND_TIMEOUT_MS into the outer Vitest test timeout.
    const bodySnippet = await evaluate<string>(
      `(document.body?.textContent || '').replace(/\\s+/g, ' ').trim().slice(0, 600)`,
      1_000,
    ).catch(() => '<unavailable>');
    const errSuffix = lastEvalError ? ` (lastEvalError: ${lastEvalError})` : '';
    throw new Error(
      `Timed out waiting for ${description} after ${stageTimeoutMs}ms${errSuffix}. Current page text: ${bodySnippet}`,
    );
  }

  return {
    binaryPath,
    browserVersion,
    async resetPage() {
      const alreadyBlank = await evaluate<boolean>(
        `window.location.href === 'about:blank' && document.readyState === 'complete'`,
        2_500,
      ).catch(() => false);
      if (alreadyBlank) return;
      await evaluate(`window.stop?.()`, STOP_LOADING_TIMEOUT_MS).catch(() => undefined);
      await sendCdp('Page.stopLoading', {}, sessionId, STOP_LOADING_TIMEOUT_MS).catch(() => undefined);
      await sendCdp('Page.navigate', { url: 'about:blank' }, sessionId, GOTO_NAVIGATE_TIMEOUT_MS);
      await waitForCondition(
        `window.location.href === 'about:blank' && document.readyState === 'complete'`,
        'reset to about:blank',
        5_000,
      );
    },
    async goto(url: string) {
      const parsed = new URL(url);
      if (parsed.hostname !== '127.0.0.1' && parsed.hostname !== 'localhost') {
        throw new Error(`Refusing non-loopback navigation in E2E harness: ${url}`);
      }
      await evaluate(`window.stop?.()`, STOP_LOADING_TIMEOUT_MS).catch(() => undefined);
      await sendCdp('Page.stopLoading', {}, sessionId, STOP_LOADING_TIMEOUT_MS).catch(() => undefined);
      let firstNavError: string | null = null;
      let navigationResult: Record<string, unknown> | undefined;
      async function navigate(): Promise<void> {
        navigationResult = await sendCdp('Page.navigate', { url }, sessionId, GOTO_NAVIGATE_TIMEOUT_MS);
        if (navigationResult.errorText) {
          throw new Error(`Page.navigate failed: ${String(navigationResult.errorText)}`);
        }
        if (navigationResult.isDownload) throw new Error('Page.navigate started a download instead of a document');
      }
      try {
        await navigate();
      } catch (err) {
        firstNavError = err instanceof Error ? err.message : String(err);
        await evaluate(`window.stop?.()`, STOP_LOADING_TIMEOUT_MS).catch(() => undefined);
        await sendCdp('Page.stopLoading', {}, sessionId, STOP_LOADING_TIMEOUT_MS).catch(() => undefined);
        await sendCdp('Page.navigate', { url: 'about:blank' }, sessionId, STOP_LOADING_TIMEOUT_MS).catch(
          () => undefined,
        );
        try {
          await navigate();
        } catch (retryErr) {
          const retryMsg = retryErr instanceof Error ? retryErr.message : String(retryErr);
          throw new Error(
            `Navigation to ${url} failed after 2 attempts (attempt 1: ${firstNavError} || attempt 2: ${retryMsg})`,
          );
        }
      }
      try {
        await waitForCondition(
          `document.readyState === 'complete' && document.querySelector('#root')?.children.length > 0`,
          `navigation to ${url}`,
          GOTO_DOCUMENT_TIMEOUT_MS,
        );
      } catch (condErr) {
        const condMsg = condErr instanceof Error ? condErr.message : String(condErr);
        const documentState = await evaluate<unknown>(
          `({url: location.href, readyState: document.readyState, rootChildren: document.querySelector('#root')?.children.length ?? null})`,
          1_000,
        ).catch(() => '<unavailable>');
        const diagnostic = `${condMsg}; navigation=${JSON.stringify(navigationResult)}; document=${JSON.stringify(documentState)}`;
        throw new Error(
          firstNavError ? `${diagnostic} (after initial Page.navigate retry: ${firstNavError})` : diagnostic,
        );
      }
    },
    async setViewportSize(width: number, height: number) {
      await sendCdp(
        'Emulation.setDeviceMetricsOverride',
        { width, height, deviceScaleFactor: 1, mobile: width <= 600 },
        sessionId,
      );
    },
    async emulateReducedMotion(reduce: boolean) {
      await sendCdp(
        'Emulation.setEmulatedMedia',
        {
          features: [{ name: 'prefers-reduced-motion', value: reduce ? 'reduce' : 'no-preference' }],
        },
        sessionId,
      );
    },
    evaluate,
    async captureScreenshot(path: string) {
      const result = await sendCdp('Page.captureScreenshot', { format: 'png' }, sessionId) as { data: string };
      mkdirSync(dirname(path), { recursive: true });
      writeFileSync(path, Buffer.from(result.data, 'base64'));
    },
    async waitForText(expectedText: string, timeoutMs = 10_000) {
      const escaped = JSON.stringify(expectedText);
      await waitForCondition(
        `(document.body?.textContent || '').includes(${escaped})`,
        `text ${escaped}`,
        timeoutMs,
      );
    },
    async waitForSelector(selector: string, timeoutMs = 10_000) {
      const escaped = JSON.stringify(selector);
      await waitForCondition(
        `document.querySelector(${escaped}) !== null`,
        `selector ${escaped}`,
        timeoutMs,
      );
    },
    async clickNav(viewLabel: string) {
      const escaped = JSON.stringify(viewLabel);
      const clicked = await evaluate<boolean>(
        `(() => {
        const buttons = [...document.querySelectorAll('nav button.nav-item')];
        const target = buttons.find(b => (b.textContent || '').includes(${escaped}));
        if (!target) return false;
        target.click();
        return true;
      })()`,
        MAX_STAGE_EVALUATE_BUDGET_MS,
      );
      if (!clicked) throw new Error(`Navigation button not found: ${viewLabel}`);
    },
    async clickButtonByText(labelText: string) {
      const escaped = JSON.stringify(labelText);
      const clicked = await evaluate<boolean>(
        `(() => {
        const buttons = [...document.querySelectorAll('button, [role="tab"], summary')];
        const target = buttons.find(b => (b.textContent || '').replace(/\\s+/g, ' ').trim().includes(${escaped}));
        if (!target) return false;
        target.click();
        return true;
      })()`,
        MAX_STAGE_EVALUATE_BUDGET_MS,
      );
      if (!clicked) throw new Error(`Button with text not found: ${labelText}`);
    },
    async clickButtonByAriaLabel(ariaLabel: string) {
      const escaped = JSON.stringify(ariaLabel);
      const clicked = await evaluate<boolean>(
        `(() => {
        const all = [...document.querySelectorAll('[aria-label], button[title]')];
        const target =
          all.find(el => el.getAttribute('aria-label') === ${escaped} || el.getAttribute('title') === ${escaped}) ??
          all.find(el => (el.getAttribute('aria-label') || '').includes(${escaped}));
        if (!target || !(target instanceof HTMLElement)) return false;
        target.click();
        return true;
      })()`,
        MAX_STAGE_EVALUATE_BUDGET_MS,
      );
      if (!clicked) throw new Error(`Control with aria-label not found: ${ariaLabel}`);
    },
    async fillInputByLabel(labelText: string, value: string, timeoutMs = FORM_CONTROL_STAGE_TIMEOUT_MS) {
      await fillInputByLabelStage(evaluate, labelText, value, timeoutMs);
    },
    async selectByLabel(labelText: string, value: string, timeoutMs = FORM_CONTROL_STAGE_TIMEOUT_MS) {
      await selectByLabelStage(evaluate, labelText, value, timeoutMs);
    },
    async pressKey(key: string) {
      // Chromium needs a virtual key code for native Tab focus and Enter
      // button activation; a key label alone only reaches event listeners.
      const keyCode = ({ Enter: 13, Tab: 9, Escape: 27, ' ': 32 } as Record<string, number>)[key];
      const fields = keyCode ? { key, windowsVirtualKeyCode: keyCode } : { key };
      await sendCdp('Input.dispatchKeyEvent', { type: 'keyDown', ...fields,
        ...(key === 'Enter' ? { code: 'Enter', text: '\r', unmodifiedText: '\r' } : {}),
      }, sessionId);
      await sendCdp('Input.dispatchKeyEvent', { type: 'keyUp', ...fields }, sessionId);
    },
    async runAxeAudit(): Promise<AxeAuditReport> {
      const axeBundle = loadPinnedAxeCoreBundle();
      const alreadyLoaded = await evaluate<boolean>(`typeof window.axe?.run === 'function'`);
      if (!alreadyLoaded) {
        await evaluate<void>(`${axeBundle.source}\nundefined;`);
      }
      const report = await evaluate<AxeAuditReport>(
        buildAxeRunExpression(WCAG21_AA_TAGS, axeBundle.sha256),
      );
      return report;
    },
    async runCustomAxTreeSmokeChecks(): Promise<CustomAxTreeSmokeSummary> {
      await sendCdp('Accessibility.enable', {}, sessionId);
      try {
        const axTree = await sendCdp('Accessibility.getFullAXTree', {}, sessionId);
        const nodes = Array.isArray(axTree.nodes)
          ? (axTree.nodes as Array<{
              ignored?: boolean;
              role?: { value?: string };
              name?: { value?: string };
            }>)
          : [];
        const unlabeledButtons: string[] = [];
        const unlabeledInputs: string[] = [];
        for (const node of nodes) {
          if (node.ignored) continue;
          const role = node.role?.value ?? '';
          const name = (node.name?.value ?? '').trim();
          if (role === 'button' && !name) unlabeledButtons.push(role);
          if ((role === 'textbox' || role === 'combobox' || role === 'searchbox') && !name) {
            unlabeledInputs.push(role);
          }
        }
        return {
          totalNodes: nodes.length,
          unlabeledButtons,
          unlabeledInputs,
        };
      } finally {
        await sendCdp('Accessibility.disable', {}, sessionId).catch(() => undefined);
      }
    },
    async close() {
      await terminateBrowserProcessTree('Browser harness closing');
    },
  };
}

export interface IsolatedNativeAdmiraltyServer {
  baseUrl: string;
  ownerToken: string;
  allowedProject: string;
  binaryPath: string;
  writeLegacySnapshot(value: unknown): void;
  close(): Promise<void>;
}

export async function startIsolatedNativeAdmiraltyServer(options?: {
  ownerToken?: string;
  allowedProjects?: string[];
  legacySnapshot?: unknown;
  legacyPublicRead?: boolean;
}): Promise<IsolatedNativeAdmiraltyServer> {
  await ensureBuiltDashboard();

  const candidates = [
    process.env.ADMIRALTY_E2E_ADMIRALTYD_BIN,
    process.env.ADMIRALTYD_BINARY,
    join(REPO_ROOT, 'target', 'debug', 'admiraltyd'),
    join(REPO_ROOT, 'target', 'release', 'admiraltyd'),
  ].filter((c): c is string => Boolean(c));

  const binaryPath = candidates.find((c) => existsSync(c));
  if (!binaryPath) {
    throw new Error(
      `No compiled admiraltyd binary found for isolated server integration test (checked: ${candidates.join(', ')}). ` +
        `Build admiraltyd with 'cargo build --locked -p admiralty-server --bin admiraltyd' first.`,
    );
  }

  // Use /tmp directly so all ancestor directories (/ and /tmp sticky 1777) satisfy startup::read_private_file
  const privateDir = mkdtempSync(join(realpathSync('/tmp'), 'adm-e2e-native-srv-'));
  chmodSync(privateDir, 0o700);

  const ownerToken = options?.ownerToken ?? 'adm-isolated-native-owner-token-0123456789abcdef';
  const allowedProjects = options?.allowedProjects ?? ['proj-001'];

  const tokenFile = join(privateDir, 'token');
  writeFileSync(tokenFile, ownerToken, { encoding: 'utf8', mode: 0o600 });
  chmodSync(tokenFile, 0o600);

  const nativeConfigFile = join(privateDir, 'native.json');
  writeFileSync(
    nativeConfigFile,
    JSON.stringify({
      schema_version: 1,
      operator: {
        principal: 'owner',
        projects: allowedProjects,
      },
      agents: [],
    }),
    { encoding: 'utf8', mode: 0o600 },
  );
  chmodSync(nativeConfigFile, 0o600);

  const legacyStatus = join(privateDir, 'status.json');
  const legacyIndex = join(privateDir, 'index.html');
  if (options?.legacySnapshot) {
    writeFileSync(legacyStatus, JSON.stringify(options.legacySnapshot), { mode: 0o600 });
    writeFileSync(legacyIndex, '<p>Isolated browser fixture</p>', { mode: 0o600 });
  }
  const dbPath = join(privateDir, 'native.sqlite');

  // Reserve an ephemeral loopback port
  const probeServer = createServer(() => undefined);
  await new Promise<void>((resolveListen, rejectListen) => {
    probeServer.once('error', rejectListen);
    probeServer.listen(0, '127.0.0.1', () => resolveListen());
  });
  const addr = probeServer.address();
  if (!addr || typeof addr === 'string') {
    rmSync(privateDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    throw new Error('Failed to allocate ephemeral port for isolated admiraltyd');
  }
  const port = addr.port;
  await new Promise<void>((resolveClose) => probeServer.close(() => resolveClose()));

  const baseUrl = `http://127.0.0.1:${port}`;
  let stderrLog = '';
  let exitState: { exited: boolean; code: number | null | undefined; signal: string | null | undefined } = {
    exited: false,
    code: undefined,
    signal: undefined,
  };

  const child = spawn(
    binaryPath,
    [
      ...(options?.legacySnapshot ? ['--legacy-status-file', legacyStatus, '--legacy-index-file', legacyIndex] : []),
      ...(options?.legacySnapshot && options.legacyPublicRead ? ['--legacy-public-read'] : []),
      '--database',
      dbPath,
      '--listen',
      `127.0.0.1:${port}`,
      '--token-file',
      tokenFile,
      '--native-config',
      nativeConfigFile,
      '--assets',
      DIST_DIR,
    ],
    {
      stdio: ['ignore', 'ignore', 'pipe'],
      env: { ...process.env, TMPDIR: privateDir },
    },
  );

  child.stderr?.on('data', (chunk: Buffer) => {
    stderrLog = (stderrLog + chunk.toString('utf8')).slice(-2_000);
  });
  child.once('exit', (code, signal) => {
    exitState = { exited: true, code, signal };
  });

  const deadline = Date.now() + 25_000;
  let ready = false;
  let lastReadyReason = 'listener has not responded';
  while (Date.now() < deadline) {
    if (exitState.exited) {
      rmSync(privateDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
      throw new Error(
        `Isolated admiraltyd exited prematurely (code=${String(exitState.code)}, signal=${String(exitState.signal)}, stderr=${stderrLog.trim()})`,
      );
    }
    try {
      const res = await fetch(`${baseUrl}/v2/fleet`, {
        headers: { Authorization: `Bearer ${ownerToken}` },
        signal: AbortSignal.timeout(Math.min(2_500, Math.max(1, deadline - Date.now()))),
      });
      void res.body?.cancel().catch(() => undefined);
      if (res.status === 200) {
        ready = true;
        break;
      }
      lastReadyReason = `HTTP ${res.status}`;
    } catch (err) {
      lastReadyReason = err instanceof Error ? err.message.slice(0, 200) : 'fetch failed';
    }
    await new Promise((r) => setTimeout(r, 40));
  }

  if (!ready) {
    const childDiag = captureOwnedProcessDiagnostics(getChildPid(child), port);
    child.kill('SIGKILL');
    await waitForOwnedProcessExit(child, 5_000);
    rmSync(privateDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    throw new Error(
      `Timed out waiting for isolated admiraltyd at ${baseUrl} (stderr=${stderrLog.trim()}; reason=${lastReadyReason}; child=${childDiag})`,
    );
  }

  return {
    baseUrl,
    ownerToken,
    allowedProject: allowedProjects[0] ?? 'proj-001',
    binaryPath,
    writeLegacySnapshot(value: unknown) { writeFileSync(legacyStatus, JSON.stringify(value), { mode: 0o600 }); },
    async close() {
      if (!exitState.exited) {
        child.kill('SIGTERM');
        await new Promise<void>((resolveExit) => {
          const timer = setTimeout(() => {
            child.kill('SIGKILL');
            resolveExit();
          }, 2_000);
          child.once('exit', () => {
            clearTimeout(timer);
            resolveExit();
          });
        });
      }
      await waitForOwnedProcessExit(child, 5_000);
      rmSync(privateDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    },
  };
}

