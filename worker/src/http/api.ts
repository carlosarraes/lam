import { HttpMiddleware, HttpRouter, HttpServerRequest, HttpServerResponse } from "@effect/platform";
import { Effect, Fiber, Option, Schema } from "effect";
import { Exec } from "../Env";
import { BadRequest, CheckLabel, Item, NewItem, NewTypedItem, Priority, Resolution, Status } from "../domain/Item";
import { Items } from "../services/Items";
import { Auth, RequestAuthority } from "../services/Auth";
import { Events } from "../services/Events";
import { Notify } from "../services/Notify";

const WAIT_MS = 25_000;
const POLL_MS = 2_000;

const IdParam = Schema.Struct({ id: Schema.String });
const CommaNumbers = Schema.transform(Schema.String, Schema.Array(Schema.Number), {
  decode: (s) => s.split(",").map(Number),
  encode: (a) => a.join(","),
});
const WaitMany = Schema.Struct({ ids: Schema.NonEmptyString, since: Schema.optional(CommaNumbers) });
/** Every field optional, and absent means today's behaviour — binaries in the field send none of them. */
const ListParams = Schema.Struct({
  status: Schema.optional(Status),
  limit: Schema.optional(Schema.NumberFromString.pipe(Schema.int(), Schema.between(1, 500))),
  before: Schema.optional(Schema.String),
});
const HistoryParams = Schema.Struct({
  limit: Schema.optionalWith(Schema.NumberFromString.pipe(Schema.int(), Schema.between(1, 500)), { default: () => 50 }),
  cursor: Schema.optional(Schema.String),
  q: Schema.optional(Schema.String),
  priority: Schema.optional(Priority),
  type: Schema.optional(Schema.Literal("plain", "choice", "checklist")),
});
const strict = { onExcessProperty: "error" as const };

/** A waiter returns when the item closed, or (with `since`) when any mutation bumped the version past it. */
const changed = (item: Item, since: number) => item.status !== "open" || item.version > since;

/** Authenticates once, then makes the typed authority available to route handlers. */
export const bearer = HttpMiddleware.make((app) =>
  Effect.gen(function* () {
    const req = yield* HttpServerRequest.HttpServerRequest;
    const authority = yield* (yield* Auth).authenticateBearer(req.headers.authorization);
    return yield* Effect.provideService(app, RequestAuthority, authority);
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
    "/v2/items",
    Effect.gen(function* () {
      const authority = yield* RequestAuthority;
      yield* (yield* Auth).requireMaster(authority);
      const decoded = yield* HttpServerRequest.schemaBodyJson(NewTypedItem, strict);
      const kind = decoded.kind;
      let input: NewItem;
      if (decoded.kind === "fyi") {
        const { kind: _kind, ...fields } = decoded;
        input = { ...fields, choices: [], checks: [], recommendation: null, recommended_choice: null };
      } else {
        const { kind: _kind, ...fields } = decoded;
        input = { ...fields, recommendation: fields.recommendation ?? null, recommended_choice: fields.recommended_choice ?? null };
      }
      const items = yield* Items;
      const existing = yield* items.findDuplicate(input, kind);
      if (Option.isSome(existing)) return yield* HttpServerResponse.json(existing.value, { status: 200 });
      const item = yield* items.create(input, kind);
      const baseUrl = yield* origin;
      yield* background((yield* Events).publish({ event: "item.created", item_id: item.id, version: item.version, status: item.status }));
      yield* background((yield* Notify).itemCreated(item, baseUrl));
      return yield* HttpServerResponse.json(item, { status: 201 });
    }),
  ),
  HttpRouter.post(
    "/v2/items/:id/seen",
    Effect.gen(function* () {
      yield* RequestAuthority;
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const { version } = yield* HttpServerRequest.schemaBodyJson(
        Schema.Struct({ version: Schema.Int.pipe(Schema.nonNegative()) }),
        strict,
      );
      const { item, changed } = yield* (yield* Items).seen(id, version);
      if (changed) {
        yield* background((yield* Events).publish({ event: "item.closed", item_id: item.id, version: item.version, status: item.status }));
      }
      return yield* HttpServerResponse.json(item);
    }),
  ),
  HttpRouter.post(
    "/items",
    Effect.gen(function* () {
      const authority = yield* RequestAuthority;
      yield* (yield* Auth).requireMaster(authority);
      const input = yield* HttpServerRequest.schemaBodyJson(NewItem);
      const items = yield* Items;
      // A retry of a push whose response was lost must not queue (and notify) twice.
      const existing = yield* items.findDuplicate(input);
      if (Option.isSome(existing)) return yield* HttpServerResponse.json(existing.value, { status: 200 });
      const item = yield* items.create(input);
      const baseUrl = yield* origin;
      yield* background((yield* Notify).itemCreated(item, baseUrl));
      return yield* HttpServerResponse.json(item, { status: 201 });
    }),
  ),
  HttpRouter.get(
    "/items",
    Effect.gen(function* () {
      yield* RequestAuthority;
      const query = yield* HttpServerRequest.schemaSearchParams(ListParams);
      return yield* HttpServerResponse.json(yield* (yield* Items).list(query));
    }),
  ),
  HttpRouter.get(
    "/history",
    Effect.gen(function* () {
      yield* RequestAuthority;
      const query = yield* HttpServerRequest.schemaSearchParams(HistoryParams);
      return yield* HttpServerResponse.json(yield* (yield* Items).history(query));
    }),
  ),
  HttpRouter.get(
    "/items/wait",
    Effect.gen(function* () {
      yield* RequestAuthority;
      const { ids, since } = yield* HttpServerRequest.schemaSearchParams(WaitMany);
      const wanted = ids.split(",").filter(Boolean);
      const versions = new Map(wanted.map((id, i) => [id, since?.[i] ?? Number.POSITIVE_INFINITY]));
      const items = yield* Items;
      const deadline = Date.now() + WAIT_MS;
      while (true) {
        const listed = yield* items.list({ ids: wanted });
        const requests = listed.filter((item) => item.kind === "request");
        const hit = requests.find((item) => changed(item, versions.get(item.id)!));
        if (hit) return yield* HttpServerResponse.json(hit);
        if (listed.length > 0 && requests.length === 0) return HttpServerResponse.empty({ status: 204 });
        if (Date.now() + POLL_MS > deadline) return HttpServerResponse.empty({ status: 204 });
        yield* Effect.sleep(POLL_MS);
      }
    }),
  ),
  HttpRouter.get(
    "/items/:id",
    Effect.gen(function* () {
      yield* RequestAuthority;
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      return yield* HttpServerResponse.json(yield* (yield* Items).get(id));
    }),
  ),
  HttpRouter.get(
    "/items/:id/wait",
    Effect.gen(function* () {
      yield* RequestAuthority;
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const { since = Number.POSITIVE_INFINITY } = yield* HttpServerRequest.schemaSearchParams(Schema.Struct({ since: Schema.optional(Schema.NumberFromString) }));
      const items = yield* Items;
      const deadline = Date.now() + WAIT_MS;
      while (true) {
        const item = yield* items.get(id);
        if (item.kind === "fyi") return yield* new BadRequest({ message: "FYIs cannot wait for a decision" });
        if (changed(item, since)) return yield* HttpServerResponse.json(item);
        if (Date.now() + POLL_MS > deadline) return HttpServerResponse.empty({ status: 204 });
        yield* Effect.sleep(POLL_MS);
      }
    }),
  ),
  HttpRouter.post(
    "/items/:id/resolve",
    Effect.gen(function* () {
      const authority = yield* RequestAuthority;
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const req = yield* HttpServerRequest.HttpServerRequest;
      // Older callers send an empty POST to mark an item done. A present body must still decode.
      const res = req.source instanceof Request && req.source.body === null ? ({} as Resolution) : yield* HttpServerRequest.schemaBodyJson(Resolution);
      const item = yield* (yield* Items).close(id, { status: "resolved", choice: res.choice, text: res.text, by: authority.kind === "device" ? "phone" : "cli" });
      yield* background((yield* Notify).itemClosed(item));
      return yield* HttpServerResponse.json(item);
    }),
  ),
  HttpRouter.post(
    "/items/:id/checks",
    Effect.gen(function* () {
      const authority = yield* RequestAuthority;
      yield* (yield* Auth).requireMaster(authority);
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const { label } = yield* HttpServerRequest.schemaBodyJson(Schema.Struct({ label: CheckLabel }));
      const item = yield* (yield* Items).addCheck(id, label);
      const baseUrl = yield* origin;
      yield* background((yield* Notify).checkAdded(item, label, baseUrl));
      return yield* HttpServerResponse.json(item);
    }),
  ),
  HttpRouter.post(
    "/items/:id/checks/:index",
    Effect.gen(function* () {
      const authority = yield* RequestAuthority;
      const { id, index } = yield* HttpRouter.schemaPathParams(Schema.Struct({ id: Schema.String, index: Schema.NumberFromString }));
      const { done } = yield* HttpServerRequest.schemaBodyJson(Schema.Struct({ done: Schema.Boolean }));
      const item = yield* (yield* Items).setCheck(id, index, done, authority.kind === "device" ? "phone" : "cli");
      if (item.status !== "open") yield* background((yield* Notify).itemClosed(item));
      return yield* HttpServerResponse.json(item);
    }),
  ),
  HttpRouter.post(
    "/items/:id/retract",
    Effect.gen(function* () {
      const authority = yield* RequestAuthority;
      yield* (yield* Auth).requireMaster(authority);
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const item = yield* (yield* Items).close(id, { status: "retracted", by: "cli" });
      yield* background((yield* Notify).itemClosed(item));
      return yield* HttpServerResponse.json(item);
    }),
  ),
  HttpRouter.post(
    "/items/:id/dismiss",
    Effect.gen(function* () {
      const authority = yield* RequestAuthority;
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const item = yield* (yield* Items).close(id, { status: "dismissed", by: authority.kind === "device" ? "phone" : "cli" });
      yield* background((yield* Notify).itemClosed(item));
      return yield* HttpServerResponse.json(item);
    }),
  ),
);
