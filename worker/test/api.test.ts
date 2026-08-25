import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";

const enc = new TextEncoder();
async function itemToken(secret: string, id: string): Promise<string> {
  const key = await crypto.subtle.importKey("raw", enc.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const sig = new Uint8Array(await crypto.subtle.sign("HMAC", key, enc.encode(id)));
  return btoa(String.fromCharCode(...sig)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

const AUTH = { Authorization: "Bearer test-token" };
const json = (body: unknown) => ({ method: "POST", headers: { ...AUTH, "content-type": "application/json" }, body: JSON.stringify(body) });
const settle = () => new Promise((r) => setTimeout(r, 50));

async function topicMessages(since = "all"): Promise<any[]> {
  await settle();
  const res = await SELF.fetch(`http://lam/test-topic/json?poll=1&since=${since}`);
  expect(res.status).toBe(200);
  return new TextDecoder().decode(await res.arrayBuffer()).trim().split("\n").filter(Boolean).map((l) => JSON.parse(l));
}
const lastMessage = async () => (await topicMessages()).at(-1)!;

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
  it("creates an item and publishes to the topic with action buttons", async () => {
    const res = await SELF.fetch("http://lam/items", json({ title: "PR 42", body: "decide", priority: "critical", choices: ["waive", "require"], source_host: "mac", source_project: "platform" }));
    const item = await res.json<any>();
    expect(item.id).toMatch(/^[a-z2-9]{5}$/);
    expect(item.status).toBe("open");
    const msg = await lastMessage();
    const t = await itemToken("test-secret", item.id);
    expect(msg).toMatchObject({ event: "message", topic: "test-topic", title: `PR 42 [${item.id}]`, message: "(mac:platform)\ndecide", priority: 5, tags: ["rotating_light"] });
    expect(msg.id).toMatch(/^[A-Za-z0-9]{12}$/);
    expect(msg.actions.map((a: any) => ({ ...a, id: undefined }))).toEqual([
      { action: "http", label: "waive", url: `http://lam/a/${item.id}/waive?t=${t}`, clear: true },
      { action: "http", label: "require", url: `http://lam/a/${item.id}/require?t=${t}`, clear: true },
      { action: "view", label: "Reply", url: `http://lam/r/${item.id}?t=${t}`, clear: true },
    ]);
    expect(msg.actions.every((a: any) => typeof a.id === "string" && a.id)).toBe(true);
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
    // the ntfy app sends http actions as POST
    const res = await SELF.fetch(`http://lam/a/${item.id}/yes?t=${t}`, { method: "POST" });
    expect(res.status).toBe(200);
    const got = await (await SELF.fetch(`http://lam/items/${item.id}`, { headers: AUTH })).json<any>();
    expect(got).toMatchObject({ status: "resolved", response_choice: "yes", response_by: "phone" });
    expect(got.resolved_at).toBeTruthy();
    expect((await lastMessage()).message).toBe("resolved via phone: yes");
    expect((await SELF.fetch(`http://lam/a/${item.id}/no?t=${t}`)).status).toBe(409);
    expect((await SELF.fetch(`http://lam/a/${item.id}/no?t=${t}`, { method: "POST" })).status).toBe(409);
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
    const res = await SELF.fetch(`http://lam/r/${item.id}?t=${t}`, { method: "POST", body: new URLSearchParams({ text: "option C" }) });
    expect(res.status).toBe(200);
    const got = await (await SELF.fetch(`http://lam/items/${item.id}`, { headers: AUTH })).json<any>();
    expect(got).toMatchObject({ response_text: "option C", response_by: "phone" });
  });
  it("wait on unknown id is 404", async () => {
    expect((await SELF.fetch("http://lam/items/zzzzz/wait", { headers: AUTH })).status).toBe(404);
  });
});

describe("ntfy-compatible topic", () => {
  it("serves only the configured topic", async () => {
    expect((await SELF.fetch("http://lam/other/json?poll=1")).status).toBe(404);
    expect((await SELF.fetch("http://lam/other", { method: "POST", body: "x" })).status).toBe(404);
    expect((await SELF.fetch("http://lam/test-topic/auth")).status).toBe(200);
    expect(await (await SELF.fetch("http://lam/v1/health")).json()).toEqual({ healthy: true });
  });
  it("accepts raw ntfy publishes with headers and Actions", async () => {
    const res = await SELF.fetch("http://lam/test-topic", {
      method: "POST",
      headers: { Title: "manual", Priority: "4", Tags: "eyes, tada", Actions: "http, Go, https://x/go, clear=true; view, Open, https://x/open" },
      body: "hello",
    });
    expect(res.status).toBe(200);
    const msg = await res.json<any>();
    expect(msg).toMatchObject({ title: "manual", message: "hello", priority: 4, tags: ["eyes", "tada"] });
    expect(msg.actions).toMatchObject([{ action: "http", label: "Go", url: "https://x/go", clear: true }, { action: "view", label: "Open", url: "https://x/open", clear: false }]);
    expect((await SELF.fetch("http://lam/test-topic", { method: "POST", headers: { Priority: "9" }, body: "x" })).status).toBe(400);
  });
  it("poll honours since=none|all|<id>", async () => {
    const a = await (await SELF.fetch("http://lam/test-topic", { method: "POST", body: "one" })).json<any>();
    const b = await (await SELF.fetch("http://lam/test-topic", { method: "POST", body: "two" })).json<any>();
    expect(await topicMessages("none")).toEqual([]);
    const all = await topicMessages("all");
    expect(all.map((m) => m.id)).toEqual(expect.arrayContaining([a.id, b.id]));
    const after = await topicMessages(a.id);
    expect(after.map((m) => m.id)).toEqual([b.id]);
  });
  it("json stream opens with an open event and delivers live messages", async () => {
    const res = await SELF.fetch("http://lam/test-topic/json?since=none");
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toBe("application/x-ndjson");
    const reader = res.body!.getReader();
    const dec = new TextDecoder();
    let buf = "";
    const nextLine = async () => {
      while (!buf.includes("\n")) buf += dec.decode((await reader.read()).value);
      const [line, ...rest] = buf.split("\n");
      buf = rest.join("\n");
      return JSON.parse(line);
    };
    expect(await nextLine()).toMatchObject({ event: "open", topic: "test-topic" });
    await SELF.fetch("http://lam/test-topic", { method: "POST", body: "live" });
    expect(await nextLine()).toMatchObject({ event: "message", message: "live" });
    await reader.cancel();
  });
});
