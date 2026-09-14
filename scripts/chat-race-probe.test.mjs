import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import { test } from 'node:test';

test('two idle contenders share one atomic pending claim', { timeout: 5000 }, async () => {
  const base = mkdtempSync(join(tmpdir(), 'lam-chat-race-test-'));
  try {
    writeFileSync(join(base, 'codex-pending.json'), JSON.stringify({ nonce: 'unit-only' }));
    const source = new URL('../docs/chat-probes/2026-09-14/fixtures/race-coordinator.mjs', import.meta.url).href;
    const run = owner => new Promise((resolve, reject) => {
      const child = spawn(process.execPath, ['--input-type=module', '-e', `import { claim } from ${JSON.stringify(source)}; console.log(JSON.stringify(claim(${JSON.stringify(base)}, ${JSON.stringify(owner)})));`], { timeout: 3000 });
      let stdout = '';
      child.stdout.on('data', chunk => { stdout += chunk; });
      child.once('error', reject);
      child.once('close', code => {
        if (code !== 0) { reject(new Error(`claim child exited ${code}`)); return; }
        try { resolve(JSON.parse(stdout)); } catch (error) { reject(error); }
      });
    });
    const results = await Promise.all([run('first'), run('second')]);
    assert.equal(results.filter(Boolean).length, 1);
    assert.equal(results.find(Boolean).nonce, 'unit-only');
  } finally {
    rmSync(base, { recursive: true });
  }
});

const evidence = new URL('../docs/chat-probes/2026-09-14/', import.meta.url);
const readRows = name => readFileSync(new URL(name, evidence), 'utf8').trim().split('\n').map(JSON.parse);

test('captured native transitions show single handoff in both ownership orderings', () => {
  const log = JSON.parse(readFileSync(new URL('transition-events.json', evidence), 'utf8'));
  const transcript = readRows('codex-transitions.jsonl');
  const messages = transcript.filter(row => row.type === 'response_item' && row.payload.type === 'message');
  const contents = row => row.payload.content.map(content => content.text).join('\n');
  const cases = [
    ['codex-stop-race', 'CODEX_STOP_P8R6', 'user'],
    ['codex-idle-queue-race', 'CODEX_ITB_QUEUE_K3V8', 'user'],
    ['codex-idle-hook-race', 'CODEX_ITB_HOOK_F7N2', 'developer'],
  ];
  for (const [caseId, nonce, role] of cases) {
    const completed = log.toolEvents.find(row => row.caseId === caseId && row.event === 'completed');
    assert.ok(completed.elapsedMs >= 20000);
    assert.equal(log.toolEvents.filter(row => row.caseId === caseId).length, 2);
    const exposure = messages.filter(row => ['user', 'developer'].includes(row.payload.role) && contents(row).includes(nonce));
    assert.equal(exposure.length, 1);
    assert.equal(exposure[0].payload.role, role);
    assert.ok(exposure[0].timestamp >= completed.at);
    const receipt = messages.filter(row => row.payload.role === 'assistant' && contents(row) === nonce);
    assert.equal(receipt.length, 1);
    assert.ok(receipt[0].timestamp > exposure[0].timestamp);
  }
  const late = log.coordinatorEvents.find(row => row.event === 'pending-written' && row.mode === 'busy-to-idle');
  const empty = log.hookEvents.find(row => row.event === 'PostToolUse');
  const stop = log.hookEvents.find(row => row.event === 'Stop' && row.present);
  assert.ok(empty.at < late.at && late.at < stop.at);
  assert.equal(stop.outcome, 'native-accepted');
  assert.equal(stop.exitCode, 0);
  assert.equal(log.hookEvents.filter(row => row.event === 'Stop' && row.present).length, 1);
  for (const mode of ['queue-wins', 'hook-wins']) {
    const rows = log.coordinatorEvents.filter(row => row.mode === mode);
    assert.ok(rows.find(row => row.event === 'sampled-idle').at < rows.find(row => row.event === 'observed-busy').at);
    assert.equal(rows.find(row => row.event === 'idle-claim').won, mode === 'queue-wins');
    assert.equal(rows.filter(row => row.event === 'native-queue').length, mode === 'queue-wins' ? 1 : 0);
  }
  assert.equal(messages.filter(row => row.payload.role === 'user' && contents(row).startsWith('Authorized ')).length, 3);
  assert.equal(transcript.filter(row => row.type === 'event_msg' && row.payload.type === 'task_complete').length, 5);
  assert.equal(log.cleanup.activeHookFileExists, false);
  assert.equal(log.cleanup.activePendingExists, false);
});

test('native Claude submission is followed by positive refusal evidence', () => {
  const rows = readRows('claude-native-refusal.jsonl');
  const receiver = JSON.parse(readFileSync(new URL('claude-native-refusal-check.json', evidence), 'utf8'));
  const calls = rows.flatMap(row => Array.isArray(row.content) ? row.content : []).filter(content => content.type === 'tool_use');
  assert.equal(calls.length, 1);
  assert.equal(calls[0].name, 'SendMessage');
  const result = rows.find(row => Array.isArray(row.content) && row.content.some(content => content.type === 'tool_result'));
  const nativeResult = JSON.parse(result.content[0].content[0].text);
  assert.equal(nativeResult.success, true);
  assert.ok(nativeResult.msg_id);
  const notice = rows.find(row => typeof row.content === 'string' && row.content.startsWith('[Cross-session delivery notice]'));
  assert.ok(notice.timestamp > result.timestamp);
  assert.match(notice.content, /Not delivered to that session's Claude/);
  assert.ok(notice.content.includes(receiver.receiverSocket));
  assert.equal(receiver.receiverTrace.length, 1);
  assert.match(receiver.receiverTrace[0], /refused inbound peer message.*dropped before attachment materialization/);
  assert.equal(receiver.socketExistsAfterExit, false);
});
