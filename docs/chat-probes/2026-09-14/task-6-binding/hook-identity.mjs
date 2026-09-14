import { closeSync, constants, fstatSync, openSync, readFileSync, readlinkSync, writeFileSync } from 'node:fs';
import { basename } from 'node:path';

// Proposed diagnostic fixture only. No context output or trust changes.
let event;
let stage = 'process';
try {
  const input = JSON.parse(readFileSync(0, 'utf8'));
  const allowed = ['SessionStart', 'SubagentStart', 'PostToolUse', 'SubagentStop'];
  if (input.cwd !== '/tmp/lam-chat-gate.Bx9ROj' || !allowed.includes(input.hook_event_name)) process.exit(0);
  event = input.hook_event_name;
  const shortString = value => typeof value === 'string' && value.length <= 256 ? value : null;
  const ancestors = [];
  let pid = process.pid;
  for (let depth = 0; depth < 16 && pid > 1; depth++) {
    const stat = readFileSync(`/proc/${pid}/stat`, 'utf8');
    const fields = stat.slice(stat.lastIndexOf(')') + 2).split(' ');
    const executable = readlinkSync(`/proc/${pid}/exe`);
    const parent = Number(fields[1]);
    ancestors.push({ pid, parent, executable, startTicks: fields[19] });
    if (basename(executable) === 'codex') break;
    pid = parent;
  }
  const record = {
    at: new Date().toISOString(),
    event: input.hook_event_name,
    sessionId: shortString(input.session_id),
    agentId: shortString(input.agent_id),
    nativeThread: shortString(process.env.CODEX_THREAD_ID),
    bootId: readFileSync('/proc/sys/kernel/random/boot_id', 'utf8').trim(),
    ancestors,
  };
  stage = 'record';
  writeFileSync('/tmp/lam-chat-task6-topology.TwJPU1/hooks.jsonl', JSON.stringify(record) + '\n', { flag: 'a', mode: 0o600 });
} catch {
  // Unparsed/out-of-scope input cannot authorize a diagnostic write.
  if (event) {
    try {
      const fd = openSync('/tmp/lam-chat-task6-topology.TwJPU1/hook-errors.jsonl', constants.O_WRONLY | constants.O_CREAT | constants.O_APPEND | constants.O_NOFOLLOW, 0o600);
      try {
        const stat = fstatSync(fd);
        if (stat.isFile() && stat.uid === process.getuid() && stat.nlink === 1 && (stat.mode & 0o777) === 0o600) {
          writeFileSync(fd, JSON.stringify({ event, status: 'error', stage }) + '\n');
        }
      } finally {
        closeSync(fd);
      }
    } catch {
      // If private diagnostics are unavailable, absence remains inconclusive.
    }
  }
}
