import { Headers, HttpMiddleware, HttpRouter, HttpServerRequest, HttpServerResponse } from "@effect/platform";
import { Effect, Schema } from "effect";
import { ReplyText, type Item } from "../domain/Item";
import { Items } from "../services/Items";
import { Auth } from "../services/Auth";
import { Notify } from "../services/Notify";
import { Events } from "../services/Events";
import { background } from "./api";

const IdParam = Schema.Struct({ id: Schema.String });
const Token = Schema.Struct({ t: Schema.optional(Schema.String) });

/** Phone-facing routes authenticated by the per-item HMAC token in `?t=`. */
const itemToken = HttpMiddleware.make((app) =>
  Effect.gen(function* () {
    const { id } = yield* HttpRouter.schemaPathParams(IdParam);
    const { t } = yield* HttpServerRequest.schemaSearchParams(Token);
    yield* (yield* Auth).requireItemToken(id, t);
    return yield* app;
  }),
);

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");

// This script is fixed Worker code. Item content is escaped text, never executable HTML.
const seenOnVisibleRender = `<script>
(() => {
  const form = document.getElementById("seen-form");
  let attempted = false;
  const visible = () => document.readyState === "complete" && document.visibilityState === "visible" && !document.prerendering;
  const markSeen = () => {
    if (attempted || !visible()) return;
    requestAnimationFrame(() => requestAnimationFrame(async () => {
      if (attempted || !visible()) return;
      attempted = true;
      try {
        const response = await fetch(form.action, { method: "POST", body: new URLSearchParams(new FormData(form)) });
        if (!response.ok) throw new Error("Seen failed");
        document.getElementById("read-status").textContent = "Seen";
        document.getElementById("fyi-actions").hidden = true;
      } catch {
        document.getElementById("read-status").textContent = "Could not mark seen. Use Mark seen to retry.";
      }
    }));
  };
  window.addEventListener("load", markSeen);
  document.addEventListener("visibilitychange", markSeen);
  document.addEventListener("prerenderingchange", markSeen);
})();
</script>`;

const fyiPage = (item: Item, t: string) => `<!doctype html><meta name=viewport content="width=device-width,initial-scale=1">
<title>${esc(item.title)}</title>
<body style="font:16px system-ui;padding:1rem;max-width:32rem;margin:auto">
<p>FYI</p><h3>${esc(item.title)}</h3>
<p style="white-space:pre-wrap;overflow-wrap:anywhere">${esc(item.body)}</p>
${item.link ? `<p><a href="${esc(item.link)}">${esc(item.link)}</a></p>` : ""}
<p id=read-status role=status>${item.seen_at ? "Seen" : item.status === "open" ? "Mark seen after reading." : esc(item.status)}</p>
${item.status === "open" ? `<div id=fyi-actions>
<form id=seen-form method=post action="/r/${item.id}/seen?t=${t}">
<input type=hidden name=version value="${item.version}">
<button style="padding:.6rem 1.2rem;font:inherit">Mark seen</button></form>
<form method=post action="/r/${item.id}/dismiss?t=${t}" style="margin-top:.5rem">
<button style="padding:.6rem 1.2rem;font:inherit">Dismiss</button></form></div>${seenOnVisibleRender}` : ""}
</body>`;

/** Item page: checklist (one form per check, so it works without JS) plus the free-text reply. */
const itemPage = (item: Item, t: string) => {
  const checks = item.checks
    .map(
      (c, i) => `<form method=post action="/r/${item.id}/checks/${i}?t=${t}" style="margin:.25rem 0">
<input type=hidden name=done value="${c.done ? "false" : "true"}">
<button style="width:100%;text-align:left;padding:.6rem;font:inherit;${c.done ? "color:#888;text-decoration:line-through" : ""}">${c.done ? "☑" : "☐"} ${esc(c.label)}</button></form>`,
    )
    .join("");
  const progress = item.checks.length ? `<p>${item.checksDone}/${item.checks.length} done</p>` : "";
  return `<!doctype html><meta name=viewport content="width=device-width,initial-scale=1">
<title>${esc(item.title)}</title>
<body style="font:16px system-ui;padding:1rem;max-width:32rem;margin:auto">
<h3>${esc(item.title)}</h3><p style="color:#666">${item.id}${item.body ? " · " + esc(item.body) : ""}</p>
${item.link ? `<p><a href="${esc(item.link)}">${esc(item.link)}</a></p>` : ""}
${progress}${checks}
<form method=post action="/r/${item.id}?t=${t}" style="margin-top:1rem">
<textarea name=text rows=4 placeholder="reply" style="width:100%;font:inherit"></textarea>
<button style="margin-top:.5rem;padding:.6rem 1.2rem;font:inherit">Send</button></form>`;
};

// ntfy `http` action buttons POST by default; GET is kept for manual use.
const pressButton = Effect.gen(function* () {
      const { id, choice } = yield* HttpRouter.schemaPathParams(Schema.Struct({ id: Schema.String, choice: Schema.String }));
      const label = decodeURIComponent(choice);
      const item = yield* (yield* Items).close(id, { status: "resolved", choice: label === "Done" ? undefined : label, by: "phone" });
      yield* background((yield* Notify).itemClosed(item));
      return HttpServerResponse.text(`ok: ${item.title} → ${label}`);
    });

export const phone = HttpRouter.empty.pipe(
  HttpRouter.get("/a/:id/:choice", pressButton),
  HttpRouter.post("/a/:id/:choice", pressButton),
  HttpRouter.get(
    "/r/:id",
    Effect.gen(function* () {
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const { t } = yield* HttpServerRequest.schemaSearchParams(Token);
      const item = yield* (yield* Items).get(id);
      if (item.kind === "fyi") return HttpServerResponse.html(fyiPage(item, t ?? "")).pipe(
        HttpServerResponse.setHeaders({
          "cache-control": "no-store",
          "referrer-policy": "no-referrer",
          "content-security-policy": "frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        }),
      );
      if (item.status !== "open") return HttpServerResponse.text(`already ${item.status}`, { status: 409 });
      return HttpServerResponse.html(itemPage(item, t ?? ""));
    }),
  ),
  HttpRouter.post(
    "/r/:id/seen",
    Effect.gen(function* () {
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const { version } = yield* HttpServerRequest.schemaBodyUrlParams(
        Schema.Struct({ version: Schema.NumberFromString.pipe(Schema.int(), Schema.nonNegative()) }),
      );
      const { item, changed } = yield* (yield* Items).seen(id, version);
      if (changed) {
        yield* background((yield* Events).publish({ event: "item.closed", item_id: item.id, version: item.version, status: item.status }));
      }
      return HttpServerResponse.text(`Seen: ${item.title}`);
    }),
  ),
  HttpRouter.post(
    "/r/:id/dismiss",
    Effect.gen(function* () {
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const item = yield* (yield* Items).close(id, { status: "dismissed", by: "phone" });
      yield* background((yield* Notify).itemClosed(item));
      return HttpServerResponse.text(`Dismissed: ${item.title}`);
    }),
  ),
  HttpRouter.post(
    "/r/:id/checks/:index",
    Effect.gen(function* () {
      const { id, index } = yield* HttpRouter.schemaPathParams(Schema.Struct({ id: Schema.String, index: Schema.NumberFromString }));
      const { t } = yield* HttpServerRequest.schemaSearchParams(Token);
      const { done } = yield* HttpServerRequest.schemaBodyUrlParams(Schema.Struct({ done: Schema.Literal("true", "false") }));
      const item = yield* (yield* Items).setCheck(id, index, done === "true", "phone");
      if (item.status !== "open") {
        yield* background((yield* Notify).itemClosed(item));
        return HttpServerResponse.text(`done: ${item.title}`);
      }
      return HttpServerResponse.empty({ status: 303, headers: Headers.fromInput({ location: `/r/${id}?t=${t ?? ""}` }) });
    }),
  ),
  HttpRouter.post(
    "/r/:id",
    Effect.gen(function* () {
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const { text } = yield* HttpServerRequest.schemaBodyUrlParams(Schema.Struct({ text: ReplyText }));
      const item = yield* (yield* Items).close(id, { status: "resolved", text: text.trim(), by: "phone" });
      yield* background((yield* Notify).itemClosed(item));
      return HttpServerResponse.text(`sent: ${text}`);
    }),
  ),
  HttpRouter.use(itemToken),
);
