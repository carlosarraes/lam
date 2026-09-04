import { HttpRouter, HttpServerRequest, HttpServerResponse } from "@effect/platform";
import { Effect, Schema } from "effect";
import { DeviceName } from "../domain/Device";
import { Auth, RequestAuthority } from "../services/Auth";
import { Devices } from "../services/Devices";

const IdParam = Schema.Struct({ id: Schema.String });
const Rename = Schema.Struct({ name: DeviceName });
const SelfUpdate = Schema.Struct({
  name: Schema.optional(DeviceName),
  fcm_token: Schema.optional(Schema.NullOr(Schema.String)),
  app_version: Schema.optional(Schema.NonEmptyString),
  android_version: Schema.optional(Schema.NonEmptyString),
});
const strict = { onExcessProperty: "error" as const };

/** Device owner administration and the narrow self-service registration API. */
export const devices = HttpRouter.empty.pipe(
  HttpRouter.get(
    "/devices",
    Effect.gen(function* () {
      const authority = yield* RequestAuthority;
      yield* (yield* Auth).requireMaster(authority);
      return yield* HttpServerResponse.json(yield* (yield* Devices).list());
    }),
  ),
  HttpRouter.patch(
    "/devices/:id",
    Effect.gen(function* () {
      const authority = yield* RequestAuthority;
      yield* (yield* Auth).requireMaster(authority);
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      const { name } = yield* HttpServerRequest.schemaBodyJson(Rename, strict);
      return yield* HttpServerResponse.json(yield* (yield* Devices).rename(id, name));
    }),
  ),
  HttpRouter.del(
    "/devices/:id",
    Effect.gen(function* () {
      const authority = yield* RequestAuthority;
      yield* (yield* Auth).requireMaster(authority);
      const { id } = yield* HttpRouter.schemaPathParams(IdParam);
      return yield* HttpServerResponse.json(yield* (yield* Devices).revoke(id));
    }),
  ),
  HttpRouter.get(
    "/device",
    Effect.gen(function* () {
      const authority = yield* RequestAuthority;
      const device = yield* (yield* Auth).requireDevice(authority);
      return yield* HttpServerResponse.json(device.device);
    }),
  ),
  HttpRouter.patch(
    "/device",
    Effect.gen(function* () {
      const authority = yield* RequestAuthority;
      const device = yield* (yield* Auth).requireDevice(authority);
      const update = yield* HttpServerRequest.schemaBodyJson(SelfUpdate, strict);
      return yield* HttpServerResponse.json(yield* (yield* Devices).updateSelf(device.device.id, update));
    }),
  ),
  HttpRouter.del(
    "/device",
    Effect.gen(function* () {
      const authority = yield* RequestAuthority;
      const device = yield* (yield* Auth).requireDevice(authority);
      return yield* HttpServerResponse.json(yield* (yield* Devices).revokeSelf(device.device.id));
    }),
  ),
);
