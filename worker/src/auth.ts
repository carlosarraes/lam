const enc = new TextEncoder();

async function hmac(secret: string, data: string): Promise<string> {
  const key = await crypto.subtle.importKey("raw", enc.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode(data));
  return btoa(String.fromCharCode(...new Uint8Array(sig))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** Per-item token embedded in ntfy action URLs so the phone never carries the bearer token. */
export function itemToken(secret: string, id: string): Promise<string> {
  return hmac(secret, id);
}

export async function verifyItemToken(secret: string, id: string, token: string | undefined): Promise<boolean> {
  if (!token) return false;
  const expected = await itemToken(secret, id);
  if (expected.length !== token.length) return false;
  let diff = 0;
  for (let i = 0; i < expected.length; i++) diff |= expected.charCodeAt(i) ^ token.charCodeAt(i);
  return diff === 0;
}

export function bearerOk(header: string | undefined, token: string): boolean {
  return header === `Bearer ${token}`;
}
