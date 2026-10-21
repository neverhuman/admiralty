// Python-compatible text helpers that keep the TypeScript demo tools' CLI and
// receipts byte-compatible with the retired Python 3 scripts.

/** Python `json.dumps(value, indent=2)` (ensure_ascii) for JSON-safe values. */
export function pyJsonDumps(value: unknown): string {
  return JSON.stringify(value, (_key, item: unknown) => item instanceof PyFloat ? `\u0000pyfloat:${item.text}` : item, 2)
    .replace(/"\\u0000pyfloat:([^"]*)"/g, '$1')
    .replace(/[\u0080-\uffff]/g, (character) => `\\u${character.charCodeAt(0).toString(16).padStart(4, '0')}`);
}

/** A float that serializes with Python's `repr` (for example `15.0`). */
export class PyFloat {
  readonly value: number;
  readonly text: string;
  constructor(value: number) {
    this.value = value;
    this.text = pyFloatRepr(value);
  }
}

/** Python `json.dumps` of a float: `repr` digits, `.0` for integral values. */
export function pyFloatRepr(value: number): string {
  if (Number.isNaN(value)) return 'NaN';
  if (!Number.isFinite(value)) return value > 0 ? 'Infinity' : '-Infinity';
  if (value === 0) return Object.is(value, -0) ? '-0.0' : '0.0';
  const [mantissa = '', exponentText] = value.toExponential().split('e');
  const exponent = Number(exponentText);
  if (exponent < -4 || exponent >= 16) {
    return `${mantissa}e${exponent < 0 ? '-' : '+'}${String(Math.abs(exponent)).padStart(2, '0')}`;
  }
  // Within this range JavaScript also prints the shortest round-trip digits in fixed notation.
  const text = String(value);
  return Number.isInteger(value) ? `${text}.0` : text;
}

/** Python `format(value, f'.{digits}f')`: exact binary value, round half to even. */
export function pyFixed(value: number, digits: number): string {
  if (!Number.isFinite(value)) return value > 0 ? 'inf' : value < 0 ? '-inf' : 'nan';
  const exact = Math.abs(value).toFixed(100);
  const [whole = '0', fraction = ''] = exact.split('.');
  const kept = `${whole}${fraction.slice(0, digits)}`;
  const rest = fraction.slice(digits);
  let rounded = BigInt(kept);
  const first = rest.charCodeAt(0) - 48;
  const tail = rest.slice(1).replace(/0+$/, '');
  if (first > 5 || (first === 5 && (tail.length > 0 || rounded % 2n === 1n))) rounded += 1n;
  let text = rounded.toString().padStart(digits + 1, '0');
  if (digits > 0) text = `${text.slice(0, -digits)}.${text.slice(-digits)}`;
  return `${value < 0 || Object.is(value, -0) ? '-' : ''}${text}`;
}

const pythonWhitespace = '(?:(?!\\ufeff)[\\s\\u001c-\\u001f\\u0085])';
const stripPattern = new RegExp(`^${pythonWhitespace}+|${pythonWhitespace}+$`, 'gu');

/** Python `str.strip()` with no arguments. */
export function pyStrip(text: string): string {
  return text.replace(stripPattern, '');
}

/** Python `str.splitlines()`. */
export function pySplitlines(text: string): string[] {
  const lines = text.split(/\r\n|[\n\r\u000b\u000c\u001c\u001d\u001e\u0085\u2028\u2029]/);
  if (lines.length && lines[lines.length - 1] === '') lines.pop();
  return lines;
}

/** Python `float(text)`; throws for text Python rejects. */
export function pyFloat(text: string): number {
  const trimmed = pyStrip(text);
  const digits = '\\d(?:_?\\d)*';
  const decimal = new RegExp(`^[+-]?(?:${digits}(?:\\.(?:${digits})?)?|\\.${digits})(?:[eE][+-]?${digits})?$`);
  if (decimal.test(trimmed)) return Number(trimmed.replace(/_/g, ''));
  const special = /^([+-]?)(inf|infinity|nan)$/i.exec(trimmed);
  if (special) {
    if ((special[2] ?? '').toLowerCase() === 'nan') return Number.NaN;
    return special[1] === '-' ? -Infinity : Infinity;
  }
  throw new Error(`could not convert string to float: '${text}'`);
}

export interface CliOption {
  flag: `--${string}`;
  dest: string;
  help?: string;
  /** Converts the raw argument; throws with argparse's "invalid <type> value" text on failure. */
  type?: { name: string; parse(text: string): unknown };
  append?: boolean;
  default: unknown;
}

export interface CliPositional {
  dest: string;
  help?: string;
}

export interface CliSpec {
  prog: string;
  description: string;
  positionals: CliPositional[];
  options: CliOption[];
}

export class CliExit extends Error {
  readonly status: number;
  readonly stdout: string;
  readonly stderr: string;
  constructor(status: number, stdout: string, stderr: string) {
    super(stderr || stdout);
    this.status = status;
    this.stdout = stdout;
    this.stderr = stderr;
  }
}

function metavar(option: CliOption): string {
  return option.dest.toUpperCase();
}

export function cliUsage(spec: CliSpec): string {
  const parts = ['[-h]', ...spec.options.map((option) => `[${option.flag} ${metavar(option)}]`), ...spec.positionals.map((positional) => positional.dest)];
  return `usage: ${spec.prog} ${parts.join(' ')}\n`;
}

export function cliHelp(spec: CliSpec): string {
  const rows: Array<[string, string]> = [];
  const lines = [cliUsage(spec), `${spec.description}\n`];
  const positionals = spec.positionals.map((positional): [string, string] => [positional.dest, positional.help ?? '']);
  const options: Array<[string, string]> = [['-h, --help', 'show this help message and exit'],
    ...spec.options.map((option): [string, string] => [`${option.flag} ${metavar(option)}`, option.help ?? ''])];
  rows.push(...positionals, ...options);
  const width = Math.min(24, Math.max(...rows.map(([name]) => name.length + 4)));
  // argparse wraps help text to the terminal width (COLUMNS, default 80) minus 2.
  const columns = Math.max(Number(process.env.COLUMNS) || 80, 22) - 2;
  const wrap = (help: string) => {
    const lines: string[] = [];
    let line = '';
    for (const word of help.split(/\s+/).filter(Boolean)) {
      if (line && line.length + 1 + word.length > Math.max(columns - width, 11)) { lines.push(line); line = word; }
      else line = line ? `${line} ${word}` : word;
    }
    if (line) lines.push(line);
    return lines.join(`\n${' '.repeat(width)}`);
  };
  const render = ([name, help]: [string, string]) => {
    const label = `  ${name}`;
    if (!help) return `${label}\n`;
    return label.length + 2 <= width ? `${label.padEnd(width)}${wrap(help)}\n` : `${label}\n${' '.repeat(width)}${wrap(help)}\n`;
  };
  if (positionals.length) lines.push(`positional arguments:\n${positionals.map(render).join('')}`);
  lines.push(`options:\n${options.map(render).join('')}`);
  return lines.join('\n');
}

/** Raises a CliExit with argparse's usage + `prog: error: message` and status 2. */
export function cliError(spec: CliSpec, message: string): never {
  throw new CliExit(2, '', `${cliUsage(spec)}${spec.prog}: error: ${message}\n`);
}

function looksLikeNegativeNumber(argument: string): boolean {
  return /^-\d+$|^-\d*\.\d+$/.test(argument);
}

/** A small argparse subset: long options, `--opt=value`, unique prefixes, `--`, `-h`. */
export function parseCli(spec: CliSpec, argv: readonly string[]): Record<string, unknown> {
  const values: Record<string, unknown> = {};
  for (const option of spec.options) values[option.dest] = option.append ? [...(option.default as unknown[])] : option.default;
  const positionals: Array<[number, string]> = [];
  const unrecognized: Array<[number, string]> = [];
  let onlyPositionals = false;
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index] ?? '';
    if (onlyPositionals || !argument.startsWith('-') || argument === '-' || looksLikeNegativeNumber(argument)) {
      positionals.push([index, argument]);
      continue;
    }
    if (argument === '--') { onlyPositionals = true; continue; }
    const [name = '', inline] = argument.includes('=') ? [argument.slice(0, argument.indexOf('=')), argument.slice(argument.indexOf('=') + 1)] : [argument, undefined];
    if (name === '-h' || (name.length > 2 && '--help'.startsWith(name) && !spec.options.some((option) => option.flag.startsWith(name)))) {
      if (inline !== undefined) cliError(spec, `argument -h/--help: ignored explicit argument '${inline}'`);
      throw new CliExit(0, cliHelp(spec), '');
    }
    const exact = spec.options.find((option) => option.flag === name);
    const candidates = exact ? [exact] : name.startsWith('--') && name.length > 2 ? spec.options.filter((option) => option.flag.startsWith(name)) : [];
    if (candidates.length > 1) {
      cliError(spec, `ambiguous option: ${name} could match ${candidates.map((option) => option.flag).join(', ')}`);
    }
    const option = candidates[0];
    if (!option) { unrecognized.push([index, argument]); continue; }
    let raw = inline;
    if (raw === undefined) {
      const next = argv[index + 1];
      if (next === undefined || (next.startsWith('-') && next !== '-' && !looksLikeNegativeNumber(next))) {
        cliError(spec, `argument ${option.flag}: expected one argument`);
      }
      raw = next;
      index += 1;
    }
    let converted: unknown = raw;
    if (option.type) {
      try { converted = option.type.parse(raw); } catch { cliError(spec, `argument ${option.flag}: invalid ${option.type.name} value: '${raw}'`); }
    }
    if (option.append) (values[option.dest] as unknown[]).push(converted);
    else values[option.dest] = converted;
  }
  if (positionals.length < spec.positionals.length) {
    cliError(spec, `the following arguments are required: ${spec.positionals.slice(positionals.length).map((positional) => positional.dest).join(', ')}`);
  }
  // argparse reports leftovers in command-line order.
  unrecognized.push(...positionals.slice(spec.positionals.length));
  unrecognized.sort(([left], [right]) => left - right);
  if (unrecognized.length) cliError(spec, `unrecognized arguments: ${unrecognized.map(([, argument]) => argument).join(' ')}`);
  spec.positionals.forEach((positional, index) => { values[positional.dest] = positionals[index]?.[1]; });
  return values;
}

/** Runs a CLI body and maps argparse exits and uncaught errors to process output. */
export async function runCli(body: () => Promise<void> | void): Promise<void> {
  try {
    await body();
  } catch (error) {
    if (error instanceof CliExit) {
      if (error.stdout) process.stdout.write(error.stdout);
      if (error.stderr) process.stderr.write(error.stderr);
      process.exitCode = error.status;
      return;
    }
    process.stderr.write(`error: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}
