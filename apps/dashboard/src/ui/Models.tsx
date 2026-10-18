import { useEffect, useState } from 'react';
import { ApiError } from '../api';
import { emptyDraft, existingSpec, getModels, newModel, saveModelPolicy, routingOrder, type ModelDraft, type ModelSpec, type ModelsResponse, type Provider } from '../models';

export default function Models({ token, onAccess }: { token?: string; onAccess: () => void }) {
  const [registry, setRegistry] = useState<ModelsResponse | null>(null), [draft, setDraft] = useState<ModelDraft>(emptyDraft);
  const [error, setError] = useState(''), [notice, setNotice] = useState(''), [auth, setAuth] = useState(false);
  const [loading, setLoading] = useState(true), [saving, setSaving] = useState(false), [reload, setReload] = useState(0);
  useEffect(() => {
    const controller = new AbortController(); setRegistry(null); setDraft(emptyDraft()); setNotice(''); setLoading(true); setError(''); setAuth(false);
    void getModels(token, controller.signal).then(value => { if (!controller.signal.aborted) setRegistry(value); }).catch(reason => {
      if (controller.signal.aborted) return;
      setAuth(reason instanceof ApiError && [401, 403].includes(reason.status));
      setError(reason instanceof ApiError && reason.status === 404 ? 'Model registry is unavailable on this server.' : reason instanceof Error ? reason.message : 'Model settings unavailable');
    }).finally(() => { if (!controller.signal.aborted) setLoading(false); });
    return () => controller.abort();
  }, [token, reload]);
  function field<K extends keyof ModelDraft>(key: K, value: ModelDraft[K]) { setDraft(current => ({ ...current, [key]: value })); }
  async function save(models: ModelSpec[], added = false) {
    if (!registry || saving) return;
    setSaving(true); setError(''); setNotice('');
    try {
      const result = await saveModelPolicy(registry, models, token);
      setRegistry(result.registry);
      if (result.conflict) setNotice('Registry changed elsewhere. Latest policy loaded; review your draft and submit again.');
      else { if (added) setDraft(emptyDraft()); setNotice('Model policy saved. This prototype registry does not enable native fleet execution or launch providers.'); }
    } catch (reason) {
      setAuth(reason instanceof ApiError && [401, 403].includes(reason.status));
      setError(reason instanceof Error ? reason.message : 'Save outcome unknown; refresh settings before retrying.');
    } finally { setSaving(false); }
  }
  const models = registry ? routingOrder(registry.models) : [];
  return <div className="models-settings">
    <div className="models-notice"><b>Model policy prototype · native execution remains disabled</b><p>Configure your own model, local capacity and explicit fallback tiers. Lower tier numbers are preferred; cost stays unknown until measured. Saving policy does not start inference, SSH sessions or workers.</p></div>
    {error && <div className="error-banner" role="alert">{error}{auth && <button className="text-button" onClick={onAccess}>Open session access</button>}</div>}
    {notice && <div className="models-status" role="status">{notice}</div>}
    {loading && <div className="loading-card">Loading private model policy…</div>}
    <section className="panel models-panel"><div className="panel-head"><div><div className="eyebrow">MODEL REGISTRY</div><h2>Configured models</h2></div><button className="text-button" disabled={saving} onClick={() => setReload(value => value + 1)}>Refresh settings</button></div>
      <p>Policy revision <code>{registry?.revision ?? 'Unknown'}</code> · Private endpoints and credential references are omitted from reads.</p>
      <div className="models-chain" aria-label="Configured preference order">{models.map(model => <span key={model.id}><b>T{model.tier}</b> {model.label}{!model.enabled ? ' (disabled)' : ''}{model.escalate_to ? ` → ${registry?.models.find(target => target.id === model.escalate_to)?.label ?? model.escalate_to}` : ''}</span>)}{registry && !models.length && <span>No configured models yet.</span>}</div>
      <div className="table-scroll"><table><thead><tr><th>Model</th><th>Provider / node</th><th>Tier</th><th>Capacity</th><th>Attempts / task</th><th>Fallback</th><th>Enabled</th></tr></thead><tbody>{models.map(model => <tr key={model.id}><td><strong>{model.label}</strong><small>{model.id} · {model.capabilities.join(', ')}</small></td><td>{model.provider.replaceAll('_', ' ')}<small>{model.provider_model}</small><small>{model.node_id ?? 'No node preference'}</small></td><td>{model.tier}</td><td>{model.max_concurrency}<small>Usage cost unknown</small></td><td>{model.task_max_attempts}</td><td>{model.escalate_to ?? 'None'}</td><td><button type="button" className="secondary-button" disabled={saving} onClick={() => void save(registry!.models.map(item => ({ ...existingSpec(item), enabled: item.id === model.id ? !item.enabled : item.enabled })))}>{model.enabled ? 'Disable' : 'Enable'}</button></td></tr>)}</tbody></table></div>
    </section>
    <section className="panel models-panel"><div className="eyebrow">ADD YOUR OWN MODEL</div><h2>New model policy</h2><form onSubmit={event => {
      event.preventDefault(); if (!registry) return;
      try { const model = newModel(draft, registry.models); void save([...registry.models.map(existingSpec), model], true); }
      catch (reason) { setError(reason instanceof Error ? reason.message : 'Review the model fields.'); }
    }}><fieldset disabled={!registry || saving} className="models-form"><label>Model ID<input required maxLength={128} value={draft.id} onChange={event => field('id', event.target.value)} placeholder="my-model" /></label><label>Display label<input required maxLength={200} value={draft.label} onChange={event => field('label', event.target.value)} placeholder="My own coding model" /></label>
      <label>Provider<select value={draft.provider} onChange={event => { field('provider', event.target.value as Provider); field('endpoint', ''); }}><option value="openai_compatible">OpenAI-compatible endpoint</option><option value="opencode_ssh">OpenCode SSH profile</option><option value="codex_cli">Codex CLI</option></select></label>
      <label>Provider model<input required value={draft.provider_model} onChange={event => field('provider_model', event.target.value)} placeholder="Provider's native model ID" /><small>Native API or OpenCode model name, separate from your stable registry ID.</small></label>
      <label>Preference tier<input required type="number" step="1" min="-2147483648" max="2147483647" value={draft.tier} onChange={event => field('tier', event.target.value)} /><small>Lower numbers preferred; tier values are policy, not prices.</small></label>
      <label>Node ID (optional)<input maxLength={128} value={draft.node_id} onChange={event => field('node_id', event.target.value)} placeholder="inference-node" /></label>
      <label>{draft.provider === 'opencode_ssh' ? 'SSH profile reference (optional)' : 'Endpoint'}<input type="text" required={draft.provider === 'openai_compatible'} disabled={draft.provider === 'codex_cli'} maxLength={2048} value={draft.endpoint} onChange={event => field('endpoint', event.target.value)} placeholder={draft.provider === 'opencode_ssh' ? 'ssh://my-profile' : draft.provider === 'codex_cli' ? 'Managed by CLI qualification' : 'http://127.0.0.1:8000/v1'} /><small>Public endpoint address only. No embedded credentials or query keys.</small></label>
      <label>Credential file reference (optional)<input type="text" maxLength={256} value={draft.credential_file_ref} onChange={event => field('credential_file_ref', event.target.value)} placeholder="credentials/my-provider" autoComplete="off" /><small>Relative private path on coordinator. Never paste a secret or API key.</small></label>
      <label>Maximum concurrency<input required type="number" min="1" max="1024" step="1" value={draft.max_concurrency} onChange={event => field('max_concurrency', event.target.value)} /><small>1–1024 policy slots. Start with measured capacity.</small></label>
      <label>Attempts per task in this tier<input required type="number" min="1" max="20" step="1" value={draft.task_max_attempts} onChange={event => field('task_max_attempts', event.target.value)} /><small>1–20; same-tier models share this limit.</small></label>
      <label>Capabilities<input required value={draft.capabilities} onChange={event => field('capabilities', event.target.value)} placeholder="coding, review" /><small>Comma-separated capability IDs.</small></label>
      <label>Explicit fallback<select value={draft.escalate_to} onChange={event => field('escalate_to', event.target.value)}><option value="">No fallback</option>{models.filter(model => model.tier >= Number(draft.tier)).map(model => <option key={model.id} value={model.id}>{model.label} · tier {model.tier}</option>)}</select><small>Used only when escalation or capacity fallback is authorized.</small></label>
      <label className="models-enabled"><input type="checkbox" checked={draft.enabled} onChange={event => field('enabled', event.target.checked)} /> Enabled in policy</label>
      <div className="models-form-actions"><button className="primary-button" type="submit">{saving ? 'Saving…' : 'Add model'}</button><span>Connection and provider qualification remain separate.</span></div>
    </fieldset></form></section>
  </div>;
}
