import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { realpathSync } from "node:fs";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const HOOK_TIMEOUT_MS = 2_000;
const MAX_HOOK_OUTPUT_BYTES = 4_096;
const MAX_BRIDGE_LINE_BYTES = 32_768;

type ActiveSession = {
  sessionId: string;
  cwd: string;
  name: string;
  bridge?: ChildProcessWithoutNullStreams;
  restart?: ReturnType<typeof setTimeout>;
  retryMs: number;
  stopping: boolean;
};

function runLifecycle(
  event: "SessionStart" | "SessionEnd",
  sessionId: string,
  cwd: string,
  name: string,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawn(process.env.LAM_CHAT_BIN ?? "lam", [
      "chat", "hook", "--client", "pi", "--event", event, "--name", name,
    ], {
      cwd,
      env: { ...process.env, PI_SESSION_ID: sessionId },
      stdio: ["pipe", "pipe", "pipe"],
    });
    let outputBytes = 0;
    const timer = setTimeout(() => child.kill("SIGTERM"), HOOK_TIMEOUT_MS);
    const count = (chunk: Buffer) => {
      outputBytes += chunk.length;
      if (outputBytes > MAX_HOOK_OUTPUT_BYTES) child.kill("SIGTERM");
    };
    child.stdout.on("data", count);
    child.stderr.on("data", count);
    child.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on("close", (code, signal) => {
      clearTimeout(timer);
      if (signal || code !== 0 || outputBytes > MAX_HOOK_OUTPUT_BYTES) {
        reject(new Error("LAM Chat Pi lifecycle hook failed"));
      } else {
        resolve();
      }
    });
    child.stdin.end(JSON.stringify({ session_id: sessionId, cwd, hook_event_name: event }));
  });
}

export default function (pi: ExtensionAPI) {
  let active: ActiveSession | undefined;

  // Pi changes its OS process title to `pi`, hiding the JS entrypoint from
  // /proc/cmdline. Its own extension can pass the canonical path to children.
  if (process.argv[1]) {
    process.env.LAM_CHAT_PI_ENTRYPOINT = realpathSync(process.argv[1]);
  }

  const startBridge = (state: ActiveSession) => {
    if (active !== state || state.stopping) return;
    const child = spawn(process.env.LAM_CHAT_BIN ?? "lam", ["chat", "bridge", "--client", "pi"], {
      cwd: state.cwd,
      env: { ...process.env, PI_SESSION_ID: state.sessionId },
      stdio: ["pipe", "pipe", "pipe"],
    });
    state.bridge = child;
    let buffer = "";
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      if (active !== state || state.stopping) return;
      buffer += chunk;
      let end: number;
      while ((end = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, end);
        buffer = buffer.slice(end + 1);
        if (Buffer.byteLength(line) > MAX_BRIDGE_LINE_BYTES) {
          child.kill("SIGTERM");
          return;
        }
        try {
          const item = JSON.parse(line) as {
            attempt?: { id?: string; batch?: { text?: string } };
          };
          const attempt = item.attempt;
          if (!attempt?.id || !attempt.batch?.text || attempt.batch.text.length > 16_384) {
            continue;
          }
          pi.sendMessage({
            customType: "lam-chat-peer",
            content: attempt.batch.text,
            display: true,
            details: { attemptId: attempt.id },
          }, { deliverAs: "steer", triggerTurn: true });
          child.stdin.write(JSON.stringify({ attempt: attempt.id, accepted: true }) + "\n");
          state.retryMs = 1_000;
        } catch {
          // No acknowledgement: the Store owner marks an uncertain handoff Unknown.
        }
      }
      if (Buffer.byteLength(buffer) > MAX_BRIDGE_LINE_BYTES) child.kill("SIGTERM");
    });
    child.stderr.resume();
    child.on("error", () => {});
    child.on("close", () => {
      if (state.bridge === child) state.bridge = undefined;
      if (active !== state || state.stopping) return;
      state.restart = setTimeout(() => startBridge(state), state.retryMs);
      state.retryMs = Math.min(state.retryMs * 2, 30_000);
    });
  };

  pi.on("session_start", async (_event, ctx) => {
    if (process.env.LAM_CHAT_DISABLE === "1") return;
    const sessionId = ctx.sessionManager.getSessionId();
    const name = ctx.sessionManager.getSessionName() ?? process.env.LAM_NAME ?? `pi-${sessionId.slice(0, 8)}`;
    const state: ActiveSession = {
      sessionId, cwd: ctx.cwd, name, retryMs: 1_000, stopping: false,
    };
    active = state;
    try {
      await runLifecycle("SessionStart", sessionId, ctx.cwd, name);
      startBridge(state);
    } catch {
      // The private hook diagnostic has the failure stage. No model context.
    }
  });

  pi.on("session_shutdown", async () => {
    const previous = active;
    active = undefined;
    if (previous) {
      previous.stopping = true;
      if (previous.restart) clearTimeout(previous.restart);
      previous.bridge?.kill("SIGTERM");
      await runLifecycle("SessionEnd", previous.sessionId, previous.cwd, previous.name).catch(() => {});
    }
  });
}
