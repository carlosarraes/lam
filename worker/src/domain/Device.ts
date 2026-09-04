import { Schema } from "effect";

export const MAX_DEVICE_NAME_CHARACTERS = 100;

export const DeviceName = Schema.Trim.pipe(
  Schema.filter((value) => value.length > 0 || "must not be empty"),
  Schema.filter((value) => Array.from(value).length <= MAX_DEVICE_NAME_CHARACTERS || `must be at most ${MAX_DEVICE_NAME_CHARACTERS} characters`),
);

const PublicDeviceFields = {
  id: Schema.String,
  name: Schema.String,
  app_version: Schema.String,
  android_version: Schema.String,
  created_at: Schema.String,
  last_seen_at: Schema.NullOr(Schema.String),
  push_registered: Schema.Boolean,
};

/** Safe active-device shape carried by authenticated requests. */
export class Device extends Schema.Class<Device>("Device")(PublicDeviceFields) {}

/** Owner-visible shape, including the soft-revocation timestamp. */
export class DeviceSummary extends Schema.Class<DeviceSummary>("DeviceSummary")({
  ...PublicDeviceFields,
  revoked_at: Schema.NullOr(Schema.String),
}) {}

/** Safe self-visible registration shape. */
export class DeviceRegistration extends Schema.Class<DeviceRegistration>("DeviceRegistration")(PublicDeviceFields) {}

/** D1-only representation. Credential digests and FCM tokens never enter public shapes. */
export const DeviceRow = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
  credential_hash: Schema.String,
  fcm_token: Schema.NullOr(Schema.String),
  app_version: Schema.String,
  android_version: Schema.String,
  created_at: Schema.String,
  last_seen_at: Schema.NullOr(Schema.String),
  revoked_at: Schema.NullOr(Schema.String),
});
export type DeviceRow = typeof DeviceRow.Type;

/** Minimal row loaded for constant-work authentication. */
export const DeviceCredentialRow = Schema.Struct({
  id: Schema.String,
  credential_hash: Schema.String,
});
export type DeviceCredentialRow = typeof DeviceCredentialRow.Type;
