import { env, SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";

const request = (query: Record<string, string>) => SELF.fetch(`https://example.com/v2/articles?${new URLSearchParams(query)}`, {
  headers: { Authorization: "Bearer test-token" },
});
async function page(query: Record<string, string>) {
  const response = await request(query);
  expect(response.status).toBe(200);
  return response.json<{ items: { id: string }[]; next_cursor: string | null }>();
}
async function article(title: string, created: string, read = false) {
  const id = crypto.randomUUID();
  await env.DB.prepare(`INSERT INTO articles
    (id, owner_id, state, idempotency_key, manifest_hash, title, summary, name, source_host, source_project, silent, created_at, expires_at, sanitized_html_key, cleanup_after, read_at)
    VALUES (?, 'owner', 'published', ?, '', ?, '', '', 'host', 'lam', 1, ?, ?, ?, ?, ?)`)
    .bind(id, id, title, created, created, `articles/${id}/index.html`, created, read ? created : null).run();
  return id;
}

describe("article calendar day query", () => {
  it.each([
    ["2026-09-09", "2026-09-09T02:59:59.999Z", "2026-09-09T03:00:00.000Z", "2026-09-10T02:59:59.999Z", "2026-09-10T03:00:00.000Z"],
    ["2024-02-29", "2024-02-29T02:59:59.999Z", "2024-02-29T03:00:00.000Z", "2024-03-01T02:59:59.999Z", "2024-03-01T03:00:00.000Z"],
    ["2018-11-04", "2018-11-04T02:59:59.999Z", "2018-11-04T03:00:00.000Z", "2018-11-05T01:59:59.999Z", "2018-11-05T02:00:00.000Z"],
    ["2019-02-16", "2019-02-16T01:59:59.999Z", "2019-02-16T02:00:00.000Z", "2019-02-17T02:59:59.999Z", "2019-02-17T03:00:00.000Z"],
  ])("selects the complete São Paulo day %s", async (day, before, start, end, after) => {
    const q = crypto.randomUUID();
    await article(q, before);
    const first = await article(q, start);
    const last = await article(q, end);
    await article(q, after);
    expect((await page({ q, day })).items.map(item => item.id)).toEqual([last, first]);
    expect((await page({ q })).items).toHaveLength(4);
  });

  it.each(["", "2026-9-09", "2026-09-9", "2026-02-29", "1900-02-29", "2026-04-31", "2026-00-10", "2026-13-01", "2026-09-00", "2026-09-32", "0000-01-01", "2026-09-09T00:00:00Z", " 2026-09-09"])("rejects invalid day %j", async day => {
    expect((await request({ day })).status).toBe(400);
  });

  it("filters before pagination and binds cursors to day, search and read state", async () => {
    const q = crypto.randomUUID();
    const day = "2026-09-09";
    const ids = [];
    for (let i = 0; i < 26; i++) ids.push(await article(q, "2026-09-09T12:00:00.000Z"));
    for (let i = 0; i < 26; i++) await article(q, "2026-09-10T12:00:00.000Z");
    const readId = await article(q, "2026-09-09T13:00:00.000Z", true);
    await article("unrelated", "2026-09-09T14:00:00.000Z");
    const first = await page({ q, day, read: "unread" });
    expect(first.items).toHaveLength(25);
    expect(first.next_cursor).toBeTypeOf("string");
    const cursor = first.next_cursor!;
    const second = await page({ q, day, read: "unread", cursor });
    expect(second.items).toHaveLength(1);
    expect(second.next_cursor).toBeNull();
    expect(new Set([...first.items, ...second.items].map(item => item.id))).toEqual(new Set(ids));
    expect((await page({ q, day, read: "read" })).items.map(item => item.id)).toEqual([readId]);
    const mismatches: Record<string, string>[] = [{ q, read: "unread" }, { q, day: "2026-09-10", read: "unread" }, { q: "other", day, read: "unread" }, { q, day, read: "read" }];
    for (const query of mismatches) {
      expect((await request({ ...query, cursor })).status).toBe(400);
    }
    const legacyCursor = btoa(encodeURIComponent(JSON.stringify({ created_at: "2026-09-09T12:00:00.000Z", id: ids.sort().at(-1), q, read: "unread" })))
      .replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    expect((await page({ q, read: "unread", cursor: legacyCursor })).items).toHaveLength(25);
    expect((await request({ q, day, read: "unread", cursor: legacyCursor })).status).toBe(400);
  });
});
