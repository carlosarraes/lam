import { Effect } from "effect";
import { Env } from "../Env";
import { Forbidden, Unauthorized } from "../domain/Item";

const enc = new TextEncoder();

const hmac = (secret: string, data: string) =>
  Effect.promise(async () => {
    const key = await crypto.subtle.importKey("raw", enc.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    const sig = new Uint8Array(await crypto.subtle.sign("HMAC", key, enc.encode(data)));
    return btoa(String.fromCharCode(...sig)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  });

const constantTimeEqual = (a: string, b: string) => {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
};

export class Auth extends Effect.Service<Auth>()("lam/Auth", {
  succeed: {
    /** Per-item token embedded in phone action URLs so the phone never carries the bearer. */
    itemToken: (id: string) => Effect.flatMap(Env, (e) => hmac(e.LAM_HMAC_SECRET, id)),

    requireItemToken: (id: string, token: string | undefined) =>
      Effect.flatMap(Env, (e) => hmac(e.LAM_HMAC_SECRET, id)).pipe(
        Effect.filterOrFail((expected) => token !== undefined && constantTimeEqual(expected, token), () => new Forbidden()),
        Effect.asVoid,
      ),

    requireBearer: (header: string | undefined) =>
      Effect.flatMap(Env, (e) => (header === `Bearer ${e.LAM_TOKEN}` ? Effect.void : Effect.fail(new Unauthorized()))),
  },
}) {}
