import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";

describe("removed articles API", () => {
  for (const path of ["/v2/articles", "/v2/articles/", "/v2/articles/id", "/v2/articles/id/assets/0", "/v2/articles/uploads"]) {
    it.each(["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD"])(`returns Gone for %s ${path}`, async (method) => {
      const response = await SELF.fetch(`http://lam${path}`, { method });
      expect(response.status).toBe(410);
      if (method !== "HEAD") expect(await response.json()).toEqual({ error: "articles removed" });
    });
  }
});
