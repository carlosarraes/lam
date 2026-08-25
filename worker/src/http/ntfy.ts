import { HttpMiddleware, HttpRouter, HttpServerRequest, HttpServerResponse } from "@effect/platform";
import { Effect, Schema } from "effect";
import { Env } from "../Env";
import { NotFound } from "../domain/Item";
import { draftFromRaw } from "../ntfy/message";
import { TopicClient } from "../services/TopicClient";

const Since = Schema.Struct({ since: Schema.optional(Schema.String), poll: Schema.optional(Schema.String) });

/** Only the configured topic exists; the secret name is the access control. */
const onlyConfiguredTopic = HttpMiddleware.make((app) =>
  Effect.gen(function* () {
    const { topic } = yield* HttpRouter.schemaPathParams(Schema.Struct({ topic: Schema.String }));
    const env = yield* Env;
    if (topic !== env.NTFY_TOPIC) return yield* new NotFound({ id: topic });
    return yield* app;
  }),
);

const publish = Effect.gen(function* () {
  const req = yield* HttpServerRequest.HttpServerRequest;
  const query = yield* HttpServerRequest.schemaSearchParams(Schema.Record({ key: Schema.String, value: Schema.String }));
  const body = yield* req.text;
  const draft = yield* draftFromRaw({ headers: req.headers, query, body });
  return yield* HttpServerResponse.json(yield* (yield* TopicClient).publish(draft));
});

const subscribe = Effect.gen(function* () {
  const req = yield* HttpServerRequest.HttpServerRequest;
  const { since = null, poll } = yield* HttpServerRequest.schemaSearchParams(Since);
  const topic = yield* TopicClient;
  if (poll) {
    const lines = yield* topic.poll(since);
    return HttpServerResponse.text(lines.map((l) => l + "\n").join(""), { contentType: "application/x-ndjson" });
  }
  return HttpServerResponse.raw(yield* topic.subscribe(since, req.headers));
});

/** ntfy-compatible surface consumed by the ntfy Android app and `lam watch`. */
export const ntfy = HttpRouter.empty.pipe(
  HttpRouter.get("/v1/health", HttpServerResponse.unsafeJson({ healthy: true })),
  HttpRouter.mount(
    "/",
    HttpRouter.empty.pipe(
      HttpRouter.get("/:topic/auth", HttpServerResponse.unsafeJson({ success: true })),
      HttpRouter.post("/:topic", publish),
      HttpRouter.put("/:topic", publish),
      HttpRouter.get("/:topic/json", subscribe),
      HttpRouter.get("/:topic/ws", subscribe),
      HttpRouter.use(onlyConfiguredTopic),
    ),
  ),
);
