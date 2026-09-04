import { Data, Schema } from "effect";
import { DeviceName, DeviceRegistration } from "./Device";

// Thirty-two bytes have 43 unpadded base64url characters; the final sextet has two zero padding bits.
export const PairingSecret = Schema.String.pipe(Schema.pattern(/^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$/));

export const PairingRegistration = Schema.Struct({
  name: DeviceName,
  fcm_token: Schema.optionalWith(Schema.NullOr(Schema.String), { default: () => null }),
  app_version: Schema.NonEmptyString,
  android_version: Schema.NonEmptyString,
});
export type PairingRegistration = typeof PairingRegistration.Type;

export const PairingClaimRequest = Schema.Struct({
  secret: PairingSecret,
  ...PairingRegistration.fields,
});

export class PairingCreated extends Schema.Class<PairingCreated>("PairingCreated")({
  session: Schema.String,
  created_at: Schema.String,
  expires_at: Schema.String,
  qr: Schema.String,
}) {}

export const pairingQr = (server: string, session: string, secret: string) =>
  JSON.stringify({ v: 1, server, session, secret });

export type PairingStatus =
  | { readonly status: "pending" }
  | { readonly status: "claimed"; readonly device: DeviceRegistration }
  | { readonly status: "expired" }
  | { readonly status: "cancelled" };

export interface PairingClaimed {
  readonly credential: string;
  readonly device: DeviceRegistration;
}

export type PairingClaimErrorCode = "expired" | "consumed" | "cancelled";
export class PairingClaimConflict extends Data.TaggedError("PairingClaimConflict")<{
  readonly code: PairingClaimErrorCode;
}> {}

export const PairingStatusRow = Schema.Struct({
  expires_at: Schema.String,
  consumed_at: Schema.NullOr(Schema.String),
  device_id: Schema.NullOr(Schema.String),
  name: Schema.NullOr(Schema.String),
  app_version: Schema.NullOr(Schema.String),
  android_version: Schema.NullOr(Schema.String),
  created_at: Schema.NullOr(Schema.String),
  last_seen_at: Schema.NullOr(Schema.String),
  push_registered: Schema.Number,
});
export type PairingStatusRow = typeof PairingStatusRow.Type;
