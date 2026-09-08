import { HttpRouter, HttpServerRequest, HttpServerResponse } from "@effect/platform";
import { Effect } from "effect";
import { Auth } from "../services/Auth";
import { Events } from "../services/Events";

const subscribe = (v2 = false) => Effect.gen(function* () {
  const request = yield* HttpServerRequest.HttpServerRequest;
  const auth = yield* Auth;
  const authority = yield* auth.authenticateBearer(request.headers.authorization);
  if (!v2) yield* auth.requireDevice(authority);
  if (request.headers.upgrade?.toLowerCase() !== "websocket") {
    return HttpServerResponse.text("WebSocket upgrade required", { status: 426 });
  }
  return HttpServerResponse.raw(yield* (yield* Events).subscribe(v2));
});

export const events = HttpRouter.empty.pipe(HttpRouter.get("/events", subscribe()), HttpRouter.get("/v2/events", subscribe(true)));
