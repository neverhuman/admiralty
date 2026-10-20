import { data, number } from './legacy';
import { formatMeasurement } from './formatNumbers';

const scoreNumber = (value: unknown): number | null => {
  const parsed = typeof value === 'string' && value.trim() !== '' ? Number(value) : number(value);
  return parsed !== null && Number.isFinite(parsed) && parsed >= 0 && parsed <= 100 ? parsed : null;
};

export const hygieneScore = (value: unknown): number | null => scoreNumber(data(value).score ?? value);

/** A reported interval is a partial audit, never a replacement final score. */
export function hygieneRange(value: unknown): [number, number] | null {
  const range = data(value).possible_score_range;
  if (!Array.isArray(range) || range.length !== 2) return null;
  const low = number(range[0]), high = number(range[1]);
  return low !== null && high !== null && low <= high && high <= 100 ? [low, high] : null;
}

export function hygienePartialLabel(value: unknown): string | null {
  const range = hygieneRange(value);
  if (!range) return null;
  const subtotal = number(data(value).known_component_subtotal);
  return `${formatMeasurement(range[0])}–${formatMeasurement(range[1])} range · partial audit${subtotal !== null && subtotal <= 100 ? ` · known subtotal ${formatMeasurement(subtotal)}` : ''}`;
}
