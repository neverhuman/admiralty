import './legacy-hierarchy.css';

export type ModelBadgeProps = { model: string; effort?: string; tier?: string };

/** Color describes a reported model family; it grants no routing or execution authority. */
export function modelFamily(model: string): 'local' | 'luna' | 'sol' | 'astra' | 'gemini' | 'claude' | 'unknown' {
  const name = model.toLowerCase();
  if (/\bastra\b/.test(name)) return 'astra';
  if (/\bluna\b/.test(name)) return 'luna';
  if (/\b(sol|terra)\b/.test(name)) return 'sol';
  if (/\bgemini\b|^agy(?:\s|\/|$)/.test(name)) return 'gemini';
  if (/\bclaude\b/.test(name)) return 'claude';
  if (/\bornith\b|\blocal\b|\bollama\b|\blm[ -]?studio\b|\bllama\.cpp\b|\bvllm\b/.test(name)) return 'local';
  return 'unknown';
}

export function ModelBadge({ model, effort, tier }: ModelBadgeProps) {
  const reported = model.trim() || 'unknown';
  const parts = [reported, effort?.trim(), tier?.trim()].filter(value => value && value !== '—');
  return <span className={`model-badge model-badge--${modelFamily(reported)}`} title={parts.join(' · ')}>{reported}{effort?.trim() ? ` ${effort.trim()}` : ''}{tier?.trim() && tier.trim() !== '—' ? ` · ${tier.trim()}` : ''}</span>;
}

export default ModelBadge;
