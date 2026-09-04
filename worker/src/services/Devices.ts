import { Effect, Option, Schema } from "effect";
import { Env } from "../Env";
import { Device, DeviceCredentialRow, DeviceRegistration, DeviceRow, DeviceSummary } from "../domain/Device";
import { DbError, NotFound } from "../domain/Item";
import { constantTimeEqual } from "./Auth";

const FIVE_MINUTES_MS = 5 * 60 * 1_000;

const db = <A>(run: (db: D1Database) => Promise<A>) =>
  Effect.flatMap(Env, ({ DB }) => Effect.tryPromise({ try: () => run(DB), catch: (cause) => new DbError({ cause }) }));

const decodeRow = (row: unknown) => Schema.decodeUnknown(DeviceRow)(row).pipe(Effect.orDie);
const decodeCredentialRow = (row: unknown) => Schema.decodeUnknown(DeviceCredentialRow)(row).pipe(Effect.orDie);

const publicFields = (row: DeviceRow) => ({
  id: row.id,
  name: row.name,
  app_version: row.app_version,
  android_version: row.android_version,
  created_at: row.created_at,
  last_seen_at: row.last_seen_at,
  push_registered: row.fcm_token !== null,
});

const toDevice = (row: DeviceRow) => new Device(publicFields(row));
const toSummary = (row: DeviceRow) => new DeviceSummary({ ...publicFields(row), revoked_at: row.revoked_at });
const toRegistration = (row: DeviceRow) => new DeviceRegistration(publicFields(row));

const getRow = (id: string) =>
  db((d) => d.prepare("SELECT * FROM devices WHERE id = ?").bind(id).first()).pipe(
    Effect.flatMap((row) => (row ? decodeRow(row) : Effect.fail(new NotFound({ id })))),
  );

const revokeRow = (id: string) =>
  Effect.gen(function* () {
    const now = new Date().toISOString();
    yield* db((d) =>
      d.prepare("UPDATE devices SET revoked_at = COALESCE(revoked_at, ?), fcm_token = NULL WHERE id = ?").bind(now, id).run(),
    );
    return toSummary(yield* getRow(id));
  });

export interface DeviceSelfUpdate {
  readonly name?: string;
  readonly fcm_token?: string | null;
  readonly app_version?: string;
  readonly android_version?: string;
}

export class Devices extends Effect.Service<Devices>()("lam/Devices", {
  succeed: {
    /** Compares the presented digest with every active candidate before selecting a match. */
    authenticate: (digest: string) =>
      Effect.gen(function* () {
        const { results } = yield* db((d) => d.prepare("SELECT id, credential_hash FROM devices WHERE revoked_at IS NULL").all());
        const rows = yield* Effect.forEach(results, decodeCredentialRow);
        let matchedId: string | undefined;
        for (const row of rows) {
          if (constantTimeEqual(digest, row.credential_hash)) matchedId = row.id;
        }
        if (matchedId === undefined) return Option.none<Device>();

        let activeDevice = yield* getRow(matchedId);
        const bucket = new Date(Math.floor(Date.now() / FIVE_MINUTES_MS) * FIVE_MINUTES_MS).toISOString();
        if (activeDevice.last_seen_at === null || activeDevice.last_seen_at < bucket) {
          yield* db((d) =>
            d
              .prepare("UPDATE devices SET last_seen_at = ? WHERE id = ? AND (last_seen_at IS NULL OR last_seen_at < ?)")
              .bind(bucket, activeDevice.id, bucket)
              .run(),
          );
          activeDevice = { ...activeDevice, last_seen_at: bucket };
        }
        return Option.some(toDevice(activeDevice));
      }),

    get: (id: string) => getRow(id).pipe(Effect.map(toSummary)),

    list: () =>
      db((d) => d.prepare("SELECT * FROM devices ORDER BY created_at DESC, id DESC").all()).pipe(
        Effect.flatMap(({ results }) => Effect.forEach(results, decodeRow)),
        Effect.map((rows) => rows.map(toSummary)),
      ),

    rename: (id: string, name: string) =>
      db((d) => d.prepare("UPDATE devices SET name = ? WHERE id = ?").bind(name, id).run()).pipe(
        Effect.flatMap(() => getRow(id)),
        Effect.map(toSummary),
      ),

    updateSelf: (id: string, update: DeviceSelfUpdate) =>
      db((d) =>
        d
          .prepare(
            `UPDATE devices SET
               name = CASE WHEN ? THEN ? ELSE name END,
               fcm_token = CASE WHEN ? THEN ? ELSE fcm_token END,
               app_version = CASE WHEN ? THEN ? ELSE app_version END,
               android_version = CASE WHEN ? THEN ? ELSE android_version END
             WHERE id = ? AND revoked_at IS NULL`,
          )
          .bind(
            update.name !== undefined ? 1 : 0, update.name ?? null,
            update.fcm_token !== undefined ? 1 : 0, update.fcm_token ?? null,
            update.app_version !== undefined ? 1 : 0, update.app_version ?? null,
            update.android_version !== undefined ? 1 : 0, update.android_version ?? null,
            id,
          )
          .run(),
      ).pipe(
        Effect.flatMap(() => getRow(id)),
        Effect.map(toRegistration),
      ),

    revoke: revokeRow,

    revokeSelf: revokeRow,
  },
}) {}
