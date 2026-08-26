import { HttpRouter, HttpServerResponse } from "@effect/platform";
import { Effect } from "effect";
import { api } from "./api";
import { ntfy } from "./ntfy";
import { phone } from "./phone";

const error = (status: number, message: string) => HttpServerResponse.unsafeJson({ error: message }, { status });

/** Maps domain/tagged errors to HTTP responses; anything else is a 500 with the cause logged. */
export const app = HttpRouter.empty.pipe(
  HttpRouter.concat(api),
  HttpRouter.concat(phone),
  HttpRouter.concat(ntfy),
  HttpRouter.catchTags({
    Unauthorized: () => Effect.succeed(error(401, "unauthorized")),
    Forbidden: () => Effect.succeed(error(403, "forbidden")),
    NotFound: () => Effect.succeed(error(404, "not found")),
    AlreadyClosed: () => Effect.succeed(error(409, "already closed")),
    Conflict: () => Effect.succeed(error(409, "concurrent update, retry")),
    BadRequest: (e) => Effect.succeed(error(400, e.message)),
    ParseError: (e) => Effect.succeed(error(400, e.message)),
    RequestError: (e) => Effect.succeed(error(400, e.message)),
    RouteNotFound: () => Effect.succeed(error(404, "no such route")),
    DbError: (e) => Effect.logError("db error", e.cause).pipe(Effect.as(error(500, "database error"))),
  }),
);
