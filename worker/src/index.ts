import { Hono, type Context } from "hono";
import { bearerOk, verifyItemToken } from "./auth";
import * as items from "./items";
import { publishClosed, publishNew } from "./ntfy";
import { MAX_CHOICES, PRIORITIES, type Env, type NewItem, type Status } from "./types";

const WAIT_MS = 25_000;
const POLL_MS = 2_000;

const app = new Hono<{ Bindings: Env }>();

// CLI-facing routes: bearer token.
const api = new Hono<{ Bindings: Env }>().basePath("/items");
api.use(async (c, next) => {
  if (!bearerOk(c.req.header("Authorization"), c.env.LAM_TOKEN)) return c.text("unauthorized", 401);
  await next();
});

function validate(input: unknown): NewItem | string {
  if (!input || typeof input !== "object") return "body must be an object";
  const b = input as Record<string, unknown>;
  if (typeof b.title !== "string" || !b.title.trim()) return "title is required";
  if (b.priority !== undefined && !PRIORITIES.includes(b.priority as never)) return `priority must be one of ${PRIORITIES.join("|")}`;
  if (b.choices !== undefined) {
    if (!Array.isArray(b.choices) || !b.choices.every((c) => typeof c === "string" && c.trim())) return "choices must be non-empty strings";
    if (b.choices.length > MAX_CHOICES) return `at most ${MAX_CHOICES} choices`;
  }
  return b as unknown as NewItem;
}

api.post("/", async (c) => {
  const parsed = validate(await c.req.json().catch(() => null));
  if (typeof parsed === "string") return c.json({ error: parsed }, 400);
  const item = await items.create(c.env.DB, parsed);
  c.executionCtx.waitUntil(publishNew(c.env, item, new URL(c.req.url).origin));
  return c.json(item, 201);
});

api.get("/", async (c) => {
  const status = c.req.query("status") as Status | undefined;
  return c.json(await items.list(c.env.DB, status));
});

api.get("/:id", async (c) => {
  const item = await items.get(c.env.DB, c.req.param("id"));
  return item ? c.json(item) : c.json({ error: "not found" }, 404);
});

api.get("/:id/wait", async (c) => {
  const id = c.req.param("id");
  const deadline = Date.now() + WAIT_MS;
  for (;;) {
    const item = await items.get(c.env.DB, id);
    if (!item) return c.json({ error: "not found" }, 404);
    if (item.status !== "open") return c.json(item);
    if (Date.now() + POLL_MS > deadline) return c.body(null, 204);
    await new Promise((r) => setTimeout(r, POLL_MS));
  }
});

async function closeAndNotify(c: { env: Env; executionCtx: { waitUntil(p: Promise<unknown>): void } }, id: string, status: "resolved" | "dismissed", res: items.Resolution) {
  const item = await items.close(c.env.DB, id, status, res);
  if (item) c.executionCtx.waitUntil(publishClosed(c.env, item));
  return item;
}

api.post("/:id/resolve", async (c) => {
  const body = (await c.req.json().catch(() => ({}))) as { choice?: string; text?: string };
  const item = await closeAndNotify(c, c.req.param("id"), "resolved", { choice: body.choice, text: body.text, by: "cli" });
  return item ? c.json(item) : c.json({ error: "not found or already closed" }, 409);
});

api.post("/:id/dismiss", async (c) => {
  const item = await closeAndNotify(c, c.req.param("id"), "dismissed", { by: "cli" });
  return item ? c.json(item) : c.json({ error: "not found or already closed" }, 409);
});

app.route("/", api);

// Phone-facing routes: authenticated by per-item HMAC token, not the bearer.
app.use("/a/:id/*", tokenGuard);
app.use("/r/:id", tokenGuard);
async function tokenGuard(c: Context<{ Bindings: Env }>, next: () => Promise<void>) {
  if (!(await verifyItemToken(c.env.LAM_HMAC_SECRET, c.req.param("id") ?? "", c.req.query("t")))) return c.text("forbidden", 403);
  await next();
}

app.get("/a/:id/:choice", async (c) => {
  const choice = decodeURIComponent(c.req.param("choice"));
  const item = await closeAndNotify(c, c.req.param("id"), "resolved", { choice: choice === "Done" ? undefined : choice, by: "phone" });
  return item ? c.text(`ok: ${item.title} → ${choice}`) : c.text("already closed", 409);
});

const replyPage = (id: string, title: string, t: string) => `<!doctype html><meta name=viewport content="width=device-width,initial-scale=1">
<title>Reply · ${title}</title>
<body style="font:16px system-ui;padding:1rem;max-width:32rem;margin:auto">
<h3>${title}</h3><p style="color:#666">${id}</p>
<form method=post action="/r/${id}?t=${t}">
<textarea name=text rows=5 autofocus style="width:100%;font:inherit"></textarea>
<button style="margin-top:.5rem;padding:.6rem 1.2rem;font:inherit">Send</button></form>`;

app.get("/r/:id", async (c) => {
  const item = await items.get(c.env.DB, c.req.param("id"));
  if (!item) return c.text("not found", 404);
  if (item.status !== "open") return c.text(`already ${item.status}`, 409);
  return c.html(replyPage(item.id, item.title, c.req.query("t") ?? ""));
});

app.post("/r/:id", async (c) => {
  const form = await c.req.parseBody();
  const text = typeof form.text === "string" ? form.text.trim() : "";
  if (!text) return c.text("text required", 400);
  const item = await closeAndNotify(c, c.req.param("id"), "resolved", { text, by: "phone" });
  return item ? c.text(`sent: ${text}`) : c.text("already closed", 409);
});

export default app;
