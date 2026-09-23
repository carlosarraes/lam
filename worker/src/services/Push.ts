import { Context, Data, Effect } from "effect";
import type { Item } from "../domain/Item";
import { pushMessage, type PushEvent, type PushMessage } from "../domain/PushMessage";
import { Devices } from "./Devices";

export class InvalidRegistration extends Data.TaggedError("InvalidRegistration")<{}> {}
export class PushDeliveryError extends Data.TaggedError("PushDeliveryError")<{}> {}

export interface FcmClientService {
  readonly send: (token: string, message: PushMessage) => Effect.Effect<void, InvalidRegistration | PushDeliveryError>;
}

export class FcmClient extends Context.Tag("lam/FcmClient")<FcmClient, FcmClientService>() {}

export class Push extends Effect.Service<Push>()("lam/Push", {
  succeed: {
    deliver: (event: PushEvent, item: Item) => Effect.gen(function* () {
      const message = pushMessage(event, item);
      if (message === null) return;
      const devices = yield* Devices;
      const fcm = yield* FcmClient;
      const targets = yield* devices.pushTargets();
      yield* Effect.forEach(
        targets,
        (target) => fcm.send(target.token, message).pipe(
          Effect.catchTag("InvalidRegistration", () => devices.clearPushToken(target.id, target.token)),
          Effect.catchTag("PushDeliveryError", () => Effect.logWarning("FCM delivery failed", { device_id: target.id })),
        ),
        { concurrency: "unbounded", discard: true },
      );
    }),
  },
}) {}
