import { useState, type ReactNode } from 'react';
import { data, list, number, text, type Data, type LegacySnapshot } from '../legacy';
import { ModelBadge, modelFamily } from './ModelBadge';
import './token-efficiency.css';

const fmt = (value: number | null, digits = 1): string => value === null ? 'Unknown' : value.toLocaleString('en-US', { maximumFractionDigits: digits });
const metric = (value: unknown, digits = 1): string => fmt(number(value), digits);
const percent = (value: unknown): string => number(value) === null || Number(value) > 100 ? 'Unknown' : `${metric(value)}%`;
const signed = (value: unknown): number | null => typeof value === 'number' && Number.isFinite(value) ? value : null;
const balance = (value: unknown): number | null => typeof value === 'string' && /^\d{1,18}(?:\.\d{1,8})?$/.test(value) ? number(Number(value)) : number(value);
const compact = (value: unknown): string => {
  const n = number(value);
  return n === null ? 'Unknown' : n >= 1e9 ? `${fmt(n / 1e9, 2)}B` : n >= 1e6 ? `${fmt(n / 1e6, 2)}M` : n >= 1e3 ? `${fmt(n / 1e3, 1)}k` : fmt(n, 0);
};
const entries = (value: unknown): Array<[string, unknown]> => Object.entries(data(value));
const summary = (value: unknown): string => entries(value).map(([key, count]) => `${key} ${metric(count, 0)}`).join(' · ') || 'Unknown';
const modelName = (agent: Data): string => `${text(agent.model)}${agent.family === 'codex' && text(agent.effort) ? ` ${text(agent.effort)}` : ''}`;
function quotaLabel(quota: Data): string {
  const minutes = number(quota.window_minutes);
  return minutes === 10_080 ? 'Weekly quota used' : minutes === null ? 'Quota used · source window' : `Quota used · ${minutes % 60 === 0 ? `${fmt(minutes / 60, 0)}h` : `${fmt(minutes, 0)}m`}`;
}
function Kpi({ label, value, note, title, accent = 'neutral' }: { label: string; value: ReactNode; note: ReactNode; title?: string; accent?: string }) {
  return <div className={`te-kpi te-${accent}`} title={title}><span className="te-kpi-label">{label}</span><strong>{value}</strong><small>{note}</small></div>;
}
function Counter({ label, value, hint }: { label: string; value: unknown; hint?: string }) {
  return <span title={hint}><b>{metric(value, 0)}</b> {label}</span>;
}
function RowsNote({ count, shown, onMore }: { count: number; shown: number; onMore: () => void }) {
  return count > shown ? <button type="button" className="te-more" onClick={onMore}>Show {Math.min(32, count - shown)} more ({count - shown} remaining)</button> : null;
}

/** Source-faithful view: real token counts, estimated credits and unknowns stay distinct. */
export function TokenEfficiency({ snapshot }: { snapshot: LegacySnapshot }) {
  const efficiency = data(snapshot.token_efficiency), quota = data(efficiency.quota);
  const windows = data(efficiency.windows), hour = data(windows['60']), quarter = data(windows['15']);
  const ledger = data(efficiency.ledger), stats = data(efficiency.ledger_stats), funnel = data(stats.funnel);
  const yieldSource = data(snapshot.token_yield), yieldTotal = data(yieldSource.total);
  const agents = list(snapshot.agents).map(data);
  const [modelLimit, setModelLimit] = useState(16), [usageLimit, setUsageLimit] = useState(16), [flagLimit, setFlagLimit] = useState(12), [projectLimit, setProjectLimit] = useState(12);
  // The producer's `agents` list contains observed agent-process records.
  // A fallback never counts remote screen names or infers running processes.
  const hasModelAggregate = efficiency.by_model !== undefined && efficiency.by_model !== null;
  const modelCounts: Data = hasModelAggregate ? data(efficiency.by_model) : Object.create(null) as Data;
  if (!hasModelAggregate) for (const agent of agents) {
    if (text(agent.model)) { const key = modelName(agent); modelCounts[key] = (number(modelCounts[key]) ?? 0) + 1; }
  }
  const models = entries(modelCounts).sort((a, b) => (number(b[1]) ?? -1) - (number(a[1]) ?? -1) || a[0].localeCompare(b[0]));
  const largestModel = Math.max(1, ...models.map(([, count]) => number(count) ?? 0));
  const usage = entries(hour.by_model).map(([model, value]) => [model, data(value)] as const)
    .sort((a, b) => (number(b[1].credits) ?? -1) - (number(a[1].credits) ?? -1) || a[0].localeCompare(b[0]));
  const incomplete = usage.some(([, row]) => row.unpriced === true);
  const quarterIncomplete = entries(quarter.by_model).some(([, row]) => data(row).unpriced === true);
  const flags = list(efficiency.flags).map(value => typeof value === 'string' ? { text: value } as Data : data(value));
  const yieldProjects = entries(yieldSource.projects).map(([id, value]) => [id, data(value)] as const);
  const sourceNotes = [efficiency.windows_note, efficiency.quota_note, efficiency.claude_note].map(text).filter(Boolean);
  const cheapest = number(efficiency.pct_codex_on_cheapest);
  const cost = number(efficiency.cost_index_vs_all_luna), used = number(quota.used_percent);
  const quotaBurn = signed(quota.burn_pct_per_hour);
  const hasEfficiency = Object.keys(efficiency).length > 0;

  return <section className="token-efficiency" aria-label="Token efficiency and model mix">
    <header className="te-heading"><h2>Token efficiency <span>· real time</span></h2><span>model mix · burn · quota</span></header>
    <p className="te-source">Producer snapshot <time dateTime={snapshot.generated_utc}>{snapshot.generated_utc}</time>{text(snapshot.host) ? ` · ${text(snapshot.host)}` : ''}</p>
    {!hasEfficiency && <p className="te-empty">Token-efficiency metrics are unavailable in this source snapshot.</p>}
    <div className="te-kpis">
      <Kpi label="Codex on source cheapest" accent={cheapest === null ? 'neutral' : cheapest >= 50 ? 'luna' : cheapest >= 20 ? 'sol' : 'astra'}
        value={<>{metric(efficiency.codex_on_cheapest, 0)}/{metric(efficiency.codex_agents, 0)} <span>· {percent(efficiency.pct_codex_on_cheapest)}</span></>}
        title="Legacy producer classifies gpt-6-luna as cheapest. This is not the configured native model registry's lowest tier."
        note={<>Detected models · <span>gpt-6-luna baseline</span></>} />
      <Kpi label="Cost index vs all-Luna" accent={cost === null ? 'neutral' : cost > 10 ? 'astra' : cost > 4 ? 'sol' : 'luna'}
        value={cost === null ? 'Unknown' : `${fmt(cost)}×`} title={text(efficiency.cost_index_note) || 'Source agent-weighted output-rate index; not token-weighted spend.'}
        note={<>{metric(efficiency.cost_index_avg_output_rate)} cr/1M output <span>· agent weighted</span></>} />
      <Kpi label="Codex burn · last 60 min" value={<>{metric(hour.credits_per_hour_est)} <span>cr/h</span></>}
        note={<><span className="te-estimate">Estimate{incomplete ? ' · priced subtotal' : ''}</span> · 15m pace {metric(quarter.credits_per_hour_est)} cr/h{quarterIncomplete ? ' (subtotal)' : ''}<br />{compact(hour.output_tokens)} out / {compact(hour.input_tokens)} in <span>· measured tokens</span></>} />
      <Kpi label={quotaLabel(quota)} accent={used === null || used > 100 ? 'neutral' : used >= 85 ? 'astra' : used >= 60 ? 'sol' : 'luna'} value={percent(quota.used_percent)}
        note={<>Measured Codex limit{number(quota.window_minutes) === null ? ' · window unknown' : ''}<br />{quotaBurn === null ? 'Burn rate unknown' : `${fmt(quotaBurn, 2)}%/h over ${metric(quota.burn_window_min, 0)}m`}{number(quota.hours_to_100pct) !== null ? ` · ~${metric(quota.hours_to_100pct)}h to 100%` : ''}</>} />
    </div>
    <p className="te-source te-quota-source">Quota observed {text(quota.observed_utc) || 'Unknown'} · resets {text(quota.resets_at_utc) || 'Unknown'} · credit balance {fmt(balance(quota.credits_balance), 0)}</p>

    <div className="te-section-heading"><h3>Observed agents by model</h3><span>{metric(efficiency.agents_total, 0)} agent processes{!hasModelAggregate && models.length ? ' · mix from agent records' : ''}</span></div>
    <div className="te-models">{models.slice(0, modelLimit).map(([model, rawCount]) => {
      const count = number(rawCount), matching = agents.filter(agent => modelName(agent) === model);
      const detail = matching.slice(0, 8).map(agent => `${text(agent.screen) || 'Unnamed screen'} · ${text(agent.host) || 'node unknown'} · ${text(agent.model_source) || 'model source unknown'}`).join('\n');
      return <div className={`te-model te-${modelFamily(model)}`} key={model} title={detail || 'Source model aggregate; individual process details unavailable'}>
        <span className="te-model-label"><ModelBadge model={model} /></span>
        <span className="te-track" role="img" aria-label={`${model}: ${fmt(count, 0)} observed agent processes`}>{count !== null && <span className="te-fill" style={{ width: `${count / largestModel * 100}%` }} />}</span>
        <b>{fmt(count, 0)}</b>
      </div>;
    })}</div>
    {!models.length && <p className="te-empty">Model mix unavailable; no model counts are inferred from screen names.</p>}
    <RowsNote count={models.length} shown={modelLimit} onMore={() => setModelLimit(value => value + 32)} />
    <p className="te-source">Codex tiers: {summary(efficiency.codex_tiers)} · families: {summary(efficiency.by_family)}</p>

    <div className="te-section-heading"><h3>Codex usage by model · last 60 min</h3><span>measured tokens · estimated credits</span></div>
    <div className="te-table-scroll"><table className="te-table"><thead><tr><th>Model</th><th>Sessions</th><th>Input</th><th>Cached input</th><th>Output</th><th>Credits <small>(est.)</small></th></tr></thead><tbody>
      {usage.slice(0, usageLimit).map(([model, row]) => {
        const input = number(row.input), cached = number(row.cached);
        const share = input !== null && input > 0 && cached !== null && cached <= input ? `${fmt(cached / input * 100, 0)}%` : 'share unknown';
        return <tr key={model}><td><ModelBadge model={model} /></td><td>{metric(row.sessions, 0)}</td><td title={`${metric(row.input, 0)} input tokens`}>{compact(row.input)}</td>
          <td title={`${metric(row.cached, 0)} cached input tokens`}>{compact(row.cached)} <small>({share})</small></td><td title={`${metric(row.output, 0)} output tokens`}>{compact(row.output)}</td>
          <td className={row.unpriced === true ? 'te-estimate' : ''}>{row.unpriced === true ? 'Unpriced' : metric(row.credits, 0)}</td></tr>;
      })}
      {!usage.length && <tr><td colSpan={6} className="te-empty">No model usage rows reported for this window.</td></tr>}
    </tbody></table></div>
    <RowsNote count={usage.length} shown={usageLimit} onMore={() => setUsageLimit(value => value + 32)} />
    {incomplete && <p className="te-estimate te-source">Unpriced models are excluded from the estimated credit subtotal. Their cost is unknown.</p>}

    <div className="te-section-heading"><h3>Routing flags</h3><span>producer policy checks</span></div>
    {flags.slice(0, flagLimit).map((flag, index) => <div className="te-flag" key={index}><b>{text(flag.screen) || 'Screen unknown'}</b>{text(flag.project) ? ` (${text(flag.project)})` : ''}: {text(flag.text) || 'Flag detail unavailable'}</div>)}
    {!flags.length && <p className="te-empty">{Array.isArray(efficiency.flags) ? 'None detected by the producer.' : 'Routing flags unavailable.'}</p>}
    <RowsNote count={flags.length} shown={flagLimit} onMore={() => setFlagLimit(value => value + 32)} />

    <div className="te-section-heading"><h3>Escalation ledger</h3><span>recorded attempts · outcomes</span></div>
    <div className="te-ledger"><Counter label="MODEL attempts" value={ledger.model_lines} /><Counter label="escalated" value={ledger.escalated} /><Counter label="classed LANE lines" value={ledger.lane_lines_with_class} /></div>
    <p className="te-source">Outcomes: {summary(ledger.outcomes)} · class @ tier: {summary(ledger.class_at_tier)}</p>
    {text(ledger.note) && <p className="te-source">{text(ledger.note)}</p>}
    {Object.keys(stats).length > 0 && <div className="te-ledger te-funnel"><Counter label="Luna tasks" value={funnel.luna_tasks} /><span>→</span><Counter label="Luna → Sol" value={funnel.luna_to_sol} /><span>→</span><Counter label="Sol → Astra" value={funnel.sol_to_astra} /><span className="te-ledger-note">Luna first pass {percent(stats.luna_first_pass_rate)} · failures {metric(stats.luna_failures_total, 0)} · {metric(stats.attempts, 0)} timestamped attempts</span></div>}

    {Object.keys(yieldSource).length > 0 && <section className="te-yield" aria-label="Estimated token yield">
      <div className="te-section-heading"><h3>Token yield <span>· estimate</span></h3><span>{number(yieldSource.window_hours) !== null ? `${metric(yieldSource.window_hours, 0)}h source window` : 'window unknown'}</span></div>
      <div className="te-yield-totals"><span><b>{percent(yieldTotal.yield_integration_credits_pct)}</b> credits → integration</span><span><b>{percent(yieldTotal.yield_main_credits_pct)}</b> credits → main</span><span><b>{percent(yieldTotal.coverage_pct)}</b> coverage</span><span><b>{compact(yieldTotal.tokens)}</b> generated tokens</span></div>
      {yieldProjects.length > 0 && <div className="te-table-scroll"><table className="te-table"><thead><tr><th>Project</th><th>Tokens</th><th>Credits <small>(est.)</small></th><th>Integration</th><th>Main</th><th>Coverage</th></tr></thead><tbody>{yieldProjects.slice(0, projectLimit).map(([id, row]) => <tr key={id}><td>{id}</td><td>{compact(row.tokens)}</td><td>{metric(row.credits, 0)}</td><td>{percent(row.yield_integration_credits_pct)}</td><td>{percent(row.yield_main_credits_pct)}</td><td>{percent(row.coverage_pct)}</td></tr>)}</tbody></table></div>}
      <RowsNote count={yieldProjects.length} shown={projectLimit} onMore={() => setProjectLimit(value => value + 32)} />
      <p className="te-source">{text(yieldSource.method) || 'Yield method unavailable.'} Updated {text(yieldSource.generated_utc) || 'Unknown'}{list(yieldSource.not_covered).length ? ` · Not covered: ${list(yieldSource.not_covered).map(text).join(', ')}` : ''}</p>
    </section>}
    {sourceNotes.map((note, index) => <p className="te-source te-method" key={index}>{note}</p>)}
    {entries(efficiency.rates_per_1m).length > 0 && <details className="te-rates"><summary>Producer rate assumptions · credits / 1M tokens</summary><p className="te-source">{text(efficiency.rates_source) || 'Rate source unknown'}</p><div className="te-table-scroll"><table className="te-table"><thead><tr><th>Model</th><th>Input</th><th>Cached</th><th>Output</th></tr></thead><tbody>{entries(efficiency.rates_per_1m).map(([model, value]) => { const row = data(value); return <tr key={model}><td><ModelBadge model={model} /></td><td>{metric(row.input, 2)}</td><td>{metric(row.cached, 2)}</td><td>{metric(row.output, 2)}</td></tr>; })}</tbody></table></div></details>}
  </section>;
}
