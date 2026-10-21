// Runtime shape checks for JSON that crosses into the demo scripts: capture
// manifests and receipts on disk, the producer feed and Chrome DevTools
// protocol messages. Each guard checks every field the scripts read, so a
// value is narrowed only after it has been validated (no `as` casts on
// parsed input).

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

const optional = <T>(value: unknown, guard: (item: unknown) => item is T): boolean =>
  value === undefined || guard(value);
const isString = (value: unknown): value is string => typeof value === 'string';
const isNumber = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value);
const isStringArray = (value: unknown): value is string[] => Array.isArray(value) && value.every(isString);

export interface Viewport extends Record<string, unknown> { pixel_width: number; pixel_height: number }
export interface CaptureFrame extends Record<string, unknown> { file: string; duration_ms: number; sha256: string }
export interface CaptureManifest extends Record<string, unknown> {
  scene: string; source: Record<string, unknown>; viewport: Viewport; frames: CaptureFrame[];
}
export interface Artifact extends Record<string, unknown> { bytes: number; sha256: string }
export interface Receipt extends Record<string, unknown> {
  scene: string; source: Record<string, unknown>; viewport: Viewport;
  artifacts: Record<string, Artifact>;
  gif_frames: number; webp_frames: number; duration_ms: number;
  encoding: Record<string, unknown> & { web_budget_mib_each: number };
  capture_manifest_sha256: string;
}

export function isViewport(value: unknown): value is Viewport {
  return isRecord(value) && isNumber(value.pixel_width) && isNumber(value.pixel_height);
}

export function isCaptureFrame(value: unknown): value is CaptureFrame {
  return isRecord(value) && isString(value.file) && isNumber(value.duration_ms) && isString(value.sha256);
}

/** A `capture.json` written by `capture.ts`. */
export function isCaptureManifest(value: unknown): value is CaptureManifest {
  return isRecord(value) && isString(value.scene) && isRecord(value.source) && isViewport(value.viewport)
    && Array.isArray(value.frames) && value.frames.every(isCaptureFrame);
}

function isArtifact(value: unknown): value is Artifact {
  return isRecord(value) && isNumber(value.bytes) && isString(value.sha256);
}

/** A `<scene>.json` receipt written by `encode.ts`. */
export function isReceipt(value: unknown): value is Receipt {
  return isRecord(value) && isString(value.scene) && isRecord(value.source) && isViewport(value.viewport)
    && isRecord(value.artifacts) && Object.values(value.artifacts).every(isArtifact)
    && isNumber(value.gif_frames) && isNumber(value.webp_frames) && isNumber(value.duration_ms)
    && isRecord(value.encoding) && isNumber(value.encoding.web_budget_mib_each)
    && isString(value.capture_manifest_sha256);
}

export interface ProducerAgent extends Record<string, unknown> { project?: string; screen?: string; id?: string }
export interface ProducerFeed extends Record<string, unknown> {
  simulated?: unknown; generated_utc?: string; host?: string; order?: string[];
  git?: Record<string, { commits?: unknown[] } | undefined>; agents?: ProducerAgent[];
}

function isProducerAgent(value: unknown): value is ProducerAgent {
  return isRecord(value) && optional(value.project, isString) && optional(value.screen, isString)
    && optional(value.id, isString);
}

function isGitProject(value: unknown): value is { commits?: unknown[] } | undefined {
  return value === undefined || (isRecord(value) && optional(value.commits, Array.isArray));
}

/** The `/v1/legacy` producer feed, as far as `capture.ts` reads it. */
export function isProducerFeed(value: unknown): value is ProducerFeed {
  return isRecord(value) && optional(value.generated_utc, isString) && optional(value.host, isString)
    && optional(value.order, isStringArray)
    && optional(value.git, (git): git is Record<string, unknown> => isRecord(git) && Object.values(git).every(isGitProject))
    && optional(value.agents, (agents): agents is unknown[] => Array.isArray(agents) && agents.every(isProducerAgent));
}

export interface CdpMessage {
  id?: number; method?: string; params?: Record<string, unknown>; result?: Record<string, unknown>;
  error?: { message: string };
}

/** One DevTools protocol frame (a command result or an event). */
export function isCdpMessage(value: unknown): value is CdpMessage {
  return isRecord(value) && optional(value.id, isNumber) && optional(value.method, isString)
    && optional(value.params, isRecord) && optional(value.result, isRecord)
    && optional(value.error, (error): error is { message: string } => isRecord(error) && isString(error.message));
}

export interface PausedRequest { requestId: string; request: { url: string; method: string } }

/** `Fetch.requestPaused` event parameters. */
export function isPausedRequest(value: unknown): value is PausedRequest {
  return isRecord(value) && isString(value.requestId) && isRecord(value.request)
    && isString(value.request.url) && isString(value.request.method);
}

export interface DevtoolsTarget { type: string; webSocketDebuggerUrl: string }

/** `/json/list`: the debuggable targets that carry a WebSocket URL. */
export function devtoolsTargets(value: unknown): DevtoolsTarget[] {
  if (!Array.isArray(value)) throw new Error('DevTools target list is not an array.');
  return value.filter((item): item is DevtoolsTarget =>
    isRecord(item) && isString(item.type) && isString(item.webSocketDebuggerUrl));
}

/** Parse JSON text and require `guard` to accept it. */
export function parseChecked<T>(text: string, guard: (value: unknown) => value is T, what: string): T {
  const value: unknown = JSON.parse(text);
  if (!guard(value)) throw new Error(`${what} does not have the expected shape.`);
  return value;
}
