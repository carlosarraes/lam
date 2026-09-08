import { HttpRouter, HttpServerRequest, HttpServerResponse } from "@effect/platform";
import { Effect, Schema } from "effect";
import { parse, serialize, defaultTreeAdapter, html as namespaces, type DefaultTreeAdapterTypes as Html } from "parse5";
import * as css from "css-tree";
import { Env } from "../Env";
import { HTML_BYTES, readBoundedBody } from "../articles/manifest";
import { ReadUpdate, type Asset } from "../domain/Article";
import { BadRequest, DbError, Forbidden, Unauthorized } from "../domain/Item";
import { Articles } from "../services/Articles";
import { hmacDigest, constantTimeEqual, RequestAuthority } from "../services/Auth";
import { bearer } from "./api";

const Id = Schema.Struct({ id: Schema.String.pipe(Schema.pattern(/^[a-f0-9-]{36}$/)) });
const AssetId = Schema.Struct({ ...Id.fields, index: Schema.NumberFromString.pipe(Schema.int(), Schema.between(0, 50)) });
const headers = { "cache-control": "private, no-store", "referrer-policy": "no-referrer", "x-content-type-options": "nosniff", "x-frame-options": "DENY" };
const closedCsp = "default-src 'none'; frame-ancestors 'none'; sandbox";
const json = (body: unknown, status = 200) => HttpServerResponse.json(body, { status, headers: { ...headers, "content-security-policy": closedCsp } });
const db = <A>(run: () => Promise<A>) => Effect.tryPromise({ try: run, catch: () => new DbError({ cause: "article viewer database operation failed" }) });
const request = Effect.gen(function* () {
  const req = yield* HttpServerRequest.HttpServerRequest;
  if (!(req.source instanceof Request)) return yield* new BadRequest({ message: "native request required" });
  return req.source;
});
const body = Effect.gen(function* () {
  const req = yield* request;
  return yield* Effect.tryPromise({ try: async () => JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(await readBoundedBody(req.body, 8192))), catch: () => new BadRequest({ message: "invalid viewer request" }) });
});
const bearerToken = Effect.gen(function* () {
  const header = (yield* request).headers.get("authorization");
  if (!header?.startsWith("Bearer ") || !/^[A-Za-z0-9_-]{43}$/.test(header.slice(7))) return yield* new Unauthorized();
  return header.slice(7);
});
const randomToken = () => btoa(String.fromCharCode(...crypto.getRandomValues(new Uint8Array(32)))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

/** Bound decoder allocations from raster headers before giving bytes to the browser. */
function imagePixels(bytes: Uint8Array, type: string): number {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const checked = (width: number, height: number) => {
    if (!width || !height || width > 8192 || height > 8192 || width * height > 16_000_000) throw new BadRequest({ message: "image dimensions exceed viewer limits" });
    return width * height;
  };
  try {
    if (type === "image/png") return checked(view.getUint32(16), view.getUint32(20));
    if (type === "image/jpeg") {
      let offset = 2;
      while (offset + 4 < bytes.length) {
        if (bytes[offset++] !== 255) break;
        while (bytes[offset] === 255) offset++;
        const marker = bytes[offset++];
        const length = view.getUint16(offset);
        if (length < 2 || offset + length > bytes.length) break;
        if (marker >= 0xc0 && marker <= 0xcf && ![0xc4, 0xc8, 0xcc].includes(marker)) return checked(view.getUint16(offset + 5), view.getUint16(offset + 3));
        offset += length;
      }
    }
    if (type === "image/webp") {
      let offset = 12;
      let pixels: number | undefined;
      while (offset + 8 <= bytes.length) {
        const kind = new TextDecoder().decode(bytes.subarray(offset, offset + 4));
        const size = view.getUint32(offset + 4, true);
        const data = offset + 8;
        if (data + size > bytes.length) break;
        if (kind === "VP8X") {
          const uint24 = (at: number) => bytes[at] + (bytes[at + 1] << 8) + (bytes[at + 2] << 16);
          pixels = checked(uint24(data + 4) + 1, uint24(data + 7) + 1);
        } else if (kind === "VP8 ") pixels = Math.max(pixels ?? 0, checked(view.getUint16(data + 6, true) & 0x3fff, view.getUint16(data + 8, true) & 0x3fff));
        else if (kind === "VP8L") {
          const bits = view.getUint32(data + 1, true);
          pixels = Math.max(pixels ?? 0, checked((bits & 0x3fff) + 1, ((bits >>> 14) & 0x3fff) + 1));
        }
        offset = data + size + (size % 2);
      }
      if (pixels) return pixels;
    }
  } catch { throw new BadRequest({ message: "image dimensions unavailable or exceed viewer limits" }); }
  throw new BadRequest({ message: "image dimensions unavailable" });
}
interface Session { bootstrap_hash: string; article_id: string; credential_hash: string; device_id: string | null; bootstrap_expires_at: string; expires_at: string; session_hash: string | null; delivered_version: number | null }
const authenticate = (id: string, kind: "bootstrap_hash" | "session_hash", token: string) => Effect.gen(function* () {
  const env = yield* Env;
  const hash = yield* hmacDigest(env.LAM_HMAC_SECRET, token);
  const row = yield* db(() => env.DB.prepare(`SELECT s.* FROM article_view_sessions s JOIN articles a ON a.id = s.article_id WHERE s.${kind} = ? AND s.article_id = ? AND a.owner_id = 'owner' AND a.state = 'published' AND s.expires_at > ?`).bind(hash, id, new Date().toISOString()).first<Session>());
  if (!row) return yield* new Unauthorized();
  if (kind === "bootstrap_hash" && (row.session_hash !== null || row.bootstrap_expires_at <= new Date().toISOString())) return yield* new Unauthorized();
  if (row.device_id === null) {
    if (!constantTimeEqual(row.credential_hash, yield* hmacDigest(env.LAM_HMAC_SECRET, env.LAM_TOKEN))) return yield* new Unauthorized();
  } else {
    const active = yield* db(() => env.DB.prepare("SELECT credential_hash FROM devices WHERE id = ? AND revoked_at IS NULL").bind(row.device_id).first<{ credential_hash: string }>());
    if (!active || !constantTimeEqual(active.credential_hash, row.credential_hash)) return yield* new Unauthorized();
  }
  return row;
});

/** Rewrite only URL-bearing parser fields in the already validated canonical document. */
export function viewerParts(source: string, assets: readonly Asset[], mode: "desktop" | "native" = "desktop"): (string | { asset: number })[] {
  const document = parse(source);
  let marker: string;
  const original = serialize(document);
  do { marker = `lam-slot-${crypto.randomUUID()}-`; } while (original.includes(marker));
  const slots: number[] = [];
  const resource = (value: string) => {
    if (!value.startsWith("lam-asset:")) return value;
    const index = Number(value.slice(10));
    const asset = assets[index];
    if (!Number.isInteger(index) || !asset || asset.disposition !== "inline" || !asset.media_type.startsWith("image/")) throw new BadRequest({ message: "invalid article resource" });
    const slot = slots.push(index) - 1;
    return `${marker}${slot}-end`;
  };
  const rewriteCss = (source: string, context: "stylesheet" | "declarationList" | "value") => {
    const ast = css.parse(source, { context });
    css.walk(ast, node => { if (node.type === "Url") node.value = resource(node.value); });
    return css.generate(ast);
  };
  const visit = (node: Html.ParentNode) => {
    for (const child of node.childNodes) {
      if (!("tagName" in child)) continue;
      for (const attr of child.attrs) {
        if (mode === "desktop" && child.tagName === "a" && attr.name === "href" && attr.value.startsWith("#")) attr.value = `about:srcdoc${attr.value}`;
        if ((child.tagName === "img" && attr.name === "src") || (child.tagName === "image" && attr.name === "href")) attr.value = resource(attr.value);
        else if (attr.name === "style") attr.value = rewriteCss(attr.value, "declarationList");
        else if (["fill", "stroke", "clip-path", "marker-start", "marker-mid", "marker-end"].includes(attr.name)) attr.value = rewriteCss(attr.value, "value");
      }
      if (child.tagName === "style") for (const text of child.childNodes) if (text.nodeName === "#text" && "value" in text) text.value = rewriteCss(text.value, "stylesheet");
      visit(child);
    }
  };
  visit(document);
  const root = document.childNodes.find((node): node is Html.Element => "tagName" in node && node.tagName === "html")!;
  const head = root.childNodes.find((node): node is Html.Element => "tagName" in node && node.tagName === "head")!;
  const images = mode === "native" ? "https://articles.lam.invalid" : "data:";
  const policy = `default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src ${images}; base-uri 'none'; form-action 'none'`;
  const meta = defaultTreeAdapter.createElement("meta", namespaces.NS.HTML, [{ name: "http-equiv", value: "Content-Security-Policy" }, { name: "content", value: policy }]);
  head.childNodes.unshift(meta);
  meta.parentNode = head;
  const serialized = serialize(document);
  const parts: (string | { asset: number })[] = [];
  let offset = 0;
  let found = 0;
  while (true) {
    const start = serialized.indexOf(marker, offset);
    if (start < 0) break;
    const end = serialized.indexOf("-end", start + marker.length);
    const slot = Number(serialized.slice(start + marker.length, end));
    if (end < 0 || !Number.isInteger(slot) || slots[slot] === undefined || ++found > 25000) throw new BadRequest({ message: "article resource limit exceeded" });
    parts.push(serialized.slice(offset, start), { asset: slots[slot] });
    offset = end + 4;
  }
  parts.push(serialized.slice(offset));
  if (found !== slots.length) throw new BadRequest({ message: "article resource serialization failed" });
  return parts;
}

// The iframe receives no JavaScript, credentials, cookie access or same-origin permission.
function wrapper(nonce: string): string {
  return `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>LAM article</title><style>body{margin:0;font:16px system-ui;background:#fafafa;color:#202020}header{padding:12px}button{font:inherit;margin:4px;padding:6px}iframe{width:100%;height:80vh;border:0;background:white}#status{white-space:pre-wrap}#attachments{display:flex;flex-wrap:wrap}</style></head><body><header><h1 id="title">Article</h1><p id="status" role="status">Opening article…</p><button id="retry" hidden>Retry</button><button id="smaller">Zoom out</button><button id="larger">Zoom in</button><div id="attachments"></div></header><main><iframe title="Article content" sandbox="allow-popups allow-popups-to-escape-sandbox" referrerpolicy="no-referrer" hidden></iframe></main><script nonce="${nonce}">
(() => {
  let bootstrap = location.hash.slice(1); history.replaceState(null, '', location.pathname);
  let token, captured, manifest, loaded = false, attempted = false, opening = false, zoom = 1, imageNotice = '';
  const base = location.pathname, frame = document.querySelector('iframe'), status = document.querySelector('#status'), retry = document.querySelector('#retry');
  const call = (path, method = 'GET', body, credential = token) => fetch(base + path, { method, credentials:'omit', cache:'no-store', referrerPolicy:'no-referrer', headers:{Authorization:'Bearer ' + credential, 'Content-Type':'application/json'}, body:body === undefined ? undefined : JSON.stringify(body) });
  const failed = message => { status.textContent = message; retry.hidden = false; };
  async function markRead() {
    const bounds = frame.getBoundingClientRect();
    if (!loaded || attempted || document.visibilityState !== 'visible' || frame.hidden || bounds.height === 0 || bounds.width === 0 || bounds.top >= innerHeight || bounds.bottom <= 0 || bounds.left >= innerWidth || bounds.right <= 0) return;
    attempted = true;
    try {
      const response = await call('/read', 'PUT', {read:true, version:captured.version});
      if (response.status === 409) { status.textContent = 'Opened. Read state changed elsewhere; the newer state was kept.'; return; }
      if (!response.ok) { failed('Article opened, but read state could not be saved. Retry keeps the original version.'); return; }
      status.textContent = 'Opened · read' + imageNotice;
    } catch { failed('Article opened, but read state could not be saved. Retry keeps the original version.'); }
  }
  async function openArticle() {
    if (opening) return;
    opening = true; retry.hidden = true;
    try {
      if (!token) {
        if (!bootstrap) throw Error();
        const response = await call('/exchange', 'POST', undefined, bootstrap);
        if (!response.ok) throw Error();
        const session = await response.json(); token = session.token; manifest = session.article; bootstrap = '';
      }
      if (loaded) { attempted = false; await markRead(); return; }
      const resources = {}; let missing = 0, total = 0, pixels = 0;
      for (const [index, asset] of manifest.assets.entries()) {
        if (asset.disposition !== 'inline' || !asset.media_type.startsWith('image/')) continue;
        try {
          if (asset.size > 20971520 || total + asset.size > 52428800) throw Error();
          const response = await call('/images/' + index);
          if (!response.ok) throw Error();
          const imagePixels = Number(response.headers.get('x-lam-image-pixels'));
          if (!Number.isSafeInteger(imagePixels) || imagePixels <= 0 || pixels + imagePixels > 32000000) { await response.body.cancel(); throw Error(); }
          const blob = await response.blob();
          if (blob.size !== asset.size) throw Error();
          total += blob.size;
          pixels += imagePixels;
          const bitmap = await createImageBitmap(blob); bitmap.close();
          const data = await new Promise((resolve, reject) => { const reader = new FileReader(); reader.onload = () => resolve(reader.result); reader.onerror = reject; reader.readAsDataURL(blob); });
          if (typeof data !== 'string' || !data.startsWith('data:' + asset.media_type + ';base64,') || !/^[A-Za-z0-9+/]*={0,2}$/.test(data.slice(data.indexOf(',') + 1))) throw Error();
          resources[index] = data;
        } catch { missing++; }
      }
      const response = await call('/content', 'POST', {});
      if (!response.ok) throw Error();
      const content = await response.json();
      captured = content.article;
      if (!Array.isArray(content.parts) || content.parts.length > 50001) throw Error();
      let markupBytes = 0, slotCount = 0;
      for (const part of content.parts) {
        if (typeof part === 'string') { markupBytes += new TextEncoder().encode(part).length; if (markupBytes > 2105344) throw Error(); }
        else {
          if (!part || Object.keys(part).length !== 1 || !Number.isInteger(part.asset) || part.asset < 0 || part.asset >= manifest.assets.length) throw Error();
          const asset = manifest.assets[part.asset];
          if (asset.disposition !== 'inline' || !['image/png','image/jpeg','image/webp'].includes(asset.media_type)) throw Error();
          slotCount++;
        }
      }
      let remaining = 75497472 - markupBytes - slotCount * 6;
      const html = content.parts.map(part => {
        if (typeof part === 'string') return part;
        const value = resources[part.asset] || 'data:,';
        if (value.length - 6 > remaining) { missing++; return 'data:,'; }
        remaining -= value.length - 6; return value;
      });
      imageNotice = missing ? ' · ' + missing + ' images or placements unavailable. Reopen from LAM to retry images.' : '';
      document.querySelector('#title').textContent = captured.title;
      document.title = captured.title + ' · LAM';
      const attachments = document.querySelector('#attachments'); attachments.replaceChildren();
      captured.assets.forEach((asset, index) => {
        if (asset.disposition !== 'attachment') return;
        const button = document.createElement('button'); button.textContent = 'Download ' + asset.path;
        button.onclick = async () => {
          button.disabled = true;
          try {
            const response = await call('/downloads/' + index);
            if (!response.ok) throw Error();
            const blob = await response.blob();
            const url = URL.createObjectURL(new Blob([blob], {type:'application/octet-stream'}));
            const anchor = document.createElement('a'); anchor.href = url; anchor.download = asset.path.split('/').pop(); anchor.rel = 'noopener noreferrer'; anchor.click();
            setTimeout(() => URL.revokeObjectURL(url), 60000);
          } catch { status.textContent = 'Download failed. Try the download button again.'; }
          finally { button.disabled = false; }
        };
        attachments.append(button);
      });
      frame.onload = () => { loaded = true; requestAnimationFrame(() => { void markRead(); }); };
      frame.hidden = false; frame.srcdoc = html.join('');
      status.textContent = 'Opening article… Images may be unavailable if the session expires.';
    } catch { failed('Could not open the article. Retry, or open it again from LAM if the link expired.'); }
    finally { opening = false; }
  }
  retry.onclick = () => { void openArticle(); };
  document.addEventListener('visibilitychange', () => { void markRead(); });
  new IntersectionObserver(() => { void markRead(); }).observe(frame);
  document.querySelector('#smaller').onclick = () => { zoom = Math.max(.5, zoom - .1); frame.style.zoom = zoom; frame.style.width = (100/zoom) + '%'; };
  document.querySelector('#larger').onclick = () => { zoom = Math.min(2, zoom + .1); frame.style.zoom = zoom; frame.style.width = (100/zoom) + '%'; };
  void openArticle();
})();</script></body></html>`;
}

export const articleViewerSessions = HttpRouter.empty.pipe(
  HttpRouter.post("/v2/articles/:id/view-session", Effect.gen(function* () {
    const { id } = yield* HttpRouter.schemaPathParams(Id);
    yield* (yield* Articles).get(id);
    const authority = yield* RequestAuthority;
    const env = yield* Env;
    const req = yield* request;
    const credential = req.headers.get("authorization")!.slice(7);
    const bootstrap = randomToken();
    const hash = yield* hmacDigest(env.LAM_HMAC_SECRET, bootstrap);
    const credentialHash = yield* hmacDigest(env.LAM_HMAC_SECRET, credential);
    const now = Date.now();
    const expires = new Date(now + 15 * 60_000).toISOString();
    const bootstrapExpires = new Date(now + 60_000).toISOString();
    yield* db(() => env.DB.batch([
      env.DB.prepare("DELETE FROM article_view_sessions WHERE bootstrap_hash IN (SELECT bootstrap_hash FROM article_view_sessions WHERE expires_at <= ? LIMIT 100)").bind(new Date(now).toISOString()),
      env.DB.prepare("INSERT INTO article_view_sessions (bootstrap_hash, article_id, credential_hash, device_id, bootstrap_expires_at, expires_at) VALUES (?, ?, ?, ?, ?, ?)").bind(hash, id, credentialHash, authority.kind === "device" ? authority.device.id : null, bootstrapExpires, expires),
    ]));
    return yield* json({ url: `${new URL(req.url).origin}/view/articles/${id}#${bootstrap}`, expires_at: bootstrapExpires }, 201);
  })),
  HttpRouter.use(Effect.provide(Articles.Default)),
  HttpRouter.use(bearer),
  HttpRouter.catchTags({
    Unauthorized: () => json({ error: "unauthorized" }, 401),
    NotFound: () => json({ error: "not found" }, 404),
    BadRequest: () => json({ error: "invalid viewer request" }, 400),
    ParseError: () => json({ error: "invalid viewer request" }, 400),
  }),
  HttpRouter.catchAll(() => json({ error: "viewer session request failed" }, 500)),
);

export const articleViewer = HttpRouter.empty.pipe(
  HttpRouter.get("/view/articles/:id", Effect.gen(function* () {
    yield* HttpRouter.schemaPathParams(Id);
    const nonce = randomToken();
    const policy = `default-src 'none'; script-src 'nonce-${nonce}'; style-src 'unsafe-inline'; connect-src 'self'; frame-src blob:; img-src data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`;
    return HttpServerResponse.text(wrapper(nonce), { headers: { ...headers, "content-type": "text/html; charset=utf-8", "content-security-policy": policy, "cross-origin-opener-policy": "same-origin" } });
  })),
  HttpRouter.post("/view/articles/:id/exchange", Effect.gen(function* () {
    const { id } = yield* HttpRouter.schemaPathParams(Id);
    const row = yield* authenticate(id, "bootstrap_hash", yield* bearerToken);
    const env = yield* Env;
    const token = randomToken();
    const hash = yield* hmacDigest(env.LAM_HMAC_SECRET, token);
    const result = yield* db(() => env.DB.prepare("UPDATE article_view_sessions SET session_hash = ? WHERE bootstrap_hash = ? AND session_hash IS NULL AND bootstrap_expires_at > ? RETURNING article_id").bind(hash, row.bootstrap_hash, new Date().toISOString()).first());
    if (!result) return yield* new Unauthorized();
    return yield* json({ token, expires_at: row.expires_at, article: yield* (yield* Articles).get(id) });
  })),
  HttpRouter.post("/view/articles/:id/content", Effect.gen(function* () {
    const { id } = yield* HttpRouter.schemaPathParams(Id);
    const row = yield* authenticate(id, "session_hash", yield* bearerToken);
    const service = yield* Articles;
    const article = yield* service.get(id);
    const input: unknown = yield* body;
    if (input === null || typeof input !== "object" || Array.isArray(input) || Object.keys(input).length !== 0) return yield* new BadRequest({ message: "content request must be an empty object" });
    const entry = article.assets.findIndex(asset => asset.path === "index.html");
    const { object } = yield* service.asset(id, entry);
    const source = yield* Effect.tryPromise({ try: async () => new TextDecoder("utf-8", { fatal: true }).decode(await readBoundedBody(object.body, HTML_BYTES)), catch: () => new BadRequest({ message: "article content unavailable" }) });
    const env = yield* Env;
    const parts = yield* Effect.try({ try: () => viewerParts(source, article.assets), catch: () => new BadRequest({ message: "article content unavailable" }) });
    yield* db(() => env.DB.prepare("UPDATE article_view_sessions SET delivered_version = ? WHERE bootstrap_hash = ?").bind(article.version, row.bootstrap_hash).run());
    return yield* json({ article, parts });
  })),
  HttpRouter.put("/view/articles/:id/read", Effect.gen(function* () {
    const { id } = yield* HttpRouter.schemaPathParams(Id);
    const row = yield* authenticate(id, "session_hash", yield* bearerToken);
    if (row.delivered_version === null) return yield* new Forbidden();
    const update = yield* Schema.decodeUnknown(ReadUpdate)(yield* body, { onExcessProperty: "error" });
    if (!update.read || update.version !== row.delivered_version) return yield* new Forbidden();
    return yield* json(yield* (yield* Articles).setRead(id, update));
  })),
  HttpRouter.get("/view/articles/:id/images/:index", Effect.gen(function* () {
    const { id, index } = yield* HttpRouter.schemaPathParams(AssetId);
    yield* authenticate(id, "session_hash", yield* bearerToken);
    const { asset, object } = yield* (yield* Articles).asset(id, index);
    if (asset.disposition !== "inline" || !asset.media_type.startsWith("image/")) return yield* new Forbidden();
    const bytes = yield* Effect.tryPromise({ try: () => readBoundedBody(object.body, asset.size), catch: () => new BadRequest({ message: "image unavailable" }) });
    const pixels = yield* Effect.try({ try: () => imagePixels(bytes, asset.media_type), catch: () => new BadRequest({ message: "image dimensions unavailable or exceed viewer limits" }) });
    return HttpServerResponse.uint8Array(bytes, { headers: { ...headers, "content-type": asset.media_type, "content-length": String(bytes.length), "content-security-policy": closedCsp, "x-lam-image-pixels": String(pixels) } });
  })),
  HttpRouter.get("/view/articles/:id/downloads/:index", Effect.gen(function* () {
    const { id, index } = yield* HttpRouter.schemaPathParams(AssetId);
    yield* authenticate(id, "session_hash", yield* bearerToken);
    const { asset, object } = yield* (yield* Articles).asset(id, index);
    if (asset.disposition !== "attachment") return yield* new Forbidden();
    return HttpServerResponse.raw(object.body, { headers: { ...headers, "content-type": "application/octet-stream", "content-length": String(object.size), "content-disposition": `attachment; filename="asset-${index}"; filename*=UTF-8''${encodeURIComponent(asset.path.split("/").at(-1)!).replace(/'/g, "%27")}`, "content-security-policy": closedCsp } });
  })),
  HttpRouter.use(Effect.provide(Articles.Default)),
  HttpRouter.catchTags({
    Unauthorized: () => json({ error: "unauthorized" }, 401),
    Forbidden: () => json({ error: "forbidden" }, 403),
    NotFound: () => json({ error: "not found" }, 404),
    Conflict: () => json({ error: "read state changed" }, 409),
    BadRequest: () => json({ error: "invalid viewer request" }, 400),
    ParseError: () => json({ error: "invalid viewer request" }, 400),
  }),
  HttpRouter.catchAll(() => json({ error: "viewer request failed" }, 500)),
);
