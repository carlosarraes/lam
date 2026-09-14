// Test-only filesystem adapter. Never configured as a native hook.
import fs from 'node:fs';
import { syncBuiltinESMExports } from 'node:module';
import { join } from 'node:path';

const dir = process.env.LAM_HOOK_TEST_DIR;
const failure = process.env.LAM_HOOK_TEST_FAILURE;
const read = fs.readFileSync;
const link = fs.readlinkSync;
const write = fs.writeFileSync;
const open = fs.openSync;
const mapped = path => typeof path === 'string' && path.startsWith('/tmp/lam-chat-task6-topology.TwJPU1/')
  ? join(dir, path.split('/').at(-1)) : path;
fs.readFileSync = function(path, ...args) {
  if (typeof path === 'string' && path.startsWith('/proc/')) {
    if (failure === 'process') throw new Error('SECRET arbitrary filesystem failure');
    if (path.endsWith('/boot_id')) return 'test-boot\n';
    return `${process.pid} (node) S 1 ${Array(17).fill('0').join(' ')} 12345\n`;
  }
  return read.call(this, path, ...args);
};
fs.readlinkSync = function(path, ...args) {
  if (typeof path === 'string' && path.startsWith('/proc/')) return '/owned/codex';
  return link.call(this, path, ...args);
};
fs.writeFileSync = function(path, ...args) {
  if (typeof path === 'string' && path.endsWith('/hooks.jsonl') && failure === 'record') {
    throw new Error('SECRET message/tool body and transcript path');
  }
  return write.call(this, mapped(path), ...args);
};
fs.openSync = function(path, ...args) {
  return open.call(this, mapped(path), ...args);
};
syncBuiltinESMExports();
