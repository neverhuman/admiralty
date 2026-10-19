import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { readFile, readdir, mkdtemp, mkdir, cp, symlink, writeFile, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { validateTaskContract, validateCoordinationMessage, validateDomainEvent, validateDomainEventV2,
  validateWorkplan, validateVerificationReceipt, validateVerificationPolicy, validateInterfaceContract,
  validateConfigLayer, validateConfigPatch, validateConfigLayerReceipt, validateProvenanceContribution,
  validateIntakeBatch, validateSourceProvenance, validateIntakePreview, validateWorkOrderRevision,
  validateRecordIntakeRequest, validateRecordIntakeReceipt, validateDeliveryRequirement,
  validateDuplicateQuery, validateDuplicatePage, validateCoordinationEnvelopeV2,
  SUPPORTED_SCHEMA_VERSION, CONTRACT_SCHEMA_FINGERPRINTS } from './generated/validators.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const CHILD_TIMEOUT_MS = 30_000;
const CHILD_MAX_BUFFER_BYTES = 1024 * 1024;
const DIAGNOSTIC_OUTPUT_CHARS = 8192;

// These trusted fixture tools have finite capture and direct-child deadlines.
// SIGKILL prevents an ignored SIGTERM from leaving spawnSync waiting forever.
// This is not a process-tree supervisor or a retry mechanism.
function runChild(args, { cwd, env = process.env, timeout = CHILD_TIMEOUT_MS,
  maxBuffer = CHILD_MAX_BUFFER_BYTES } = {}) {
  const started = performance.now();
  const result = spawnSync(process.execPath, args, {
    cwd, env, encoding: 'utf8', timeout, maxBuffer, killSignal: 'SIGKILL',
  });
  return { ...result, command: [process.execPath, ...args], cwd, timeout, maxBuffer,
    elapsed_ms: Math.ceil(performance.now() - started) };
}

function childDiagnostic(stage, result) {
  const output = text => typeof text === 'string' ? {
    utf8_bytes: Buffer.byteLength(text), truncated: text.length > DIAGNOSTIC_OUTPUT_CHARS,
    text: text.slice(0, DIAGNOSTIC_OUTPUT_CHARS),
  } : null;
  return JSON.stringify({ stage, command: result.command, cwd: result.cwd,
    elapsed_ms: result.elapsed_ms, timeout_ms: result.timeout, max_buffer_bytes: result.maxBuffer,
    status: result.status, signal: result.signal,
    error: result.error ? { name: result.error.name, code: result.error.code,
      message: result.error.message, errno: result.error.errno, syscall: result.error.syscall } : null,
    stdout: output(result.stdout), stderr: output(result.stderr) }, null, 2);
}

function expectChildExit(result, expectedStatus, stage) {
  const diagnostic = childDiagnostic(stage, result);
  assert.equal(result.error, undefined, diagnostic);
  assert.equal(result.signal, null, diagnostic);
  assert.equal(result.status, expectedStatus, diagnostic);
}

// Exercise the real outcome shape, including errors which have status=null.
// Expected application failure must never accidentally accept timeout or ENOBUFS.
test('subprocess assertions distinguish bounded tool failures from expected exit codes', () => {
  const nonzero = runChild(['-e', 'process.stdout.write("fixture stdout"); process.stderr.write("fixture stderr"); process.exitCode = 7;']);
  expectChildExit(nonzero, 7, 'diagnostic-fixture:nonzero');
  assert.throws(() => expectChildExit(nonzero, 0, 'diagnostic-fixture:unexpected-exit'), error =>
    error.message.includes('diagnostic-fixture:unexpected-exit') && error.message.includes('fixture stdout')
      && error.message.includes('fixture stderr') && error.message.includes('"status": 7'));
  const timedOut = runChild(['-e', 'process.on("SIGTERM", () => {}); setInterval(() => {}, 1000);'], { timeout: 100 });
  assert.equal(timedOut.error?.code, 'ETIMEDOUT', childDiagnostic('diagnostic-fixture:timeout', timedOut));
  assert.throws(() => expectChildExit(timedOut, 1, 'diagnostic-fixture:timeout'), error =>
    error.message.includes('ETIMEDOUT') && error.message.includes('SIGKILL') && error.message.includes('timeout_ms'));
  const overflow = runChild(['-e', 'process.stdout.write("x".repeat(1024 * 1024));'], { maxBuffer: 128 });
  assert.equal(overflow.error?.code, 'ENOBUFS', childDiagnostic('diagnostic-fixture:output-limit', overflow));
  assert.throws(() => expectChildExit(overflow, 1, 'diagnostic-fixture:output-limit'), /ENOBUFS/);
  const summary = JSON.parse(childDiagnostic('diagnostic-fixture:bounded-log', {
    stdout: 'x'.repeat(DIAGNOSTIC_OUTPUT_CHARS * 2), stderr: null,
  }));
  assert.equal(summary.stdout.text.length, DIAGNOSTIC_OUTPUT_CHARS);
  assert.equal(summary.stdout.truncated, true);
});

const contract = () => ({ schema_version: 1, project_id: 'project', root_id: 'root', task_id: 'task', revision: 1,
  title: 'Scoped task', objective: 'Deliver a checked change', non_goals: ['No publication'], owned_paths: ['src/task.rs'],
  forbidden_paths: ['private'], read_first: ['README.md'], dependencies: [],
  criteria: [{ id: 'C1', text: 'Named check passes', check_ids: ['T1'] }],
  checks: [{ id: 'T1', program: 'cargo', args: ['test'], cwd: '.', timeout_secs: 60 }],
  authority_ref: 'policy:test', max_attempts: 2, simulated: true });
const payloads = () => [
  { kind: 'task_assigned', task_id: 'task', revision: 1, contract_digest: 'sha256:abc' },
  { kind: 'question', task_id: 'task', requirement: 'R1', question: 'Which interface?', options: ['A', 'B'] },
  { kind: 'answer', question_id: 'question', answer: 'A', decision_ref: null },
  { kind: 'artifact_ready', task_id: 'task', artifact_ref: 'sha256:artifact', interface_revision: 'v1' },
  { kind: 'handoff', task_id: 'task', candidate_ref: 'candidate', failed_check: null, next_action: 'Review exact candidate' },
];
const message = payload => ({ schema_version: 1, message_id: 'message', project_id: 'project', sender: 'worker', recipient: 'captain',
  causation_id: null, context_epoch: 'epoch-1', authority_ref: 'policy:test', payload });

test('native lifecycle is explicitly v2 and cannot extend the frozen v1 union', () => {
  const native = { kind: 'task_lifecycle', command: { protocol_version: 2, task_id: 'task',
    expected_task_digest: `sha256:${'a'.repeat(64)}`, actor: 'owner',
    action: { action: 'cancel_pending', reason: 'no longer needed' } } };
  assert.equal(validateDomainEvent(native), false);
  assert.equal(validateDomainEventV2(native), true);
  assert.equal(validateDomainEventV2({ kind: 'task_submitted', contract: contract() }), true);
  for (const version of [0, 1, 3, 2.5]) {
    assert.equal(validateDomainEventV2({ ...native, command: { ...native.command, protocol_version: version } }), false);
  }
  assert.equal(validateDomainEventV2({ ...native, command: { ...native.command, trusted: true } }), false);
  assert.equal(validateDomainEventV2({ ...native, command: { ...native.command,
    action: { ...native.command.action, execution_qualified: true } } }), false);
  // Shape checking alone still cannot authorize the action or prove CAS.
});

test('standalone package entry has no compiler dependency and exports supported version', async () => {
  const publicApi = await import('@admiralty/contracts');
  assert.equal(publicApi.validateTaskContract, validateTaskContract);
  assert.equal(SUPPORTED_SCHEMA_VERSION, 1);
  const generated = path.join(root, 'packages/contracts/generated');
  const runtimeModules = (await readdir(generated)).filter(name => /^validators(?:-[a-z0-9-]+)?\.mjs$/.test(name));
  assert.equal(runtimeModules.length, 21, 'compatibility entry, shared base and nineteen separate public roots');
  for (const name of runtimeModules) {
    const source = await readFile(path.join(generated, name), 'utf8');
    assert.doesNotMatch(source, /\brequire\s*\(|\bimport\s|\beval\s*\(|\bnew Function\s*\(/, name);
  }
  const manifest = JSON.parse(await readFile(path.join(root, 'packages/contracts/package.json'), 'utf8'));
  assert.equal(manifest.sideEffects, false, 'generated modules have no externally observable initialization effects');
  const base = await readFile(path.join(generated, 'validators-base.mjs'), 'utf8');
  for (const name of ['validateTaskContract', 'validateCoordinationMessage', 'validateDomainEvent', 'validateDomainEventV2']) {
    assert.match(base, new RegExp(`export const ${name} =`));
  }
  for (const name of ['validateWorkplan', 'validateVerificationReceipt', 'validateInterfaceContract', 'validateConfigPatch']) {
    assert.doesNotMatch(base, new RegExp(`export const ${name} =`));
  }
  for (const [stem, fingerprint] of Object.entries(CONTRACT_SCHEMA_FINGERPRINTS)) {
    const bytes = await readFile(path.join(root, `schemas/${stem}.schema.json`));
    assert.equal(createHash('sha256').update(bytes).digest('hex'), fingerprint);
  }
});

test('every current event and message variant validates, including optional nulls', () => {
  const events = [
    { kind: 'task_submitted', contract: contract() },
    { kind: 'task_claimed', task_id: 'task', revision: 1, attempt_id: 'attempt', owner: 'worker' },
    { kind: 'task_started', task_id: 'task', attempt_id: 'attempt' },
    { kind: 'candidate_submitted', task_id: 'task', attempt_id: 'attempt', candidate_ref: 'candidate' },
    { kind: 'message_acknowledged', message_id: 'message', recipient: 'captain', context_epoch: 'epoch-1' },
  ];
  assert.equal(validateTaskContract(contract()), true);
  for (const payload of payloads()) {
    const item = message(payload);
    assert.equal(validateCoordinationMessage(item), true);
    events.push({ kind: 'message_queued', message: item });
    assert.equal(validateCoordinationMessage(message({ ...payload, unexpected: true })), false);
    assert.equal(validateDomainEvent({ kind: 'message_queued', message: message({ ...payload, unexpected: true }) }), false);
  }
  for (const event of events) assert.equal(validateDomainEvent(event), true, JSON.stringify(event));
});

test('unknown properties, wrong nested types, missing fields and schema versions fail conservatively', () => {
  for (const mutate of [
    value => { value.unexpected = true; },
    value => { value.checks[0].unexpected = true; },
    value => { value.checks[0].args = [false]; },
    value => { delete value.criteria[0].check_ids; },
    value => { value.schema_version = 2; },
    value => { value.simulated = 'true'; },
  ]) {
    const value = contract(); mutate(value);
    const before = JSON.stringify(value);
    assert.equal(validateTaskContract(value), false);
    assert.equal(validateDomainEvent({ kind: 'task_submitted', contract: value }), false);
    assert.equal(JSON.stringify(value), before, 'guards must not coerce, strip or default data');
  }
  const invalid = message(payloads()[0]); invalid.schema_version = 2;
  assert.equal(validateCoordinationMessage(invalid), false);
  assert.equal(validateDomainEvent({ kind: 'message_queued', message: invalid }), false);
  assert.equal(validateDomainEvent({ kind: 'self_accepted' }), false);
  assert.equal(validateDomainEvent({ kind: 'task_started', task_id: 'task', attempt_id: 'attempt', accepted: true }), false);
  const answer = message(payloads()[2]); answer.payload.decision_ref = { arbitrary: 'object' };
  assert.equal(validateCoordinationMessage(answer), false);
});

test('numeric formats enforce Rust unsigned widths and reject nonfinite or fractional values', () => {
  for (const number of [-1, 1.5, 4294967296, Infinity, NaN]) {
    const value = contract(); value.revision = number;
    assert.equal(validateTaskContract(value), false);
    assert.equal(validateDomainEvent({ kind: 'task_claimed', task_id: 'task', attempt_id: 'attempt', owner: 'worker', revision: number }), false);
  }
  const value = contract(); value.max_attempts = 65536;
  assert.equal(validateTaskContract(value), false);
});

test('generation and drift checks cover standalone code and declarations without modifying checked files', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'admiralty-contract-pipeline-'));
  try {
    await mkdir(path.join(directory, 'scripts/contracts'), { recursive: true });
    await mkdir(path.join(directory, 'packages/contracts'), { recursive: true });
    await cp(path.join(root, 'scripts/contracts/generate.mjs'), path.join(directory, 'scripts/contracts/generate.mjs'));
    await cp(path.join(root, 'schemas'), path.join(directory, 'schemas'), { recursive: true });
    await symlink(path.join(root, 'node_modules'), path.join(directory, 'node_modules'), 'dir');
    const invoke = args => runChild(['scripts/contracts/generate.mjs', ...args], { cwd: directory });
    expectChildExit(invoke([]), 0, 'contracts:initial-generation');
    expectChildExit(invoke(['--check']), 0, 'contracts:initial-check');
    const runtime = path.join(directory, 'packages/contracts/generated/validators.mjs');
    const declarations = path.join(directory, 'packages/contracts/generated/validators.d.mts');
    const separateGuard = path.join(directory, 'packages/contracts/generated/validators-workplan.mjs');
    await writeFile(runtime, '// drift\n'); await writeFile(declarations, '// drift\n'); await writeFile(separateGuard, '// drift\n');
    const checked = invoke(['--check']);
    expectChildExit(checked, 1, 'contracts:intentional-drift-check');
    assert.match(checked.stderr, /validators\.mjs/); assert.match(checked.stderr, /validators\.d\.mts/); assert.match(checked.stderr, /validators-workplan\.mjs/);
    assert.equal(await readFile(runtime, 'utf8'), '// drift\n');
    assert.equal(await readFile(declarations, 'utf8'), '// drift\n');
    assert.equal(await readFile(separateGuard, 'utf8'), '// drift\n');
    expectChildExit(invoke([]), 0, 'contracts:regenerate-after-drift');
    expectChildExit(invoke(['--check']), 0, 'contracts:check-after-regeneration');
    // WorkOrderRevision has a real domain field named format. Schema-map
    // traversal must preserve it, while unknown schema annotations still deny.
    const orderSchema = JSON.parse(await readFile(path.join(directory, 'schemas/work-order-revision.schema.json'), 'utf8'));
    assert.ok(orderSchema.properties.format);
    const provenancePath = path.join(directory, 'schemas/source-provenance.schema.json');
    const originalProvenance = await readFile(provenancePath, 'utf8');
    const unsupported = JSON.parse(originalProvenance);
    unsupported.properties.captured_at.format = 'unsupported-format';
    await writeFile(provenancePath, JSON.stringify(unsupported));
    const formatFailure = invoke([]);
    expectChildExit(formatFailure, 1, 'contracts:reject-unsupported-format');
    assert.match(formatFailure.stderr, /unsupported Rust schema format unsupported-format/);
    await writeFile(provenancePath, originalProvenance);
    const schema = path.join(directory, 'schemas/task-contract.schema.json');
    const value = JSON.parse(await readFile(schema, 'utf8')); value.properties.title = { $ref: 'https://example.com/schema' };
    await writeFile(schema, JSON.stringify(value));
    const external = invoke([]);
    expectChildExit(external, 1, 'contracts:reject-external-reference'); assert.match(external.stderr, /external retrieval is disabled/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

const publicFixtures = JSON.parse(await readFile(new URL('./fixtures/public-roots.json', import.meta.url), 'utf8'));
const publicValidators = { Workplan: validateWorkplan, VerificationReceipt: validateVerificationReceipt,
  VerificationPolicy: validateVerificationPolicy, InterfaceContract: validateInterfaceContract,
  ConfigLayer: validateConfigLayer, ConfigPatch: validateConfigPatch,
  ConfigLayerReceipt: validateConfigLayerReceipt, ProvenanceContribution: validateProvenanceContribution,
  IntakeBatch: validateIntakeBatch, SourceProvenance: validateSourceProvenance, IntakePreview: validateIntakePreview,
  WorkOrderRevision: validateWorkOrderRevision, RecordIntakeRequest: validateRecordIntakeRequest,
  RecordIntakeReceipt: validateRecordIntakeReceipt, DeliveryRequirement: validateDeliveryRequirement,
  DuplicateQuery: validateDuplicateQuery, DuplicatePage: validateDuplicatePage,
  CoordinationEnvelopeV2: validateCoordinationEnvelopeV2 };

test('v2 coordination keeps full header identity and precise counters without importing authority', () => {
  const value = structuredClone(publicFixtures.CoordinationEnvelopeV2);
  assert.equal(validateCoordinationEnvelopeV2(value), true);
  assert.equal(validateCoordinationMessage(value), false);
  for (const protocol_version of [0, 1, 3, '2']) {
    rejectUnchanged(validateCoordinationEnvelopeV2, { ...value, protocol_version });
  }
  for (const id of ['', '.', '..', 'x'.repeat(129), 'é', 'worker\n']) {
    rejectUnchanged(validateCoordinationEnvelopeV2, { ...value, sender: id });
  }
  for (const reference of ['', 'x'.repeat(513), 'policy\n', 'é']) {
    rejectUnchanged(validateCoordinationEnvelopeV2, { ...value, authority: { ...value.authority, reference } });
  }
  for (const generation of ['0', '01', '18446744073709551616', 9007199254740992]) {
    rejectUnchanged(validateCoordinationEnvelopeV2, { ...value, authority: { ...value.authority, generation } });
  }
  for (const field of ['causation_id', 'context_epoch', 'baseline_digest', 'expires_at_unix_ms']) {
    const missing = structuredClone(value); delete missing[field];
    rejectUnchanged(validateCoordinationEnvelopeV2, missing);
  }
  assert.equal(value.source_cursor.event_seq, '18446744073709551615');
  rejectUnchanged(validateCoordinationEnvelopeV2, { ...value, trusted: true });
  // Shape checks do not authenticate the supplied authority or join scope digests.
});

function rejectUnchanged(validate, value) {
  const before = JSON.stringify(value);
  assert.equal(validate(value), false, before);
  assert.equal(JSON.stringify(value), before, 'guards must preserve rejected data');
}

test('public roots share Rust roundtrip fixtures and package guards', async () => {
  const publicApi = await import('@admiralty/contracts');
  assert.equal(Object.keys(CONTRACT_SCHEMA_FINGERPRINTS).length, 23);
  assert.deepEqual(Object.keys(publicFixtures).sort(), Object.keys(publicValidators).sort());
  for (const [name, validate] of Object.entries(publicValidators)) {
    const fixture = structuredClone(publicFixtures[name]);
    const before = JSON.stringify(fixture);
    assert.equal(publicApi[`validate${name}`], validate);
    assert.equal(validate(fixture), true, name);
    assert.equal(JSON.stringify(fixture), before, `roundtrip does not rewrite ${name}`);
    const extended = { ...fixture, untrusted_extra: true };
    if (['ConfigLayerReceipt', 'ProvenanceContribution'].includes(name)) {
      const before = JSON.stringify(extended);
      assert.equal(validate(extended), true, `${name} preserves serde metadata extensibility`);
      assert.equal(JSON.stringify(extended), before);
    } else rejectUnchanged(validate, extended);
    const stem = name.replace(/[A-Z]/g, (letter, index) => `${index ? '-' : ''}${letter.toLowerCase()}`);
    const schema = JSON.parse(await readFile(path.join(root, `schemas/${stem}.schema.json`), 'utf8'));
    for (const required of schema.required ?? []) {
      const missing = structuredClone(fixture); delete missing[required];
      rejectUnchanged(validate, missing);
    }
    if ('schema_version' in fixture) rejectUnchanged(validate, { ...fixture, schema_version: 2 });
  }
  // A provider's shaped receipt remains a claim; neither absent exits nor
  // forged trust/acceptance fields become successful verification evidence.
  const receipt = structuredClone(publicFixtures.VerificationReceipt);
  assert.equal(receipt.producer, 'provider_claim');
  assert.equal(receipt.checks[0].exit_code, null);
  for (const field of ['acceptance_granted', 'verifier_authenticated', 'execution_qualified', 'evidence_custody_verified']) {
    rejectUnchanged(validateVerificationReceipt, { ...receipt, [field]: true });
  }
  assert.equal(validateVerificationReceipt(receipt), true);
  assert.equal(receipt.checks[0].exit_code, null);
});

test('shared decimal-u64 fixtures retain full precision and exactly match canonical Rust range', async () => {
  const cases = JSON.parse(await readFile(new URL('./fixtures/decimal-u64.json', import.meta.url), 'utf8'));
  for (const item of cases) {
    const value = { ...structuredClone(publicFixtures.InterfaceContract), revision: item.value };
    const before = JSON.stringify(value);
    assert.equal(validateInterfaceContract(value), item.accepted, `decimal wire value ${JSON.stringify(item.value)}`);
    assert.equal(JSON.stringify(value), before, 'never coerce a decimal counter');
  }
  // Additional deterministic boundary coverage around every prefix of u64::MAX.
  const maximum = (1n << 64n) - 1n;
  const strings = new Set(['0', '1', maximum.toString(), (maximum + 1n).toString()]);
  for (let power = 0n; power <= 20n; power += 1n) {
    const boundary = 10n ** power;
    for (const delta of [-1n, 0n, 1n]) strings.add((boundary + delta).toString());
  }
  for (let index = 1; index <= 20; index += 1) {
    const prefix = maximum.toString().slice(0, index);
    strings.add(prefix.padEnd(20, '0')); strings.add(prefix.padEnd(20, '9'));
  }
  for (const revision of strings) {
    const expected = /^(0|[1-9][0-9]*)$/.test(revision) && BigInt(revision) <= maximum;
    assert.equal(validateInterfaceContract({ ...publicFixtures.InterfaceContract, revision }), expected, revision);
  }
});

test('public nested objects, bounded collections, optionals and integer ranges remain conservative', () => {
  for (const [name, mutate] of [
    ['Workplan', value => { value.tasks[0].execution_qualified = true; }],
    ['VerificationReceipt', value => { value.inputs.accepted = true; }],
    ['VerificationReceipt', value => { value.checks[0].exit_code = 256; }],
    ['VerificationReceipt', value => { value.checks[0].exit_code = -1; }],
    ['VerificationReceipt', value => { value.checks[0].exit_code = 0.5; }],
    ['VerificationReceipt', value => { value.checks[0].check.id = false; }],
    ['VerificationPolicy', value => { value.required_negative_checks[0].expected_exit_code = 256; }],
    ['VerificationPolicy', value => { value.required_negative_checks = []; }],
    ['InterfaceContract', value => { value.scope.project_id = []; }],
    ['ConfigLayer', value => { value.values.private_policy = { max_agents: 65535 }; }],
    ['ConfigPatch', value => { value.models[0].credentials = 'not accepted'; }],
    ['ConfigPatch', value => { value.max_agents = 0; }],
    ['ConfigPatch', value => { value.max_agents = 65536; }],
    ['ConfigPatch', value => { value.max_context_tokens = 4294967296; }],
    ['ConfigPatch', value => { value.models[0].max_concurrency = 0; }],
    ['ConfigLayerReceipt', value => { value.layer = 'private'; }],
    ['ProvenanceContribution', value => { value.origin = 'unknown'; }],
  ]) {
    const value = structuredClone(publicFixtures[name]); mutate(value);
    rejectUnchanged(publicValidators[name], value);
  }
  for (const [name, key, maximum] of [
    ['Workplan', 'tasks', 1000], ['VerificationReceipt', 'checks', 256],
    ['VerificationReceipt', 'negative_checks', 64], ['VerificationPolicy', 'required_negative_checks', 64],
    ['InterfaceContract', 'consumers', 1024], ['ConfigPatch', 'nodes', 256],
    ['ConfigPatch', 'models', 256], ['ConfigPatch', 'denied_paths', 256],
  ]) {
    const value = structuredClone(publicFixtures[name]);
    value[key] = Array(maximum).fill(value[key][0]);
    assert.equal(publicValidators[name](value), true, `schema collection boundary ${name}.${key}`);
    value[key].push(value[key][0]);
    rejectUnchanged(publicValidators[name], value);
  }
  for (const value of [{}, { listen: null, max_agents: null, max_context_tokens: null,
    nodes: null, models: null, denied_paths: null }]) {
    const before = JSON.stringify(value);
    assert.equal(validateConfigPatch(value), true);
    assert.equal(JSON.stringify(value), before, 'missing optionals stay missing; explicit nulls stay null');
  }
  const unknownExit = structuredClone(publicFixtures.VerificationReceipt);
  delete unknownExit.checks[0].exit_code;
  assert.equal(validateVerificationReceipt(unknownExit), true);
  assert.equal(Object.hasOwn(unknownExit.checks[0], 'exit_code'), false);
  assert.equal(validateProvenanceContribution({ origin: 'private' }), true);
});

test('a real TypeScript consumer resolves every public root, subpath and precision-safe counter', () => {
  // Reuse the pinned dashboard build tool; it is not a runtime dependency.
  // Invoke the actual CLI so the fixture also covers the native TypeScript 7 toolchain.
  const require = createRequire(path.join(root, 'apps/dashboard/package.json'));
  const manifest = require('typescript/package.json');
  const compiler = path.resolve(path.dirname(require.resolve('typescript/package.json')), manifest.bin.tsc);
  const checked = runChild([compiler, '--project', 'packages/contracts/tsconfig.json', '--pretty', 'false'], {
    cwd: root,
    env: { ...process.env, GOMAXPROCS: '1' },
  });
  expectChildExit(checked, 0, 'contracts:typescript-consumer');
});


test('public intake generations retain exact positive u64 strings in every nested location', async () => {
  const cases = JSON.parse(await readFile(new URL('./fixtures/decimal-u64.json', import.meta.url), 'utf8'));
  for (const item of cases) {
    const expected = item.accepted && item.value !== '0';
    for (const [name, locate] of [
      ['IntakePreview', value => value.authorized_scope],
      ['WorkOrderRevision', value => value.scope],
      ['RecordIntakeRequest', value => value],
      ['RecordIntakeRequest', value => value.work_order.scope],
    ]) {
      const value = structuredClone(publicFixtures[name]);
      locate(value).authority_generation = item.value;
      const before = JSON.stringify(value);
      assert.equal(publicValidators[name](value), expected, `${name}: ${JSON.stringify(item.value)}`);
      assert.equal(JSON.stringify(value), before, 'never coerce or round an authority generation');
    }
  }
  for (const generation of ['9007199254740993', '18446744073709551615']) {
    const value = structuredClone(publicFixtures.RecordIntakeRequest);
    value.authority_generation = generation;
    value.work_order.scope.authority_generation = generation;
    assert.equal(validateRecordIntakeRequest(value), true);
    assert.equal(value.authority_generation, generation);
  }
});

test('intake schemas reject forged authority fields, malformed boundaries and nested claims', () => {
  for (const [name, mutate] of [
    ['IntakeBatch', value => { value.items[0].outcomes[0].task.execution_qualified = true; }],
    ['IntakeBatch', value => { value.items[0].outcomes[0].source_spans[0].start = 4294967296; }],
    ['SourceProvenance', value => { value.requester = { kind: 'authenticated', requester: 'owner' }; }],
    ['SourceProvenance', value => { value.origin.authenticated = true; }],
    ['IntakePreview', value => { value.authorized_scope.authorized = true; }],
    ['IntakePreview', value => { value.format = 'infer_from_content'; }],
    ['IntakePreview', value => { value.requests[0].index = 65536; }],
    ['WorkOrderRevision', value => { delete value.format; }],
    ['WorkOrderRevision', value => { value.source_payload.custody_verified = true; }],
    ['RecordIntakeRequest', value => { value.authenticated = true; }],
    ['RecordIntakeRequest', value => { delete value.policy_digest; }],
    ['RecordIntakeRequest', value => { value.selected_tasks[0].binding.request_index = 65536; }],
    ['RecordIntakeRequest', value => { value.selected_tasks[0].task.acceptance_granted = true; }],
    ['RecordIntakeReceipt', value => { value.execution_qualified = true; }],
    ['RecordIntakeReceipt', value => { value.duplicate = 'true'; }],
    ['DeliveryRequirement', value => { value.targets[0].requirement = { kind: 'unknown', stage: 'released' }; }],
    ['DeliveryRequirement', value => { value.targets[0].requirement.stage = 'accepted'; }],
    ['DuplicateQuery', value => { value.limit = 65536; }],
    ['DuplicatePage', value => { value.coverage = { kind: 'partial' }; }],
    ['DuplicatePage', value => { value.candidates[0].accepted = true; }],
  ]) {
    const value = structuredClone(publicFixtures[name]); mutate(value);
    rejectUnchanged(publicValidators[name], value);
  }
  // Structurally valid data remains a claim. Only current Rust policy/source/
  // selection/custody checks can reject an invented digest or grant an effect.
  const shapedClaim = structuredClone(publicFixtures.RecordIntakeRequest);
  shapedClaim.policy_digest = `sha256:${'b'.repeat(64)}`;
  assert.equal(validateRecordIntakeRequest(shapedClaim), true);
  assert.equal(validateSourceProvenance({ ...publicFixtures.SourceProvenance,
    requester: { kind: 'claimed', requester: 'owner' } }), true);
  const artifactClaim = structuredClone(publicFixtures.WorkOrderRevision);
  artifactClaim.source_payload = { kind: 'artifact_claim', raw_digest: artifactClaim.raw_digest,
    size_bytes: 123, artifact_ref: 'artifact:claim', pin_receipt_ref: 'pin:claim' };
  assert.equal(validateWorkOrderRevision(artifactClaim), true, 'shape does not establish artifact custody');
  rejectUnchanged(validateWorkOrderRevision, { ...artifactClaim,
    source_payload: { ...artifactClaim.source_payload, custody_verified: true } });
});
