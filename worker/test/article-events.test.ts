import { env, SELF, createExecutionContext, createScheduledController, waitOnExecutionContext } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import worker from "../src/index";
import type { Message } from "../src/ntfy/message";
import { Effect, Schema } from "effect";
import { VersionedEvent } from "../src/domain/Event";
import { Env, type Bindings } from "../src/Env";
import { ArticleNotifications } from "../src/services/ArticleNotifications";
import { TopicClient } from "../src/services/TopicClient";

const bindings: Bindings = { ...env, LAM_TOKEN: "test-token", LAM_HMAC_SECRET: "test-secret", NTFY_TOPIC: "test-topic" };
const topic = Effect.runSync(TopicClient.pipe(Effect.provide(TopicClient.Default)));
const deliver = (transport = topic) => Effect.runPromise(Effect.flatMap(ArticleNotifications, service => service.deliver()).pipe(
  Effect.provide(ArticleNotifications.DefaultWithoutDependencies), Effect.provideService(TopicClient, transport), Effect.provideService(Env, bindings)));

const messagesFor = async (id: string) => (await env.TOPIC.get(env.TOPIC.idFromName("test-topic")).poll("all"))
  .map(value => JSON.parse(value) as Message).filter(message => message.actions?.some(action => action.url === `lam://articles/${id}`));

const headers = { Authorization: "Bearer test-token" };
const request = (path: string, method = "GET", body?: unknown) => SELF.fetch(`https://lam${path}`, {
  method, headers: { ...headers, "Idempotency-Key": crypto.randomUUID() },
  body: body === undefined ? undefined : typeof body === "string" ? body : JSON.stringify(body),
});
async function stage(silent = false) {
  const html = "<!doctype html><p>Private report</p>";
  const bytes = new TextEncoder().encode(html);
  const sha256 = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)), b => b.toString(16).padStart(2, "0")).join("");
  const response = await request("/v2/articles/uploads", "POST", { title: "Report", summary: "Summary", name: "agent", source_host: "host", source_project: "lam", silent,
    assets: [{ path: "index.html", media_type: "text/html", size: bytes.length, sha256, disposition: "inline" }] });
  expect(response.status).toBe(201);
  const { id } = await response.json<{ id: string }>();
  expect((await request(`/v2/articles/${id}/assets/0`, "PUT", html)).status).toBe(204);
  return id;
}
async function device() {
  const id = crypto.randomUUID(), credential = `device-${crypto.randomUUID()}`;
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey("raw", enc.encode("test-secret"), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const hash = btoa(String.fromCharCode(...new Uint8Array(await crypto.subtle.sign("HMAC", key, enc.encode(credential))))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  await env.DB.prepare("INSERT INTO devices (id, name, credential_hash, app_version, android_version, created_at) VALUES (?, 'Phone', ?, 'test', 'test', ?)").bind(id, hash, new Date().toISOString()).run();
  return { id, credential };
}
async function socket(path: string, token: string) {
  const response = await SELF.fetch(`https://lam${path}`, { headers: { Authorization: `Bearer ${token}`, Upgrade: "websocket" } });
  expect(response.status).toBe(101);
  const ws = response.webSocket!;
  ws.accept();
  return ws;
}
function next(ws: WebSocket) {
  return new Promise<unknown>((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("event missing")), 1000);
    ws.addEventListener("message", event => { clearTimeout(timeout); resolve(JSON.parse(event.data as string)); }, { once: true });
  });
}
describe("article invalidation and quiet jobs", () => {
  it("rejects HTML, capabilities, fake items, and invalid versions at the event boundary", () => {
    const event = { event: "article.published", article_id: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", version: 0 };
    for (const invalid of [{ ...event, html: "private" }, { ...event, token: "secret" }, { ...event, version: -1 }, { ...event, version: "1" },
      { ...event, item_id: event.article_id }, { ...event, event: "article.deleted" }]) {
      expect(() => Schema.decodeUnknownSync(VersionedEvent)(invalid)).toThrow();
    }
  });
  it("authenticates v2 master/devices and rejects invalid or revoked credentials", async () => {
    const paired = await device();
    const ws = await socket("/v2/events", paired.credential); ws.close();
    const admin = await socket("/v2/events", "test-token"); admin.close();
    expect((await SELF.fetch("https://lam/v2/events", { headers: { Upgrade: "websocket" } })).status).toBe(401);
    expect((await request("/v2/events")).status).toBe(426);
    expect((await request(`/devices/${paired.id}`, "DELETE")).status).toBe(200);
    expect((await SELF.fetch("https://lam/v2/events", { headers: { Authorization: `Bearer ${paired.credential}`, Upgrade: "websocket" } })).status).toBe(401);
  });
  it("keeps legacy sockets item-only and invalidates both v2 clients after committed publish/read", async () => {
    const paired = await device();
    const legacy = await socket("/events", paired.credential);
    const one = await socket("/v2/events", paired.credential);
    const two = await socket("/v2/events", "test-token");
    const old: unknown[] = []; legacy.addEventListener("message", event => { old.push(JSON.parse(event.data as string)); });
    try {
      const id = await stage();
      let incoming = [next(one), next(two)];
      expect((await request(`/v2/articles/${id}/publish`, "POST")).status).toBe(200);
      expect(await Promise.all(incoming)).toEqual(Array(2).fill({ event: "article.published", article_id: id, version: 0 }));
      for (const [read, version] of [[true, 0], [false, 1]] as const) {
        incoming = [next(one), next(two)];
        expect((await request(`/v2/articles/${id}/read`, "PUT", { read, version })).status).toBe(200);
        expect(await Promise.all(incoming)).toEqual(Array(2).fill({ event: "article.read_changed", article_id: id, version: version + 1 }));
      }
      expect(old).toEqual([]);
      const item = { event: "item.changed" as const, item_id: "item", version: 2, status: "open" as const };
      incoming = [next(legacy), next(one), next(two)];
      await env.EVENTS.get(env.EVENTS.idFromName("global")).publish(item);
      expect(await Promise.all(incoming)).toEqual(Array(3).fill(item));
      expect(await env.DB.prepare("SELECT count(*) AS n FROM article_notification_jobs WHERE article_id = ?").bind(id).first()).toEqual({ n: 1 });
    } finally { legacy.close(); one.close(); two.close(); }
  });
  it("runs local scheduled delivery and cleanup, with one quiet view-only job and no silent/read jobs", async () => {
    const id = await stage(); const silent = await stage(true); const abandoned = await stage();
    for (const article of [id, silent]) expect((await request(`/v2/articles/${article}/publish`, "POST")).status).toBe(200);
    await env.DB.prepare("UPDATE articles SET expires_at = '2000-01-01T00:00:00.000Z' WHERE id = ?").bind(abandoned).run();
    const ctx = createExecutionContext();
    await worker.scheduled(createScheduledController(), bindings, ctx);
    await waitOnExecutionContext(ctx);
    const messages = await messagesFor(id);
    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({ title: "Report", message: "Summary", priority: 2,
      actions: [{ action: "view", url: `lam://articles/${id}`, clear: false }] });
    expect(messages[0].actions).toHaveLength(1);
    expect(await env.DB.prepare("SELECT state, attempts FROM article_notification_jobs WHERE article_id = ?").bind(id).first()).toEqual({ state: "delivered", attempts: 1 });
    expect(await env.DB.prepare("SELECT count(*) AS n FROM article_notification_jobs WHERE article_id = ?").bind(silent).first()).toEqual({ n: 0 });
    expect(await env.ARTICLE_BUCKET.head(`articles/${abandoned}/original/0`)).toBeNull();
    expect(await env.ARTICLE_BUCKET.head(`articles/${id}/sanitized/index.html`)).not.toBeNull();
    expect((await request(`/v2/articles/${id}/read`, "PUT", { read: true, version: 0 })).status).toBe(200);
    const again = createExecutionContext(); await worker.scheduled(createScheduledController(), bindings, again); await waitOnExecutionContext(again);
    expect(await messagesFor(id)).toHaveLength(1);
    expect(await messagesFor(silent)).toHaveLength(0);
  });
  it("emits silent publication but no invalidation for read no-op or rejected conflict", async () => {
    const ws = await socket("/v2/events", "test-token");
    const frames: unknown[] = [];
    ws.addEventListener("message", event => { frames.push(JSON.parse(event.data as string)); });
    try {
      const id = await stage(true);
      let received = next(ws);
      await request(`/v2/articles/${id}/publish`, "POST"); await received;
      received = next(ws);
      await request(`/v2/articles/${id}/read`, "PUT", { read: true, version: 0 }); await received;
      expect((await request(`/v2/articles/${id}/read`, "PUT", { read: true, version: 0 })).status).toBe(200);
      expect((await request(`/v2/articles/${id}/read`, "PUT", { read: false, version: 0 })).status).toBe(409);
      received = next(ws);
      await env.EVENTS.get(env.EVENTS.idFromName("global")).publish({ event: "item.changed", item_id: "barrier", version: 1, status: "open" });
      await received;
      expect(frames).toEqual([{ event: "article.published", article_id: id, version: 0 }, { event: "article.read_changed", article_id: id, version: 1 },
        { event: "item.changed", item_id: "barrier", version: 1, status: "open" }]);
      expect(await env.DB.prepare("SELECT count(*) AS n FROM article_notification_jobs WHERE article_id = ?").bind(id).first()).toEqual({ n: 0 });
    } finally { ws.close(); }
  });
  it("excludes live leases, retries uncertain delivery with backoff, and never creates a second logical job", async () => {
    const id = await stage(); await request(`/v2/articles/${id}/publish`, "POST");
    const uncertain = TopicClient.make({ ...topic, publish: draft => topic.publish(draft).pipe(Effect.andThen(Effect.die("simulated lost acknowledgement"))) });
    await deliver(uncertain);
    expect(await messagesFor(id)).toHaveLength(1);
    const retry = await env.DB.prepare("SELECT state, attempts, next_attempt_at FROM article_notification_jobs WHERE article_id = ?").bind(id).first<{ state: string; attempts: number; next_attempt_at: string }>();
    expect(retry).toMatchObject({ state: "pending", attempts: 1 });
    expect(Date.parse(retry!.next_attempt_at)).toBeGreaterThan(Date.now());
    await deliver(); expect(await messagesFor(id)).toHaveLength(1);
    await env.DB.prepare("UPDATE article_notification_jobs SET state = 'sending', lease_token = 'old', lease_until = '2999-01-01T00:00:00.000Z' WHERE article_id = ?").bind(id).run();
    await deliver(); expect(await messagesFor(id)).toHaveLength(1);
    await env.DB.prepare("UPDATE article_notification_jobs SET lease_until = '2000-01-01T00:00:00.000Z' WHERE article_id = ?").bind(id).run();
    await Promise.all([deliver(), deliver()]);
    expect(await messagesFor(id)).toHaveLength(2);
    expect(await env.DB.prepare("SELECT state, attempts FROM article_notification_jobs WHERE article_id = ?").bind(id).first()).toEqual({ state: "delivered", attempts: 2 });
  });
  it("fences a late sender from completing a replacement lease", async () => {
    const id = await stage(); await request(`/v2/articles/${id}/publish`, "POST");
    let release!: () => void; let started!: () => void;
    const held = new Promise<void>(resolve => { release = resolve; });
    const sending = new Promise<void>(resolve => { started = resolve; });
    const blocked = TopicClient.make({ ...topic, publish: draft => Effect.promise(async () => { started(); await held; }).pipe(Effect.andThen(topic.publish(draft))) });
    const first = deliver(blocked); await sending;
    await deliver(); expect(await messagesFor(id)).toHaveLength(0);
    await env.DB.prepare("UPDATE article_notification_jobs SET lease_token = 'replacement', lease_until = '2999-01-01T00:00:00.000Z' WHERE article_id = ?").bind(id).run();
    release(); await first;
    expect(await messagesFor(id)).toHaveLength(1);
    expect(await env.DB.prepare("SELECT state, lease_token FROM article_notification_jobs WHERE article_id = ?").bind(id).first()).toEqual({ state: "sending", lease_token: "replacement" });
  });
});
