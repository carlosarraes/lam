import { env } from "cloudflare:test";
import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";
import { Env, type Bindings } from "../src/Env";
import { Item } from "../src/domain/Item";
import type { PushMessage } from "../src/domain/PushMessage";
import { Devices } from "../src/services/Devices";

const bindings = env as unknown as Bindings;
let sequence = 0;

const item = (priority: "normal" | "critical" = "critical") => new Item({
  id: `push-item-${++sequence}`,
  kind: "request",
  name: "pm",
  title: "Release decision",
  body: "private",
  source_host: "box",
  source_project: "lam",
  priority,
  choices: [],
  checks: [],
  recommendation: "Proceed",
  recommended_choice: null,
  link: "",
  status: "open",
  response_choice: null,
  response_text: null,
  response_by: null,
  created_at: "2026-09-23T00:00:00.000Z",
  resolved_at: null,
  seen_at: null,
  expires_at: null,
  version: 0,
});

async function seed(token: string, revoked = false) {
  const id = `push-device-${++sequence}`;
  await env.DB.prepare(`INSERT INTO devices
    (id, name, credential_hash, fcm_token, app_version, android_version, created_at, revoked_at)
    VALUES (?, 'Phone', ?, ?, 'test', '16', ?, ?)`)
    .bind(id, `hash-${id}`, token, new Date().toISOString(), revoked ? new Date().toISOString() : null).run();
  return id;
}

describe("Push", () => {
  it("delivers critical invalidations to every active registered device", async () => {
    const module = await import("../src/services/Push").catch(() => null);
    expect(module).not.toBeNull();
    if (!module) return;
    await seed("token-a");
    await seed("token-b");
    await seed("revoked", true);
    const sent: Array<{ token: string; message: PushMessage }> = [];
    const client = Layer.succeed(module.FcmClient, {
      send: (token: string, message: PushMessage) => Effect.sync(() => { sent.push({ token, message }); }),
    });
    const program = Effect.flatMap(module.Push, (push) => push.deliver("item.created", item()));

    await Effect.runPromise(program.pipe(
      Effect.provide(module.Push.Default),
      Effect.provide(Devices.Default),
      Effect.provide(client),
      Effect.provideService(Env, bindings),
    ));

    expect(sent.map(({ token }) => token).sort()).toEqual(["token-a", "token-b"]);
    expect(sent.every(({ message }) => message.data.event === "item.created")).toBe(true);
  });

  it("does not contact FCM for a normal item", async () => {
    const module = await import("../src/services/Push").catch(() => null);
    expect(module).not.toBeNull();
    if (!module) return;
    await seed("normal-token");
    let sends = 0;
    const client = Layer.succeed(module.FcmClient, {
      send: () => Effect.sync(() => { sends++; }),
    });

    await Effect.runPromise(Effect.flatMap(module.Push, (push) => push.deliver("item.created", item("normal"))).pipe(
      Effect.provide(module.Push.Default),
      Effect.provide(Devices.Default),
      Effect.provide(client),
      Effect.provideService(Env, bindings),
    ));

    expect(sends).toBe(0);
  });

  it("clears only the registration token rejected by FCM", async () => {
    const module = await import("../src/services/Push").catch(() => null);
    expect(module).not.toBeNull();
    if (!module) return;
    const staleId = await seed("stale-token");
    const currentId = await seed("current-token");
    const client = Layer.succeed(module.FcmClient, {
      send: (token: string) => token === "stale-token"
        ? Effect.fail(new module.InvalidRegistration())
        : Effect.void,
    });

    await Effect.runPromise(Effect.flatMap(module.Push, (push) => push.deliver("item.created", item())).pipe(
      Effect.provide(module.Push.Default),
      Effect.provide(Devices.Default),
      Effect.provide(client),
      Effect.provideService(Env, bindings),
    ));

    expect(await env.DB.prepare("SELECT fcm_token FROM devices WHERE id = ?").bind(staleId).first()).toEqual({ fcm_token: null });
    expect(await env.DB.prepare("SELECT fcm_token FROM devices WHERE id = ?").bind(currentId).first()).toEqual({ fcm_token: "current-token" });
  });
});
