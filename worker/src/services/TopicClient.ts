import { Effect } from "effect";
import { Env } from "../Env";
import type { Draft, Message } from "../ntfy/message";

const stub = Effect.map(Env, (e) => ({ topic: e.NTFY_TOPIC, stub: e.TOPIC.get(e.TOPIC.idFromName(e.NTFY_TOPIC)) }));

/** Client for the single Topic Durable Object. */
export class TopicClient extends Effect.Service<TopicClient>()("lam/TopicClient", {
  succeed: {
    publish: (draft: Draft): Effect.Effect<Message, never, Env> =>
      Effect.flatMap(stub, ({ stub, topic }) => Effect.promise(() => stub.publish(topic, draft))),

    poll: (since: string | null) => Effect.flatMap(stub, ({ stub }) => Effect.promise(() => stub.poll(since))),

    /** Streaming (JSON lines or WebSocket) subscription; returns the DO's native Response. */
    subscribe: (since: string | null, headers: HeadersInit) =>
      Effect.flatMap(stub, ({ stub, topic }) => {
        const url = new URL("https://topic/");
        url.searchParams.set("topic", topic);
        if (since) url.searchParams.set("since", since);
        return Effect.promise(() => stub.fetch(new Request(url, { headers })));
      }),
  },
}) {}
