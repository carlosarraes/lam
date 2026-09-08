import { Data, Effect } from "effect";
import { Env, type Bindings } from "../Env";
import type { ArticleEvent, ItemEvent } from "../domain/Event";

const stub = Effect.map(Env, (env) => env.EVENTS.get(env.EVENTS.idFromName("global")));

/** Deliberately excludes transport errors, which can contain credentials. */
export class EventPublishError extends Data.TaggedError("EventPublishError")<{}> {}

/** The canonical write has committed. Loss here is repaired by reconnect/foreground refresh. */
export async function articleInvalidated(env: Bindings, event: ArticleEvent): Promise<void> {
  try { await env.EVENTS.get(env.EVENTS.idFromName("global")).publishV2(event); }
  catch { console.warn("Article invalidation delivery failed"); }
}

export class Events extends Effect.Service<Events>()("lam/Events", {
  succeed: {
    publish: (event: ItemEvent) => Effect.flatMap(Env, (env) => Effect.tryPromise({
      try: () => env.EVENTS.get(env.EVENTS.idFromName("global")).publish(event),
      catch: () => new EventPublishError(),
    })),

    subscribe: (v2 = false) => Effect.flatMap(stub, (stream) =>
      Effect.promise(() => stream.fetch(new Request(v2 ? "https://events/v2" : "https://events/", { headers: { Upgrade: "websocket" } })))),
  },
}) {}
