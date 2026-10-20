import { describe, expect, it } from 'vitest';
import { formatCompactMeasurement, formatDecimalBalance, formatJsonMeasurements, formatMeasurement } from './formatNumbers';

describe('live measurement formatting', () => {
  it('rounds displayed JSON literals while retaining exact source data and string identities', () => {
    const value = { load: 26.203125, nested: [96.42857142857143, -0.0001, 1.23456e25],
      id: 'model-v1.23456', decimal: '123456789012345678.999', escaped: '"12.3456"' };
    const exact = JSON.stringify(value);
    const rendered = formatJsonMeasurements(value);
    expect(JSON.parse(rendered)).toMatchObject({ load: 26.2, nested: [96.43, 0, 1.23456e25],
      id: value.id, decimal: value.decimal, escaped: value.escaped });
    expect(rendered).toContain('12345600000000000000000000');
    expect(JSON.stringify(value)).toBe(exact);
    expect(formatMeasurement(1.23456e25)).not.toMatch(/\.\d{3}/);
    expect(formatMeasurement(1.23456789, 10)).toBe('1.23');
  });
  it('rounds finite numbers to at most two decimals and removes negative zero', () => {
    expect(formatMeasurement(96.42857142857143)).toBe('96.43');
    expect(formatMeasurement(26.203125)).toBe('26.2');
    expect(formatMeasurement(13.593749999999998)).toBe('13.59');
    expect(formatMeasurement(87.64277401681029)).toBe('87.64');
    expect(formatMeasurement(-0)).toBe('0');
    expect(formatMeasurement(-0.001)).toBe('0');
    expect(formatMeasurement(0)).toBe('0');
  });

  it('keeps absent and non-finite values unknown', () => {
    expect(formatMeasurement(null)).toBe('Unknown');
    expect(formatMeasurement(undefined)).toBe('Unknown');
    expect(formatMeasurement(Number.NaN)).toBe('Unknown');
    expect(formatMeasurement(Number.POSITIVE_INFINITY)).toBe('Unknown');
    expect(formatMeasurement(Number.NEGATIVE_INFINITY)).toBe('Unknown');
  });

  it('rounds decimal-string balances exactly without losing large integer precision', () => {
    expect(formatDecimalBalance('123456789012345678.999')).toBe('123,456,789,012,345,679');
    expect(formatDecimalBalance('987654321012345678.126')).toBe('987,654,321,012,345,678.13');
    expect(formatDecimalBalance('1.005')).toBe('1.01');
    expect(formatDecimalBalance('12.3400')).toBe('12.34');
    expect(formatDecimalBalance('not a balance')).toBe('Unknown');
  });

  it('limits compact suffixes to two decimals and preserves exact zero', () => {
    expect(formatCompactMeasurement(1234567)).toBe('1.23M');
    expect(formatCompactMeasurement(9999)).toBe('10k');
    expect(formatCompactMeasurement(0)).toBe('0');
    expect(formatCompactMeasurement(Number.NaN)).toBe('Unknown');
  });
});
