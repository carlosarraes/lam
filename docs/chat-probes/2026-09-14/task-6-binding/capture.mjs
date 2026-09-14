import { readFileSync, readlinkSync, writeFileSync } from 'node:fs';
import { basename } from 'node:path';

const label = process.argv[2];
if (!['root', 'child', 'root-two'].includes(label)) throw new Error('invalid owned case');
const bootId = readFileSync('/proc/sys/kernel/random/boot_id', 'utf8').trim();
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
const record = { label, at: new Date().toISOString(), nativeThread: process.env.CODEX_THREAD_ID ?? null, bootId, ancestors };
writeFileSync(`/tmp/lam-chat-task6-topology.TwJPU1/${label}.json`, JSON.stringify(record) + '\n', { mode: 0o600, flag: 'wx' });
console.log(JSON.stringify(record));
