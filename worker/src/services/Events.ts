import { Data, Effect } from "effect";
import { Env } from "../Env";
import type { ItemEvent } from "../domain/Event";

const stub = Effect.map(Env, (env) => env.EVENTS.get(env.EVENTS.idFromName("global")));

/** Deliberately excludes transport errors, which can contain credentials. */
export class EventPublishError extends Data.TaggedError("EventPublishError")<{}> {}

export class Events extends Effect.Service<Events>()("lam/Events", {
  succeed: {
    publish: (event: ItemEvent) => Effect.flatMap(Env, (env) => Effect.tryPromise({
      try: () => env.EVENTS.get(env.EVENTS.idFromName("global")).publish(event),
      catch: () => new EventPublishError(),
    })),

    subscribe: () => Effect.flatMap(stub, (stream) =>
      Effect.promise(() => stream.fetch(new Request("https://events/", { headers: { Upgrade: "websocket" } })))),
  },
}) {}
