import { env, SELF } from "cloudflare:test";
import { Effect } from "effect";
import { describe, expect, it } from "vitest";
import { Env, type Bindings } from "../src/Env";
import { Articles } from "../src/services/Articles";

const enc = new TextEncoder();
const bindings: Bindings = { ...env, LAM_TOKEN: "test-token", LAM_HMAC_SECRET: "test-secret", NTFY_TOPIC: "test-topic" };
const run = <A, E>(effect: Effect.Effect<A, E, Articles | Env>) => Effect.runPromise(effect.pipe(Effect.provide(Articles.Default), Effect.provideService(Env, bindings)));
const html = "<!doctype html><html><body>Report</body></html>";
async function draft() {
  const bytes = enc.encode(html);
  const sha256 = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)), b => b.toString(16).padStart(2, "0")).join("");
  return { title: "Report", summary: "Summary", name: "", source_host: "host", source_project: "lam", silent: false,
    assets: [{ path: "index.html", media_type: "text/html", size: bytes.length, sha256, disposition: "inline" }] };
}
const request = (path: string, method = "GET", body?: unknown, token = "test-token", headers: Record<string, string> = {}) =>
  SELF.fetch(`https://example.com/v2/articles${path}`, { method, headers: { Authorization: `Bearer ${token}`, ...headers }, body: body === undefined ? undefined : typeof body === "string" ? body : JSON.stringify(body) });
async function stage(payload?: Awaited<ReturnType<typeof draft>>, key = crypto.randomUUID()) {
  const response = await request("/uploads", "POST", payload ?? await draft(), "test-token", { "Idempotency-Key": key });
  expect(response.status).toBe(201);
  return await response.json<{ id: string }>();
}
async function stageBundle(sources: { path: string; media_type: string; disposition: string; bytes: Uint8Array<ArrayBuffer> }[]) {
  const payload = await draft();
  payload.assets = await Promise.all(sources.map(async ({ bytes, ...asset }) => ({ ...asset, size: bytes.byteLength,
    sha256: Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)), b => b.toString(16).padStart(2, "0")).join("") })));
  const { id } = await stage(payload);
  for (let index = 0; index < sources.length; index++) {
    const response = await SELF.fetch(`https://example.com/v2/articles/${id}/assets/${index}`, { method: "PUT", headers: { Authorization: "Bearer test-token" }, body: sources[index].bytes });
    expect(response.status).toBe(204);
  }
  return id;
}
async function device() {
  const credential = `article-device-${crypto.randomUUID()}`;
  const key = await crypto.subtle.importKey("raw", enc.encode("test-secret"), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const signature = new Uint8Array(await crypto.subtle.sign("HMAC", key, enc.encode(credential)));
  const hash = btoa(String.fromCharCode(...signature)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  await env.DB.prepare("INSERT INTO devices (id, name, credential_hash, app_version, android_version, created_at) VALUES (?, 'Phone', ?, 'test', 'test', ?)").bind(crypto.randomUUID(), hash, new Date().toISOString()).run();
  return credential;
}

// Published fixtures bypass no production route. They represent Task 2's persisted result.
async function published(title = "Report", created = "2026-09-01T12:00:00.000Z", name = "") {
  const { id } = await stage({ ...await draft(), title, name });
  expect((await request(`/${id}/assets/0`, "PUT", html)).status).toBe(204);
  const row = await env.DB.prepare("SELECT sanitized_html_key FROM articles WHERE id = ?").bind(id).first<{ sanitized_html_key: string }>();
  await env.ARTICLE_BUCKET.put(row!.sanitized_html_key, "<!doctype html><html><body>Sanitized report</body></html>");
  await env.DB.prepare("UPDATE articles SET state = 'published', created_at = ? WHERE id = ?").bind(created, id).run();
  return id;
}

describe("private article staging", () => {
  it("stages idempotently, hides drafts, and publishes validated canonical HTML after upload", async () => {
    const payload = await draft();
    const key = crypto.randomUUID();
    const { id } = await stage(payload, key);
    expect(await stage(payload, key)).toEqual({ id });
    expect((await request("/uploads", "POST", { ...payload, title: "Different" }, "test-token", { "Idempotency-Key": key })).status).toBe(409);
    const listed = await (await request("")).json<{ items: { id: string }[] }>();
    expect(listed.items.some(a => a.id === id)).toBe(false);
    expect((await request(`/${id}`)).status).toBe(404);
    expect((await request(`/${id}/assets/0`)).status).toBe(404);
    expect((await request(`/${id}/publish`, "POST")).status).toBe(400);
    expect((await request(`/${id}/assets/0`, "PUT", html)).status).toBe(204);
    expect((await request(`/${id}/assets/0`, "PUT", html)).status).toBe(204);
    const response = await request(`/${id}/publish`, "POST");
    expect(response.status).toBe(200);
    expect(await response.json()).toMatchObject({ id, assets: payload.assets });
    expect((await request(`/${id}`)).status).toBe(200);
    const canonical = await request(`/${id}/assets/0`);
    const content = await canonical.text();
    expect(content).toBe("<!DOCTYPE html><html><head></head><body>Report</body></html>");
    expect(canonical.headers.get("content-length")).toBe(String(enc.encode(content).length));
    const stored = await env.ARTICLE_BUCKET.get(`articles/${id}/sanitized/index.html`);
    expect(stored!.customMetadata?.sha256).not.toBe(payload.assets[0].sha256);
    expect(await (await env.ARTICLE_BUCKET.get(`articles/${id}/original/0`))!.text()).toBe(html);
    expect((await request(`/${id}/publish`, "POST")).status).toBe(200);
    expect(await env.DB.prepare("SELECT count(*) AS count FROM article_notification_jobs WHERE article_id = ?").bind(id).first()).toEqual({ count: 1 });
  });

  it("publishes competing calls once and leaves silent publications without notification jobs", async () => {
    for (const silent of [false, true]) {
      const { id } = await stage({ ...await draft(), silent });
      expect((await request(`/${id}/assets/0`, "PUT", html)).status).toBe(204);
      const responses = await Promise.all(Array.from({ length: 3 }, () => request(`/${id}/publish`, "POST")));
      expect(responses.map(response => response.status)).toEqual([200, 200, 200]);
      const articles = await Promise.all(responses.map(response => response.json()));
      expect(articles[0]).toEqual(articles[1]);
      expect(articles[1]).toEqual(articles[2]);
      const object = await env.ARTICLE_BUCKET.head(`articles/${id}/sanitized/index.html`);
      expect(object).not.toBeNull();
      expect(await env.DB.prepare("SELECT count(*) AS count FROM article_notification_jobs WHERE article_id = ?").bind(id).first()).toEqual({ count: silent ? 0 : 1 });
    }
  });

  it("keeps malicious uploaded HTML private without canonical objects or notifications", async () => {
    const source = '<script>alert(1)</script>';
    const payload = await draft();
    const hash = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", enc.encode(source))), b => b.toString(16).padStart(2, "0")).join("");
    const { id } = await stage({ ...payload, assets: [{ ...payload.assets[0], size: enc.encode(source).length, sha256: hash }] });
    expect((await request(`/${id}/assets/0`, "PUT", source)).status).toBe(204);
    const response = await request(`/${id}/publish`, "POST");
    expect(response.status).toBe(400);
    expect(await response.json()).toMatchObject({ error: expect.stringContaining("script") });
    expect((await request(`/${id}`)).status).toBe(404);
    expect(await env.ARTICLE_BUCKET.head(`articles/${id}/sanitized/index.html`)).toBeNull();
    expect(await env.DB.prepare("SELECT count(*) AS count FROM article_notification_jobs WHERE article_id = ?").bind(id).first()).toEqual({ count: 0 });
  });

  it("checks attachment byte formats before publishing and rejects invalid UTF-8 HTML", async () => {
    for (const media_type of ["image/png", "image/jpeg", "image/webp", "application/pdf"]) {
      const id = await stageBundle([
        { path: "index.html", media_type: "text/html", disposition: "inline", bytes: enc.encode(html) },
        { path: "fake", media_type, disposition: "attachment", bytes: enc.encode("<script>alert(1)</script>") },
      ]);
      const response = await request(`/${id}/publish`, "POST");
      expect(response.status).toBe(400);
      expect(await response.json()).toMatchObject({ error: expect.stringContaining("format") });
      expect((await request(`/${id}`)).status).toBe(404);
    }
    const id = await stageBundle([{ path: "index.html", media_type: "text/html", disposition: "inline", bytes: new Uint8Array([0xff, 0xfe, 0x61]) }]);
    expect((await request(`/${id}/publish`, "POST")).status).toBe(400);
  });

  it("publishes bundled raster images and inert PDF and text download labels", async () => {
    const source = '<img src="chart.png"><a href="notes.txt" class="notes" style="color: blue">Notes</a><a href="report.pdf" download>Report PDF</a>';
    const png = Uint8Array.from(atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aTHsAAAAASUVORK5CYII="), c => c.charCodeAt(0));
    const id = await stageBundle([
      { path: "index.html", media_type: "text/html", disposition: "inline", bytes: enc.encode(source) },
      { path: "chart.png", media_type: "image/png", disposition: "inline", bytes: png },
      { path: "notes.txt", media_type: "text/plain", disposition: "attachment", bytes: enc.encode("Notes\n") },
      { path: "report.pdf", media_type: "application/pdf", disposition: "attachment", bytes: enc.encode("%PDF-1.7\n1 0 obj<</Type/Catalog>>endobj\n%%EOF\n") },
    ]);
    expect((await request(`/${id}/publish`, "POST")).status).toBe(200);
    const canonical = await (await request(`/${id}/assets/0`)).text();
    expect(canonical).toContain('<img src="lam-asset:1">');
    expect(canonical).toContain('<span class="notes" style="color:blue" data-lam-attachment="2">Notes</span>');
    expect(canonical).toContain('<span data-lam-attachment="3">Report PDF</span>');
    const attachment = await request(`/${id}/assets/3`);
    expect(attachment.headers.get("content-disposition")).toContain("attachment;");
    expect(attachment.headers.get("content-security-policy")).toContain("sandbox");
  });

  it("verifies stored original bytes rather than trusting R2 custom metadata", async () => {
    const { id } = await stage();
    expect((await request(`/${id}/assets/0`, "PUT", html)).status).toBe(204);
    const payload = await draft();
    await env.ARTICLE_BUCKET.put(`articles/${id}/original/0`, html.replace("Report", "Broken"), { customMetadata: { sha256: payload.assets[0].sha256 } });
    expect((await request(`/${id}/publish`, "POST")).status).toBe(400);
    expect((await request(`/${id}`)).status).toBe(404);
  });

  it("never overwrites a conflicting canonical object", async () => {
    const { id } = await stage();
    expect((await request(`/${id}/assets/0`, "PUT", html)).status).toBe(204);
    const key = `articles/${id}/sanitized/index.html`;
    await env.ARTICLE_BUCKET.put(key, "conflicting canonical bytes");
    expect((await request(`/${id}/publish`, "POST")).status).toBe(409);
    expect(await (await env.ARTICLE_BUCKET.get(key))!.text()).toBe("conflicting canonical bytes");
    expect((await request(`/${id}`)).status).toBe(404);
  });

  it("does not publish an abandoned or expired stage after a late canonical write", async () => {
    const service = await run(Articles);
    for (const cleanup of [false, true]) {
      const { id } = await stage();
      expect((await request(`/${id}/assets/0`, "PUT", html)).status).toBe(204);
      const bucket = new Proxy(env.ARTICLE_BUCKET, { get(target, property) {
        if (property === "put") return async (...args: Parameters<R2Bucket["put"]>) => {
          await env.DB.prepare("UPDATE articles SET expires_at = '2000-01-01T00:00:00.000Z', cleanup_after = '2000-01-01T00:00:00.000Z' WHERE id = ?").bind(id).run();
          if (cleanup) await run(service.cleanupAbandoned(new Date(), 100));
          return target.put(...args);
        };
        const value = Reflect.get(target, property, target);
        return typeof value === "function" ? value.bind(target) : value;
      } });
      await expect(Effect.runPromise(service.publish(id).pipe(Effect.provideService(Env, { ...bindings, ARTICLE_BUCKET: bucket })))).rejects.toThrow();
      expect((await request(`/${id}`)).status).toBe(404);
      expect(await env.ARTICLE_BUCKET.head(`articles/${id}/sanitized/index.html`)).toBeNull();
      expect(await env.DB.prepare("SELECT count(*) AS count FROM article_notification_jobs WHERE article_id = ?").bind(id).first()).toEqual({ count: 0 });
    }
  });

  it("rolls publication back when its notification insert fails and retries the canonical object", async () => {
    const { id } = await stage();
    expect((await request(`/${id}/assets/0`, "PUT", html)).status).toBe(204);
    // Fixed SQL trigger scoped to this generated id exercises real D1 transaction rollback.
    await env.DB.exec(`CREATE TRIGGER fail_article_job BEFORE INSERT ON article_notification_jobs WHEN NEW.article_id = '${id}' BEGIN SELECT RAISE(ABORT, 'injected job failure'); END`);
    try {
      expect((await request(`/${id}/publish`, "POST")).status).toBe(500);
      expect((await request(`/${id}`)).status).toBe(404);
      expect(await env.ARTICLE_BUCKET.head(`articles/${id}/sanitized/index.html`)).not.toBeNull();
      expect(await env.DB.prepare("SELECT count(*) AS count FROM article_notification_jobs WHERE article_id = ?").bind(id).first()).toEqual({ count: 0 });
    } finally { await env.DB.exec("DROP TRIGGER fail_article_job"); }
    expect((await request(`/${id}/publish`, "POST")).status).toBe(200);
    expect(await env.DB.prepare("SELECT count(*) AS count FROM article_notification_jobs WHERE article_id = ?").bind(id).first()).toEqual({ count: 1 });
  });

  it("requires bearer credentials and restricts uploads and publication to master", async () => {
    expect((await request("", "GET", undefined, "invalid")).status).toBe(401);
    const token = await device();
    const { id } = await stage();
    expect((await request("", "GET", undefined, token)).status).toBe(200);
    expect((await request("/uploads", "POST", await draft(), token, { "Idempotency-Key": crypto.randomUUID() })).status).toBe(403);
    expect((await request(`/${id}/assets/0`, "PUT", html, token)).status).toBe(403);
    expect((await request(`/${id}/publish`, "POST", undefined, token)).status).toBe(403);
  });

  it("rejects corrupt uploads and out-of-manifest indexes", async () => {
    const { id } = await stage();
    expect((await request(`/${id}/assets/0`, "PUT", html.replace("Report", "Broken"))).status).toBe(400);
    expect((await request(`/${id}/assets/0`, "PUT", html + "extra")).status).toBe(400);
    expect((await request(`/${id}/assets/1`, "PUT", html)).status).toBe(404);
    expect((await request(`/${id}/assets/-1`, "PUT", html)).status).toBe(400);
    expect((await request(`/${id}/publish`, "POST")).status).toBe(400);
  });

  it("serves only published owner-scoped metadata and canonical HTML", async () => {
    const id = await published();
    const token = await device();
    const response = await request(`/${id}`, "GET", undefined, token);
    expect(response.status).toBe(200);
    expect(Object.keys(await response.json()).sort()).toEqual(["assets", "created_at", "id", "name", "read_at", "source_host", "source_project", "summary", "title", "version"]);
    const asset = await request(`/${id}/assets/0`, "GET", undefined, token);
    expect(asset.status).toBe(200);
    expect(asset.headers.get("content-type")).toBe("text/html; charset=utf-8");
    expect(asset.headers.get("x-content-type-options")).toBe("nosniff");
    expect(asset.headers.get("cache-control")).toBe("private, no-store");
    expect(await asset.text()).toContain("Sanitized report");
    expect((await request(`/${id}/assets/1`)).status).toBe(404);
    await env.DB.prepare("UPDATE articles SET owner_id = 'another-owner' WHERE id = ?").bind(id).run();
    expect((await request(`/${id}`)).status).toBe(404);
    expect((await request(`/${id}/assets/0`)).status).toBe(404);
    expect((await request(`/${id}/read`, "PUT", { read: true, version: 0 })).status).toBe(404);
  });

  it("rejects missing canonical objects instead of falling back to original HTML", async () => {
    const id = await published();
    const row = await env.DB.prepare("SELECT sanitized_html_key FROM articles WHERE id = ?").bind(id).first<{ sanitized_html_key: string }>();
    await env.ARTICLE_BUCKET.delete(row!.sanitized_html_key);
    expect((await request(`/${id}/assets/0`)).status).toBe(404);
  });

  it("makes repeated read state canonical and rejects stale opposite writes", async () => {
    const id = await published();
    const token = await device();
    const changed = await request(`/${id}/read`, "PUT", { read: true, version: 0 }, token);
    expect(changed.status).toBe(200);
    const first = await changed.json<{ version: number; read_at: string }>();
    expect(first.version).toBe(1);
    expect(first.read_at).toBeTypeOf("string");
    const repeat = await request(`/${id}/read`, "PUT", { read: true, version: 0 });
    expect(await repeat.json()).toEqual(first);
    expect((await request(`/${id}/read`, "PUT", { read: false, version: 0 })).status).toBe(409);
    const opposite = await request(`/${id}/read`, "PUT", { read: false, version: 1 });
    expect(await opposite.json()).toMatchObject({ read_at: null, version: 2 });
    expect((await request(`/${id}/assets/0`, "PUT", html)).status).toBe(409);
    expect((await request(`/${id}/publish`, "POST")).status).toBe(200);
  });

  it("validates list/read input and paginates tied timestamps without duplicate rows", async () => {
    const marker = crypto.randomUUID();
    const ids = [];
    for (let i = 0; i < 26; i++) ids.push(await published(`${marker} ${i}`));
    const first = await (await request(`?q=${marker}&read=unread`)).json<{ items: { id: string }[]; next_cursor: string }>();
    expect(first.items).toHaveLength(25);
    expect(first.next_cursor).toBeTypeOf("string");
    await published(`${marker} new`, "2026-09-02T12:00:00.000Z");
    const second = await (await request(`?q=${marker}&read=unread&cursor=${encodeURIComponent(first.next_cursor)}`)).json<{ items: { id: string }[]; next_cursor: null }>();
    expect(second.items).toHaveLength(1);
    expect(second.next_cursor).toBeNull();
    expect(new Set([...first.items, ...second.items].map(a => a.id))).toEqual(new Set(ids));
    for (const query of ["?read=invalid", "?cursor=garbage", `?q=${"a".repeat(201)}`, `?q=other&cursor=${encodeURIComponent(first.next_cursor)}`])
      expect((await request(query)).status).toBe(400);
    for (const input of [{ read: "yes", version: 0 }, { read: true, version: -1 }, { read: true, version: 0, extra: true }])
      expect((await request(`/${ids[0]}/read`, "PUT", input)).status).toBe(400);
  });

  it("expires staging without making metadata or objects readable", async () => {
    const { id } = await stage();
    await env.DB.prepare("UPDATE articles SET expires_at = '2000-01-01T00:00:00.000Z' WHERE id = ?").bind(id).run();
    expect((await request(`/${id}/assets/0`, "PUT", html)).status).toBe(409);
    expect((await request(`/${id}/publish`, "POST")).status).toBe(409);
    expect((await request(`/${id}`)).status).toBe(404);
  });

  it("bounds streaming bodies even with missing or false Content-Length", async () => {
    const { id } = await stage();
    const headerCases: Record<string, string>[] = [{}, { "Content-Length": "1" }];
    for (const headers of headerCases) {
      const stream = new ReadableStream<Uint8Array>({ start(controller) { controller.enqueue(enc.encode(html)); controller.enqueue(enc.encode("extra")); controller.close(); } });
      const response = await SELF.fetch(`https://example.com/v2/articles/${id}/assets/0`, { method: "PUT", body: stream, headers: { Authorization: "Bearer test-token", ...headers } });
      expect(response.status).toBe(400);
    }
    const asset = await env.DB.prepare("SELECT object_key, complete FROM article_assets WHERE article_id = ?").bind(id).first<{ object_key: string; complete: number }>();
    expect(asset!.complete).toBe(0);
    expect(await env.ARTICLE_BUCKET.head(asset!.object_key)).toBeNull();
    expect((await request("/uploads", "POST", " ".repeat(128 * 1024 + 1))).status).toBe(400);
  });

  it("converges competing idempotent stages, uploads and read updates", async () => {
    const payload = await draft();
    const key = crypto.randomUUID();
    const stages = await Promise.all(Array.from({ length: 3 }, () => stage(payload, key)));
    expect(new Set(stages.map(a => a.id)).size).toBe(1);
    const id = stages[0].id;
    expect(await Promise.all(Array.from({ length: 3 }, async () => (await request(`/${id}/assets/0`, "PUT", html)).status))).toEqual([204, 204, 204]);
    const publishedId = await published();
    const writes = await Promise.all(Array.from({ length: 3 }, () => request(`/${publishedId}/read`, "PUT", { read: true, version: 0 })));
    expect(writes.map(r => r.status)).toEqual([200, 200, 200]);
    const canonical = await (await request(`/${publishedId}`)).json<{ version: number; read_at: string }>();
    expect(canonical.version).toBe(1);
    expect(await Promise.all(writes.map(r => r.json()))).toEqual([canonical, canonical, canonical]);
  });

  it("keeps objects scoped to their article and catches missing uploaded objects", async () => {
    const first = await stage();
    const second = await stage();
    expect((await request(`/${first.id}/assets/0`, "PUT", html)).status).toBe(204);
    const objects = (await env.DB.prepare("SELECT article_id, object_key, complete FROM article_assets WHERE article_id IN (?, ?)").bind(first.id, second.id).all<{ article_id: string; object_key: string; complete: number }>()).results;
    const uploaded = objects.find(a => a.article_id === first.id)!;
    const pending = objects.find(a => a.article_id === second.id)!;
    expect(uploaded.object_key).not.toBe(pending.object_key);
    expect(pending.complete).toBe(0);
    expect((await request(`/${second.id}/assets/0`)).status).toBe(404);
    await env.ARTICLE_BUCKET.delete(uploaded.object_key);
    const response = await request(`/${first.id}/publish`, "POST");
    expect(response.status).toBe(400);
    expect(await response.json()).toEqual({ error: "article upload is missing or invalid" });
  });

  it("retries bounded abandoned-stage cleanup, including late writes, without touching published bundles", async () => {
    const service = await run(Articles);
    const { id } = await stage();
    expect((await request(`/${id}/assets/0`, "PUT", html)).status).toBe(204);
    const live = await published();
    await env.DB.prepare("UPDATE articles SET expires_at = '2000-01-01T00:00:00.000Z', cleanup_after = '2000-01-01T00:00:00.000Z' WHERE id = ?").bind(id).run();
    const now = new Date();
    const original = await env.DB.prepare("SELECT object_key FROM article_assets WHERE article_id = ?").bind(id).first<{ object_key: string }>();
    expect(await env.ARTICLE_BUCKET.head(original!.object_key)).not.toBeNull();
    const results = await Promise.all([run(service.cleanupAbandoned(now, 10)), run(service.cleanupAbandoned(now, 10))]);
    expect(results.flat().filter(cleaned => cleaned === id)).toHaveLength(1);
    expect(await env.ARTICLE_BUCKET.head(original!.object_key)).toBeNull();
    expect((await request(`/${live}/assets/0`)).status).toBe(200);
    expect((await request(`/${id}/assets/0`, "PUT", html)).status).toBe(409);
    // Simulates an R2 write completing after cleanup and its request being interrupted.
    await env.ARTICLE_BUCKET.put(original!.object_key, html);
    await run(service.cleanupAbandoned(new Date(now.getTime() + 25 * 60 * 60 * 1000), 100));
    expect(await env.ARTICLE_BUCKET.head(original!.object_key)).toBeNull();
    expect((await request(`/${live}/assets/0`)).status).toBe(200);
    await expect(run(service.cleanupAbandoned(now, 0))).rejects.toThrow();
  });

  it("reclaims a failed R2 cleanup after its lease expires", async () => {
    const service = await run(Articles);
    const { id } = await stage();
    expect((await request(`/${id}/assets/0`, "PUT", html)).status).toBe(204);
    await env.DB.prepare("UPDATE articles SET expires_at = '1900-01-01T00:00:00.000Z', cleanup_after = '1900-01-01T00:00:00.000Z' WHERE id = ?").bind(id).run();
    const object = await env.DB.prepare("SELECT object_key FROM article_assets WHERE article_id = ?").bind(id).first<{ object_key: string }>();
    // Only deletion fails. Real D1 and real R2 state prove the lease and retry behavior.
    const failingBucket = new Proxy(env.ARTICLE_BUCKET, { get(target, property) {
      if (property === "delete") return async () => { throw new Error("injected storage outage"); };
      const value = Reflect.get(target, property, target);
      return typeof value === "function" ? value.bind(target) : value;
    } });
    const now = new Date();
    await expect(Effect.runPromise(service.cleanupAbandoned(now, 1).pipe(Effect.provideService(Env, { ...bindings, ARTICLE_BUCKET: failingBucket })))).rejects.toThrow();
    expect(await env.ARTICLE_BUCKET.head(object!.object_key)).not.toBeNull();
    expect(await run(service.cleanupAbandoned(now, 1))).not.toContain(id);
    expect(await run(service.cleanupAbandoned(new Date(now.getTime() + 6 * 60 * 1000), 1))).toContain(id);
    expect(await env.ARTICLE_BUCKET.head(object!.object_key)).toBeNull();
  });

  it("serves allowed download attachments with declared types and safe filenames", async () => {
    const payload = await draft();
    const text = "notes\n";
    const hash = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", enc.encode(text))), b => b.toString(16).padStart(2, "0")).join("");
    const { id } = await stage({ ...payload, assets: [...payload.assets, { path: "notes.txt", media_type: "text/plain", size: 6, sha256: hash, disposition: "attachment" }] });
    expect((await request(`/${id}/assets/0`, "PUT", html)).status).toBe(204);
    expect((await request(`/${id}/assets/1`, "PUT", text)).status).toBe(204);
    await env.DB.prepare("UPDATE articles SET state = 'published' WHERE id = ?").bind(id).run();
    const response = await request(`/${id}/assets/1`);
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toBe("text/plain; charset=utf-8");
    expect(response.headers.get("content-disposition")).toContain("attachment;");
    expect(response.headers.get("content-disposition")).toContain("filename*=UTF-8''notes.txt");
    expect(await response.text()).toBe(text);
  });

  it("rejects malformed manifests and replays equivalent object property orders", async () => {
    for (const body of ["{broken", { ...await draft(), extra: "unexpected" }, { ...await draft(), silent: "yes" }])
      expect((await request("/uploads", "POST", body, "test-token", { "Idempotency-Key": crypto.randomUUID() })).status).toBe(400);
    expect((await request("/uploads", "POST", await draft())).status).toBe(400);
    const payload = await draft();
    const key = crypto.randomUUID();
    const created = await stage(payload, key);
    const reordered = { assets: payload.assets, silent: payload.silent, source_project: payload.source_project, source_host: payload.source_host, name: payload.name, summary: payload.summary, title: payload.title };
    expect(await stage(reordered, key)).toEqual(created);
    await env.DB.prepare("UPDATE articles SET expires_at = '2000-01-01T00:00:00.000Z' WHERE id = ?").bind(created.id).run();
    expect((await request("/uploads", "POST", payload, "test-token", { "Idempotency-Key": key })).status).toBe(409);
  });

  it("does not let a competing stale opposite update overwrite the winning transition", async () => {
    const marker = crypto.randomUUID();
    const id = await published(marker);
    expect((await request(`/${id}/read`, "PUT", { read: true, version: 0 })).status).toBe(200);
    const [unread, read] = await Promise.all([
      request(`/${id}/read`, "PUT", { read: false, version: 1 }),
      request(`/${id}/read`, "PUT", { read: true, version: 1 }),
    ]);
    expect(unread.status).toBe(200);
    expect([200, 409]).toContain(read.status);
    expect(await (await request(`/${id}`)).json()).toMatchObject({ read_at: null, version: 2 });
    const unreadItems = await (await request(`?q=${marker}&read=unread`)).json<{ items: { id: string }[] }>();
    expect(unreadItems.items.map(item => item.id)).toEqual([id]);
    expect(await (await request(`?q=${marker}&read=read`)).json()).toEqual({ items: [], next_cursor: null });
  });

  it("persists at most one notification job per article", async () => {
    const id = await published();
    const statement = () => env.DB.prepare("INSERT INTO article_notification_jobs (article_id, next_attempt_at) VALUES (?, ?)").bind(id, new Date().toISOString()).run();
    await statement();
    await expect(statement()).rejects.toThrow();
    const job = await env.DB.prepare("SELECT state, attempts, delivered_at FROM article_notification_jobs WHERE article_id = ?").bind(id).first();
    expect(job).toEqual({ state: "pending", attempts: 0, delivered_at: null });
  });

  it("finds a published article by agent name when title and summary do not match", async () => {
    const name = `agent-${crypto.randomUUID()}`;
    const id = await published("Unrelated title", "2026-09-01T12:00:00.000Z", name);
    await stage({ ...await draft(), name });
    const page = await (await request(`?q=${name}`)).json<{ items: { id: string }[] }>();
    expect(page.items.map(item => item.id)).toEqual([id]);
  });
});
