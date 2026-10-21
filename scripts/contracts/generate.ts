#!/usr/bin/env node
/** Rust owns schemas; one pipeline generates TypeScript and standalone guards. */
import { createHash, randomUUID } from 'node:crypto';
import { readFile, mkdir, readdir, rename, unlink, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

type JsonObject = { [key: string]: unknown };
type Compile = (schema: unknown, name: string, options: JsonObject) => Promise<string>;
interface AjvInstance { addSchema(schema: unknown, key: string): unknown }
type AjvConstructor = new (options: JsonObject) => AjvInstance;
type StandaloneCode = (ajv: AjvInstance, refs: Record<string, string>) => string;

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const generatedDirectory = path.join(root, 'packages/contracts/generated');
const indexPath = path.join(root, 'packages/contracts/index.ts');
// Generated banners keep naming the historical generator path so that every
// output (embedded by Rust include_bytes! and the release bundle) stays
// byte-identical. Renaming it is a separate, reviewed regeneration.
const generatorLabel = 'scripts/contracts/generate.mjs';
const version = '16.0.0';
const ajvVersion = '8.20.0';
const supportedSchemaVersion = 1;
const specifications: ReadonlyArray<readonly [stem: string, title: string]> = [
  ['task-contract', 'TaskContract'],
  ['coordination-message', 'CoordinationMessage'],
  ['domain-event', 'DomainEvent'],
  ['domain-event-v2', 'DomainEventV2'],
  ['workplan', 'Workplan'],
  ['verification-receipt', 'VerificationReceipt'],
  ['verification-policy', 'VerificationPolicy'],
  ['interface-contract', 'InterfaceContract'],
  ['config-layer', 'ConfigLayer'],
  ['config-patch', 'ConfigPatch'],
  ['config-layer-receipt', 'ConfigLayerReceipt'],
  ['provenance-contribution', 'ProvenanceContribution'],
  ['views-snapshot', 'ViewsSnapshot'],
  ['intake-batch', 'IntakeBatch'],
  ['source-provenance', 'SourceProvenance'],
  ['intake-preview', 'IntakePreview'],
  ['work-order-revision', 'WorkOrderRevision'],
  ['record-intake-request', 'RecordIntakeRequest'],
  ['record-intake-receipt', 'RecordIntakeReceipt'],
  ['delivery-requirement', 'DeliveryRequirement'],
  ['duplicate-query', 'DuplicateQuery'],
  ['duplicate-page', 'DuplicatePage'],
  ['coordination-envelope-v2', 'CoordinationEnvelopeV2'],
  ['task-attempt-outcome-ledger', 'TaskAttemptOutcomeLedger'],
  ['attempt-outcome-projection', 'AttemptOutcomeProjection'],
  ['execution-requirements-v2', 'ExecutionRequirementsV2'],
  ['execution-requirements-capsule-binding', 'ExecutionRequirementsCapsuleBinding'],
  ['task-interface-contract-v2', 'TaskInterfaceContractV2'],
];
const check = process.argv.length === 3 && process.argv[2] === '--check';
if (process.argv.length > 2 && !check) {
  console.error('Usage: node scripts/contracts/generate.ts [--check]');
  process.exitCode = 2;
} else {
  try {
    await generate();
  } catch (error) {
    console.error(`contracts: ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 1;
  }
}

function isObject(value: unknown): value is JsonObject {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function errorCode(error: unknown): unknown {
  return error && typeof error === 'object' ? (error as { code?: unknown }).code : undefined;
}

function assertLocalReferences(value: unknown): void {
  if (!value || typeof value !== 'object') return;
  for (const [key, child] of Object.entries(value)) {
    if (key === '$ref' && (typeof child !== 'string' || !child.startsWith('#/'))) {
      throw new Error('only internal JSON Schema references are supported; external retrieval is disabled');
    }
    assertLocalReferences(child);
  }
}

function stable(value: unknown): string {
  if (Array.isArray(value)) return JSON.stringify(value.map((item: unknown) => JSON.parse(stable(item))));
  if (value && typeof value === 'object') {
    const object = value as JsonObject;
    return JSON.stringify(Object.fromEntries(Object.keys(object).sort().map((key) => [key, JSON.parse(stable(object[key]))])));
  }
  return JSON.stringify(value);
}

// schemars emits Rust integer formats as annotations. Enforce their actual
// wire bounds without introducing runtime format helpers or schema mirrors.
function runtimeSchema(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(runtimeSchema);
  if (!value || typeof value !== 'object') return value;
  const schemaMaps = new Set(['properties', 'patternProperties', '$defs', 'definitions', 'dependentSchemas']);
  const dataKeywords = new Set(['const', 'enum', 'default', 'examples']);
  const result: JsonObject = Object.fromEntries(Object.entries(value).map(([key, child]: [string, unknown]) => {
    // A domain property named `format` is not a schema format annotation on
    // the surrounding properties map. Likewise literal data is never a schema.
    if (schemaMaps.has(key) && isObject(child)) {
      return [key, Object.fromEntries(Object.entries(child).map(([name, schema]) => [name, runtimeSchema(schema)]))];
    }
    return [key, dataKeywords.has(key) ? child : runtimeSchema(child)];
  }));
  if (result.format !== undefined) {
    const allBounds: Record<string, readonly [number, number]> = { uint16: [0, 65535], uint32: [0, 4294967295], int32: [-2147483648, 2147483647] };
    const bounds = typeof result.format === 'string' && Object.hasOwn(allBounds, result.format) ? allBounds[result.format] : undefined;
    const type = result.type;
    const integer = type === 'integer' || (Array.isArray(type) && type.includes('integer') && type.every((item: unknown) => item === 'integer' || item === 'null'));
    if (!bounds || !integer) throw new Error(`unsupported Rust schema format ${String(result.format)}`);
    const [minimum, maximum] = bounds;
    result.minimum = Math.max((result.minimum as number | undefined) ?? minimum, minimum);
    result.maximum = Math.min((result.maximum as number | undefined) ?? maximum, maximum);
    delete result.format;
  }
  const properties = result.properties;
  if (isObject(properties) && properties.schema_version) {
    const field = properties.schema_version as JsonObject;
    // All five added Rust roots (including the V2-named models) and their
    // versioned nested records currently use v1. An explicit Rust const must
    // agree; a future version needs its own reviewed mapping, never coercion.
    if (field.type !== 'integer' || (field.const !== undefined && field.const !== supportedSchemaVersion)
      || (field.enum !== undefined && stable(field.enum) !== stable([supportedSchemaVersion]))
      || (field.minimum !== undefined && (field.minimum as number) > supportedSchemaVersion)
      || (field.maximum !== undefined && (field.maximum as number) < supportedSchemaVersion)) {
      throw new Error('schema_version must remain compatible with the explicit supported runtime version');
    }
    field.const = supportedSchemaVersion;
  }
  return result;
}

async function atomicWrite(destination: string, contents: string): Promise<void> {
  const temporary = `${destination}.tmp-${randomUUID()}`;
  try {
    await writeFile(temporary, contents, { flag: 'wx' });
    await rename(temporary, destination);
  } finally {
    await unlink(temporary).catch((error: unknown) => {
      if (errorCode(error) !== 'ENOENT') throw error;
    });
  }
}

async function generate(): Promise<void> {
  const require = createRequire(import.meta.url);
  const installed = (require('json-schema-to-typescript/package.json') as { version: string }).version;
  if (installed !== version) {
    throw new Error(`json-schema-to-typescript must be ${version}, found ${installed}; install the pinned workspace dependencies`);
  }
  const { compile } = require('json-schema-to-typescript') as { compile: Compile };
  const installedAjv = (require('ajv/package.json') as { version: string }).version;
  if (installedAjv !== ajvVersion) throw new Error(`ajv must be ${ajvVersion}, found ${installedAjv}; install pinned workspace dependencies`);
  const Ajv2020 = (require('ajv/dist/2020.js') as { default: AjvConstructor }).default;
  const standaloneCode = (require('ajv/dist/standalone/index.js') as { default: StandaloneCode }).default;
  const ajv = new Ajv2020({ code: { source: true, esm: true, lines: true }, strict: true,
    allErrors: false, ownProperties: true, inlineRefs: false, coerceTypes: false,
    removeAdditional: false, useDefaults: false });
  const validators: Record<string, string> = {}, fingerprints: Record<string, string> = {};
  const outputs = new Map<string, string>();
  const definitions = new Map<string, string>();
  const exported = new Map<string, string>();
  const index = [
    `// Generated by ${generatorLabel}. Do not edit.`,
    '// Domain types and standalone schema guards; no browser AJV compiler or Node dependency.',
    '// Shared definitions use their first canonical module; module namespaces retain all definitions.',
    '',
  ];

  for (const [stem, title] of specifications) {
    const source = path.join(root, 'schemas', `${stem}.schema.json`);
    const sourceBytes = await readFile(source);
    const parsed: unknown = JSON.parse(sourceBytes.toString('utf8'));
    if (!isObject(parsed)) throw new Error(`${stem}: expected root title ${title}`);
    const schema = parsed;
    if (schema.title !== title) throw new Error(`${stem}: expected root title ${title}`);
    assertLocalReferences(schema);
    if (title === 'ExecutionRequirementsV2') {
      // placement::DecimalU64 deserializes canonical u64 strings, but its
      // derived schema is only String. Reuse the existing Rust-owned exact
      // interfaces::DecimalU64 definition; never duplicate its range regex.
      const canonical = definitions.get('DecimalU64');
      const defs = isObject(schema.$defs) ? schema.$defs : undefined;
      if (!canonical || !defs || stable(defs.DecimalU64) !== stable({ type: 'string' })) {
        throw new Error('ExecutionRequirementsV2: review changed placement DecimalU64 schema before exporting');
      }
      defs.DecimalU64 = JSON.parse(canonical);
    }
    // A root type also appears as a nested definition in event schemas. Compare
    // both forms before choosing one canonical public export for that name.
    const rootDefinition = Object.fromEntries(Object.entries(schema).filter(([key]) => !['$schema', '$defs', 'title'].includes(key)));
    const nested: Array<[string, unknown]> = Object.entries((schema.$defs ?? {}) as object);
    for (const [name, definition] of [[title, rootDefinition] as [string, unknown], ...nested]) {
      const previous = definitions.get(name);
      const serialized = stable(definition);
      if (previous && previous !== serialized) {
        throw new Error(`conflicting shared JSON Schema definition ${name}; reconcile Rust contracts before generating exports`);
      }
      definitions.set(name, serialized);
    }
    const sha256 = createHash('sha256').update(sourceBytes).digest('hex');
    fingerprints[stem] = sha256;
    const schemaId = `admiralty:${stem}`;
    ajv.addSchema(runtimeSchema(schema), schemaId);
    validators[`validate${title}`] = schemaId;
    const contents = await compile(schema, title, {
      bannerComment: `/**\n * Generated from schemas/${stem}.schema.json by json-schema-to-typescript@${version}.\n * Source SHA-256: ${sha256}\n * Do not edit. Rust domain validation still applies at runtime.\n */`,
      cwd: path.dirname(source),
      // Absent additionalProperties follows JSON Schema / serde defaults.
      // Closed Rust structs already carry explicit false in their schemas.
      additionalProperties: true,
      unreachableDefinitions: true,
      style: { singleQuote: true, semi: true, trailingComma: 'all', printWidth: 100 },
      $refOptions: { resolve: { file: false, http: false } },
    });
    const names = [...contents.matchAll(/^export (?:interface|type|enum) ([A-Za-z_$][\w$]*)/gm)].map((match) => match[1] ?? '');
    if (!names.includes(title)) throw new Error(`${stem}: compiler did not export expected root type ${title}`);
    const unique = names.filter((name) => !exported.has(name));
    for (const name of unique) exported.set(name, stem);
    if (unique.length) index.push(`export type { ${unique.join(', ')} } from './generated/${stem}.js';`);
    index.push(`export type * as ${title}Types from './generated/${stem}.js';`);
    outputs.set(path.join(generatedDirectory, `${stem}.ts`), contents);
  }
  const runtimeExports = [...Object.keys(validators), 'SUPPORTED_SCHEMA_VERSION', 'CONTRACT_SCHEMA_FINGERPRINTS'];
  index.push(`export { ${runtimeExports.join(', ')} } from './generated/validators.mjs';`);
  outputs.set(indexPath, `${index.join('\n')}\n`);
  const banner = `// Generated by ${generatorLabel} with ajv@${ajvVersion}. Do not edit.\n// Source SHA-256: ${JSON.stringify(fingerprints)}\n// Schema validation only; Rust retains semantic checks, scope and acceptance authority.\n`;
  // Keep the original four related roots together so their shared definitions
  // are emitted once. Additional roots live in pure standalone modules: an
  // application importing only the original guards must not load every public
  // schema merely because the compatibility entry re-exports it.
  const baseSpecifications = specifications.slice(0, 4);
  const baseValidators: Record<string, string> = Object.fromEntries(baseSpecifications.map(([, title]) => [`validate${title}`, validators[`validate${title}`] ?? '']));
  const guardModules: Array<[string, Record<string, string>]> = [
    ['validators-base.mjs', baseValidators],
    ...specifications.slice(4).map(([stem, title]): [string, Record<string, string>] => [`validators-${stem}.mjs`, { [`validate${title}`]: validators[`validate${title}`] ?? '' }]),
  ];
  const reexports: string[] = [];
  for (const [filename, guards] of guardModules) {
    const code = standaloneCode(ajv, guards);
    if (/\brequire\s*\(|\bimport\s|\beval\s*\(|\bnew Function\s*\(/.test(code)) {
      throw new Error(`${filename}: standalone guards unexpectedly require runtime imports; review and bundle helpers before shipping`);
    }
    const sources = Object.fromEntries(Object.entries(guards).map(([, id]) => {
      const stem = id.slice('admiralty:'.length);
      return [stem, fingerprints[stem]];
    }));
    const moduleBanner = `// Generated by ${generatorLabel} with ajv@${ajvVersion}. Do not edit.\n// Source SHA-256: ${JSON.stringify(sources)}\n// Pure standalone schema guards; Rust retains semantic and acceptance authority.\n`;
    outputs.set(path.join(generatedDirectory, filename), `${moduleBanner}${code.trimEnd()}\n`);
    reexports.push(`export { ${Object.keys(guards).join(', ')} } from './${filename}';`);
  }
  const runtime = `${banner}${reexports.join('\n')}\nexport const SUPPORTED_SCHEMA_VERSION = ${supportedSchemaVersion};\nexport const CONTRACT_SCHEMA_FINGERPRINTS = Object.freeze(${JSON.stringify(fingerprints, null, 2)});\n`;
  outputs.set(path.join(generatedDirectory, 'validators.mjs'), runtime);
  const declarations = [banner.trimEnd(),
    ...specifications.map(([stem, title]) => `import type { ${title} } from './${stem}.js';`),
    ...specifications.map(([, title]) => `export declare function validate${title}(value: unknown): value is ${title};`),
    `export declare const SUPPORTED_SCHEMA_VERSION: ${supportedSchemaVersion};`,
    'export declare const CONTRACT_SCHEMA_FINGERPRINTS: Readonly<{',
    ...specifications.map(([stem]) => `  '${stem}': string;`), '}>;', '',
  ].join('\n');
  outputs.set(path.join(generatedDirectory, 'validators.d.mts'), declarations);

  const expected = new Set([...specifications.map(([stem]) => `${stem}.ts`), ...guardModules.map(([filename]) => filename), 'validators.mjs', 'validators.d.mts']);
  let existing: string[] = [];
  try {
    existing = await readdir(generatedDirectory);
  } catch (error) {
    if (errorCode(error) !== 'ENOENT') throw error;
  }
  const unexpected = existing.filter((name) => /\.(?:[cm]?ts|[cm]?js)$/.test(name) && !expected.has(name));
  if (unexpected.length) {
    throw new Error(`unexpected generated TypeScript files: ${unexpected.join(', ')}; remove obsolete files through a reviewed change`);
  }

  if (check) {
    const drift: string[] = [];
    for (const [destination, contents] of outputs) {
      try {
        if (await readFile(destination, 'utf8') === contents) continue;
      } catch (error) {
        if (errorCode(error) !== 'ENOENT') throw error;
      }
      drift.push(path.relative(root, destination));
    }
    if (drift.length) throw new Error(`generated contract drift: ${drift.join(', ')}; run pnpm contracts:generate`);
    console.log(`Contract types and standalone guards match ${specifications.length} Rust schemas; no files changed.`);
    return;
  }

  await mkdir(generatedDirectory, { recursive: true });
  for (const [destination, contents] of outputs) await atomicWrite(destination, contents);
  console.log(`Generated ${specifications.length} type modules, standalone guards and packages/contracts/index.ts.`);
}
