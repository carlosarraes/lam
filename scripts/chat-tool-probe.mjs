import { setTimeout } from 'node:timers/promises';

const caseId = process.argv[2];
if (!caseId) throw new Error('case id required');
const started = Date.now();
process.on('SIGINT', () => {
  process.stdout.write(JSON.stringify({ caseId, event: 'interrupted' }) + '\n');
  process.exit(130);
});
process.stdout.write(JSON.stringify({ caseId, event: 'ready' }) + '\n');
await setTimeout(20_000);
process.stdout.write(JSON.stringify({ caseId, event: 'completed', elapsedMs: Date.now() - started }) + '\n');
