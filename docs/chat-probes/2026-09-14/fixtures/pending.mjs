import { readFileSync, existsSync, writeFileSync, appendFileSync } from 'node:fs';
import { setTimeout } from 'node:timers/promises';
const [caseId, nonce, body = `Report nonce ${nonce} after the current tool completes.`] = process.argv.slice(2);
const base = '/tmp/lam-chat-gate.Bx9ROj';
const deadline = Date.now() + 90_000;
while (!existsSync(`${base}/events.jsonl`) || !readFileSync(`${base}/events.jsonl`, 'utf8').split('\n').filter(Boolean).map(JSON.parse).some(event => event.caseId === caseId && event.event === 'ready')) {
  if (Date.now() > deadline) throw Error('no readiness event');
  await setTimeout(50);
}
const payload = { origin: 'agent', senderId: 'owned-lam-probe', senderName: 'Carlos', nonce, body };
writeFileSync(`${base}/codex-pending.json`, JSON.stringify(payload));
const event = { at: new Date().toISOString(), client: 'codex', event: 'pending-written', caseId, payload };
appendFileSync(`${base}/deliveries.jsonl`, JSON.stringify(event) + '\n');
console.log(JSON.stringify(event));
