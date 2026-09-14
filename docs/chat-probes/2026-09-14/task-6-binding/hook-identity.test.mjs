import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

const script = fileURLToPath(new URL('./hook-identity.mjs', import.meta.url));
const loader = fileURLToPath(new URL('./hook-test-loader.mjs', import.meta.url));
const input = { cwd: '/tmp/lam-chat-gate.Bx9ROj', hook_event_name: 'PostToolUse', session_id: 'root-id', agent_id: 'child-id', tool_input: 'SECRET', tool_response: 'SECRET', transcript_path: 'SECRET' };
function fixture(t) {
  const dir = mkdtempSync(join(tmpdir(), 'lam-hook-diagnostic-test-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}
function run(dir, failure = '', payload = input) {
  const result = spawnSync(process.execPath, ['--import', loader, script], {
    input: typeof payload === 'string' ? payload : JSON.stringify(payload),
    encoding: 'utf8', timeout: 2000,
    env: { ...process.env, CODEX_THREAD_ID: 'native-id', LAM_HOOK_TEST_DIR: dir, LAM_HOOK_TEST_FAILURE: failure },
  });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, '');
  assert.equal(result.stderr, '');
}

test('successful capture stays silent and excludes tool/transcript data', t => {
  const dir = fixture(t);
  run(dir);
  assert.deepEqual(readdirSync(dir), ['hooks.jsonl']);
  const raw = readFileSync(join(dir, 'hooks.jsonl'), 'utf8');
  const record = JSON.parse(raw);
  assert.equal(record.sessionId, 'root-id');
  assert.equal(record.agentId, 'child-id');
  assert.equal(record.nativeThread, 'native-id');
  assert.equal(record.ancestors[0].startTicks, '12345\n');
  assert.equal(raw.includes('SECRET'), false);
  assert.equal(statSync(join(dir, 'hooks.jsonl')).mode & 0o777, 0o600);
});

for (const failure of ['process', 'record']) {
  test(`${failure} failure leaves a private bounded diagnostic without output or error text`, t => {
    const dir = fixture(t);
    run(dir, failure);
    const path = join(dir, 'hook-errors.jsonl');
    const raw = readFileSync(path, 'utf8');
    assert.ok(Buffer.byteLength(raw) <= 128);
    assert.deepEqual(JSON.parse(raw), { event: 'PostToolUse', status: 'error', stage: failure });
    assert.equal(raw.includes('SECRET'), false);
    assert.equal(statSync(path).mode & 0o777, 0o600);
    assert.deepEqual(readdirSync(dir), ['hook-errors.jsonl']);
  });
}

test('unvalidated cwd, event and malformed input never create diagnostic files', t => {
  const dir = fixture(t);
  run(dir, 'process', { ...input, cwd: '/unrelated' });
  run(dir, 'process', { ...input, hook_event_name: 'Other' });
  run(dir, 'process', '{invalid SECRET');
  assert.deepEqual(readdirSync(dir), []);
});

test('later failures append without replacing the first diagnostic', t => {
  const dir = fixture(t);
  run(dir, 'process');
  run(dir, 'record');
  const records = readFileSync(join(dir, 'hook-errors.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
  assert.deepEqual(records.map(record => record.stage), ['process', 'record']);
});

test('diagnostic symlink cannot redirect a write', t => {
  const dir = fixture(t);
  const target = join(dir, 'unrelated');
  writeFileSync(target, 'preserve', { mode: 0o600 });
  symlinkSync(target, join(dir, 'hook-errors.jsonl'));
  run(dir, 'process');
  assert.equal(readFileSync(target, 'utf8'), 'preserve');
});

test('unwritable diagnostic destination keeps the hook silent', t => {
  const dir = fixture(t);
  writeFileSync(join(dir, 'hook-errors.jsonl'), '', { mode: 0o400 });
  run(dir, 'process');
  assert.equal(readFileSync(join(dir, 'hook-errors.jsonl'), 'utf8'), '');
});
