import { env, SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { viewerParts } from "../src/http/article-viewer";

const enc = new TextEncoder();
const api = (path: string, method = "GET", body?: unknown) => SELF.fetch(`https://example.com/v2/articles${path}`, { method, headers: { Authorization: "Bearer test-token", "Idempotency-Key": crypto.randomUUID() }, body: body === undefined ? undefined : JSON.stringify(body) });
async function published() {
  const html = '<h1 id="report">Report</h1><p>literal lam-asset:1</p><details><summary>More</summary>Detail</details><a href="#report">Top</a><a href="https://example.org/visit">Visit</a>';
  const bytes = enc.encode(html);
  const hash = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)), b => b.toString(16).padStart(2, "0")).join("");
  const staged = await api("/uploads", "POST", { title: "Report", summary: "Summary", name: "Agent", source_host: "host", source_project: "lam", silent: true, assets: [{ path: "index.html", media_type: "text/html", size: bytes.length, sha256: hash, disposition: "inline" }] });
  const { id } = await staged.json<{ id: string }>();
  await SELF.fetch(`https://example.com/v2/articles/${id}/assets/0`, { method: "PUT", headers: { Authorization: "Bearer test-token" }, body: bytes });
  expect((await api(`/${id}/publish`, "POST")).status).toBe(200);
  return id;
}
const view = (id: string, path: string, token?: string, method = "GET", body?: unknown) => SELF.fetch(`https://example.com/view/articles/${id}${path}`, { method, headers: token ? { Authorization: `Bearer ${token}`, "Content-Type": "application/json" } : {}, body: body === undefined ? undefined : JSON.stringify(body) });
async function session(id: string) {
  const response = await api(`/${id}/view-session`, "POST");
  expect(response.status).toBe(201);
  const { url } = await response.json<{ url: string }>();
  const parsed = new URL(url);
  expect(parsed.search).toBe("");
  expect(url).not.toContain("test-token");
  return { url, bootstrap: parsed.hash.slice(1) };
}
async function exchange(id: string) {
  const { bootstrap } = await session(id);
  const response = await view(id, "/exchange", bootstrap, "POST");
  expect(response.status).toBe(200);
  return response.json<{ token: string }>();
}

async function inlineImage(id: string, width = 1, height = 1) {
  const bytes = Uint8Array.from(atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6N1cAAAAASUVORK5CYII="), char => char.charCodeAt(0));
  const view = new DataView(bytes.buffer);
  view.setUint32(16, width); view.setUint32(20, height);
  await env.DB.prepare("INSERT INTO article_assets (article_id, asset_index, path, media_type, size, sha256, disposition, object_key, complete) VALUES (?, 1, 'pixel.png', 'image/png', ?, ?, 'inline', ?, 1)").bind(id, bytes.length, "a".repeat(64), `articles/${id}/original/1`).run();
  await env.ARTICLE_BUCKET.put(`articles/${id}/original/1`, bytes);
}

describe("isolated article viewer", () => {
  it("keeps session authorization failures private and unframeable", async () => {
    const id = await published();
    const response = await SELF.fetch(`https://example.com/v2/articles/${id}/view-session`, { method: "POST" });
    expect(response.status).toBe(401);
    expect(response.headers.get("cache-control")).toContain("no-store");
    expect(response.headers.get("content-security-policy")).toContain("frame-ancestors 'none'");
  });
  it("limits image dimensions before browser decoding and scopes header-only image access", async () => {
    const id = await published();
    await inlineImage(id, 100000, 100000);
    const { token } = await exchange(id);
    expect((await view(id, "/images/1", token)).status).toBe(400);
    expect((await view(id, "/images/1")).status).toBe(401);
    const other = await published();
    await inlineImage(other);
    expect((await view(other, "/images/1", token)).status).toBe(401);
    const own = await exchange(other);
    const image = await view(other, "/images/1", own.token);
    expect(image.status).toBe(200);
    expect(image.headers.get("x-lam-image-pixels")).toBe("1");
    expect((await view(other, "/images/0", own.token)).status).toBe(403);
    expect((await view(other, "/downloads/1", own.token)).status).toBe(403);
  });
  it("emits typed slots only for parsed HTML SVG and CSS resources and leaves labels inert", () => {
    const assets = [{ path: "pixel.png", media_type: "image/png", disposition: "inline" as const, size: 10, sha256: "a".repeat(64) }];
    const parts = viewerParts('<img src="lam-asset:0"><svg><image href="lam-asset:0"></image></svg><style>p{background:url(lam-asset:0)}</style><p style="background:url(lam-asset:0)">lam-asset:0</p><span data-lam-attachment="1">Notes</span>', assets);
    expect(parts.filter(part => typeof part !== "string")).toEqual([{ asset: 0 }, { asset: 0 }, { asset: 0 }, { asset: 0 }]);
    const rendered = parts.map(part => typeof part === "string" ? part : "data:,").join("");
    expect(rendered).toContain('>lam-asset:0</p>');
    expect(rendered).toContain('<span data-lam-attachment="1">Notes</span>');
    expect(rendered).toContain('src="data:,"');
  });

  it("rejects client-supplied resource substitutions without enabling read", async () => {
    const id = await published();
    const { token } = await exchange(id);
    for (const resources of [{ "0": "blob:https://example.com/11111111-1111-4111-8111-111111111111" }, { "9": "https://evil.example/image" }, { ["__proto__"]: "blob:null/id" }]) {
      expect((await view(id, "/content", token, "POST", { resources })).status).toBe(400);
    }
    expect((await view(id, "/read", token, "PUT", { read: true, version: 0 })).status).toBe(403);
  });

  it("invalidates device sessions at revocation and master sessions at credential rotation", async () => {
    const id = await published();
    const credential = crypto.randomUUID();
    const deviceId = crypto.randomUUID();
    const key = await crypto.subtle.importKey("raw", enc.encode("test-secret"), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    const digest = btoa(String.fromCharCode(...new Uint8Array(await crypto.subtle.sign("HMAC", key, enc.encode(credential))))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    await env.DB.prepare("INSERT INTO devices (id, name, credential_hash, app_version, android_version, created_at) VALUES (?, 'Browser', ?, 'test', 'test', ?)").bind(deviceId, digest, new Date().toISOString()).run();
    const issued = await SELF.fetch(`https://example.com/v2/articles/${id}/view-session`, { method: "POST", headers: { Authorization: `Bearer ${credential}` } });
    expect(issued.status).toBe(201);
    const { url } = await issued.json<{ url: string }>();
    const exchanged = await view(id, "/exchange", new URL(url).hash.slice(1), "POST");
    const { token } = await exchanged.json<{ token: string }>();
    expect((await view(id, "/content", token, "POST", {})).status).toBe(200);
    await env.DB.prepare("UPDATE devices SET revoked_at = ? WHERE id = ?").bind(new Date().toISOString(), deviceId).run();
    for (const route of ["/images/0", "/downloads/0"]) expect((await view(id, route, token)).status).toBe(401);
    expect((await view(id, "/content", token, "POST", {})).status).toBe(401);
    expect((await view(id, "/read", token, "PUT", { read: true, version: 0 })).status).toBe(401);
    const master = await exchange(id);
    await env.DB.prepare("UPDATE article_view_sessions SET credential_hash = 'rotated' WHERE article_id = ? AND device_id IS NULL").bind(id).run();
    expect((await view(id, "/content", master.token, "POST", {})).status).toBe(401);
  });
  it("issues fragment-only previews and GET never marks read", async () => {
    const id = await published();
    const { url } = await session(id);
    const preview = await SELF.fetch(url);
    expect(preview.status).toBe(200);
    expect(preview.headers.get("cache-control")).toContain("no-store");
    expect(preview.headers.get("content-security-policy")).toContain("frame-ancestors 'none'");
    expect(preview.headers.get("referrer-policy")).toBe("no-referrer");
    const wrapper = await preview.text();
    expect(wrapper).toContain('sandbox="allow-popups allow-popups-to-escape-sandbox"');
    expect(wrapper).not.toContain("allow-scripts");
    expect(wrapper).not.toContain("allow-same-origin");
    expect(await (await api(`/${id}`)).json()).toMatchObject({ read_at: null, version: 0 });
  });

  it("denies missing, wrong-article, expired and replayed bootstrap sessions", async () => {
    const id = await published();
    const other = await published();
    const { bootstrap } = await session(id);
    expect((await view(id, "/content", undefined, "POST", {})).status).toBe(401);
    expect((await view(id, "/read", undefined, "PUT", { read: true, version: 0 })).status).toBe(401);
    expect((await view(other, "/exchange", bootstrap, "POST")).status).toBe(401);
    expect((await view(id, "/exchange", bootstrap, "POST")).status).toBe(200);
    expect((await view(id, "/exchange", bootstrap, "POST")).status).toBe(401);
    const expired = await session(id);
    await env.DB.prepare("UPDATE article_view_sessions SET bootstrap_expires_at = '2000-01-01' WHERE article_id = ?").bind(id).run();
    expect((await view(id, "/exchange", expired.bootstrap, "POST")).status).toBe(401);
  });

  it("requires successful retrieval and preserves a concurrent newer unread", async () => {
    const id = await published();
    const { token } = await exchange(id);
    expect((await view(id, "/read", token, "PUT", { read: true, version: 0 })).status).toBe(403);
    const content = await view(id, "/content", token, "POST", {});
    expect(content.status).toBe(200);
    const delivered = await content.json<{ parts: string[] }>();
    expect(delivered.parts.join("")).toContain("literal lam-asset:1");
    expect(delivered.parts.join("")).toContain("script-src 'none'");
    expect((await view(id, "/read", token, "PUT", { read: true, version: 0 })).status).toBe(200);
    expect((await api(`/${id}/read`, "PUT", { read: false, version: 1 })).status).toBe(200);
    expect((await view(id, "/read", token, "PUT", { read: true, version: 0 })).status).toBe(409);
    expect(await (await api(`/${id}`)).json()).toMatchObject({ read_at: null, version: 2 });
  });

  it("denies expired runtime sessions and missing canonical objects without permitting read", async () => {
    const id = await published();
    const { token } = await exchange(id);
    await env.ARTICLE_BUCKET.delete(`articles/${id}/sanitized/index.html`);
    expect((await view(id, "/content", token, "POST", {})).status).toBe(404);
    expect((await view(id, "/read", token, "PUT", { read: true, version: 0 })).status).toBe(403);
    await env.DB.prepare("UPDATE article_view_sessions SET expires_at = '2000-01-01' WHERE article_id = ?").bind(id).run();
    expect((await view(id, "/content", token, "POST", {})).status).toBe(401);
  });
});
