import { readFileSync } from 'node:fs';

const cases = ['busy', 'idle', 'busy-to-idle', 'idle-to-busy', 'empty', 'provenance', 'refusal'];
const nonempty = value => typeof value === 'string' && value.trim().length > 0;

try {
  if (!process.argv[2]) throw new Error('usage: node scripts/chat-probe-check.mjs REPORT.json');
  const report = JSON.parse(readFileSync(process.argv[2], 'utf8'));
  const failures = [];
  for (const client of ['claude', 'codex', 'pi']) {
    if (!nonempty(report?.[client]?.version)) failures.push(`${client}: version required`);
    for (const name of cases) {
      const result = report?.[client]?.cases?.[name];
      if (result?.passed !== true) failures.push(`${client}/${name}: not passed`);
      if (!nonempty(result?.evidence)) failures.push(`${client}/${name}: evidence required`);
    }
  }
  if (failures.length) throw new Error(failures.join('\n'));
  process.stdout.write('All client compatibility gates passed. Review linked evidence before rollout.\n');
} catch (error) {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
}
