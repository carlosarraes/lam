import { Context, Effect, Option } from "effect";
import { Env } from "../Env";
import type { Device } from "../domain/Device";
import { Forbidden, Unauthorized } from "../domain/Item";
import { Devices } from "./Devices";

const enc = new TextEncoder();

export const hmacDigest = (secret: string, data: string) =>
  Effect.promise(async () => {
    const key = await crypto.subtle.importKey("raw", enc.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    const sig = new Uint8Array(await crypto.subtle.sign("HMAC", key, enc.encode(data)));
    return btoa(String.fromCharCode(...sig)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  });

/** Byte-wise comparison whose work does not stop at the first difference. */
export const constantTimeEqual = (a: string, b: string) => {
  const aBytes = enc.encode(a);
  const bBytes = enc.encode(b);
  let diff = aBytes.length ^ bBytes.length;
  const length = Math.max(aBytes.length, bBytes.length);
  for (let i = 0; i < length; i++) diff |= (aBytes[i] ?? 0) ^ (bBytes[i] ?? 0);
  return diff === 0;
};

export type Authority =
  | { readonly kind: "master" }
  | { readonly kind: "device"; readonly device: Device };

export class RequestAuthority extends Context.Tag("lam/RequestAuthority")<RequestAuthority, Authority>() {}

export class Auth extends Effect.Service<Auth>()("lam/Auth", {
  succeed: {
    /** Per-item token embedded in phone action URLs so the phone never carries the bearer. */
    itemToken: (id: string) => Effect.flatMap(Env, (e) => hmacDigest(e.LAM_HMAC_SECRET, id)),

    requireItemToken: (id: string, token: string | undefined) =>
      Effect.flatMap(Env, (e) => hmacDigest(e.LAM_HMAC_SECRET, id)).pipe(
        Effect.filterOrFail((expected) => token !== undefined && constantTimeEqual(expected, token), () => new Forbidden()),
        Effect.asVoid,
      ),

    authenticateBearer: (header: string | undefined) =>
      Effect.gen(function* () {
        if (header === undefined || !header.startsWith("Bearer ")) return yield* new Unauthorized();
        const credential = header.slice("Bearer ".length);
        if (credential.length === 0) return yield* new Unauthorized();
        const env = yield* Env;
        if (constantTimeEqual(credential, env.LAM_TOKEN)) return { kind: "master" } as const;
        const digest = yield* hmacDigest(env.LAM_HMAC_SECRET, credential);
        const device = yield* (yield* Devices).authenticate(digest);
        return Option.isSome(device)
          ? ({ kind: "device", device: device.value } as const)
          : yield* new Unauthorized();
      }),

    requireMaster: (authority: Authority) =>
      authority.kind === "master" ? Effect.succeed(authority) : Effect.fail(new Forbidden()),

    requireDevice: (authority: Authority) =>
      authority.kind === "device" ? Effect.succeed(authority) : Effect.fail(new Forbidden()),
  },
}) {}
