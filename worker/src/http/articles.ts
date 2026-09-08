import { HttpRouter, HttpServerRequest, HttpServerResponse } from "@effect/platform";
import { Effect, Schema } from "effect";
import { HTML_BYTES, MANIFEST_BYTES, readBoundedBody, validateManifest } from "../articles/manifest";
import { ReadUpdate } from "../domain/Article";
import { BadRequest } from "../domain/Item";
import { Articles } from "../services/Articles";
import { Auth, RequestAuthority } from "../services/Auth";
import { viewerParts } from "./article-viewer";

const Id = Schema.Struct({ id: Schema.String.pipe(Schema.pattern(/^[a-f0-9-]{36}$/)) });
const AssetId = Schema.Struct({ ...Id.fields, index: Schema.NumberFromString.pipe(Schema.int(), Schema.between(0, 50)) });
const Query = Schema.Struct({ q: Schema.optional(Schema.String), read: Schema.optional(Schema.Literal("all", "read", "unread")), cursor: Schema.optional(Schema.String) });
const strict = { onExcessProperty: "error" as const };
const master = Effect.gen(function* () { yield* (yield* Auth).requireMaster(yield* RequestAuthority); });
const body = Effect.gen(function* () {
  const request = yield* HttpServerRequest.HttpServerRequest;
  if (!(request.source instanceof Request)) return yield* new BadRequest({ message: "native request required" });
  return request.source.body;
});
const jsonBody = (limit: number) => Effect.gen(function* () {
  const stream = yield* body;
  return yield* Effect.tryPromise({ try: async () => JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(await readBoundedBody(stream, limit))),
    catch: cause => cause instanceof BadRequest ? cause : new BadRequest({ message: "invalid JSON body" }) });
});
const json = (value: unknown, status = 200) => HttpServerResponse.json(value, { status, headers: { "cache-control": "private, no-store" } });

export const articles = HttpRouter.empty.pipe(
  HttpRouter.post("/v2/articles/uploads", Effect.gen(function* () {
    yield* master;
    const request = yield* HttpServerRequest.HttpServerRequest;
    const input = yield* jsonBody(MANIFEST_BYTES);
    const draft = yield* Effect.try({ try: () => validateManifest(input), catch: cause => cause instanceof BadRequest ? cause : new BadRequest({ message: "invalid manifest" }) });
    return yield* json(yield* (yield* Articles).stage(draft, request.headers["idempotency-key"]), 201);
  })),
  HttpRouter.put("/v2/articles/:id/assets/:index", Effect.gen(function* () {
    yield* master;
    const { id, index } = yield* HttpRouter.schemaPathParams(AssetId);
    yield* (yield* Articles).upload(id, index, yield* body);
    return HttpServerResponse.empty({ status: 204 });
  })),
  HttpRouter.post("/v2/articles/:id/publish", Effect.gen(function* () {
    yield* master;
    const { id } = yield* HttpRouter.schemaPathParams(Id);
    return yield* json(yield* (yield* Articles).publish(id));
  })),
  HttpRouter.get("/v2/articles", Effect.gen(function* () {
    yield* RequestAuthority;
    const query = yield* HttpServerRequest.schemaSearchParams(Query, strict);
    return yield* json(yield* (yield* Articles).list(query));
  })),
  HttpRouter.get("/v2/articles/:id", Effect.gen(function* () {
    yield* RequestAuthority;
    const { id } = yield* HttpRouter.schemaPathParams(Id);
    return yield* json(yield* (yield* Articles).get(id));
  })),
  HttpRouter.get("/v2/articles/:id/assets/:index", Effect.gen(function* () {
    yield* RequestAuthority;
    const { id, index } = yield* HttpRouter.schemaPathParams(AssetId);
    const { asset, object } = yield* (yield* Articles).asset(id, index);
    const contentType = asset.media_type.startsWith("text/") ? `${asset.media_type}; charset=utf-8` : asset.media_type;
    return HttpServerResponse.raw(object.body, { headers: {
      "content-type": contentType,
      "content-length": String(object.size),
      "content-disposition": `${asset.disposition}; filename="asset-${index}"; filename*=UTF-8''${encodeURIComponent(asset.path.split("/").at(-1)!).replace(/'/g, "%27")}`,
      "cache-control": "private, no-store",
      "x-content-type-options": "nosniff",
      "content-security-policy": "default-src 'none'; sandbox",
    } });
  })),
  HttpRouter.get("/v2/articles/:id/content", Effect.gen(function* () {
    yield* RequestAuthority;
    const { id } = yield* HttpRouter.schemaPathParams(Id);
    const service = yield* Articles;
    const article = yield* service.get(id);
    const { object } = yield* service.asset(id, article.assets.findIndex(asset => asset.path === "index.html"));
    const source = yield* Effect.tryPromise({
      try: async () => new TextDecoder("utf-8", { fatal: true }).decode(await readBoundedBody(object.body, HTML_BYTES)),
      catch: () => new BadRequest({ message: "article content unavailable" }),
    });
    const parts = yield* Effect.try({ try: () => viewerParts(source, article.assets, "native"), catch: () => new BadRequest({ message: "article content unavailable" }) });
    return yield* json({ article, parts });
  })),
  HttpRouter.put("/v2/articles/:id/read", Effect.gen(function* () {
    yield* RequestAuthority;
    const { id } = yield* HttpRouter.schemaPathParams(Id);
    const input = yield* jsonBody(1024);
    const update = yield* Schema.decodeUnknown(ReadUpdate)(input, strict);
    return yield* json(yield* (yield* Articles).setRead(id, update));
  })),
  HttpRouter.use(Effect.provide(Articles.Default)),
);
