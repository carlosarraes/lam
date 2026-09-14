import { spawnSync } from 'node:child_process';
import { performance } from 'node:perf_hooks';
const root = "/tmp/lam-chat-task6-native.KlGYDF/hook-case-1";
if (process.cwd() !== root || !/^[0-9a-f-]{36}$/.test(process.env.CODEX_THREAD_ID ?? '')) throw new Error('owned native context required');
const input = JSON.stringify({hook_event_name:'PostToolUse',session_id:process.env.CODEX_THREAD_ID,cwd:root});
const samples = [];
let stdoutBytes=0, stderrBytes=0;
for (let i=0;i<100;i++) {
  const start=performance.now();
  const r=spawnSync(root+'/bin/lam-8bc82a6',['chat','hook','--client','codex','--event','PostToolUse'],{
    input,env:{...process.env,LAM_CHAT_CONFIG:root+'/chat.toml',LAM_CHAT_DATA_DIR:root+'/data'},timeout:2500
  });
  samples.push(performance.now()-start);
  stdoutBytes += r.stdout?.length ?? 0; stderrBytes += r.stderr?.length ?? 0;
  if(r.error || r.status!==0) throw new Error('owned hook invocation failed');
}
const sorted=[...samples].sort((a,b)=>a-b);
console.log(JSON.stringify({nativeThread:process.env.CODEX_THREAD_ID,count:100,stdoutBytes,stderrBytes,medianMs:(sorted[49]+sorted[50])/2,p95Ms:sorted[94],maxMs:sorted[99],samplesMs:samples}));
