import { Effect, Layer } from "effect";
import { Env, type Bindings } from "../Env";
import type { PushMessage } from "../domain/PushMessage";
import { FcmClient, type FcmClientService, InvalidRegistration, PushDeliveryError } from "./Push";

const TOKEN_URI = "https://oauth2.googleapis.com/token";
const FCM_SCOPE = "https://www.googleapis.com/auth/firebase.messaging";
const MAX_RESPONSE_BYTES = 64 * 1024;

interface ServiceAccount {
  readonly client_email: string;
  readonly private_key: string;
  readonly token_uri: typeof TOKEN_URI;
}

interface CachedToken {
  readonly value: string;
  readonly validUntil: number;
}

const encode = (value: string | Uint8Array) => {
  const bytes = typeof value === "string" ? new TextEncoder().encode(value) : value;
  return btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
};

function decodeServiceAccount(raw: string | undefined): ServiceAccount {
  const parsed: unknown = JSON.parse(raw ?? "null");
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("missing FCM service account");
  const record = parsed as Record<string, unknown>;
  if (typeof record.client_email !== "string" || !record.client_email.endsWith(".iam.gserviceaccount.com")) throw new Error("invalid FCM client email");
  if (typeof record.private_key !== "string" || !record.private_key.includes("BEGIN PRIVATE KEY")) throw new Error("invalid FCM private key");
  if (record.token_uri !== TOKEN_URI) throw new Error("invalid FCM token endpoint");
  return { client_email: record.client_email, private_key: record.private_key, token_uri: TOKEN_URI };
}

function decodePrivateKey(pem: string): ArrayBuffer {
  const encoded = pem.replace(/-----BEGIN PRIVATE KEY-----|-----END PRIVATE KEY-----|\s/g, "");
  const decoded = atob(encoded);
  const bytes = new Uint8Array(decoded.length);
  for (let index = 0; index < decoded.length; index++) bytes[index] = decoded.charCodeAt(index);
  return bytes.buffer;
}

async function boundedJson(response: Response): Promise<unknown> {
  const declared = response.headers.get("content-length");
  if (declared !== null && Number(declared) > MAX_RESPONSE_BYTES) throw new Error("FCM response too large");
  if (response.body === null) return null;
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    length += value.byteLength;
    if (length > MAX_RESPONSE_BYTES) {
      await reader.cancel();
      throw new Error("FCM response too large");
    }
    chunks.push(value);
  }
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return JSON.parse(new TextDecoder().decode(bytes));
}

function isUnregistered(body: unknown): boolean {
  if (body === null || typeof body !== "object" || Array.isArray(body)) return false;
  const error = (body as { error?: unknown }).error;
  if (error === null || typeof error !== "object" || Array.isArray(error)) return false;
  const record = error as { status?: unknown; details?: unknown };
  if (record.status === "UNREGISTERED") return true;
  return Array.isArray(record.details) && record.details.some((detail) =>
    detail !== null && typeof detail === "object" && !Array.isArray(detail)
      && (detail as { errorCode?: unknown }).errorCode === "UNREGISTERED",
  );
}

export function makeFcmClient(
  env: Bindings,
  fetcher: typeof fetch = fetch,
  now: () => number = Date.now,
): FcmClientService {
  let cached: CachedToken | null = null;
  let key: Promise<CryptoKey> | null = null;

  const accessToken = async (): Promise<string> => {
    if (cached !== null && cached.validUntil > now()) return cached.value;
    const account = decodeServiceAccount(env.FCM_SERVICE_ACCOUNT_JSON);
    const issuedAt = Math.floor(now() / 1000);
    const header = encode(JSON.stringify({ alg: "RS256", typ: "JWT" }));
    const claims = encode(JSON.stringify({
      iss: account.client_email,
      scope: FCM_SCOPE,
      aud: account.token_uri,
      iat: issuedAt,
      exp: issuedAt + 3600,
    }));
    key ??= crypto.subtle.importKey(
      "pkcs8",
      decodePrivateKey(account.private_key),
      { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" },
      false,
      ["sign"],
    );
    const unsigned = `${header}.${claims}`;
    const signature = await crypto.subtle.sign("RSASSA-PKCS1-v1_5", await key, new TextEncoder().encode(unsigned));
    const assertion = `${unsigned}.${encode(new Uint8Array(signature))}`;
    const body = new URLSearchParams({
      grant_type: "urn:ietf:params:oauth:grant-type:jwt-bearer",
      assertion,
    });
    const response = await fetcher(account.token_uri, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body,
    });
    if (!response.ok) throw new Error("FCM OAuth request failed");
    const decoded = await boundedJson(response);
    if (decoded === null || typeof decoded !== "object" || Array.isArray(decoded)) throw new Error("invalid FCM OAuth response");
    const token = decoded as Record<string, unknown>;
    if (typeof token.access_token !== "string" || typeof token.expires_in !== "number") throw new Error("invalid FCM OAuth response");
    cached = { value: token.access_token, validUntil: now() + token.expires_in * 1000 - 60_000 };
    return cached.value;
  };

  return {
    send: (deviceToken, message) => Effect.tryPromise({
      try: async () => {
        if (env.FCM_PROJECT_ID === undefined || !/^[a-z][a-z0-9-]{4,29}$/.test(env.FCM_PROJECT_ID)) {
          throw new Error("missing FCM project ID");
        }
        const token = await accessToken();
        const response = await fetcher(`https://fcm.googleapis.com/v1/projects/${env.FCM_PROJECT_ID}/messages:send`, {
          method: "POST",
          headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
          body: JSON.stringify({ message: { fid: deviceToken, ...message } }),
        });
        if (response.ok) return;
        if (response.status === 404) {
          const body = await boundedJson(response);
          if (isUnregistered(body)) throw new InvalidRegistration();
        }
        throw new Error("FCM send failed");
      },
      catch: (cause) => cause instanceof InvalidRegistration ? cause : new PushDeliveryError(),
    }),
  };
}

export const FcmClientLive = Layer.effect(FcmClient, Effect.map(Env, (env) => makeFcmClient(env)));
