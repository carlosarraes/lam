import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

const checker = new URL('./chat-probe-check.mjs', import.meta.url);
const probe = new URL('./chat-tool-probe.mjs', import.meta.url);
const names = ['busy', 'idle', 'busy-to-idle', 'idle-to-busy', 'empty', 'provenance', 'refusal'];

function check(report) {
  const directory = mkdtempSync(join(tmpdir(), 'lam-chat-check-test-'));
  try {
    const path = join(directory, 'report.json');
    writeFileSync(path, JSON.stringify(report));
    return spawnSync(process.execPath, [checker.pathname, path], { encoding: 'utf8' });
  } finally {
    rmSync(directory, { recursive: true });
  }
}

function completeReport() {
  return Object.fromEntries(['claude', 'codex', 'pi'].map(client => [client, {
    version: 'test-fixture-only',
    cases: Object.fromEntries(names.map(name => [name, { passed: true, evidence: 'fixture evidence' }])),
  }]));
}

test('empty report fails with a useful missing-client diagnostic', () => {
  const result = check({});
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /claude: version required/);
});

test('complete report passes', () => {
  assert.equal(check(completeReport()).status, 0);
});

test('every absent or failed client case prevents success', () => {
  for (const client of ['claude', 'codex', 'pi']) {
    for (const name of names) {
      for (const result of [undefined, { passed: false, evidence: 'observed failure' }]) {
        const report = completeReport();
        report[client].cases[name] = result;
        const checked = check(report);
        assert.notEqual(checked.status, 0);
        assert.ok(checked.stderr.includes(`${client}/${name}`));
      }
    }
  }
});

test('empty evidence and blank versions cannot certify a gate', () => {
  for (const evidence of [undefined, '', '  ', {}, []]) {
    const report = completeReport();
    report.pi.cases.provenance.evidence = evidence;
    assert.notEqual(check(report).status, 0);
  }
  const report = completeReport();
  report.codex.version = '  ';
  assert.notEqual(check(report).status, 0);
});

test('tool probe requires a case ID', () => {
  const result = spawnSync(process.execPath, [probe.pathname], { encoding: 'utf8' });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /case id required/);
});

test('tool probe reports readiness then completes after at least twenty seconds', { timeout: 35_000 }, async () => {
  const child = spawn(process.execPath, [probe.pathname, 'unit-owned-long-tool'], { timeout: 30_000 });
  let stdout = '';
  child.stdout.on('data', chunk => { stdout += chunk; });
  const code = await new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('close', resolve);
  });
  assert.equal(code, 0);
  const events = stdout.trim().split('\n').map(line => JSON.parse(line));
  assert.deepEqual(events[0], { caseId: 'unit-owned-long-tool', event: 'ready' });
  assert.equal(events.length, 2);
  assert.equal(events[1].event, 'completed');
  assert.equal(events[1].caseId, 'unit-owned-long-tool');
  assert.ok(events[1].elapsedMs >= 20_000);
});
