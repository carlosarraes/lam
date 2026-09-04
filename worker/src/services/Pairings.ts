import { Effect, Schema } from "effect";
import { Env } from "../Env";
import { DeviceRegistration } from "../domain/Device";
import {
  PairingCreated,
  type PairingClaimed,
  type PairingRegistration,
  type PairingStatus,
  PairingStatusRow,
  pairingQr,
} from "../domain/Pairing";
import { Conflict, DbError, NotFound, Unauthorized } from "../domain/Item";
import { constantTimeEqual, hmacDigest } from "./Auth";

const PAIRING_LIFETIME_MS = 5 * 60 * 1_000;

const db = <A>(run: (db: D1Database) => Promise<A>) =>
  Effect.flatMap(Env, ({ DB }) => Effect.tryPromise({ try: () => run(DB), catch: (cause) => new DbError({ cause }) }));

const randomBase64Url32 = () => {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
};

const decodeStatusRow = (row: unknown) => Schema.decodeUnknown(PairingStatusRow)(row).pipe(Effect.orDie);

const status = (id: string): Effect.Effect<PairingStatus, DbError | NotFound, Env> =>
  db((database) =>
    database
      .prepare(
        `SELECT p.expires_at, p.consumed_at, p.device_id,
                d.name, d.app_version, d.android_version, d.created_at, d.last_seen_at,
                CASE WHEN d.fcm_token IS NULL THEN 0 ELSE 1 END AS push_registered
           FROM pairing_sessions p
           LEFT JOIN devices d ON d.id = p.device_id
          WHERE p.id = ?`,
      )
      .bind(id)
      .first(),
  ).pipe(
    Effect.flatMap((row) => (row ? decodeStatusRow(row) : Effect.fail(new NotFound({ id })))),
    Effect.map((row): PairingStatus => {
      if (row.consumed_at !== null && row.device_id === null) return { status: "cancelled" };
      if (row.device_id !== null) {
        if (row.name === null || row.app_version === null || row.android_version === null || row.created_at === null) {
          throw new Error(`pairing ${id} references a missing device`);
        }
        return {
          status: "claimed",
          device: new DeviceRegistration({
            id: row.device_id,
            name: row.name,
            app_version: row.app_version,
            android_version: row.android_version,
            created_at: row.created_at,
            last_seen_at: row.last_seen_at,
            push_registered: row.push_registered === 1,
          }),
        };
      }
      return Date.parse(row.expires_at) <= Date.now() ? { status: "expired" } : { status: "pending" };
    }),
  );

export class Pairings extends Effect.Service<Pairings>()("lam/Pairings", {
  succeed: {
    create: (origin: string) =>
      Effect.gen(function* () {
        const id = crypto.randomUUID();
        const secret = randomBase64Url32();
        const createdAt = new Date();
        const expiresAt = new Date(createdAt.getTime() + PAIRING_LIFETIME_MS);
        const createdAtIso = createdAt.toISOString();
        const expiresAtIso = expiresAt.toISOString();
        const env = yield* Env;
        const secretHash = yield* hmacDigest(env.LAM_HMAC_SECRET, secret);
        yield* db((database) =>
          database
            .prepare(
              `INSERT INTO pairing_sessions
                 (id, secret_hash, created_at, expires_at, consumed_at, device_id)
               VALUES (?, ?, ?, ?, NULL, NULL)`,
            )
            .bind(id, secretHash, createdAtIso, expiresAtIso)
            .run(),
        );
        return new PairingCreated({
          session: id,
          created_at: createdAtIso,
          expires_at: expiresAtIso,
          qr: pairingQr(origin, id, secret),
        });
      }),

    status,

    cancel: (id: string) =>
      Effect.gen(function* () {
        const now = new Date().toISOString();
        const result = yield* db((database) =>
          database
            .prepare(
              `UPDATE pairing_sessions
                  SET consumed_at = ?
                WHERE id = ? AND consumed_at IS NULL AND expires_at > ?`,
            )
            .bind(now, id, now)
            .run(),
        );
        return result.meta.changes === 1 ? ({ status: "cancelled" } as const) : yield* status(id);
      }),

    claim: (id: string, secret: string, registration: PairingRegistration) =>
      Effect.gen(function* () {
        const stored = yield* db((database) =>
          database.prepare("SELECT secret_hash FROM pairing_sessions WHERE id = ?").bind(id).first<{ secret_hash: string }>(),
        );
        if (stored === null) return yield* new NotFound({ id });

        const env = yield* Env;
        const secretHash = yield* hmacDigest(env.LAM_HMAC_SECRET, secret);
        if (!constantTimeEqual(secretHash, stored.secret_hash)) return yield* new Unauthorized();

        const deviceId = crypto.randomUUID();
        const credential = randomBase64Url32();
        const credentialHash = yield* hmacDigest(env.LAM_HMAC_SECRET, credential);
        const now = new Date().toISOString();
        const [inserted, consumed] = yield* db((database) =>
          database.batch([
            database
              .prepare(
                `INSERT INTO devices
                   (id, name, credential_hash, fcm_token, app_version, android_version,
                    created_at, last_seen_at, revoked_at)
                 SELECT ?, ?, ?, ?, ?, ?, ?, NULL, NULL
                  WHERE EXISTS (
                    SELECT 1 FROM pairing_sessions
                     WHERE id = ? AND secret_hash = ? AND consumed_at IS NULL AND expires_at > ?
                  )`,
              )
              .bind(
                deviceId,
                registration.name,
                credentialHash,
                registration.fcm_token,
                registration.app_version,
                registration.android_version,
                now,
                id,
                secretHash,
                now,
              ),
            database
              .prepare(
                `UPDATE pairing_sessions
                    SET consumed_at = ?, device_id = ?
                  WHERE id = ? AND secret_hash = ? AND consumed_at IS NULL AND expires_at > ?`,
              )
              .bind(now, deviceId, id, secretHash, now),
          ]),
        );
        if (inserted.meta.changes !== 1 || consumed.meta.changes !== 1) return yield* new Conflict({ id });

        return {
          credential,
          device: new DeviceRegistration({
            id: deviceId,
            name: registration.name,
            app_version: registration.app_version,
            android_version: registration.android_version,
            created_at: now,
            last_seen_at: null,
            push_registered: registration.fcm_token !== null,
          }),
        } satisfies PairingClaimed;
      }),
  },
}) {}
