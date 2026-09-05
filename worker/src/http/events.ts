import { HttpRouter, HttpServerRequest, HttpServerResponse } from "@effect/platform";
import { Effect } from "effect";
import { Auth } from "../services/Auth";
import { Events } from "../services/Events";

const subscribe = Effect.gen(function* () {
  const request = yield* HttpServerRequest.HttpServerRequest;
  const auth = yield* Auth;
  yield* auth.requireDevice(yield* auth.authenticateBearer(request.headers.authorization));
  if (request.headers.upgrade?.toLowerCase() !== "websocket") {
    return HttpServerResponse.text("WebSocket upgrade required", { status: 426 });
  }
  return HttpServerResponse.raw(yield* (yield* Events).subscribe());
});

export const events = HttpRouter.empty.pipe(HttpRouter.get("/events", subscribe));
