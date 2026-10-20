const MAX_FRACTION_DIGITS = 2;
const formatters = [0, 1, 2].map(maximumFractionDigits => new Intl.NumberFormat('en-US', { maximumFractionDigits }));

function digitsLimit(digits: number): number {
  return Number.isInteger(digits) ? Math.min(MAX_FRACTION_DIGITS, Math.max(0, digits)) : MAX_FRACTION_DIGITS;
}

/** Format a finite measurement for display without changing its source value. */
export function formatMeasurement(value: unknown, digits = MAX_FRACTION_DIGITS): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return 'Unknown';
  const formatted = formatters[digitsLimit(digits)]
    .format(Object.is(value, -0) ? 0 : value);
  return /^-0(?:\.0+)?$/.test(formatted) ? formatted.slice(1) : formatted;
}

/** Round numeric literals only in the displayed source inspector, without mutating data or string identifiers. */
export function formatJsonMeasurements(value: unknown): string {
  return JSON.stringify(value, null, 2)?.replace(/("(?:\\.|[^"\\])*")|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)/g,
    (token: string, quoted: string | undefined) => quoted ? token : formatMeasurement(Number(token)).replaceAll(',', '')) ?? 'Unknown';
}

/** Format decimal-string balances without converting large integer parts to Number. */
export function formatDecimalBalance(value: unknown): string {
  if (typeof value !== 'string' || !/^\d{1,18}(?:\.\d{1,32})?$/.test(value)) return formatMeasurement(value, 0);
  const [integer, fraction = ''] = value.split('.');
  const retained = fraction.slice(0, MAX_FRACTION_DIGITS).padEnd(MAX_FRACTION_DIGITS, '0');
  let scaled = BigInt(integer) * 100n + BigInt(retained || '0');
  if ((fraction[MAX_FRACTION_DIGITS] ?? '0') >= '5') scaled += 1n;
  const roundedInteger = (scaled / 100n).toString();
  const roundedFraction = (scaled % 100n).toString().padStart(2, '0').replace(/0+$/, '');
  const grouped = roundedInteger.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
  return roundedFraction ? `${grouped}.${roundedFraction}` : grouped;
}

/** Compact a finite count while keeping no more than two fraction digits. */
export function formatCompactMeasurement(value: unknown): string {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) return 'Unknown';
  const suffix = value >= 1e9 ? 'B' : value >= 1e6 ? 'M' : value >= 1e3 ? 'k' : '';
  const divisor = suffix === 'B' ? 1e9 : suffix === 'M' ? 1e6 : suffix === 'k' ? 1e3 : 1;
  return `${formatMeasurement(value / divisor, suffix ? 2 : 0)}${suffix}`;
}
