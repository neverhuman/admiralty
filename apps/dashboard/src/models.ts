import { ApiError, readBoundedJson } from './api';
import { assertBoundedJson } from './validation';

export type Provider = 'openai_compatible' | 'opencode_ssh' | 'codex_cli';
export type PublicModel = { id: string; label: string; provider: Provider; provider_model: string; tier: number; enabled: boolean; node_id: string | null;
  max_concurrency: number; capabilities: string[]; task_max_attempts: number; escalate_to: string | null; usage_cost: { status: 'unknown' } };
export type ModelsResponse = { schema_version: 1; revision: string; models: PublicModel[]; native_execution_qualified: false };
export type ModelSpec = Omit<PublicModel, 'usage_cost'> & { endpoint: string | null; credential_file_ref: string | null };
export type ModelDraft = { id: string; label: string; provider: Provider; provider_model: string; tier: string; enabled: boolean; node_id: string;
  endpoint: string; credential_file_ref: string; max_concurrency: string; capabilities: string; task_max_attempts: string; escalate_to: string };
export const emptyDraft = (): ModelDraft => ({ id: '', label: '', provider: 'openai_compatible', tier: '0', enabled: true,
  provider_model: '', node_id: '', endpoint: '', credential_file_ref: '', max_concurrency: '16', capabilities: 'coding', task_max_attempts: '1', escalate_to: '' });
const identity = (value: unknown): value is string => typeof value === 'string' && /^[A-Za-z0-9_.-]{1,128}$/.test(value) && value !== '.' && value !== '..';
const integer = (value: unknown, min: number, max: number): value is number => typeof value === 'number' && Number.isInteger(value) && value >= min && value <= max;
const revision = (value: unknown): value is string => typeof value === 'string' && /^(0|[1-9][0-9]{0,19})$/.test(value) && BigInt(value) <= 18446744073709551615n;
const readable = (value: unknown, bytes: number): value is string => typeof value === 'string' && !!value.trim() && new TextEncoder().encode(value).byteLength <= bytes && !/\p{Cc}/u.test(value);
export function parseModels(value: unknown): ModelsResponse {
  assertBoundedJson(value);
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Unsupported model registry response');
  const registry = value as Record<string, unknown>;
  if (registry.schema_version !== 1 || !revision(registry.revision) || registry.native_execution_qualified !== false
    || !Array.isArray(registry.models) || registry.models.length > 256) throw new Error('Unsupported model registry envelope');
  const ids = new Set<string>();
  for (const value of registry.models) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid model registry row');
    const model = value as Record<string, unknown>;
    if (!identity(model.id) || ids.has(model.id) || !readable(model.label, 200) || !readable(model.provider_model, 256)
      || !['openai_compatible', 'opencode_ssh', 'codex_cli'].includes(String(model.provider)) || !integer(model.tier, -2147483648, 2147483647)
      || typeof model.enabled !== 'boolean' || !(model.node_id === null || identity(model.node_id))
      || !integer(model.max_concurrency, 1, 1024) || !integer(model.task_max_attempts, 1, 20)
      || !Array.isArray(model.capabilities) || !model.capabilities.length || model.capabilities.length > 64 || !model.capabilities.every(identity)
      || !(model.escalate_to === null || identity(model.escalate_to)) || (model.usage_cost as { status?: unknown } | null)?.status !== 'unknown'
      || Object.hasOwn(model, 'endpoint') || Object.hasOwn(model, 'credential_file_ref')) throw new Error('Invalid or private model registry fields');
    ids.add(model.id);
  }
  return value as ModelsResponse;
}
export async function getModels(token?: string, signal?: AbortSignal, fetcher: typeof fetch = fetch): Promise<ModelsResponse> {
  const response = await fetcher('/v1/models', { headers: token ? { Authorization: `Bearer ${token}` } : undefined, signal, cache: 'no-store' });
  if (!response.ok) throw new ApiError(`Model settings request failed (${response.status})`, response.status);
  return parseModels(await readBoundedJson(response));
}
/** Public reads omit private refs. Null means preserve existing refs by ID server-side. */
export function existingSpec(model: PublicModel): ModelSpec {
  return { id: model.id, label: model.label, provider: model.provider, provider_model: model.provider_model, tier: model.tier,
    enabled: model.enabled, node_id: model.node_id, max_concurrency: model.max_concurrency, capabilities: [...model.capabilities],
    task_max_attempts: model.task_max_attempts, escalate_to: model.escalate_to, endpoint: null, credential_file_ref: null };
}
export function routingOrder(models: PublicModel[]): PublicModel[] {
  return [...models].sort((left, right) => left.tier - right.tier || (left.id < right.id ? -1 : left.id > right.id ? 1 : 0));
}
function numeric(value: string, name: string, min: number, max: number): number {
  const number = /^-?\d+$/.test(value) ? Number(value) : NaN;
  if (!integer(number, min, max)) throw new Error(`${name} must be an integer from ${min} to ${max}.`);
  return number;
}
export function newModel(draft: ModelDraft, existing: PublicModel[]): ModelSpec {
  if (!['openai_compatible', 'opencode_ssh', 'codex_cli'].includes(draft.provider)) throw new Error('Choose a supported provider adapter.');
  const id = draft.id.trim(), label = draft.label.trim(), node_id = draft.node_id.trim() || null;
  const provider_model = draft.provider_model.trim();
  const endpoint = draft.endpoint.trim() || null, credential_file_ref = draft.credential_file_ref.trim() || null;
  const tier = numeric(draft.tier, 'Tier', -2147483648, 2147483647);
  const max_concurrency = numeric(draft.max_concurrency, 'Concurrency', 1, 1024);
  const task_max_attempts = numeric(draft.task_max_attempts, 'Attempts per task', 1, 20);
  const capabilities = [...new Set(draft.capabilities.split(',').map(value => value.trim()).filter(Boolean))];
  const escalate_to = draft.escalate_to || null;
  if (!identity(id) || existing.some(model => model.id === id)) throw new Error('Use a unique model ID: letters, digits, dots, underscores or hyphens.');
  if (!readable(label, 200)) throw new Error('Model label requires readable text within 200 UTF-8 bytes.');
  if (!readable(provider_model, 256)) throw new Error('Provider model requires a native model ID within 256 UTF-8 bytes.');
  if (node_id && !identity(node_id)) throw new Error('Node ID must use letters, digits, dots, underscores or hyphens.');
  if (!capabilities.length || capabilities.length > 64 || !capabilities.every(identity)) throw new Error('Add 1–64 comma-separated capability IDs.');
  if (existing.some(model => model.tier === tier && model.task_max_attempts !== task_max_attempts)) throw new Error('Models in the same tier must share the per-task attempt limit.');
  if (credential_file_ref && (!/^credentials\/[A-Za-z0-9_./-]+$/.test(credential_file_ref) || credential_file_ref.split('/').includes('..')
    || credential_file_ref.length > 256)) throw new Error('Use a relative credentials/ file reference, never paste an API key.');
  if ([id, label, provider_model, node_id, endpoint, credential_file_ref].some(value => value && /(?:sk-|ghp_|github_pat_|xox[bp]-|AIza|AKIA)[A-Za-z0-9_-]{16,}|PRIVATE KEY-----/.test(value))) throw new Error('Secret values are forbidden. Enter only a private credential file reference.');
  if (endpoint && (endpoint.length > 2048 || /[\s@?#]/.test(endpoint))) throw new Error('Endpoint must exclude credentials, query strings, fragments and whitespace.');
  if (draft.provider === 'openai_compatible') {
    let valid = false;
    try { const url = new URL(endpoint ?? ''); valid = ['http:', 'https:'].includes(url.protocol) && !!url.hostname; } catch { /* reject below */ }
    if (!valid) throw new Error('OpenAI-compatible models require an explicit HTTP(S) endpoint.');
  } else if (draft.provider === 'opencode_ssh') {
    if (!node_id || (endpoint && !/^ssh:\/\/[A-Za-z0-9_.-]{1,128}$/.test(endpoint))) throw new Error('OpenCode SSH requires a node ID and optional ssh://profile reference.');
  } else if (endpoint) throw new Error('Codex CLI transport is qualified separately; leave endpoint empty.');
  if (escalate_to && !existing.some(model => model.id === escalate_to && model.tier >= tier)) throw new Error('Fallback must reference an existing model at the same or higher tier.');
  if (existing.length >= 256) throw new Error('Registry supports at most 256 configured models.');
  return { id, label, provider: draft.provider, provider_model, tier, enabled: draft.enabled, node_id, endpoint, credential_file_ref,
    max_concurrency, capabilities, task_max_attempts, escalate_to };
}
export async function putModels(expected_revision: string, models: ModelSpec[], token?: string, signal?: AbortSignal, fetcher: typeof fetch = fetch): Promise<ModelsResponse> {
  const body = JSON.stringify({ expected_revision, models });
  if (!revision(expected_revision) || new TextEncoder().encode(body).byteLength > 1024 * 1024) throw new Error('Invalid or oversized model registry update');
  const response = await fetcher('/v1/models', { method: 'PUT', headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) }, body, signal });
  if (!response.ok) {
    let detail = '';
    try { const value = await readBoundedJson(response) as { detail?: unknown }; if (typeof value.detail === 'string') detail = `: ${value.detail}`; } catch { /* bounded generic error */ }
    throw new ApiError(`Model update failed (${response.status})${detail}`, response.status);
  }
  return parseModels(await readBoundedJson(response));
}
/** A stale revision triggers a read and explicit review, never an automatic write retry. */
export async function saveModelPolicy(current: ModelsResponse, models: ModelSpec[], token?: string, fetcher: typeof fetch = fetch): Promise<{ registry: ModelsResponse; conflict: boolean }> {
  try { return { registry: await putModels(current.revision, models, token, undefined, fetcher), conflict: false }; }
  catch (error) {
    if (error instanceof ApiError && error.status === 409) return { registry: await getModels(token, undefined, fetcher), conflict: true };
    throw error;
  }
}
