import { SELF } from "cloudflare:test";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { itemToken } from "../src/auth";

const AUTH = { Authorization: "Bearer test-token" };
const json = (body: unknown) => ({ method: "POST", headers: { ...AUTH, "content-type": "application/json" }, body: JSON.stringify(body) });

type Captured = { headers: Record<string, string>; body: string };
let ntfyCalls: Captured[];
const realFetch = globalThis.fetch;

beforeEach(() => {
  ntfyCalls = [];
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const url = String(input instanceof Request ? input.url : input);
    if (!url.startsWith("https://ntfy.sh/")) return realFetch(input, init);
    expect(url).toBe("https://ntfy.sh/test-topic");
    ntfyCalls.push({ headers: Object.fromEntries(new Headers(init?.headers).entries()), body: String(init?.body) });
    return new Response("{}");
  });
});
afterEach(() => vi.restoreAllMocks());

const settle = () => new Promise((r) => setTimeout(r, 50));

async function push(body: object = { title: "hi" }) {
  const res = await SELF.fetch("http://lam/items", json(body));
  expect(res.status).toBe(201);
  return res.json<any>();
}

describe("auth", () => {
  it("rejects missing bearer", async () => {
    expect((await SELF.fetch("http://lam/items")).status).toBe(401);
  });
  it("rejects bad item token on action", async () => {
    const item = await push();
    expect((await SELF.fetch(`http://lam/a/${item.id}/Done?t=nope`)).status).toBe(403);
  });
});

describe("POST /items", () => {
  it("validates title, priority, choices", async () => {
    for (const bad of [{}, { title: "x", priority: "urgent" }, { title: "x", choices: ["a", "b", "c", "d"] }, { title: "x", choices: [""] }]) {
      expect((await SELF.fetch("http://lam/items", json(bad))).status).toBe(400);
    }
  });
  it("creates an item and publishes to ntfy with action buttons", async () => {
    const res = await SELF.fetch("http://lam/items", json({ title: "PR 42", body: "decide", priority: "critical", choices: ["waive", "require"], source_host: "mac", source_project: "platform" }));
    const item = await res.json<any>();
    expect(item.id).toMatch(/^[a-z2-9]{5}$/);
    expect(item.status).toBe("open");
    await settle();
    const captured = ntfyCalls.at(-1)!;
    expect(captured.headers.title).toBe(`PR 42 [${item.id}]`);
    expect(captured.headers.priority).toBe("5");
    const t = await itemToken("test-secret", item.id);
    expect(captured.headers.actions).toBe(
      `http, waive, http://lam/a/${item.id}/waive?t=${t}, clear=true; http, require, http://lam/a/${item.id}/require?t=${t}, clear=true; view, Reply, http://lam/r/${item.id}?t=${t}, clear=true`,
    );
    expect(captured.body).toBe("(mac:platform)\ndecide");
  });
});

describe("read", () => {
  it("lists open by default filter and gets by id", async () => {
    const a = await push({ title: "a" });
    await SELF.fetch(`http://lam/items/${a.id}/dismiss`, { method: "POST", headers: AUTH });
    const open = await (await SELF.fetch("http://lam/items?status=open", { headers: AUTH })).json<any[]>();
    expect(open.find((i) => i.id === a.id)).toBeUndefined();
    const all = await (await SELF.fetch("http://lam/items", { headers: AUTH })).json<any[]>();
    expect(all.find((i) => i.id === a.id).status).toBe("dismissed");
    expect((await SELF.fetch("http://lam/items/zzzzz", { headers: AUTH })).status).toBe(404);
  });
});

describe("resolution", () => {
  it("phone button resolves with choice and publishes closed message", async () => {
    const item = await push({ title: "q", choices: ["yes", "no"] });
    const t = await itemToken("test-secret", item.id);
    const res = await SELF.fetch(`http://lam/a/${item.id}/yes?t=${t}`);
    expect(res.status).toBe(200);
    const got = await (await SELF.fetch(`http://lam/items/${item.id}`, { headers: AUTH })).json<any>();
    expect(got).toMatchObject({ status: "resolved", response_choice: "yes", response_by: "phone" });
    expect(got.resolved_at).toBeTruthy();
    await settle();
    expect(ntfyCalls.at(-1)!.body).toBe("resolved via phone: yes");
    // second press is a no-op
    expect((await SELF.fetch(`http://lam/a/${item.id}/no?t=${t}`)).status).toBe(409);
  });
  it("cli resolve with text; wait returns immediately once closed", async () => {
    const item = await push();
    const res = await SELF.fetch(`http://lam/items/${item.id}/resolve`, json({ text: "go with B" }));
    expect((await res.json<any>()).response_text).toBe("go with B");
    const w = await SELF.fetch(`http://lam/items/${item.id}/wait`, { headers: AUTH });
    expect(w.status).toBe(200);
    expect((await w.json<any>()).response_by).toBe("cli");
  });
  it("reply page serves form and accepts text", async () => {
    const item = await push();
    const t = await itemToken("test-secret", item.id);
    const page = await SELF.fetch(`http://lam/r/${item.id}?t=${t}`);
    expect(page.status).toBe(200);
    expect(await page.text()).toContain("<form");
    const form = new URLSearchParams({ text: "option C" });
    const res = await SELF.fetch(`http://lam/r/${item.id}?t=${t}`, { method: "POST", body: form });
    expect(res.status).toBe(200);
    const got = await (await SELF.fetch(`http://lam/items/${item.id}`, { headers: AUTH })).json<any>();
    expect(got).toMatchObject({ response_text: "option C", response_by: "phone" });
  });
  it("wait on unknown id is 404", async () => {
    expect((await SELF.fetch("http://lam/items/zzzzz/wait", { headers: AUTH })).status).toBe(404);
  });
});
