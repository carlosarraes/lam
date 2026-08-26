import { HttpMiddleware, HttpRouter, HttpServerRequest, HttpServerResponse } from "@effect/platform";
import { Effect, Fiber, Option, Schema } from "effect";
import { Exec } from "../Env";
import { Item, NewItem, Resolution, Status } from "../domain/Item";
import { Items } from "../services/Items";
import { Auth } from "../services/Auth";
import { Notify } from "../services/Notify";

const WAIT_MS = 25_000;
const POLL_MS = 2_000;

const IdParam = Schema.Struct({ id: Schema.String });
const CommaNumbers = Schema.transform(Schema.String, Schema.Array(Schema.Number), {
  decode: (s) => s.split(",").map(Number),
  encode: (a) => a.join(","),
});
const WaitMany = Schema.Struct({ ids: Schema.NonEmptyString, since: Schema.optional(CommaNumbers) });

/** A waiter returns when the item closed, or (with `since`) when any mutation bumped the version past it. */
const changed = (item: Item, since: number) => item.status !== "open" || item.version > since;

const bearer = HttpMiddleware.make((app) =>
  Effect.gen(function* () {
    const req = yield* HttpServerRequest.HttpServerRequest;
    yield* (yield* Auth).requireBearer(req.headers.authorization);
    return yield* app;
  }),
);

/** Runs `eff` after the response is sent, tied to the Worker's lifetime via waitUntil (inline when no ExecutionContext). */
export const background = <E, R>(eff: Effect.Effect<void, E, R>) =>
  Effect.gen(function* () {
    const exec = yield* Effect.serviceOption(Exec);
    if (Option.isNone(exec)) return yield* Effect.ignoreLogged(eff);
    const fiber = yield* Effect.forkDaemon(Effect.ignoreLogged(eff));
    exec.value.waitUntil(Effect.runPromise(Fiber.join(fiber)));
  });

/** Public origin for phone-facing URLs; `req.url` is path-only, so read the native Request. */
const origin = Effect.map(HttpServerRequest.HttpServerRequest, (req) =>
  req.source instanceof Request ? new URL(req.source.url).origin : `https://${req.headers.host}`,
);

/** CLI-facing routes, bearer-protected. */
export const api = HttpRouter.empty.pipe(
  HttpRouter.post(
    "/items",
    Effect.gen(function* () {
      const input = yield* HttpServerRequest.schemaBodyJson(NewItem);
      const item = yield* (yield* Items).create(input);
      const baseUrl = yield* origin;
      yield* background((yield* Notify).itemCreated(item, baseUrl));
      return yield* HttpServerResponse.json(item, { status: 201 });
    }),
  ),
  HttpRouter.get(
    "/items",
    Effect.gen(function* () {
      const { status } = yield* HttpServerRequest.schemaSearchParams(Schema.Struct({ status: Schema.optional(Status) }));
      return yield* HttpServerResponse.json(yield* (yield* Items).list(status));
    }),
  ),
  HttpRouter.get(
    "/items/wait",
    Effect.gen(function* () {
      const { ids, since } = yield* HttpServerRequest.schemaSearchParams(WaitMany);
      const wanted = ids.split(",").filter(Boolean);
      const versions = new Map(wanted.map((id, i) => [id, since?.[i] ?? Number.POSITIVE_INFINITY]));
      const items = yield* Items;
      const deadline = Date.now() + WAIT_MS;
      while (true) {
        const hit = (yield* items.list(undefined, wanted)).find((i) => changed(i, versions.get(i.id)!));
        if (hit) return yield* HttpServerResponse.json(hit);
        if (Date.now() + POLL_MS > deadline) return HttpServerResponse.empty({ status: 204 });
        yield* Effect.sleep(POLL_MS);
      }
    }),
  ),
  HttpRouter.get(
    "/items/:id",
    Effect.gen(function* () {
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      return yield* HttpServerResponse.json(yield* (yield* Items).get(id));
    }),
  ),
  HttpRouter.get(
    "/items/:id/wait",
    Effect.gen(function* () {
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const { since = Number.POSITIVE_INFINITY } = yield* HttpServerRequest.schemaSearchParams(Schema.Struct({ since: Schema.optional(Schema.NumberFromString) }));
      const items = yield* Items;
      const deadline = Date.now() + WAIT_MS;
      while (true) {
        const item = yield* items.get(id);
        if (changed(item, since)) return yield* HttpServerResponse.json(item);
        if (Date.now() + POLL_MS > deadline) return HttpServerResponse.empty({ status: 204 });
        yield* Effect.sleep(POLL_MS);
      }
    }),
  ),
  HttpRouter.post(
    "/items/:id/resolve",
    Effect.gen(function* () {
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const res = yield* HttpServerRequest.schemaBodyJson(Resolution).pipe(Effect.orElseSucceed(() => ({} as Resolution)));
      const item = yield* (yield* Items).close(id, { status: "resolved", choice: res.choice, text: res.text, by: "cli" });
      yield* background((yield* Notify).itemClosed(item));
      return yield* HttpServerResponse.json(item);
    }),
  ),
  HttpRouter.post(
    "/items/:id/checks",
    Effect.gen(function* () {
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const { label } = yield* HttpServerRequest.schemaBodyJson(Schema.Struct({ label: Schema.NonEmptyTrimmedString }));
      const item = yield* (yield* Items).addCheck(id, label);
      const baseUrl = yield* origin;
      yield* background((yield* Notify).checkAdded(item, label, baseUrl));
      return yield* HttpServerResponse.json(item);
    }),
  ),
  HttpRouter.post(
    "/items/:id/checks/:index",
    Effect.gen(function* () {
      const { id, index } = yield* HttpRouter.schemaPathParams(Schema.Struct({ id: Schema.String, index: Schema.NumberFromString }));
      const { done } = yield* HttpServerRequest.schemaBodyJson(Schema.Struct({ done: Schema.Boolean }));
      const item = yield* (yield* Items).setCheck(id, index, done, "cli");
      if (item.status !== "open") yield* background((yield* Notify).itemClosed(item));
      return yield* HttpServerResponse.json(item);
    }),
  ),
  HttpRouter.post(
    "/items/:id/retract",
    Effect.gen(function* () {
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const item = yield* (yield* Items).close(id, { status: "retracted", by: "cli" });
      yield* background((yield* Notify).itemClosed(item));
      return yield* HttpServerResponse.json(item);
    }),
  ),
  HttpRouter.post(
    "/items/:id/dismiss",
    Effect.gen(function* () {
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const item = yield* (yield* Items).close(id, { status: "dismissed", by: "cli" });
      yield* background((yield* Notify).itemClosed(item));
      return yield* HttpServerResponse.json(item);
    }),
  ),
  HttpRouter.use(bearer),
);
