import { env } from "cloudflare:test";
import { Effect, Option } from "effect";
import { describe, expect, it } from "vitest";
import { Env, type Bindings } from "../src/Env";
import { Devices } from "../src/services/Devices";

const enc = new TextEncoder();
const bindings = env as unknown as Bindings;
const run = <A, E>(effect: Effect.Effect<A, E, Devices | Env>) =>
  Effect.runPromise(effect.pipe(Effect.provide(Devices.Default), Effect.provideService(Env, bindings)));

let seq = 0;

async function digest(credential: string): Promise<string> {
  const key = await crypto.subtle.importKey("raw", enc.encode("test-secret"), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const signature = new Uint8Array(await crypto.subtle.sign("HMAC", key, enc.encode(credential)));
  return btoa(String.fromCharCode(...signature)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

async function seedDevice(options: { fcmToken?: string | null; revokedAt?: string | null } = {}) {
  const suffix = ++seq;
  const id = `service-device-${suffix}`;
  const credential = `fake-service-credential-${suffix}`;
  const credentialHash = await digest(credential);
  await env.DB.prepare(
    `INSERT INTO devices
       (id, name, credential_hash, fcm_token, app_version, android_version, created_at, last_seen_at, revoked_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?)`,
  ).bind(
    id,
    `Phone ${suffix}`,
    credentialHash,
    options.fcmToken === undefined ? `fake-fcm-token-${suffix}` : options.fcmToken,
    "1.0.0-test",
    "16-test",
    new Date(1_700_000_000_000 + suffix).toISOString(),
    options.revokedAt ?? null,
  ).run();
  return { id, credentialHash };
}

const summaryKeys = ["android_version", "app_version", "created_at", "id", "last_seen_at", "name", "push_registered", "revoked_at"];
const registrationKeys = ["android_version", "app_version", "created_at", "id", "last_seen_at", "name", "push_registered"];

describe("Devices", () => {
  it("get returns an owner-safe summary", async () => {
    const seeded = await seedDevice();
    const service = await run(Devices);

    const got = await run(service.get(seeded.id));
    expect(Object.keys(got).sort()).toEqual(summaryKeys);
    expect(got.push_registered).toBe(true);
  });

  it("list returns owner-safe summaries", async () => {
    const first = await seedDevice();
    const second = await seedDevice({ fcmToken: null });
    const service = await run(Devices);

    const listed = await run(service.list());
    const relevant = listed.filter((device) => device.id === first.id || device.id === second.id);
    expect(relevant).toHaveLength(2);
    expect(relevant.map((device) => Object.keys(device).sort())).toEqual([summaryKeys, summaryKeys]);
    expect(relevant.find((device) => device.id === second.id)?.push_registered).toBe(false);
  });

  it("rename returns an owner-safe summary with the new name", async () => {
    const seeded = await seedDevice();
    const service = await run(Devices);
    const renamed = await run(service.rename(seeded.id, "Renamed phone"));

    expect(Object.keys(renamed).sort()).toEqual(summaryKeys);
    expect(renamed.name).toBe("Renamed phone");
  });

  it("updateSelf updates registration fields and returns a self-safe shape", async () => {
    const seeded = await seedDevice({ fcmToken: null });
    const service = await run(Devices);
    const updated = await run(service.updateSelf(seeded.id, {
      name: "Updated phone",
      fcm_token: "fake-rotated-fcm-token",
      app_version: "2.0.0-test",
      android_version: "17-test",
    }));

    expect(Object.keys(updated).sort()).toEqual(registrationKeys);
    expect(updated).toMatchObject({
      name: "Updated phone",
      app_version: "2.0.0-test",
      android_version: "17-test",
      push_registered: true,
    });
  });

  it("authenticate returns a safe device and writes last_seen_at once in a five-minute bucket", async () => {
    const seeded = await seedDevice();
    const service = await run(Devices);
    await env.DB.prepare("CREATE TABLE seen_update_audit (device_id TEXT NOT NULL)").run();
    await env.DB.prepare(
      `CREATE TRIGGER audit_seen_updates AFTER UPDATE OF last_seen_at ON devices
       BEGIN INSERT INTO seen_update_audit (device_id) VALUES (NEW.id); END`,
    ).run();

    const first = await run(service.authenticate(seeded.credentialHash));
    const second = await run(service.authenticate(seeded.credentialHash));

    expect(Option.isSome(first)).toBe(true);
    expect(Option.isSome(second)).toBe(true);
    if (Option.isSome(first) && Option.isSome(second)) {
      expect(Object.keys(first.value).sort()).toEqual(registrationKeys);
      expect(second.value.last_seen_at).toBe(first.value.last_seen_at);
    }
    const audit = await env.DB.prepare("SELECT COUNT(*) AS count FROM seen_update_audit WHERE device_id = ?").bind(seeded.id).first<{ count: number }>();
    expect(audit?.count).toBe(1);
  });

  it("revoke retains the row, clears its FCM token, returns a safe summary, and blocks authentication", async () => {
    const seeded = await seedDevice();
    const service = await run(Devices);
    const revoked = await run(service.revoke(seeded.id));

    expect(Object.keys(revoked).sort()).toEqual(summaryKeys);
    expect(revoked).toMatchObject({ id: seeded.id, push_registered: false });
    expect(revoked.revoked_at).not.toBeNull();
    const stored = await env.DB.prepare("SELECT fcm_token, revoked_at FROM devices WHERE id = ?").bind(seeded.id).first<{
      fcm_token: string | null;
      revoked_at: string | null;
    }>();
    expect(stored).toMatchObject({ fcm_token: null, revoked_at: revoked.revoked_at });
    expect(Option.isNone(await run(service.authenticate(seeded.credentialHash)))).toBe(true);
  });

  it("revokeSelf retains the row, clears its FCM token, and returns a safe summary", async () => {
    const seeded = await seedDevice();
    const service = await run(Devices);
    const revoked = await run(service.revokeSelf(seeded.id));

    expect(Object.keys(revoked).sort()).toEqual(summaryKeys);
    const stored = await env.DB.prepare("SELECT fcm_token, revoked_at FROM devices WHERE id = ?").bind(seeded.id).first<{
      fcm_token: string | null;
      revoked_at: string | null;
    }>();
    expect(stored).toEqual({ fcm_token: null, revoked_at: revoked.revoked_at });
  });
});
