import { describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { ApiError } from './api';
import { emptyDraft, existingSpec, getModels, newModel, parseModels, putModels, routingOrder, saveModelPolicy, type ModelsResponse, type PublicModel } from './models';
import Models from './ui/Models';

const model = (patch: Partial<PublicModel> = {}): PublicModel => ({ id: 'fallback', label: 'Configured fallback', provider: 'codex_cli', provider_model: 'provider-native-id',
  tier: 1, enabled: true, node_id: null, max_concurrency: 2, capabilities: ['coding'], task_max_attempts: 1, escalate_to: null, usage_cost: { status: 'unknown' }, ...patch });
const registry = (): ModelsResponse => ({ schema_version: 1, revision: '9007199254740993', models: [model()], native_execution_qualified: false });
const draft = () => ({ ...emptyDraft(), id: 'own-model', label: 'Own model', provider_model: 'native/model-id', endpoint: 'http://127.0.0.1:8000/v1', escalate_to: 'fallback' });

describe('private model policy forms and DTOs', () => {
  it('keeps a stable registry ID separate from the actual provider model and explicit fallback', () => {
    const value = newModel(draft(), registry().models);
    expect(value.id).toBe('own-model'); expect(value.provider_model).toBe('native/model-id');
    expect(value.tier).toBe(0); expect(value.max_concurrency).toBe(16); expect(value.task_max_attempts).toBe(1);
    expect(value.escalate_to).toBe('fallback'); expect(value.credential_file_ref).toBeNull();
  });
  it('supports SSH profiles and credential paths without accepting shell commands or secret values', () => {
    const value = draft(); value.provider = 'opencode_ssh'; value.node_id = 'arbitrary-node'; value.endpoint = 'ssh://my-profile'; value.credential_file_ref = 'credentials/my-provider';
    expect(newModel(value, registry().models).node_id).toBe('arbitrary-node');
    for (const path of ['../secret', '/home/secret', 'credentials/../secret', 'sk-123456789012345678901234']) expect(() => newModel({ ...value, credential_file_ref: path }, registry().models)).toThrow();
    expect(() => newModel({ ...value, endpoint: 'ssh://profile;touch /tmp/x' }, registry().models)).toThrow();
    expect(() => newModel({ ...draft(), endpoint: 'https://user:secret@example.com/v1' }, registry().models)).toThrow();
    expect(() => newModel({ ...draft(), endpoint: 'https://example.com?key=value' }, registry().models)).toThrow();
  });
  it('enforces exact backend concurrency, attempt, UTF-8 and tier constraints', () => {
    for (const patch of [{ max_concurrency: '0' }, { max_concurrency: '1025' }, { task_max_attempts: '21' }, { tier: '1.5' },
      { provider_model: '' }, { provider_model: '⚓'.repeat(86) }, { label: '⚓'.repeat(67) }, { id: 'fallback' }]) expect(() => newModel({ ...draft(), ...patch }, registry().models)).toThrow();
    expect(newModel({ ...draft(), max_concurrency: '1024', task_max_attempts: '20' }, registry().models).max_concurrency).toBe(1024);
    expect(() => newModel({ ...draft(), tier: '1', task_max_attempts: '2' }, registry().models)).toThrow('same tier');
  });
  it('sorts configured tiers using backend identity order without hardcoded provider IDs', () => {
    expect(routingOrder([model({ id: 'a', tier: 1 }), model({ id: 'Z', tier: 1 }), model({ id: 'custom-local', tier: -1 })]).map(value => value.id)).toEqual(['custom-local', 'Z', 'a']);
  });
  it('rejects leaked connection refs, unsupported schema and numeric revision coercion', () => {
    expect(parseModels(registry()).revision).toBe('9007199254740993');
    for (const value of [{ ...registry(), revision: 1 }, { ...registry(), schema_version: 2 },
      { ...registry(), models: [{ ...model(), endpoint: 'http://private' }] }, { ...registry(), models: [{ ...model(), credential_file_ref: null }] },
      { ...registry(), models: [{ ...model(), provider_model: undefined }] }]) expect(() => parseModels(value)).toThrow();
  });
  it('strips read metadata and preserves hidden connection refs by sending explicit nulls', () => {
    const spec = existingSpec({ ...model(), unexpected: 'must not propagate' } as PublicModel);
    expect(spec).not.toHaveProperty('usage_cost'); expect(spec).not.toHaveProperty('unexpected');
    expect(spec.endpoint).toBeNull(); expect(spec.credential_file_ref).toBeNull(); expect(spec.provider_model).toBe('provider-native-id');
  });
  it('renders only reference inputs and states policy never starts native execution', () => {
    const html = renderToStaticMarkup(<Models onAccess={() => {}} />);
    expect(html).toContain('Provider model'); expect(html).toContain('Credential file reference');
    expect(html).toContain('Never paste a secret or API key'); expect(html).toContain('native execution remains disabled');
    expect(html).not.toContain('type="password"');
  });
});

describe('model registry authenticated optimistic writes', () => {
  it('uses bearer headers, canonical revision strings and no credential-bearing URLs', async () => {
    const fetcher = vi.fn<typeof fetch>().mockImplementation(async () => Response.json(registry()));
    await getModels('private-token', undefined, fetcher);
    await putModels(registry().revision, [existingSpec(model())], 'private-token', undefined, fetcher);
    for (const [url, init] of fetcher.mock.calls) { expect(url).toBe('/v1/models'); expect(new Headers(init?.headers).get('Authorization')).toBe('Bearer private-token'); }
    const body = JSON.parse(String(fetcher.mock.calls[1][1]?.body));
    expect(body.expected_revision).toBe('9007199254740993'); expect(body.models[0].provider_model).toBe('provider-native-id');
    expect(body.models[0]).not.toHaveProperty('usage_cost');
  });
  it('refetches a 409 conflict and never automatically repeats the stale write', async () => {
    const next = registry(); next.revision = '9007199254740994';
    const fetcher = vi.fn<typeof fetch>().mockImplementation(async (_url, init) => init?.method === 'PUT'
      ? Response.json({ error: 'revision_conflict' }, { status: 409 }) : Response.json(next));
    const result = await saveModelPolicy(registry(), [existingSpec(model())], 'private-token', fetcher);
    expect(result.conflict).toBe(true); expect(result.registry.revision).toBe('9007199254740994');
    expect(fetcher.mock.calls.filter(([, init]) => init?.method === 'PUT')).toHaveLength(1);
    expect(fetcher.mock.calls.filter(([, init]) => init?.method !== 'PUT')).toHaveLength(1);
  });
  it('retains authentication status for the shared session-access action', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(new Response('', { status: 401 }));
    await expect(getModels(undefined, undefined, fetcher)).rejects.toBeInstanceOf(ApiError);
    await expect(getModels(undefined, undefined, fetcher)).rejects.toMatchObject({ status: 401 });
  });
});
