import { HttpMiddleware, HttpRouter, HttpServerRequest, HttpServerResponse } from "@effect/platform";
import { Effect, Schema } from "effect";
import { Items } from "../services/Items";
import { Auth } from "../services/Auth";
import { Notify } from "../services/Notify";
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

const replyPage = (id: string, title: string, t: string) => `<!doctype html><meta name=viewport content="width=device-width,initial-scale=1">
<title>Reply · ${title}</title>
<body style="font:16px system-ui;padding:1rem;max-width:32rem;margin:auto">
<h3>${title}</h3><p style="color:#666">${id}</p>
<form method=post action="/r/${id}?t=${t}">
<textarea name=text rows=5 autofocus style="width:100%;font:inherit"></textarea>
<button style="margin-top:.5rem;padding:.6rem 1.2rem;font:inherit">Send</button></form>`;

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
      if (item.status !== "open") return HttpServerResponse.text(`already ${item.status}`, { status: 409 });
      return HttpServerResponse.html(replyPage(item.id, item.title, t ?? ""));
    }),
  ),
  HttpRouter.post(
    "/r/:id",
    Effect.gen(function* () {
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const { text } = yield* HttpServerRequest.schemaBodyUrlParams(Schema.Struct({ text: Schema.NonEmptyTrimmedString }));
      const item = yield* (yield* Items).close(id, { status: "resolved", text: text.trim(), by: "phone" });
      yield* background((yield* Notify).itemClosed(item));
      return HttpServerResponse.text(`sent: ${text}`);
    }),
  ),
  HttpRouter.use(itemToken),
);
