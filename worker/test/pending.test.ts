import { env, SELF } from "cloudflare:test";
import { beforeEach, describe, expect, it } from "vitest";

const headers = { Authorization: "Bearer test-token", "content-type": "application/json" };
const post = (path: string, body: unknown) => SELF.fetch(`http://lam${path}`, { method: "POST", headers, body: JSON.stringify(body) });
const list = async (query = "") => (await SELF.fetch(`http://lam/items${query}`, { headers })).json<any[]>();
const messages = async () => {
  await new Promise(resolve => setTimeout(resolve, 100));
  const response = await SELF.fetch("http://lam/test-topic/json?poll=1&since=all");
  return new TextDecoder().decode(await response.arrayBuffer()).trim().split("\n").filter(Boolean).map(line => JSON.parse(line));
};
const draft = (external_key: string, fields = {}) => ({ kind: "request", name: "producer", title: external_key, recommendation: "Continue", external_key, ...fields });

describe("pending items", () => {
  beforeEach(async () => {
    await env.DB.prepare("DELETE FROM items").run();
  });

  it.each([{ pending: true }, { pending: false }, { external_key: "legacy-key" }])("rejects new creation fields on the legacy route: %j", async (fields) => {
    const before = await messages();
    const response = await post("/items", { title: "Legacy item", ...fields });
    expect(response.status).toBe(400);
    expect(await list()).toEqual([]);
    expect(await messages()).toEqual(before);
  });

  it("reuses an expired external key for a new open item", async () => {
    const first = await (await post("/v2/items", draft("expired-key", { pending: true, ttl: 60 }))).json<any>();
    await env.DB.prepare("UPDATE items SET expires_at = '2000-01-01T00:00:00Z' WHERE id = ?").bind(first.id).run();
    const response = await post("/v2/items", draft("expired-key", { pending: true }));
    expect(response.status).toBe(201);
    const current = await response.json<any>();
    expect(current.id).not.toBe(first.id);
    expect(current.status).toBe("open");
    const previous = await (await SELF.fetch(`http://lam/items/${first.id}`, { headers })).json<any>();
    expect(previous.status).toBe("expired");
    expect((await list("?status=open&pending=1")).map(item => item.id)).toEqual([current.id]);
  });

  it("returns 404 when a key only identifies an expired item", async () => {
    const first = await (await post("/v2/items", draft("expired-resolution", { pending: true, ttl: 60 }))).json<any>();
    await env.DB.prepare("UPDATE items SET expires_at = '2000-01-01T00:00:00Z' WHERE id = ?").bind(first.id).run();
    expect((await post("/v2/items/resolve", { external_key: "expired-resolution" })).status).toBe(404);
  });

  it("upserts the open external key with the same id and preserves its origin and lane", async () => {
    const created = await post("/v2/items", draft("dedupe", { pending: true, link: "https://example.com/old" }));
    expect(created.status).toBe(201);
    const first = await created.json<any>();
    const updated = await post("/v2/items", draft("dedupe", { name: "other", title: "Updated", body: "Progress", link: "https://example.com/new", priority: "critical" }));
    expect(updated.status).toBe(200);
    const second = await updated.json<any>();
    expect(second).toMatchObject({ id: first.id, name: "producer", title: "Updated", body: "Progress", link: "https://example.com/new", priority: "critical", pending_at: first.pending_at, version: 1 });
    expect(first.pending_at).toEqual(expect.any(String));
    expect(await list("?pending=1")).toHaveLength(1);
    expect(await messages()).toEqual([]);
  });

  it("keeps pending out of the default queue and moves without changing origin fields", async () => {
    const first = await (await post("/v2/items", draft("move", { link: "https://example.com", pending: true }))).json<any>();
    expect((await list()).map(item => item.id)).not.toContain(first.id);
    expect((await list("?status=open&pending=1")).map(item => item.id)).toContain(first.id);
    const before = await messages();
    for (const pending of [false, true, true]) {
      const response = await post(`/v2/items/${first.id}/pending`, { pending });
      expect(response.status).toBe(200);
      const item = await response.json<any>();
      expect(item).toMatchObject({ name: first.name, kind: first.kind, link: first.link, status: "open" });
      expect(item.pending_at === null).toBe(!pending);
      expect((await list()).some(item => item.id === first.id)).toBe(!pending);
    }
    expect(await messages()).toEqual(before);
  });

  it("resolves by key and releases the key for a new item", async () => {
    const first = await (await post("/v2/items", draft("resolve", { pending: true }))).json<any>();
    const response = await post("/v2/items/resolve", { external_key: "resolve", text: "Completed upstream" });
    expect(response.status).toBe(200);
    expect(await response.json()).toMatchObject({ id: first.id, status: "resolved", response_text: "Completed upstream", response_by: "cli", version: 1 });
    expect((await post("/v2/items/resolve", { external_key: "resolve" })).status).toBe(404);
    expect((await post(`/v2/items/${first.id}/pending`, { pending: false })).status).toBe(409);
    const recreated = await post("/v2/items", draft("resolve", { pending: true }));
    expect(recreated.status).toBe(201);
    expect((await recreated.json<any>()).id).not.toBe(first.id);
    expect(await messages()).toEqual([]);
  });

  it("keeps pending checklist changes and closing silent", async () => {
    const first = await (await post("/v2/items", { kind: "request", title: "Pending checks", checks: ["One"], pending: true })).json<any>();
    expect((await post(`/items/${first.id}/checks`, { label: "Two" })).status).toBe(200);
    expect((await post(`/items/${first.id}/resolve`, {})).status).toBe(200);
    expect(await messages()).toEqual([]);
  });

  it("resolves a pending FYI by id without requiring a reply", async () => {
    const created = await post("/v2/items", { kind: "fyi", title: "Background work", pending: true, external_key: "background" });
    expect(created.status).toBe(201);
    const first = await created.json<any>();
    const response = await post(`/items/${first.id}/resolve`, {});
    expect(response.status).toBe(200);
    expect(await response.json()).toMatchObject({ id: first.id, status: "resolved" });
    expect(await messages()).toEqual([]);
  });
});
