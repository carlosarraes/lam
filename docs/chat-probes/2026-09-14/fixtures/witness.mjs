import { writeFileSync, appendFileSync } from 'node:fs';
import { spawn } from 'node:child_process';
const caseId = process.argv[2];
const base = '/tmp/lam-chat-gate.Bx9ROj';
const record = value => appendFileSync(`${base}/events.jsonl`, JSON.stringify({ at: new Date().toISOString(), ...value }) + '\n');
writeFileSync(`${base}/${caseId}-identity.json`, JSON.stringify({ caseId, pid: process.pid, parentPid: process.ppid, claudeSocket: process.env.CLAUDE_CODE_MESSAGING_SOCKET, piSession: process.env.PI_SESSION_ID, codexThread: process.env.CODEX_THREAD_ID }));
const child = spawn(process.execPath, ['/home/carraes/projs/lam/.worktrees/lam-chat/scripts/chat-tool-probe.mjs', caseId], { stdio: ['ignore', 'pipe', 'inherit'] });
let pending = '';
child.stdout.on('data', data => {
  process.stdout.write(data);
  pending += data;
  let newline;
  while ((newline = pending.indexOf('\n')) >= 0) {
    record(JSON.parse(pending.slice(0, newline)));
    pending = pending.slice(newline + 1);
  }
});
child.on('exit', code => { process.exitCode = code; });
