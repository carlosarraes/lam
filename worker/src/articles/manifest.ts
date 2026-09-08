import { Schema } from "effect";
import { ArticleDraft } from "../domain/Article";
import { BadRequest } from "../domain/Item";

export const HTML_BYTES = 2 * 1024 * 1024;
export const ASSET_BYTES = 20 * 1024 * 1024;
export const TOTAL_BYTES = 50 * 1024 * 1024;
export const MANIFEST_BYTES = 128 * 1024;
const rasterTypes = new Set(["image/png", "image/jpeg", "image/webp"]);
const downloadTypes = new Set(["application/pdf", "text/plain", ...rasterTypes]);
const invalid = (message: string): never => { throw new BadRequest({ message }); };

export function normalizeAssetPath(path: string): string {
  if (typeof path !== "string" || path.length === 0 || path.length > 512 || /[%\\:?#\u0000-\u001f\u007f\ud800-\udfff]/u.test(path))
    return invalid("invalid asset path");
  const normalized = path.normalize("NFC");
  if (normalized.split("/").some(part => part === "" || part === "." || part === "..")) return invalid("invalid asset path");
  return normalized;
}

/** Decode at runtime too: this boundary is also used by non-HTTP callers. */
export function validateManifest(input: unknown): ArticleDraft {
  let draft: ArticleDraft;
  try { draft = Schema.decodeUnknownSync(ArticleDraft)(input, { onExcessProperty: "error" }); }
  catch { return invalid("invalid article manifest"); }
  if (draft.title.trim() === "" || [...draft.title].length > 200 || [...draft.summary].length > 2000)
    return invalid("article title or summary exceeds limits");
  for (const value of [draft.name, draft.source_host, draft.source_project])
    if ([...value].length > 200 || /[\u0000-\u001f\u007f]/u.test(value)) return invalid("invalid article source metadata");
  if (draft.assets.length < 1 || draft.assets.length > 51) return invalid("an article allows 50 attachments plus index.html");
  const paths = new Set<string>();
  let htmlCount = 0;
  let total = 0;
  const assets = draft.assets.map(asset => {
    const path = normalizeAssetPath(asset.path);
    if (paths.has(path)) return invalid("duplicate asset path");
    paths.add(path);
    const entry = path === "index.html";
    if (entry) htmlCount++;
    if (!Number.isSafeInteger(asset.size) || asset.size < 0 || asset.size > (entry ? HTML_BYTES : ASSET_BYTES))
      return invalid("asset size exceeds limits");
    if (!/^[a-f0-9]{64}$/.test(asset.sha256)) return invalid("asset hash must be lowercase SHA-256 hex");
    const allowedTypes = asset.disposition === "inline" ? rasterTypes : downloadTypes;
    const allowed = entry
      ? asset.media_type === "text/html" && asset.disposition === "inline"
      : allowedTypes.has(asset.media_type);
    if (!allowed)
      return invalid("unsupported asset media type or disposition");
    total += asset.size;
    return { path, media_type: asset.media_type, size: asset.size, sha256: asset.sha256, disposition: asset.disposition };
  });
  if (htmlCount !== 1 || total > TOTAL_BYTES) return invalid("article requires index.html and at most 50 MiB total");
  return { title: draft.title, summary: draft.summary, name: draft.name, source_host: draft.source_host, source_project: draft.source_project, silent: draft.silent, assets };
}

/** Content-Length is advisory. Allocate at most the declared manifest cap and count actual bytes. */
export async function readBoundedBody(body: ReadableStream<Uint8Array> | null, limit: number): Promise<Uint8Array<ArrayBuffer>> {
  const result = new Uint8Array(limit);
  if (!body) return result.subarray(0, 0);
  const reader = body.getReader();
  let length = 0;
  try {
    while (true) {
      const chunk = await reader.read();
      if (chunk.done) break;
      if (length + chunk.value.byteLength > limit) {
        await reader.cancel();
        return invalid("request body exceeds byte limit");
      }
      result.set(chunk.value, length);
      length += chunk.value.byteLength;
    }
    return result.subarray(0, length);
  } finally { reader.releaseLock(); }
}
