import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile, mkdtemp, mkdir, cp, symlink, writeFile, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { validateTaskContract, validateCoordinationMessage, validateDomainEvent,
  SUPPORTED_SCHEMA_VERSION, CONTRACT_SCHEMA_FINGERPRINTS } from './generated/validators.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
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

test('standalone package entry has no compiler dependency and exports supported version', async () => {
  const publicApi = await import('@admiralty/contracts');
  assert.equal(publicApi.validateTaskContract, validateTaskContract);
  assert.equal(SUPPORTED_SCHEMA_VERSION, 1);
  const source = await readFile(path.join(root, 'packages/contracts/generated/validators.mjs'), 'utf8');
  assert.doesNotMatch(source, /\brequire\s*\(|\bimport\s|\beval\s*\(|\bnew Function\s*\(/);
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
    const invoke = args => spawnSync(process.execPath, ['scripts/contracts/generate.mjs', ...args], { cwd: directory, encoding: 'utf8', timeout: 30_000 });
    assert.equal(invoke([]).status, 0);
    assert.equal(invoke(['--check']).status, 0);
    const runtime = path.join(directory, 'packages/contracts/generated/validators.mjs');
    const declarations = path.join(directory, 'packages/contracts/generated/validators.d.mts');
    await writeFile(runtime, '// drift\n'); await writeFile(declarations, '// drift\n');
    const checked = invoke(['--check']);
    assert.notEqual(checked.status, 0);
    assert.match(checked.stderr, /validators\.mjs/); assert.match(checked.stderr, /validators\.d\.mts/);
    assert.equal(await readFile(runtime, 'utf8'), '// drift\n');
    assert.equal(await readFile(declarations, 'utf8'), '// drift\n');
    assert.equal(invoke([]).status, 0);
    assert.equal(invoke(['--check']).status, 0);
    const schema = path.join(directory, 'schemas/task-contract.schema.json');
    const value = JSON.parse(await readFile(schema, 'utf8')); value.properties.title = { $ref: 'https://example.com/schema' };
    await writeFile(schema, JSON.stringify(value));
    const external = invoke([]);
    assert.notEqual(external.status, 0); assert.match(external.stderr, /external retrieval is disabled/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});
