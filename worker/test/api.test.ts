import { env, SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";

const enc = new TextEncoder();
async function itemToken(secret: string, id: string): Promise<string> {
  const key = await crypto.subtle.importKey("raw", enc.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const sig = new Uint8Array(await crypto.subtle.sign("HMAC", key, enc.encode(id)));
  return btoa(String.fromCharCode(...sig)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

const AUTH = { Authorization: "Bearer test-token" };
const json = (body: unknown) => ({ method: "POST", headers: { ...AUTH, "content-type": "application/json" }, body: JSON.stringify(body) });
const publicJson = (body: unknown) => ({ method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
const settle = () => new Promise((r) => setTimeout(r, 50));

async function topicMessages(since = "all"): Promise<any[]> {
  await settle();
  const res = await SELF.fetch(`http://lam/test-topic/json?poll=1&since=${since}`);
  expect(res.status).toBe(200);
  return new TextDecoder().decode(await res.arrayBuffer()).trim().split("\n").filter(Boolean).map((l) => JSON.parse(l));
}
const lastMessage = async () => (await topicMessages()).at(-1)!;

let seq = 0;

async function registerFakeDevice(credential = `fake-device-credential-${++seq}`) {
  const id = `device-${seq}`;
  const credentialHash = await itemToken("test-secret", credential);
  await env.DB.prepare(
    `INSERT INTO devices (id, name, credential_hash, fcm_token, app_version, android_version, created_at)
     VALUES (?, ?, ?, NULL, ?, ?, ?)`,
  ).bind(id, "Test phone", credentialHash, "1.0.0-test", "16-test", new Date().toISOString()).run();
  return { id, headers: { Authorization: `Bearer ${credential}` } };
}

/** Pushes an item whose title is made unique, so the duplicate guard never fires by accident. */
async function push(body: { title?: string } & Record<string, unknown> = {}) {
  const { title = "item", ...rest } = body;
  const res = await SELF.fetch("http://lam/items", json({ name: "0:agent", title: `${title} #${++seq}`, ...rest }));
  expect(res.status).toBe(201);
  return res.json<any>();
}

describe("auth", () => {
  it("rejects missing bearer", async () => {
    expect((await SELF.fetch("http://lam/items")).status).toBe(401);
  });
  it("rejects bad item token on action", async () => {
    const item = await push();
    expect((await SELF.fetch(`http://lam/a/${item.id}/Done?t=nope`)).status).toBe(403);
  });

  it("keeps the master bearer authorized on every existing API route", async () => {
    const resolved = await push({ title: "master resolve" });
    expect((await SELF.fetch(`http://lam/items/${resolved.id}/resolve`, json({}))).status).toBe(200);
    expect((await SELF.fetch(`http://lam/items/${resolved.id}/wait`, { headers: AUTH })).status).toBe(200);

    const checklist = await push({ title: "master checklist", checks: ["first"] });
    expect((await SELF.fetch(`http://lam/items/${checklist.id}/checks`, json({ label: "second" }))).status).toBe(200);
    expect((await SELF.fetch(`http://lam/items/${checklist.id}/checks/0`, json({ done: true }))).status).toBe(200);

    const retracted = await push({ title: "master retract" });
    expect((await SELF.fetch(`http://lam/items/${retracted.id}/retract`, { method: "POST", headers: AUTH })).status).toBe(200);
    const dismissed = await push({ title: "master dismiss" });
    expect((await SELF.fetch(`http://lam/items/${dismissed.id}/dismiss`, { method: "POST", headers: AUTH })).status).toBe(200);

    expect((await SELF.fetch("http://lam/items", { headers: AUTH })).status).toBe(200);
    expect((await SELF.fetch(`http://lam/items/${resolved.id}`, { headers: AUTH })).status).toBe(200);
    expect((await SELF.fetch(`http://lam/items/wait?ids=${resolved.id}`, { headers: AUTH })).status).toBe(200);
  });

  it("rejects an unknown device bearer", async () => {
    expect((await SELF.fetch("http://lam/items", { headers: { Authorization: "Bearer fake-unknown-device-credential" } })).status).toBe(401);
  });

  it("allows a valid device bearer to list and get items", async () => {
    const item = await push({ title: "device read" });
    const device = await registerFakeDevice();
    expect((await SELF.fetch("http://lam/items", { headers: device.headers })).status).toBe(200);
    expect((await SELF.fetch(`http://lam/items/${item.id}`, { headers: device.headers })).status).toBe(200);
  });

  it("allows a valid device bearer to wait for one closed item", async () => {
    const item = await push({ title: "device single wait" });
    await SELF.fetch(`http://lam/items/${item.id}/dismiss`, { method: "POST", headers: AUTH });
    const device = await registerFakeDevice();
    const response = await SELF.fetch(`http://lam/items/${item.id}/wait`, { headers: device.headers });
    expect(response.status).toBe(200);
    expect((await response.json<any>()).id).toBe(item.id);
  });

  it("allows a valid device bearer to wait for any closed item", async () => {
    const open = await push({ title: "device wait open" });
    const closed = await push({ title: "device wait closed" });
    await SELF.fetch(`http://lam/items/${closed.id}/dismiss`, { method: "POST", headers: AUTH });
    const device = await registerFakeDevice();
    const response = await SELF.fetch(`http://lam/items/wait?ids=${open.id},${closed.id}`, { headers: device.headers });
    expect(response.status).toBe(200);
    expect((await response.json<any>()).id).toBe(closed.id);
  });

  it("rejects a revoked device bearer", async () => {
    const device = await registerFakeDevice();
    await env.DB.prepare("UPDATE devices SET revoked_at = ? WHERE id = ?").bind(new Date().toISOString(), device.id).run();
    expect((await SELF.fetch("http://lam/items", { headers: device.headers })).status).toBe(401);
  });

  it("records device resolve and dismiss responses as phone responses", async () => {
    const resolved = await push({ title: "device resolve" });
    const dismissed = await push({ title: "device dismiss" });
    const device = await registerFakeDevice();

    const resolvedResponse = await SELF.fetch(`http://lam/items/${resolved.id}/resolve`, {
      ...json({ text: "handled on device" }),
      headers: { ...device.headers, "content-type": "application/json" },
    });
    expect(resolvedResponse.status).toBe(200);
    expect((await resolvedResponse.json<any>()).response_by).toBe("phone");

    const dismissedResponse = await SELF.fetch(`http://lam/items/${dismissed.id}/dismiss`, { method: "POST", headers: device.headers });
    expect(dismissedResponse.status).toBe(200);
    expect((await dismissedResponse.json<any>()).response_by).toBe("phone");
  });

  it("records a device checklist completion as a phone response", async () => {
    const item = await push({ title: "device checklist", checks: ["first"] });
    const device = await registerFakeDevice();
    const response = await SELF.fetch(`http://lam/items/${item.id}/checks/0`, {
      ...json({ done: true }),
      headers: { ...device.headers, "content-type": "application/json" },
    });
    expect(response.status).toBe(200);
    expect((await response.json<any>()).response_by).toBe("phone");
  });

  it("forbids a device bearer from pushing items", async () => {
    const device = await registerFakeDevice();
    const response = await SELF.fetch("http://lam/items", {
      ...json({ title: "forbidden device push" }),
      headers: { ...device.headers, "content-type": "application/json" },
    });
    expect(response.status).toBe(403);
  });

  it("forbids a device bearer from retracting items", async () => {
    const item = await push({ title: "forbidden device retract" });
    const device = await registerFakeDevice();
    expect((await SELF.fetch(`http://lam/items/${item.id}/retract`, { method: "POST", headers: device.headers })).status).toBe(403);
  });

  it("forbids a device bearer from adding checks", async () => {
    const item = await push({ title: "forbidden device check", checks: ["first"] });
    const device = await registerFakeDevice();
    const response = await SELF.fetch(`http://lam/items/${item.id}/checks`, {
      ...json({ label: "second" }),
      headers: { ...device.headers, "content-type": "application/json" },
    });
    expect(response.status).toBe(403);
  });
});

describe("device pairing", () => {
  const registration = (name: string) => ({
    name,
    fcm_token: `fake-fcm-${name}`,
    app_version: "1.0.0-test",
    android_version: "16-test",
  });

  async function createPairing(origin = "https://lam.example") {
    const response = await SELF.fetch(`${origin}/pairings`, { method: "POST", headers: AUTH });
    expect(response.status).toBe(201);
    return response.json<any>();
  }

  async function claim(session: string, secret: string, name: string) {
    return SELF.fetch(`https://lam.example/pairings/${session}/claim`, publicJson({ secret, ...registration(name) }));
  }

  it("creates an exact v1 QR payload backed by a five-minute hashed session secret", async () => {
    const created = await createPairing();
    const qr = JSON.parse(created.qr);

    expect(Object.keys(qr)).toEqual(["v", "server", "session", "secret"]);
    expect(created.qr).toBe(JSON.stringify({
      v: 1,
      server: "https://lam.example",
      session: created.session,
      secret: qr.secret,
    }));
    expect(qr.secret).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(Date.parse(created.expires_at) - Date.parse(created.created_at)).toBe(5 * 60 * 1_000);

    const stored = await env.DB.prepare("SELECT secret_hash FROM pairing_sessions WHERE id = ?").bind(created.session).first<{
      secret_hash: string;
    }>();
    expect(stored?.secret_hash).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(stored?.secret_hash).not.toBe(qr.secret);
  });

  it("rejects malformed pairing secrets before creating a device", async () => {
    const created = await createPairing();
    const before = await env.DB.prepare("SELECT COUNT(*) AS count FROM devices").first<{ count: number }>();

    const response = await claim(created.session, "not-base64url-32-bytes", "Malformed secret phone");

    expect(response.status).toBe(400);
    const after = await env.DB.prepare("SELECT COUNT(*) AS count FROM devices").first<{ count: number }>();
    expect(after?.count).toBe(before?.count);
  });

  it("authenticates a well-formed pairing secret without a bearer", async () => {
    const created = await createPairing();
    const response = await claim(created.session, "A".repeat(43), "Wrong-secret phone");

    expect(response.status).toBe(401);
    expect(await env.DB.prepare("SELECT id FROM devices WHERE name = ?").bind("Wrong-secret phone").first()).toBeNull();
  });

  it("expires after five minutes and cannot be claimed", async () => {
    const created = await createPairing();
    await env.DB.prepare("UPDATE pairing_sessions SET expires_at = ? WHERE id = ?")
      .bind("2000-01-01T00:05:00.000Z", created.session)
      .run();

    const statusResponse = await SELF.fetch(`https://lam.example/pairings/${created.session}/wait`, { headers: AUTH });
    expect(statusResponse.status).toBe(200);
    expect(await statusResponse.json()).toEqual({ status: "expired" });
    expect((await claim(created.session, JSON.parse(created.qr).secret, "Expired phone")).status).toBe(409);
  });

  it("consumes a session once and returns a permanent credential only to the winner", async () => {
    const created = await createPairing();
    const secret = JSON.parse(created.qr).secret;

    const winner = await claim(created.session, secret, "One-use phone");
    expect(winner.status).toBe(201);
    const body = await winner.json<any>();
    expect(body.credential).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(Object.keys(body.device).sort()).toEqual([
      "android_version",
      "app_version",
      "created_at",
      "id",
      "last_seen_at",
      "name",
      "push_registered",
    ]);
    expect(JSON.stringify(body)).not.toContain("fake-fcm-One-use phone");
    expect((await claim(created.session, secret, "Losing retry phone")).status).toBe(409);

    const session = await env.DB.prepare("SELECT consumed_at, device_id, secret_hash FROM pairing_sessions WHERE id = ?")
      .bind(created.session)
      .first<{ consumed_at: string | null; device_id: string | null; secret_hash: string }>();
    const device = await env.DB.prepare("SELECT credential_hash FROM devices WHERE id = ?")
      .bind(session?.device_id)
      .first<{ credential_hash: string }>();
    expect(session?.consumed_at).not.toBeNull();
    expect(session?.device_id).toBe(body.device.id);
    expect(device?.credential_hash).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(device?.credential_hash).not.toBe(body.credential);
  });

  it("allows exactly one winner when two claims race", async () => {
    const created = await createPairing();
    const secret = JSON.parse(created.qr).secret;

    const responses = await Promise.all([
      claim(created.session, secret, "Concurrent phone A"),
      claim(created.session, secret, "Concurrent phone B"),
    ]);

    expect(responses.map((response) => response.status).sort()).toEqual([201, 409]);
    const linkedDevices = await env.DB.prepare(
      "SELECT COUNT(*) AS count FROM devices WHERE id = (SELECT device_id FROM pairing_sessions WHERE id = ?)",
    ).bind(created.session).first<{ count: number }>();
    const namedDevices = await env.DB.prepare(
      "SELECT COUNT(*) AS count FROM devices WHERE name IN (?, ?)",
    ).bind("Concurrent phone A", "Concurrent phone B").first<{ count: number }>();
    expect(linkedDevices?.count).toBe(1);
    expect(namedDevices?.count).toBe(1);
  });

  it("does not create a device for an unknown session", async () => {
    const before = await env.DB.prepare("SELECT COUNT(*) AS count FROM devices").first<{ count: number }>();
    const secret = "A".repeat(43);

    const response = await claim("missing-pairing-session", secret, "Wrong-session phone");

    expect(response.status).toBe(404);
    const after = await env.DB.prepare("SELECT COUNT(*) AS count FROM devices").first<{ count: number }>();
    expect(after?.count).toBe(before?.count);
  });

  it("cancels a pending session and prevents a later claim", async () => {
    const created = await createPairing();
    const secret = JSON.parse(created.qr).secret;

    const cancelled = await SELF.fetch(`https://lam.example/pairings/${created.session}`, { method: "DELETE", headers: AUTH });
    expect(cancelled.status).toBe(200);
    expect(await cancelled.json()).toEqual({ status: "cancelled" });
    const waited = await SELF.fetch(`https://lam.example/pairings/${created.session}/wait`, { headers: AUTH });
    expect(await waited.json()).toEqual({ status: "cancelled" });
    expect((await claim(created.session, secret, "Cancelled phone")).status).toBe(409);
  });

  it("wait returns claimed device data without secrets", async () => {
    const created = await createPairing();
    const secret = JSON.parse(created.qr).secret;
    const claimed = await claim(created.session, secret, "Waited-for phone");
    expect(claimed.status).toBe(201);
    const claimedBody = await claimed.json<any>();

    const response = await SELF.fetch(`https://lam.example/pairings/${created.session}/wait`, { headers: AUTH });
    expect(response.status).toBe(200);
    const body = await response.json<any>();
    expect(body).toEqual({ status: "claimed", device: claimedBody.device });
    expect(JSON.stringify(body)).not.toContain(secret);
    expect(JSON.stringify(body)).not.toContain(claimedBody.credential);
  });

  it("bounds a pending wait at 25 seconds and returns a typed pending status", async () => {
    const created = await createPairing();
    const startedAt = Date.now();

    const response = await SELF.fetch(`https://lam.example/pairings/${created.session}/wait`, { headers: AUTH });

    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ status: "pending" });
    expect(Date.now() - startedAt).toBeGreaterThanOrEqual(25_000);
    expect(Date.now() - startedAt).toBeLessThan(27_000);
  }, 30_000);

  it("keeps pairing lifecycle routes master-only while claim uses no bearer", async () => {
    expect((await SELF.fetch("https://lam.example/pairings", { method: "POST" })).status).toBe(401);
    const device = await registerFakeDevice();
    expect((await SELF.fetch("https://lam.example/pairings", { method: "POST", headers: device.headers })).status).toBe(403);
  });
});

describe("device administration", () => {
  const patch = (headers: HeadersInit, body: unknown) => ({
    method: "PATCH",
    headers: { ...headers, "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const summaryKeys = ["android_version", "app_version", "created_at", "id", "last_seen_at", "name", "push_registered", "revoked_at"];
  const registrationKeys = ["android_version", "app_version", "created_at", "id", "last_seen_at", "name", "push_registered"];

  it("lets the master list devices and rename one without exposing its FCM token", async () => {
    const device = await registerFakeDevice();
    await env.DB.prepare("UPDATE devices SET fcm_token = ? WHERE id = ?").bind("fake-admin-fcm-token", device.id).run();

    const listed = await SELF.fetch("http://lam/devices", { headers: AUTH });
    expect(listed.status).toBe(200);
    const before = (await listed.json<any[]>()).find((entry) => entry.id === device.id);
    expect(Object.keys(before).sort()).toEqual(summaryKeys);
    expect(JSON.stringify(before)).not.toContain("fake-admin-fcm-token");

    const renamed = await SELF.fetch(`http://lam/devices/${device.id}`, patch(AUTH, { name: "  Renamed master phone  " }));
    expect(renamed.status).toBe(200);
    const body = await renamed.json<any>();
    expect(Object.keys(body).sort()).toEqual(summaryKeys);
    expect(body.name).toBe("Renamed master phone");
    expect(JSON.stringify(body)).not.toContain("fake-admin-fcm-token");
  });

  it("forbids a device from administering a different device", async () => {
    const target = await registerFakeDevice();
    const other = await registerFakeDevice();

    expect((await SELF.fetch("http://lam/devices", { headers: other.headers })).status).toBe(403);
    expect((await SELF.fetch(`http://lam/devices/${target.id}`, patch(other.headers, { name: "attacker rename" }))).status).toBe(403);
    expect((await SELF.fetch(`http://lam/devices/${target.id}`, { method: "DELETE", headers: other.headers })).status).toBe(403);
    expect((await env.DB.prepare("SELECT name FROM devices WHERE id = ?").bind(target.id).first<{ name: string }>())?.name).toBe("Test phone");
  });

  it("lets a device read and update only its own safe registration", async () => {
    const device = await registerFakeDevice();
    await env.DB.prepare("UPDATE devices SET fcm_token = ? WHERE id = ?").bind("fake-self-old-fcm", device.id).run();

    const current = await SELF.fetch("http://lam/device", { headers: device.headers });
    expect(current.status).toBe(200);
    expect(Object.keys(await current.json<any>()).sort()).toEqual(registrationKeys);

    const updated = await SELF.fetch("http://lam/device", patch(device.headers, {
      name: "  Self managed phone  ",
      fcm_token: "fake-self-new-fcm",
      app_version: "2.0.0-test",
      android_version: "17-test",
    }));
    expect(updated.status).toBe(200);
    const body = await updated.json<any>();
    expect(Object.keys(body).sort()).toEqual(registrationKeys);
    expect(body).toMatchObject({
      id: device.id,
      name: "Self managed phone",
      app_version: "2.0.0-test",
      android_version: "17-test",
      push_registered: true,
    });
    expect(JSON.stringify(body)).not.toContain("fake-self-old-fcm");
    expect(JSON.stringify(body)).not.toContain("fake-self-new-fcm");
    expect((await SELF.fetch("http://lam/device", { headers: AUTH })).status).toBe(403);
  });

  it("rejects invalid and unsupported self updates without changing registration", async () => {
    const device = await registerFakeDevice();

    expect((await SELF.fetch("http://lam/device", patch(device.headers, { name: "x".repeat(101) }))).status).toBe(400);
    expect((await SELF.fetch("http://lam/device", patch(device.headers, { credential_hash: "not-allowed" }))).status).toBe(400);
    const registration = await (await SELF.fetch("http://lam/device", { headers: device.headers })).json<any>();
    expect(registration.name).toBe("Test phone");
  });

  it("revokes through either route idempotently, clears FCM, and rejects the revoked bearer", async () => {
    const masterTarget = await registerFakeDevice();
    await env.DB.prepare("UPDATE devices SET fcm_token = ? WHERE id = ?").bind("fake-master-revoke-fcm", masterTarget.id).run();

    const first = await SELF.fetch(`http://lam/devices/${masterTarget.id}`, { method: "DELETE", headers: AUTH });
    expect(first.status).toBe(200);
    const firstBody = await first.json<any>();
    expect(Object.keys(firstBody).sort()).toEqual(summaryKeys);
    expect(firstBody).toMatchObject({ id: masterTarget.id, push_registered: false });
    expect(firstBody.revoked_at).toEqual(expect.any(String));
    expect(JSON.stringify(firstBody)).not.toContain("fake-master-revoke-fcm");
    const second = await SELF.fetch(`http://lam/devices/${masterTarget.id}`, { method: "DELETE", headers: AUTH });
    expect(second.status).toBe(200);
    expect((await second.json<any>()).revoked_at).toBe(firstBody.revoked_at);
    expect((await env.DB.prepare("SELECT fcm_token FROM devices WHERE id = ?").bind(masterTarget.id).first<{ fcm_token: string | null }>())?.fcm_token).toBeNull();
    expect((await SELF.fetch("http://lam/items", { headers: masterTarget.headers })).status).toBe(401);

    const selfTarget = await registerFakeDevice();
    await env.DB.prepare("UPDATE devices SET fcm_token = ? WHERE id = ?").bind("fake-self-revoke-fcm", selfTarget.id).run();
    const selfRevoked = await SELF.fetch("http://lam/device", { method: "DELETE", headers: selfTarget.headers });
    expect(selfRevoked.status).toBe(200);
    const selfBody = await selfRevoked.json<any>();
    expect(Object.keys(selfBody).sort()).toEqual(summaryKeys);
    expect(selfBody).toMatchObject({ id: selfTarget.id, push_registered: false });
    expect(JSON.stringify(selfBody)).not.toContain("fake-self-revoke-fcm");
    expect((await env.DB.prepare("SELECT fcm_token FROM devices WHERE id = ?").bind(selfTarget.id).first<{ fcm_token: string | null }>())?.fcm_token).toBeNull();
    expect((await SELF.fetch("http://lam/items", { headers: selfTarget.headers })).status).toBe(401);
  });
});

describe("POST /items", () => {
  it("validates title, priority, choices", async () => {
    for (const bad of [{}, { name: "n" }, { name: "n", title: "x", priority: "urgent" }, { name: "n", title: "x", choices: ["a", "b", "c", "d"] }, { name: "n", title: "x", choices: [""] }]) {
      expect((await SELF.fetch("http://lam/items", json(bad))).status).toBe(400);
    }
  });
  it("accepts a push from an older binary that sends no name", async () => {
    const res = await SELF.fetch("http://lam/items", json({ title: "legacy", source_host: "mac", source_project: "platform" }));
    expect(res.status).toBe(201);
    const item = await res.json<any>();
    expect(item.name).toBe("");
    expect(item.recommendation).toBeNull();
    expect(item.recommended_choice).toBeNull();
    expect((await lastMessage()).message).toBe("(mac:platform)");
  });

  it("round-trips recommendation fields", async () => {
    const res = await SELF.fetch("http://lam/items", json({
      title: "Release?",
      choices: ["ship", "hold"],
      recommendation: "Ship after the smoke test.",
      recommended_choice: "ship",
    }));
    expect(res.status).toBe(201);
    expect(await res.json<any>()).toMatchObject({
      recommendation: "Ship after the smoke test.",
      recommended_choice: "ship",
    });
  });

  it("rejects a recommended choice that is not an exact choice", async () => {
    const res = await SELF.fetch("http://lam/items", json({
      title: "Release?",
      choices: ["ship"],
      recommended_choice: "Ship",
    }));
    expect(res.status).toBe(400);
  });

  it("enforces push content limits by code point, byte length, and item count", async () => {
    const tooLongChoice = "x".repeat(201);
    const tooLongCheck = "x".repeat(201);
    const tooLongTitle = "x".repeat(201);
    const bodyOver64KiB = "é".repeat(32_769);
    const recommendationOver2_000CodePoints = "😀".repeat(2_001);
    const fiftyOneChecks = Array.from({ length: 51 }, (_, i) => `check ${i}`);

    for (const body of [
      { title: "choice", choices: [tooLongChoice] },
      { title: "choices", choices: ["a", "b", "c", "d"] },
      { title: "check", checks: [tooLongCheck] },
      { title: "checks", checks: fiftyOneChecks },
      { title: tooLongTitle },
      { title: "body", body: bodyOver64KiB },
      { title: "recommendation", recommendation: recommendationOver2_000CodePoints },
    ]) {
      expect((await SELF.fetch("http://lam/items", json(body))).status).toBe(400);
    }
  });

  it("accepts content exactly at the byte and code-point limits", async () => {
    const res = await SELF.fetch("http://lam/items", json({
      title: "😀".repeat(200),
      body: "é".repeat(32_768),
      choices: ["😀".repeat(200)],
      recommendation: "😀".repeat(2_000),
      recommended_choice: "😀".repeat(200),
    }));
    expect(res.status).toBe(201);
  });

  it("creates an item and publishes to the topic with action buttons", async () => {
    const res = await SELF.fetch("http://lam/items", json({ name: "mp:2529", title: "PR 42", body: "decide", priority: "critical", choices: ["waive", "require"], source_host: "mac", source_project: "platform" }));
    const item = await res.json<any>();
    expect(item.id).toMatch(/^[a-z2-9]{5}$/);
    expect(item.status).toBe("open");
    const msg = await lastMessage();
    const t = await itemToken("test-secret", item.id);
    expect(msg).toMatchObject({ event: "message", topic: "test-topic", title: `PR 42 [${item.id}]`, message: "(mp:2529)\ndecide", priority: 5, tags: ["rotating_light"] });
    expect(item.name).toBe("mp:2529");
    expect(msg.id).toMatch(/^[A-Za-z0-9]{12}$/);
    expect(msg.actions.map((a: any) => ({ ...a, id: undefined }))).toEqual([
      { action: "http", label: "waive", url: `http://lam/a/${item.id}/waive?t=${t}`, clear: true },
      { action: "http", label: "require", url: `http://lam/a/${item.id}/require?t=${t}`, clear: true },
      { action: "view", label: "Reply", url: `http://lam/r/${item.id}?t=${t}`, clear: true },
    ]);
    expect(msg.actions.every((a: any) => typeof a.id === "string" && a.id)).toBe(true);
  });
});

describe("duplicate pushes", () => {
  it("a fully identical push while the first is open returns the same item and does not notify twice", async () => {
    const body = {
      name: "0:agent",
      title: "PR #2720 green",
      body: "trigger review",
      source_host: "mac",
      source_project: "lam",
      priority: "critical",
      choices: ["ship", "wait"],
      link: "https://example.com/pr/2720",
      ttl: 17,
      recommendation: "Ship it.",
      recommended_choice: "ship",
    };
    const first = await SELF.fetch("http://lam/items", json(body));
    expect(first.status).toBe(201);
    const a = await first.json<any>();
    await settle();
    const before = (await topicMessages()).length;

    const second = await SELF.fetch("http://lam/items", json(body));
    expect(second.status).toBe(200);
    expect((await second.json<any>()).id).toBe(a.id);
    await settle();
    expect((await topicMessages()).length).toBe(before);
  });

  it("a different body, a different agent, or a closed original all push a new item", async () => {
    const base = { name: "0:agent", title: "same title", body: "b" };
    const a = await (await SELF.fetch("http://lam/items", json(base))).json<any>();
    const other = await SELF.fetch("http://lam/items", json({ ...base, body: "different" }));
    expect(other.status).toBe(201);
    const otherAgent = await SELF.fetch("http://lam/items", json({ ...base, name: "1:other" }));
    expect(otherAgent.status).toBe(201);
    await SELF.fetch(`http://lam/items/${a.id}/dismiss`, { method: "POST", headers: AUTH });
    const afterClose = await SELF.fetch("http://lam/items", json(base));
    expect(afterClose.status).toBe(201);
    expect((await afterClose.json<any>()).id).not.toBe(a.id);
  });

  it("a different recommendation or exact TTL seconds creates a distinct item", async () => {
    const body = { name: "0:agent", title: "same contract", body: "b", ttl: 17, recommendation: "do it" };
    const first = await (await SELF.fetch("http://lam/items", json(body))).json<any>();

    const differentRecommendation = await SELF.fetch("http://lam/items", json({ ...body, recommendation: "wait" }));
    expect(differentRecommendation.status).toBe(201);
    expect((await differentRecommendation.json<any>()).id).not.toBe(first.id);

    const differentTtl = await SELF.fetch("http://lam/items", json({ ...body, ttl: 18 }));
    expect(differentTtl.status).toBe(201);
    expect((await differentTtl.json<any>()).id).not.toBe(first.id);
  });
});

describe("read", () => {
  it("lists open by default filter and gets by id", async () => {
    const a = await push({ title: "a" });
    await SELF.fetch(`http://lam/items/${a.id}/dismiss`, { method: "POST", headers: AUTH });
    const open = await (await SELF.fetch("http://lam/items?status=open", { headers: AUTH })).json<any[]>();
    expect(open.find((i) => i.id === a.id)).toBeUndefined();
    const all = await (await SELF.fetch("http://lam/items", { headers: AUTH })).json<any[]>();
    expect(all.find((i) => i.id === a.id).status).toBe("dismissed");
    expect((await SELF.fetch("http://lam/items/zzzzz", { headers: AUTH })).status).toBe(404);
  });
});

describe("paging", () => {
  const list = async (query = "") => (await SELF.fetch(`http://lam/items${query}`, { headers: AUTH })).json<any[]>();

  it("limit caps the page, newest first", async () => {
    const a = await push({ title: "p-a" });
    const b = await push({ title: "p-b" });
    expect((await list("?limit=2")).map((i) => i.id)).toEqual([b.id, a.id]);
  });

  it("before pages backwards without overlap", async () => {
    const a = await push({ title: "q-a" });
    const b = await push({ title: "q-b" });
    const first = await list("?limit=1");
    expect(first.map((i) => i.id)).toEqual([b.id]);
    const second = await list(`?limit=1&before=${encodeURIComponent(first.at(-1)!.created_at)}`);
    expect(second.map((i) => i.id)).toEqual([a.id]);
    expect(second[0].created_at < first[0].created_at).toBe(true);
  });

  it("an exhausted cursor returns an empty page", async () => {
    expect(await list("?limit=5&before=1970-01-01T00:00:00.000Z")).toEqual([]);
  });

  // Guard for the five machines running older binaries: they send no params and must still get
  // the whole table. Do not "tidy" this into a limited call.
  it("no params returns everything, as an older binary expects", async () => {
    const a = await push({ title: "r-a" });
    const all = await list();
    expect(all.find((i) => i.id === a.id)).toBeDefined();
    expect(all.length).toBeGreaterThan(1);
  });

  it("rejects a bad limit", async () => {
    for (const bad of ["0", "abc", "501", "-1"]) {
      expect((await SELF.fetch(`http://lam/items?limit=${bad}`, { headers: AUTH })).status).toBe(400);
    }
  });

  it("status filters within the page, it is not a search", async () => {
    await push({ title: "s-a" });
    const page = await list("?status=open&limit=1");
    expect(page.length).toBeLessThanOrEqual(1);
    expect(page.every((i) => i.status === "open")).toBe(true);
  });

  // Regression guard: whoever moves the status filter into SQL will break this, because `expired`
  // is derived on read and never stored.
  it("a paged expired item still reports its derived status", async () => {
    const item = await push({ title: "t-a", ttl: 1 });
    await new Promise((r) => setTimeout(r, 1100));
    const page = await list("?limit=20");
    expect(page.find((i) => i.id === item.id).status).toBe("expired");
  });
});

describe("resolution", () => {
  it("rejects oversized replies and malformed resolution JSON while retaining an empty resolve body", async () => {
    const cliItem = await push({ title: "cli reply" });
    const oversizedReply = "😀".repeat(2_049);
    expect((await SELF.fetch(`http://lam/items/${cliItem.id}/resolve`, json({ text: oversizedReply }))).status).toBe(400);
    expect((await SELF.fetch(`http://lam/items/${cliItem.id}/resolve`, {
      method: "POST",
      headers: { ...AUTH, "content-type": "application/json" },
      body: "{",
    })).status).toBe(400);
    expect((await SELF.fetch(`http://lam/items/${cliItem.id}/resolve`, { method: "POST", headers: AUTH })).status).toBe(200);

    const phoneItem = await push({ title: "phone reply" });
    const token = await itemToken("test-secret", phoneItem.id);
    expect((await SELF.fetch(`http://lam/r/${phoneItem.id}?t=${token}`, {
      method: "POST",
      body: new URLSearchParams({ text: oversizedReply }),
    })).status).toBe(400);
  });

  it("phone button resolves with choice and publishes closed message", async () => {
    const item = await push({ title: "q", choices: ["yes", "no"] });
    const t = await itemToken("test-secret", item.id);
    // the ntfy app sends http actions as POST
    const res = await SELF.fetch(`http://lam/a/${item.id}/yes?t=${t}`, { method: "POST" });
    expect(res.status).toBe(200);
    const got = await (await SELF.fetch(`http://lam/items/${item.id}`, { headers: AUTH })).json<any>();
    expect(got).toMatchObject({ status: "resolved", response_choice: "yes", response_by: "phone" });
    expect(got.resolved_at).toBeTruthy();
    expect((await lastMessage()).message).toBe("resolved via phone: yes");
    expect((await SELF.fetch(`http://lam/a/${item.id}/no?t=${t}`)).status).toBe(409);
    expect((await SELF.fetch(`http://lam/a/${item.id}/no?t=${t}`, { method: "POST" })).status).toBe(409);
  });
  it("cli resolve with text; wait returns immediately once closed", async () => {
    const item = await push();
    const res = await SELF.fetch(`http://lam/items/${item.id}/resolve`, json({ text: "go with B" }));
    expect((await res.json<any>()).response_text).toBe("go with B");
    const w = await SELF.fetch(`http://lam/items/${item.id}/wait`, { headers: AUTH });
    expect(w.status).toBe(200);
    expect((await w.json<any>()).response_by).toBe("cli");
  });
  it("reply page serves form and accepts text", async () => {
    const item = await push();
    const t = await itemToken("test-secret", item.id);
    const page = await SELF.fetch(`http://lam/r/${item.id}?t=${t}`);
    expect(page.status).toBe(200);
    expect(await page.text()).toContain("<form");
    const res = await SELF.fetch(`http://lam/r/${item.id}?t=${t}`, { method: "POST", body: new URLSearchParams({ text: "option C" }) });
    expect(res.status).toBe(200);
    const got = await (await SELF.fetch(`http://lam/items/${item.id}`, { headers: AUTH })).json<any>();
    expect(got).toMatchObject({ response_text: "option C", response_by: "phone" });
  });
  it("wait on unknown id is 404", async () => {
    expect((await SELF.fetch("http://lam/items/zzzzz/wait", { headers: AUTH })).status).toBe(404);
  });
});

describe("link, ttl, retract, wait-any", () => {
  it("link becomes an Open button; invalid link rejected", async () => {
    expect((await SELF.fetch("http://lam/items", json({ title: "x", link: "ftp://nope" }))).status).toBe(400);
    const item = await push({ title: "PR", choices: ["yes"], link: "https://github.com/x/pr/1" });
    expect(item.link).toBe("https://github.com/x/pr/1");
    const labels = (await lastMessage()).actions.map((a: any) => a.label);
    expect(labels).toEqual(["yes", "Open", "Reply"]);
  });
  it("ttl expires items: derived status, hidden from open list, cannot be closed", async () => {
    const item = await push({ title: "short", ttl: 1 });
    expect(item.expires_at).toBeTruthy();
    expect(item.status).toBe("open");
    await new Promise((r) => setTimeout(r, 1100));
    const got = await (await SELF.fetch(`http://lam/items/${item.id}`, { headers: AUTH })).json<any>();
    expect(got.status).toBe("expired");
    const open = await (await SELF.fetch("http://lam/items?status=open", { headers: AUTH })).json<any[]>();
    expect(open.find((i) => i.id === item.id)).toBeUndefined();
    expect((await SELF.fetch(`http://lam/items/${item.id}/resolve`, json({}))).status).toBe(409);
    const w = await SELF.fetch(`http://lam/items/${item.id}/wait`, { headers: AUTH });
    expect((await w.json<any>()).status).toBe("expired");
    expect((await SELF.fetch("http://lam/items", json({ title: "x", ttl: 0 }))).status).toBe(400);
  });
  it("retract closes with status retracted and notifies", async () => {
    const item = await push({ title: "self-solved" });
    const res = await SELF.fetch(`http://lam/items/${item.id}/retract`, { method: "POST", headers: AUTH });
    expect((await res.json<any>()).status).toBe("retracted");
    expect((await lastMessage()).message).toBe("retracted via cli: retracted");
    expect((await SELF.fetch(`http://lam/items/${item.id}/retract`, { method: "POST", headers: AUTH })).status).toBe(409);
  });
  it("wait?ids= returns the first closed item among many", async () => {
    const a = await push({ title: "a" });
    const b = await push({ title: "b" });
    await SELF.fetch(`http://lam/items/${b.id}/resolve`, json({ choice: "ok" }));
    const w = await SELF.fetch(`http://lam/items/wait?ids=${a.id},${b.id}`, { headers: AUTH });
    expect(w.status).toBe(200);
    expect((await w.json<any>()).id).toBe(b.id);
    expect((await SELF.fetch("http://lam/items/wait", { headers: AUTH })).status).toBe(400);
  });
});

describe("checklists", () => {
  const checkPost = (id: string, i: number, done: boolean) =>
    SELF.fetch(`http://lam/items/${id}/checks/${i}`, json({ done }));

  it("push with checks: exclusive with choices, listed in the push, Checks button", async () => {
    expect((await SELF.fetch("http://lam/items", json({ title: "x", choices: ["a"], checks: ["b"] }))).status).toBe(400);
    const item = await push({ title: "PRs", checks: ["PR 1", "PR 2"], link: "https://x" });
    expect(item.checks).toEqual([{ label: "PR 1", done: false, at: null }, { label: "PR 2", done: false, at: null }]);
    expect(item.version).toBe(0);
    const msg = await lastMessage();
    expect(msg.message).toContain("☐ PR 1\n☐ PR 2");
    expect(msg.actions.map((a: any) => a.label)).toEqual(["Open", "Checks"]);
  });

  it("ticks bump version, wait?since returns on change, all ticked auto-resolves", async () => {
    const item = await push({ title: "PRs", checks: ["a", "b"] });
    const t1 = await (await checkPost(item.id, 0, true)).json<any>();
    expect(t1).toMatchObject({ status: "open", version: 1 });
    expect(t1.checks[0].done).toBe(true);
    expect(t1.checks[0].at).toBeTruthy();
    const w1 = await (await SELF.fetch(`http://lam/items/${item.id}/wait?since=0`, { headers: AUTH })).json<any>();
    expect(w1.version).toBe(1);
    // untick is allowed while open
    const t2 = await (await checkPost(item.id, 0, false)).json<any>();
    expect(t2.checks[0]).toEqual({ label: "a", done: false, at: null });
    await checkPost(item.id, 0, true);
    const done = await (await checkPost(item.id, 1, true)).json<any>();
    expect(done).toMatchObject({ status: "resolved", response_by: "cli", version: 4 });
    expect((await lastMessage()).message).toBe("resolved via cli: 2/2 checks");
    expect((await checkPost(item.id, 0, false)).status).toBe(409);
    expect((await checkPost(item.id, 9, true)).status).toBe(409);
  });

  it("bad index is 400; add check appends and notifies; wait-any honours since", async () => {
    const item = await push({ title: "PRs", checks: ["a"] });
    expect((await checkPost(item.id, 5, true)).status).toBe(400);
    const added = await (await SELF.fetch(`http://lam/items/${item.id}/checks`, json({ label: "b" }))).json<any>();
    expect(added.checks.map((c: any) => c.label)).toEqual(["a", "b"]);
    expect(added.version).toBe(1);
    expect((await lastMessage()).message).toBe("new check: b (0/2)");
    const other = await push({ title: "other" });
    const w = await SELF.fetch(`http://lam/items/wait?ids=${other.id},${item.id}&since=0,0`, { headers: AUTH });
    expect((await w.json<any>()).id).toBe(item.id);
  });

  it("rejects an appended check label over 200 code points", async () => {
    const item = await push({ title: "PRs", checks: ["a"] });
    const res = await SELF.fetch(`http://lam/items/${item.id}/checks`, json({ label: "😀".repeat(201) }));
    expect(res.status).toBe(400);
  });

  it("phone page toggles checks via form and closes on last tick", async () => {
    const item = await push({ title: "PRs", checks: ["a", "b"] });
    const t = await itemToken("test-secret", item.id);
    const page = await (await SELF.fetch(`http://lam/r/${item.id}?t=${t}`)).text();
    expect(page).toContain("☐ a");
    expect(page).toContain("0/2 done");
    const r = await SELF.fetch(`http://lam/r/${item.id}/checks/0?t=${t}`, { method: "POST", body: new URLSearchParams({ done: "true" }), redirect: "manual" });
    expect(r.status).toBe(303);
    expect(r.headers.get("location")).toBe(`/r/${item.id}?t=${t}`);
    expect((await SELF.fetch(`http://lam/r/${item.id}/checks/1?t=bad`, { method: "POST", body: new URLSearchParams({ done: "true" }) })).status).toBe(403);
    const last = await SELF.fetch(`http://lam/r/${item.id}/checks/1?t=${t}`, { method: "POST", body: new URLSearchParams({ done: "true" }) });
    expect(last.status).toBe(200);
    const got = await (await SELF.fetch(`http://lam/items/${item.id}`, { headers: AUTH })).json<any>();
    expect(got).toMatchObject({ status: "resolved", response_by: "phone" });
  });
});

describe("ntfy-compatible topic", () => {
  it("serves only the configured topic", async () => {
    expect((await SELF.fetch("http://lam/other/json?poll=1")).status).toBe(404);
    expect((await SELF.fetch("http://lam/other", { method: "POST", body: "x" })).status).toBe(404);
    expect((await SELF.fetch("http://lam/test-topic/auth")).status).toBe(200);
    expect(await (await SELF.fetch("http://lam/v1/health")).json()).toEqual({ healthy: true });
  });
  it("accepts raw ntfy publishes with headers and Actions", async () => {
    const res = await SELF.fetch("http://lam/test-topic", {
      method: "POST",
      headers: { Title: "manual", Priority: "4", Tags: "eyes, tada", Actions: "http, Go, https://x/go, clear=true; view, Open, https://x/open" },
      body: "hello",
    });
    expect(res.status).toBe(200);
    const msg = await res.json<any>();
    expect(msg).toMatchObject({ title: "manual", message: "hello", priority: 4, tags: ["eyes", "tada"] });
    expect(msg.actions).toMatchObject([{ action: "http", label: "Go", url: "https://x/go", clear: true }, { action: "view", label: "Open", url: "https://x/open", clear: false }]);
    expect((await SELF.fetch("http://lam/test-topic", { method: "POST", headers: { Priority: "9" }, body: "x" })).status).toBe(400);
  });
  it("poll honours since=none|all|<id>", async () => {
    const a = await (await SELF.fetch("http://lam/test-topic", { method: "POST", body: "one" })).json<any>();
    const b = await (await SELF.fetch("http://lam/test-topic", { method: "POST", body: "two" })).json<any>();
    expect(await topicMessages("none")).toEqual([]);
    const all = await topicMessages("all");
    expect(all.map((m) => m.id)).toEqual(expect.arrayContaining([a.id, b.id]));
    const after = await topicMessages(a.id);
    expect(after.map((m) => m.id)).toEqual([b.id]);
  });
  it("json stream opens with an open event and delivers live messages", async () => {
    const res = await SELF.fetch("http://lam/test-topic/json?since=none");
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toBe("application/x-ndjson");
    const reader = res.body!.getReader();
    const dec = new TextDecoder();
    let buf = "";
    const nextLine = async () => {
      while (!buf.includes("\n")) buf += dec.decode((await reader.read()).value);
      const [line, ...rest] = buf.split("\n");
      buf = rest.join("\n");
      return JSON.parse(line);
    };
    expect(await nextLine()).toMatchObject({ event: "open", topic: "test-topic" });
    await SELF.fetch("http://lam/test-topic", { method: "POST", body: "live" });
    expect(await nextLine()).toMatchObject({ event: "message", message: "live" });
    await reader.cancel();
  });
});
