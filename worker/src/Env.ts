import { Context } from "effect";
import type { Topic } from "./ntfy/topic";

export interface Bindings {
  DB: D1Database;
  LAM_TOKEN: string;
  LAM_HMAC_SECRET: string;
  NTFY_TOPIC: string;
  TOPIC: DurableObjectNamespace<Topic>;
}

export class Env extends Context.Tag("lam/Env")<Env, Bindings>() {}

/** Per-request Cloudflare ExecutionContext, for waitUntil. */
export class Exec extends Context.Tag("lam/Exec")<Exec, ExecutionContext>() {}
