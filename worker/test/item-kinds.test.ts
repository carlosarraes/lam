import type { D1Migration } from "@cloudflare/vitest-pool-workers";
import { applyD1Migrations, env, SELF } from "cloudflare:test";
import { Effect } from "effect";
import { describe, expect, it } from "vitest";
import { Env } from "../src/Env";
import { Items } from "../src/services/Items";

const AUTH = { Authorization: "Bearer test-token" };
const typedJson = (body: unknown) => ({
  method: "POST",
  headers: { ...AUTH, "Content-Type": "application/json" },
  body: JSON.stringify(body),
});
let sequence = 0;
const migrations = (env as unknown as { TEST_MIGRATIONS: D1Migration[] }).TEST_MIGRATIONS;
const migrationTables = ["article_view_sessions", "article_notification_jobs", "article_assets", "articles", "pairing_sessions", "devices", "items", "d1_migrations"];

async function dropMigrationTables(): Promise<void> {
  for (const table of migrationTables) await env.DB.prepare(`DROP TABLE IF EXISTS ${table}`).run();
}

async function restoreMigrations(): Promise<void> {
  await dropMigrationTables();
  await applyD1Migrations(env.DB, migrations);
}

async function createTyped(body: Record<string, unknown>) {
  const response = await SELF.fetch("http://lam/v2/items", typedJson({ ...body, title: `${body.title} #${++sequence}` }));
  expect(response.status).toBe(201);
  return response.json<any>();
}

async function createFyi(body: Record<string, unknown> = {}) {
  return createTyped({ kind: "fyi", title: "FYI", ...body });
}

async function itemToken(id: string): Promise<string> {
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode("test-secret"),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const signature = new Uint8Array(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(id)));
  return btoa(String.fromCharCode(...signature)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

async function registerDevice() {
  const credential = `fyi-device-${++sequence}`;
  const credentialHash = await itemToken(credential);
  await env.DB.prepare(
    `INSERT INTO devices (id, name, credential_hash, fcm_token, app_version, android_version, created_at)
     VALUES (?, ?, ?, NULL, ?, ?, ?)`,
  ).bind(`fyi-device-id-${sequence}`, "FYI test device", credentialHash, "test", "test", new Date().toISOString()).run();
  return { Authorization: `Bearer ${credential}` };
}

async function connectEvents(): Promise<WebSocket> {
  const authorization = await registerDevice();
  const response = await SELF.fetch("http://lam/events", { headers: { ...authorization, Upgrade: "websocket" } });
  expect(response.status).toBe(101);
  const socket = response.webSocket!;
  socket.accept();
  return socket;
}

function nextEvent(socket: WebSocket): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("item event timed out")), 1_000);
    socket.addEventListener("message", (message) => {
      clearTimeout(timer);
      resolve(JSON.parse(message.data as string));
    }, { once: true });
  });
}

function eventWithin(socket: WebSocket, milliseconds: number): Promise<Record<string, unknown> | null> {
  return new Promise((resolve) => {
    const onMessage = (message: MessageEvent) => {
      clearTimeout(timer);
      resolve(JSON.parse(message.data as string));
    };
    const timer = setTimeout(() => {
      socket.removeEventListener("message", onMessage);
      resolve(null);
    }, milliseconds);
    socket.addEventListener("message", onMessage, { once: true });
  });
}

function expireAfterFirstRead(statement: D1PreparedStatement, id: string): D1PreparedStatement {
  return new Proxy(statement, {
    get(target, property) {
      if (property === "bind") {
        return (...values: unknown[]) => expireAfterFirstRead(target.bind(...values), id);
      }
      if (property === "first") {
        return async <T>() => {
          const row = await target.first<T>();
          await env.DB.prepare("UPDATE items SET expires_at = ? WHERE id = ?")
            .bind("2000-01-01T00:00:00.000Z", id)
            .run();
          return row;
        };
      }
      const value = Reflect.get(target, property, target);
      return typeof value === "function" ? value.bind(target) : value;
    },
  });
}

describe("typed item creation", () => {
  it("creates an FYI without decision fields", async () => {
    const response = await SELF.fetch("http://lam/v2/items", typedJson({ kind: "fyi", title: "Backup finished" }));

    expect(response.status).toBe(201);
    const item = await response.json<{ kind: string; recommendation: string | null }>();
    expect(item.kind).toBe("fyi");
    expect(item.recommendation).toBeNull();
  });

  it("creates a typed decision request with its exact preferred choice", async () => {
    const response = await SELF.fetch("http://lam/v2/items", typedJson({
      kind: "request",
      title: "Release now?",
      choices: ["ship", "hold"],
      recommendation: "Ship after the smoke test.",
      recommended_choice: "ship",
    }));

    expect(response.status).toBe(201);
    expect(await response.json()).toMatchObject({
      kind: "request",
      recommendation: "Ship after the smoke test.",
      recommended_choice: "ship",
    });
  });

  it.each([
    ["missing recommendation", { kind: "request", title: "Decide" }],
    ["blank recommendation", { kind: "request", title: "Decide", recommendation: " \t\n " }],
    ["missing preferred choice", { kind: "request", title: "Decide", choices: ["a", "b"], recommendation: "Pick a." }],
    ["mismatched preferred choice", { kind: "request", title: "Decide", choices: ["a", "b"], recommendation: "Pick a.", recommended_choice: "A" }],
    ["checklist recommendation", { kind: "request", title: "Run checks", checks: ["tests"], recommendation: "Run tests." }],
    ["legacy low request priority", { kind: "request", title: "Low", priority: "low", recommendation: "Proceed." }],
  ])("rejects typed request with %s", async (_case, body) => {
    expect((await SELF.fetch("http://lam/v2/items", typedJson(body))).status).toBe(400);
  });

  it("keeps the checklist recommendation exemption", async () => {
    const response = await SELF.fetch("http://lam/v2/items", typedJson({
      kind: "request",
      title: "Run checks",
      checks: ["tests", "lint"],
      priority: "critical",
    }));

    expect(response.status).toBe(201);
    expect(await response.json()).toMatchObject({
      kind: "request",
      checks: [
        { label: "tests", done: false, at: null },
        { label: "lint", done: false, at: null },
      ],
      recommendation: null,
    });
  });

  it.each(["choices", "checks", "recommendation", "recommended_choice"])(
    "rejects FYI decision field %s even when empty or null",
    async (field) => {
      const value = field === "choices" || field === "checks" ? [] : null;
      const response = await SELF.fetch("http://lam/v2/items", typedJson({ kind: "fyi", title: "FYI", [field]: value }));
      expect(response.status).toBe(400);
    },
  );

  it("requires explicit kind and allows low urgency only for FYIs", async () => {
    expect((await SELF.fetch("http://lam/v2/items", typedJson({ title: "Missing kind" }))).status).toBe(400);

    const response = await SELF.fetch("http://lam/v2/items", typedJson({ kind: "fyi", title: "Low FYI", priority: "low" }));
    expect(response.status).toBe(201);
    expect(await response.json()).toMatchObject({ kind: "fyi", priority: "low" });
  });

  it("keeps legacy low-priority creation request-only", async () => {
    const response = await SELF.fetch("http://lam/items", typedJson({ title: `Legacy low #${++sequence}`, priority: "low" }));
    expect(response.status).toBe(201);
    const item = await response.json<any>();
    expect(item).toMatchObject({ kind: "request", priority: "low", recommendation: null, seen_at: null });

    const stored = await env.DB.prepare("SELECT kind, priority, seen_at FROM items WHERE id = ?").bind(item.id).first();
    expect(stored).toEqual({ kind: "request", priority: "low", seen_at: null });
  });

  it("does not return a weak pre-dedupe match for typed request identity", async () => {
    const name = `typed-dedupe-${++sequence}`;
    const title = `Canonical request #${sequence}`;
    await env.DB.prepare(
      `INSERT INTO items
        (id, kind, name, title, body, source_host, source_project, priority, choices, checks, link, status,
         response_choice, response_text, response_by, created_at, resolved_at, seen_at, expires_at,
         recommendation, recommended_choice, version, dedupe_key)
       VALUES (?, 'request', ?, ?, ?, '', '', 'low', '[]', '[]', '', 'open',
         NULL, NULL, NULL, ?, NULL, NULL, NULL, NULL, NULL, 0, NULL)`,
    ).bind("weak-legacy-match", name, title, "same body", "2026-01-01T00:00:00.000Z").run();

    const response = await SELF.fetch("http://lam/v2/items", typedJson({
      kind: "request",
      name,
      title,
      body: "same body",
      priority: "critical",
      choices: ["ship", "hold"],
      recommendation: "Ship after validation.",
      recommended_choice: "ship",
    }));

    expect(response.status).toBe(201);
    const created = await response.json<any>();
    expect(created).toMatchObject({
      kind: "request",
      priority: "critical",
      choices: ["ship", "hold"],
      recommendation: "Ship after validation.",
      recommended_choice: "ship",
    });
    expect(created.id).not.toBe("weak-legacy-match");
    expect((await env.DB.prepare("SELECT COUNT(*) AS count FROM items WHERE name = ?").bind(name).first<{ count: number }>())?.count).toBe(2);
  });
});

describe("FYI lifecycle", () => {
  it("marks an FYI seen with one atomic close and no fabricated response", async () => {
    const fyi = await createFyi();
    const response = await SELF.fetch(`http://lam/v2/items/${fyi.id}/seen`, typedJson({ version: fyi.version }));

    expect(response.status).toBe(200);
    const seen = await response.json<any>();
    expect(seen).toMatchObject({
      id: fyi.id,
      kind: "fyi",
      status: "dismissed",
      version: 1,
      response_choice: null,
      response_text: null,
      response_by: null,
    });
    expect(seen.seen_at).toEqual(expect.any(String));
    expect(seen.resolved_at).toBe(seen.seen_at);

    const stored = await env.DB.prepare(
      "SELECT kind, status, version, seen_at, resolved_at, response_choice, response_text, response_by FROM items WHERE id = ?",
    ).bind(fyi.id).first();
    expect(stored).toEqual({
      kind: "fyi",
      status: "dismissed",
      version: 1,
      seen_at: seen.seen_at,
      resolved_at: seen.seen_at,
      response_choice: null,
      response_text: null,
      response_by: null,
    });
  });

  it("rejects a stale seen write without changing the FYI", async () => {
    const fyi = await createFyi();
    const response = await SELF.fetch(`http://lam/v2/items/${fyi.id}/seen`, typedJson({ version: fyi.version + 1 }));

    expect(response.status).toBe(409);
    const stored = await env.DB.prepare("SELECT status, version, seen_at, resolved_at FROM items WHERE id = ?").bind(fyi.id).first();
    expect(stored).toEqual({ status: "open", version: 0, seen_at: null, resolved_at: null });
  });

  it("rejects seen when the FYI expires after its read but before its write", async () => {
    const fyi = await createFyi({ ttl: 60 });
    const delayedDb = new Proxy(env.DB, {
      get(target, property) {
        if (property === "prepare") {
          return (query: string) => {
            const statement = target.prepare(query);
            return query === "SELECT * FROM items WHERE id = ?"
              ? expireAfterFirstRead(statement, fyi.id)
              : statement;
          };
        }
        const value = Reflect.get(target, property, target);
        return typeof value === "function" ? value.bind(target) : value;
      },
    });

    const result = await Effect.runPromise(
      Effect.flatMap(Items, (items) => items.seen(fyi.id, 0)).pipe(
        Effect.provide(Items.Default),
        Effect.provideService(Env, {
          ...env,
          DB: delayedDb,
          LAM_TOKEN: "test-token",
          LAM_HMAC_SECRET: "test-secret",
          NTFY_TOPIC: "test-topic",
        }),
        Effect.either,
      ),
    );

    expect(result).toMatchObject({ _tag: "Left", left: { _tag: "Conflict", id: fyi.id } });
    const stored = await env.DB.prepare("SELECT status, version, seen_at, resolved_at FROM items WHERE id = ?").bind(fyi.id).first();
    expect(stored).toEqual({ status: "open", version: 0, seen_at: null, resolved_at: null });
  });

  it("returns the first seen result idempotently without another version bump", async () => {
    const fyi = await createFyi();
    const first = await SELF.fetch(`http://lam/v2/items/${fyi.id}/seen`, typedJson({ version: fyi.version }));
    expect(first.status).toBe(200);
    const firstSeen = await first.json<any>();

    const duplicate = await SELF.fetch(`http://lam/v2/items/${fyi.id}/seen`, typedJson({ version: fyi.version }));
    expect(duplicate.status).toBe(200);
    const duplicateSeen = await duplicate.json<any>();
    expect(duplicateSeen).toMatchObject({ version: 1, seen_at: firstSeen.seen_at, resolved_at: firstSeen.resolved_at });
  });

  it("rejects request targets and unauthenticated seen calls", async () => {
    const request = await createTyped({ kind: "request", title: "Decide", recommendation: "Proceed." });
    expect((await SELF.fetch(`http://lam/v2/items/${request.id}/seen`, typedJson({ version: 0 }))).status).toBe(400);

    const fyi = await createFyi();
    expect((await SELF.fetch(`http://lam/v2/items/${fyi.id}/seen`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ version: 0 }),
    })).status).toBe(401);
  });

  it("rejects typed creation from a paired device but permits it to mark an FYI seen", async () => {
    const deviceAuth = await registerDevice();
    const creation = await SELF.fetch("http://lam/v2/items", {
      method: "POST",
      headers: { ...deviceAuth, "Content-Type": "application/json" },
      body: JSON.stringify({ kind: "fyi", title: "Device cannot create" }),
    });
    expect(creation.status).toBe(403);

    const fyi = await createFyi();
    const seen = await SELF.fetch(`http://lam/v2/items/${fyi.id}/seen`, {
      method: "POST",
      headers: { ...deviceAuth, "Content-Type": "application/json" },
      body: JSON.stringify({ version: 0 }),
    });
    expect(seen.status).toBe(200);
    expect(await seen.json()).toMatchObject({ id: fyi.id, kind: "fyi", status: "dismissed", version: 1 });
  });

  it("keeps explicitly dismissed and expired FYIs unseen and at their prior version", async () => {
    const dismissed = await createFyi();
    const dismissResponse = await SELF.fetch(`http://lam/items/${dismissed.id}/dismiss`, { method: "POST", headers: AUTH });
    expect(dismissResponse.status).toBe(200);
    expect(await dismissResponse.json()).toMatchObject({ status: "dismissed", version: 1, seen_at: null });
    expect((await SELF.fetch(`http://lam/v2/items/${dismissed.id}/seen`, typedJson({ version: 1 }))).status).toBe(409);

    const expired = await createFyi();
    await env.DB.prepare("UPDATE items SET expires_at = ? WHERE id = ?").bind("2000-01-01T00:00:00.000Z", expired.id).run();
    expect((await SELF.fetch(`http://lam/v2/items/${expired.id}/seen`, typedJson({ version: 0 }))).status).toBe(409);
    const stored = await env.DB.prepare("SELECT status, version, seen_at FROM items WHERE id = ?").bind(expired.id).first();
    expect(stored).toEqual({ status: "open", version: 0, seen_at: null });
  });

  it("rejects every answering and check mutation for an FYI while preserving explicit dismiss", async () => {
    const endpoints: Array<(id: string, token: string) => Promise<Response>> = [
      (id) => SELF.fetch(`http://lam/items/${id}/resolve`, typedJson({ text: "answer" })),
      (id) => SELF.fetch(`http://lam/items/${id}/checks`, typedJson({ label: "check" })),
      (id) => SELF.fetch(`http://lam/items/${id}/checks/0`, typedJson({ done: true })),
      (id, token) => SELF.fetch(`http://lam/a/${id}/Done?t=${token}`, { method: "POST" }),
      (id, token) => SELF.fetch(`http://lam/r/${id}?t=${token}`, { method: "POST", body: new URLSearchParams({ text: "answer" }) }),
      (id, token) => SELF.fetch(`http://lam/r/${id}/checks/0?t=${token}`, { method: "POST", body: new URLSearchParams({ done: "true" }) }),
    ];

    for (const call of endpoints) {
      const fyi = await createFyi();
      expect((await call(fyi.id, await itemToken(fyi.id))).status).toBe(400);
      const stored = await env.DB.prepare("SELECT status, version, seen_at FROM items WHERE id = ?").bind(fyi.id).first();
      expect(stored).toEqual({ status: "open", version: 0, seen_at: null });
    }

    const dismissible = await createFyi();
    const response = await SELF.fetch(`http://lam/items/${dismissible.id}/dismiss`, { method: "POST", headers: AUTH });
    expect(response.status).toBe(200);
    expect(await response.json()).toMatchObject({ status: "dismissed", version: 1, seen_at: null });
  });

  it("rejects an explicit FYI wait and excludes closed FYIs from wait-any selection", async () => {
    const request = await createTyped({ kind: "request", title: "Decide", recommendation: "Proceed." });
    await SELF.fetch(`http://lam/items/${request.id}/resolve`, typedJson({ text: "done" }));

    const fyi = await createFyi();
    await env.DB.prepare("UPDATE items SET created_at = ? WHERE id = ?").bind("2999-01-01T00:00:00.000Z", fyi.id).run();
    await SELF.fetch(`http://lam/v2/items/${fyi.id}/seen`, typedJson({ version: 0 }));

    expect((await SELF.fetch(`http://lam/items/${fyi.id}/wait`, { headers: AUTH })).status).toBe(400);
    const waitAny = await SELF.fetch(`http://lam/items/wait?ids=${fyi.id},${request.id}`, { headers: AUTH });
    expect(waitAny.status).toBe(200);
    expect(await waitAny.json()).toMatchObject({ id: request.id, kind: "request", status: "resolved" });
  });

  it("returns immediately when wait-any contains only FYIs", async () => {
    const fyi = await createFyi();
    const response = await SELF.fetch(`http://lam/items/wait?ids=${fyi.id}`, { headers: AUTH });
    expect(response.status).toBe(204);
  });

  it("publishes canonical invalidations after typed creation and seen persistence", async () => {
    const socket = await connectEvents();
    try {
      const createdFrame = nextEvent(socket);
      const fyi = await createFyi();
      expect(await createdFrame).toEqual({ event: "item.created", item_id: fyi.id, version: 0, status: "open" });

      const closedFrame = nextEvent(socket);
      const seenResponse = await SELF.fetch(`http://lam/v2/items/${fyi.id}/seen`, typedJson({ version: 0 }));
      expect(seenResponse.status).toBe(200);
      const seen = await seenResponse.json<any>();
      expect(await closedFrame).toEqual({ event: "item.closed", item_id: fyi.id, version: 1, status: "dismissed" });
      expect((await env.DB.prepare("SELECT status, version, seen_at FROM items WHERE id = ?").bind(fyi.id).first())).toEqual({
        status: "dismissed",
        version: 1,
        seen_at: seen.seen_at,
      });

      const duplicateFrame = eventWithin(socket, 100);
      const duplicate = await SELF.fetch(`http://lam/v2/items/${fyi.id}/seen`, typedJson({ version: 0 }));
      expect(duplicate.status).toBe(200);
      expect(await duplicateFrame).toBeNull();
    } finally {
      socket.close();
    }
  });
});

describe("item kind migration", () => {
  it("backfills pre-0009 rows as unseen requests without changing legacy outcomes", async () => {
    const migrationIndex = migrations.findIndex(migration => migration.name === "0009_item_kinds.sql");
    expect(migrationIndex).toBeGreaterThanOrEqual(0);
    try {
      await dropMigrationTables();
      await applyD1Migrations(env.DB, migrations.slice(0, migrationIndex));
      await env.DB.prepare(
        `INSERT INTO items
          (id, name, title, body, source_host, source_project, priority, choices, checks, link, status,
           response_choice, response_text, response_by, created_at, resolved_at, expires_at,
           recommendation, recommended_choice, version, dedupe_key)
         VALUES (?, ?, ?, '', '', '', 'low', '[]', '[]', '', 'resolved', ?, NULL, 'cli', ?, ?, NULL, NULL, NULL, 3, NULL)`,
      ).bind(
        "pre-0009-request",
        "legacy:agent",
        "Legacy outcome",
        "keep",
        "2026-01-01T00:00:00.000Z",
        "2026-01-01T00:01:00.000Z",
      ).run();

      await applyD1Migrations(env.DB, migrations.slice(migrationIndex));

      const response = await SELF.fetch("http://lam/items/pre-0009-request", { headers: AUTH });
      expect(response.status).toBe(200);
      expect(await response.json()).toMatchObject({
        id: "pre-0009-request",
        kind: "request",
        priority: "low",
        status: "resolved",
        response_choice: "keep",
        response_by: "cli",
        version: 3,
        seen_at: null,
      });
    } finally {
      await restoreMigrations();
    }
  });
});
