import { HttpApp } from "@effect/platform";
import { Context, Layer } from "effect";
import { Env, Exec, type Bindings } from "./Env";
import { app } from "./http/app";
import { Auth } from "./services/Auth";
import { Devices } from "./services/Devices";
import { Events } from "./services/Events";
import { Items } from "./services/Items";
import { Notify } from "./services/Notify";
import { Pairings } from "./services/Pairings";
import { TopicClient } from "./services/TopicClient";

type Handler = (request: Request, context?: Context.Context<never>) => Promise<Response>;

// One runtime per bindings object: stable across requests in production, fresh per isolated test.
const handlers = new WeakMap<Bindings, Handler>();

function handlerFor(env: Bindings): Handler {
  let h = handlers.get(env);
  if (!h) {
    const services = Layer.mergeAll(Items.Default, Devices.Default, Pairings.Default, Auth.Default, TopicClient.Default, Notify.Default, Events.Default);
    h = HttpApp.toWebHandlerLayer(app, services.pipe(Layer.provideMerge(Layer.succeed(Env, env)))).handler;
    handlers.set(env, h);
  }
  return h;
}

export default {
  fetch: (request: Request, env: Bindings, ctx: ExecutionContext) =>
    handlerFor(env)(request, Context.make(Exec, ctx) as Context.Context<never>),
} satisfies ExportedHandler<Bindings>;

export { Topic } from "./ntfy/topic";
export { EventStream } from "./events/stream";
