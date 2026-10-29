import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";

const headers = { Authorization: "Bearer test-token", "Content-Type": "application/json" };
const post = (path: string, body: unknown) => SELF.fetch(`http://lam${path}`, {
  method: "POST", headers, body: JSON.stringify(body),
});
let sequence = 0;

const cases = [
  { name: "legacy creation", path: "/items", fields: {}, upsert: false },
  { name: "typed creation", path: "/v2/items", fields: { kind: "request", recommendation: "Continue" }, upsert: false },
  { name: "pending add", path: "/v2/items", fields: { kind: "fyi", pending: true }, upsert: false },
  { name: "pending upsert", path: "/v2/items", fields: { kind: "request", recommendation: "Continue", pending: true }, upsert: true },
];

describe.each(cases)("item links on $name", ({ path, fields, upsert }) => {
  async function submit(link: string) {
    const title = `Link validation #${++sequence}`;
    const body = { title, ...fields, ...(upsert ? { external_key: title } : {}) };
    const previous = upsert ? await post(path, { ...body, link: "https://example.com/original" }) : null;
    if (previous) expect(previous.status).toBe(201);
    const original = previous ? await previous.json<{ id: string; link: string; version: number }>() : null;
    return { response: await post(path, { ...body, link }), original };
  }

  it.each([
    "http://example.com",
    "https://example.com/path?query=value",
    "grokbot://app/v1/agent?id=agent-123",
    "HTTPS://example.com",
    "GrOkBoT://app/v1/agent?id=agent-123",
  ])("accepts %s and preserves the link", async (link) => {
    const { response, original } = await submit(link);
    expect(response.status).toBe(upsert ? 200 : 201);
    const item = await response.json<{ id: string; link: string; pending_at: string | null }>();
    expect(item.link).toBe(link);
    if (original) expect(item.id).toBe(original.id);
    if ("pending" in fields) expect(item.pending_at).toEqual(expect.any(String));

    const stored = await SELF.fetch(`http://lam/items/${item.id}`, { headers });
    expect(stored.status).toBe(200);
    expect(await stored.json()).toMatchObject({ link });
  });

  it.each([
    "javascript:alert(1)",
    "file:///etc/passwd",
    "ftp://x",
    "data:text/html,x",
    "mailto:person@example.com",
    "bare text",
    "grokbot:app/v1/agent",
    "grokbot://",
    "grokbot:// \t",
    "http://",
    "https://",
    "",
  ])("rejects %j with the allowed schemes", async (link) => {
    const { response, original } = await submit(link);
    expect(response.status).toBe(400);
    const { error } = await response.json<{ error: string }>();
    expect(error).toContain("http://");
    expect(error).toContain("https://");
    expect(error).toContain("grokbot://");

    if (original) {
      const stored = await SELF.fetch(`http://lam/items/${original.id}`, { headers });
      expect(await stored.json()).toMatchObject({ link: original.link, version: original.version });
    }
  });
});
