import { number, type Data } from './legacy';

/** Same-sample estimates; neither these ratios nor their counts imply full coverage. */
export function observedTokenSample(total: Data): Data | null {
  const fields = { tok: 'tokens', int: 'integration_tokens', main: 'main_tokens', attributed: 'attributed_tokens' };
  if (!Object.values(fields).some(key => Object.hasOwn(total, `lower_bound_${key}`))) return null;
  const sample: Data = Object.fromEntries(Object.entries(fields).map(([key, source]) => [key, number(total[`lower_bound_${source}`])]));
  const denominator = number(sample.tok);
  for (const [key, source] of Object.entries({ yi: 'int', ym: 'main', cov: 'attributed' })) {
    const numerator = number(sample[source]);
    sample[key] = denominator !== null && denominator > 0 && numerator !== null && numerator <= denominator
      ? 100 * numerator / denominator : null;
  }
  return sample;
}
