import { describe, expect, it } from "vitest";
import { Item } from "../src/domain/Item";

const critical = new Item({
  id: "critical-1",
  kind: "request",
  name: "pm",
  title: "Production needs a decision",
  body: "private body",
  source_host: "box",
  source_project: "lam",
  priority: "critical",
  choices: ["Ship", "Wait"],
  checks: [],
  recommendation: "Ship",
  recommended_choice: "Ship",
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

describe("critical push messages", () => {
  it("builds a high-priority data invalidation without private item content", async () => {
    const module = await import("../src/domain/PushMessage").catch(() => null);

    expect(module?.pushMessage("item.created", critical)).toEqual({
      android: { priority: "high" },
      data: {
        event: "item.created",
        item_id: "critical-1",
        version: "0",
        status: "open",
        kind: "request",
        name: "pm",
        priority: "critical",
        title: "Production needs a decision",
      },
    });
  });

  it("keeps non-critical items silent", async () => {
    const module = await import("../src/domain/PushMessage").catch(() => null);

    expect(module?.pushMessage("item.created", new Item({ ...critical, id: "normal-1", priority: "normal" }))).toBeNull();
  });

  it("sends a close invalidation so Android can cancel a critical notification", async () => {
    const module = await import("../src/domain/PushMessage").catch(() => null);
    const closed = new Item({
      ...critical,
      status: "resolved",
      response_choice: "Ship",
      response_by: "cli",
      resolved_at: "2026-09-23T00:01:00.000Z",
      version: 1,
    });

    expect(module?.pushMessage("item.closed", closed)?.data).toEqual({
      event: "item.closed",
      item_id: "critical-1",
      version: "1",
      status: "resolved",
      kind: "request",
      name: "pm",
      priority: "critical",
      title: "Production needs a decision",
    });
  });
});
