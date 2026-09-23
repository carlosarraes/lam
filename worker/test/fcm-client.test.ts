import { Effect } from "effect";
import { describe, expect, it } from "vitest";
import type { Bindings } from "../src/Env";
import type { PushMessage } from "../src/domain/PushMessage";

const message: PushMessage = {
  android: { priority: "high" },
  data: {
    event: "item.created",
    item_id: "item-1",
    version: "0",
    status: "open",
    kind: "request",
    name: "pm",
    priority: "critical",
    title: "Release decision",
  },
};

async function serviceAccount() {
  const pair = await crypto.subtle.generateKey(
    { name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" },
    true,
    ["sign", "verify"],
  );
  const der = await crypto.subtle.exportKey("pkcs8", pair.privateKey);
  const encoded = btoa(String.fromCharCode(...new Uint8Array(der))).match(/.{1,64}/g)!.join("\n");
  return JSON.stringify({
    type: "service_account",
    project_id: "lam-test",
    private_key_id: "key-id",
    private_key: `-----BEGIN PRIVATE KEY-----\n${encoded}\n-----END PRIVATE KEY-----\n`,
    client_email: "lam-push@lam-test.iam.gserviceaccount.com",
    token_uri: "https://oauth2.googleapis.com/token",
  });
}

const env = (json: string) => ({
  FCM_PROJECT_ID: "lam-test",
  FCM_SERVICE_ACCOUNT_JSON: json,
}) as Bindings;

describe("FCM HTTP v1 client", () => {
  it("mints one scoped OAuth token and reuses it for valid sends", async () => {
    const module = await import("../src/services/FcmClient").catch(() => null);
    expect(module).not.toBeNull();
    if (!module) return;
    const requests: Request[] = [];
    const fetcher: typeof fetch = async (input, init) => {
      const request = new Request(input, init);
      requests.push(request);
      if (request.url === "https://oauth2.googleapis.com/token") {
        return Response.json({ access_token: "access-token", expires_in: 3600, token_type: "Bearer" });
      }
      return Response.json({ name: "projects/lam-test/messages/1" });
    };
    const client = module.makeFcmClient(env(await serviceAccount()), fetcher, () => 1_800_000_000_000);

    await Effect.runPromise(client.send("device-token-a", message));
    await Effect.runPromise(client.send("device-token-b", message));

    expect(requests.map((request) => request.url)).toEqual([
      "https://oauth2.googleapis.com/token",
      "https://fcm.googleapis.com/v1/projects/lam-test/messages:send",
      "https://fcm.googleapis.com/v1/projects/lam-test/messages:send",
    ]);
    const tokenBody = new URLSearchParams(await requests[0]!.text());
    const assertion = tokenBody.get("assertion")!;
    const payload = JSON.parse(atob(assertion.split(".")[1]!.replace(/-/g, "+").replace(/_/g, "/")));
    expect(payload).toMatchObject({
      iss: "lam-push@lam-test.iam.gserviceaccount.com",
      aud: "https://oauth2.googleapis.com/token",
      scope: "https://www.googleapis.com/auth/firebase.messaging",
      iat: 1_800_000_000,
      exp: 1_800_003_600,
    });
    expect(requests[1]!.headers.get("authorization")).toBe("Bearer access-token");
    expect(await requests[1]!.json()).toEqual({ message: { fid: "device-token-a", ...message } });
  });

  it("classifies an unregistered device without leaking its token", async () => {
    const module = await import("../src/services/FcmClient").catch(() => null);
    expect(module).not.toBeNull();
    if (!module) return;
    const fetcher: typeof fetch = async (input) => new URL(input instanceof Request ? input.url : input.toString()).hostname === "oauth2.googleapis.com"
      ? Response.json({ access_token: "access-token", expires_in: 3600, token_type: "Bearer" })
      : Response.json({
          error: {
            status: "NOT_FOUND",
            details: [{
              "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError",
              errorCode: "UNREGISTERED",
            }],
          },
        }, { status: 404 });
    const client = module.makeFcmClient(env(await serviceAccount()), fetcher, () => 1_800_000_000_000);

    const failure = await Effect.runPromise(Effect.flip(client.send("secret-device-token", message)));

    expect(failure._tag).toBe("InvalidRegistration");
    expect(JSON.stringify(failure)).not.toContain("secret-device-token");
  });
});
