import { HttpRouter, HttpServerRequest, HttpServerResponse } from "@effect/platform";
import { Effect, Schema } from "effect";
import { BadRequest } from "../domain/Item";
import { PairingClaimRequest } from "../domain/Pairing";
import { Auth, RequestAuthority } from "../services/Auth";
import { Pairings } from "../services/Pairings";

const requestOrigin = Effect.map(HttpServerRequest.HttpServerRequest, (request) =>
  request.source instanceof Request ? new URL(request.source.url).origin : `https://${request.headers.host}`,
);

const WAIT_MS = 25_000;
const POLL_MS = 250;
const IdParam = Schema.Struct({ id: Schema.String });
const claimRequest = HttpServerRequest.schemaBodyJson(PairingClaimRequest).pipe(
  Effect.catchTags({
    ParseError: () => Effect.fail(new BadRequest({ message: "invalid pairing claim" })),
    RequestError: () => Effect.fail(new BadRequest({ message: "invalid pairing claim" })),
  }),
);

export const pairingAdmin = HttpRouter.empty.pipe(
  HttpRouter.post(
    "/pairings",
    Effect.gen(function* () {
      const authority = yield* RequestAuthority;
      yield* (yield* Auth).requireMaster(authority);
      const created = yield* (yield* Pairings).create(yield* requestOrigin);
      return yield* HttpServerResponse.json(created, { status: 201 });
    }),
  ),
  HttpRouter.get(
    "/pairings/:id/wait",
    Effect.gen(function* () {
      const authority = yield* RequestAuthority;
      yield* (yield* Auth).requireMaster(authority);
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const pairings = yield* Pairings;
      const deadline = Date.now() + WAIT_MS;
      while (true) {
        const current = yield* pairings.status(id);
        if (current.status !== "pending" || Date.now() >= deadline) {
          return yield* HttpServerResponse.json(current);
        }
        yield* Effect.sleep(Math.min(POLL_MS, deadline - Date.now()));
      }
    }),
  ),
  HttpRouter.del(
    "/pairings/:id",
    Effect.gen(function* () {
      const authority = yield* RequestAuthority;
      yield* (yield* Auth).requireMaster(authority);
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      return yield* HttpServerResponse.json(yield* (yield* Pairings).cancel(id));
    }),
  ),
);

export const pairingClaims = HttpRouter.empty.pipe(
  HttpRouter.post(
    "/pairings/:id/claim",
    Effect.gen(function* () {
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const { secret, ...registration } = yield* claimRequest;
      const claimed = yield* (yield* Pairings).claim(id, secret, registration);
      return yield* HttpServerResponse.json(claimed, { status: 201 });
    }),
  ),
);
