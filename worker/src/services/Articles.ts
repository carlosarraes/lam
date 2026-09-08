import { Effect, Schema } from "effect";
import { Env, type Bindings } from "../Env";
import { ArticleStorageError, ReadUpdate, type Article, type ArticleDraft, type ArticlePage, type Asset } from "../domain/Article";
import { BadRequest, Conflict, DbError, NotFound } from "../domain/Item";
import { readBoundedBody, validateManifest } from "../articles/manifest";

// LAM has one owner. Master credentials and active paired devices belong to this owner.
const OWNER = "owner";
const DAY = 24 * 60 * 60 * 1000;
const PAGE_SIZE = 25;
interface ArticleRow extends Omit<Article, "assets"> {
  owner_id: string;
  state: "staging" | "published" | "abandoned";
  silent: number;
  manifest_hash: string;
  expires_at: string;
  sanitized_html_key: string;
}
interface AssetRow extends Asset { article_id: string; asset_index: number; object_key: string; complete: number }
export interface ArticleQuery { q?: string; read?: "all" | "read" | "unread"; cursor?: string }
const operation = <A>(run: (env: Bindings) => Promise<A>) => Effect.flatMap(Env, env => Effect.tryPromise({
  try: () => run(env),
  catch: cause => cause instanceof BadRequest || cause instanceof Conflict || cause instanceof NotFound || cause instanceof ArticleStorageError
    ? cause : new DbError({ cause }),
}));
const storage = async <A>(run: () => Promise<A>): Promise<A> => {
  try { return await run(); } catch (cause) { throw new ArticleStorageError({ cause }); }
};
const sha256 = async (bytes: Uint8Array<ArrayBuffer>) => Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)), b => b.toString(16).padStart(2, "0")).join("");
const assetsFor = async (db: D1Database, id: string) => (await db.prepare("SELECT * FROM article_assets WHERE article_id = ? ORDER BY asset_index LIMIT 51").bind(id).all<AssetRow>()).results;
async function rowFor(db: D1Database, id: string, published = false): Promise<ArticleRow> {
  const row = await db.prepare(`SELECT * FROM articles WHERE id = ? AND owner_id = ?${published ? " AND state = 'published'" : ""}`).bind(id, OWNER).first<ArticleRow>();
  if (!row) throw new NotFound({ id });
  return row;
}
const requireStaging = (row: ArticleRow) => {
  if (row.state !== "staging" || row.expires_at <= new Date().toISOString()) throw new Conflict({ id: row.id });
};
function publicArticle(row: ArticleRow, assets: readonly AssetRow[]): Article {
  return { id: row.id, title: row.title, summary: row.summary, name: row.name, source_host: row.source_host, source_project: row.source_project,
    created_at: row.created_at, read_at: row.read_at, version: row.version,
    assets: assets.map(({ path, media_type, size, sha256, disposition }) => ({ path, media_type, size, sha256, disposition })) };
}
const getArticle = async (db: D1Database, id: string) => publicArticle(await rowFor(db, id, true), await assetsFor(db, id));

const Cursor = Schema.Struct({ created_at: Schema.String, id: Schema.String, q: Schema.String, read: Schema.Literal("all", "read", "unread") });
function decodeCursor(cursor: string, q: string, read: string): typeof Cursor.Type {
  try {
    if (cursor.length > 8192 || !/^[A-Za-z0-9_-]+$/.test(cursor)) throw new Error();
    const decoded = Schema.decodeUnknownSync(Cursor)(JSON.parse(decodeURIComponent(atob(cursor.replace(/-/g, "+").replace(/_/g, "/")))), { onExcessProperty: "error" });
    if (decoded.q !== q || decoded.read !== read || !/^[a-f0-9-]{36}$/.test(decoded.id) || new Date(decoded.created_at).toISOString() !== decoded.created_at) throw new Error();
    return decoded;
  } catch { throw new BadRequest({ message: "invalid article cursor" }); }
}
const encodeCursor = (cursor: typeof Cursor.Type) => btoa(encodeURIComponent(JSON.stringify(cursor))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

export class Articles extends Effect.Service<Articles>()("lam/Articles", {
  succeed: {
    stage: (input: ArticleDraft, key: string | undefined) => operation(async ({ DB }) => {
      if (!key || !/^[\x21-\x7e]{1,128}$/.test(key)) throw new BadRequest({ message: "Idempotency-Key is required and must be 1-128 printable ASCII characters" });
      const draft = validateManifest(input);
      const hash = await sha256(new TextEncoder().encode(JSON.stringify(draft)));
      const id = crypto.randomUUID();
      const now = new Date().toISOString();
      const expires = new Date(Date.now() + DAY).toISOString();
      // The batch is transactional. Only the winning idempotency insert can insert assets.
      await DB.batch([
        DB.prepare(`INSERT INTO articles (id, owner_id, idempotency_key, manifest_hash, title, summary, name, source_host, source_project, silent, created_at, expires_at, sanitized_html_key, cleanup_after)
          VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(owner_id, idempotency_key) DO NOTHING`)
          .bind(id, OWNER, key, hash, draft.title, draft.summary, draft.name, draft.source_host, draft.source_project, draft.silent ? 1 : 0, now, expires, `articles/${id}/sanitized/index.html`, expires),
        ...draft.assets.map((asset, index) => DB.prepare(`INSERT INTO article_assets (article_id, asset_index, path, media_type, size, sha256, disposition, object_key)
          SELECT id, ?, ?, ?, ?, ?, ?, ? FROM articles WHERE id = ? AND owner_id = ?`)
          .bind(index, asset.path, asset.media_type, asset.size, asset.sha256, asset.disposition, `articles/${id}/original/${index}`, id, OWNER)),
      ]);
      const row = await DB.prepare("SELECT * FROM articles WHERE owner_id = ? AND idempotency_key = ?").bind(OWNER, key).first<ArticleRow>();
      if (!row || row.manifest_hash !== hash) throw new Conflict({ id: row?.id ?? id });
      if (row.state !== "published") requireStaging(row);
      return { id: row.id };
    }),

    upload: (id: string, index: number, body: ReadableStream<Uint8Array> | null) => operation(async ({ DB, ARTICLE_BUCKET }) => {
      requireStaging(await rowFor(DB, id));
      const asset = await DB.prepare("SELECT * FROM article_assets WHERE article_id = ? AND asset_index = ?").bind(id, index).first<AssetRow>();
      if (!asset) throw new NotFound({ id });
      const bytes = await readBoundedBody(body, asset.size);
      if (bytes.byteLength !== asset.size || await sha256(bytes) !== asset.sha256) throw new BadRequest({ message: "asset size or SHA-256 does not match manifest" });
      requireStaging(await rowFor(DB, id));
      // All writers have verified the same immutable bytes; conditional put avoids overwriting on retries.
      await storage(() => ARTICLE_BUCKET.put(asset.object_key, bytes, {
        onlyIf: { etagDoesNotMatch: "*" }, sha256: asset.sha256,
        httpMetadata: { contentType: asset.media_type }, customMetadata: { sha256: asset.sha256 },
      }));
      const updated = await DB.prepare(`UPDATE article_assets SET complete = 1 WHERE article_id = ? AND asset_index = ?
        AND EXISTS (SELECT 1 FROM articles WHERE id = ? AND owner_id = ? AND state = 'staging' AND expires_at > ?)`)
        .bind(id, index, id, OWNER, new Date().toISOString()).run();
      if (updated.meta.changes === 0) {
        const current = await rowFor(DB, id);
        // Never delete a published object if publication won the race.
        if (current.state !== "published") await storage(() => ARTICLE_BUCKET.delete(asset.object_key));
        throw new Conflict({ id });
      }
    }),

    publish: (id: string) => operation(async ({ DB, ARTICLE_BUCKET }) => {
      const row = await rowFor(DB, id);
      if (row.state === "published") return getArticle(DB, id);
      requireStaging(row);
      const assets = await assetsFor(DB, id);
      if (assets.length === 0 || assets.some(asset => !asset.complete)) throw new BadRequest({ message: "article uploads are incomplete" });
      for (const asset of assets) {
        const object = await storage(() => ARTICLE_BUCKET.head(asset.object_key));
        if (!object || object.size !== asset.size || object.customMetadata?.sha256 !== asset.sha256)
          throw new BadRequest({ message: "article upload is missing or invalid" });
      }
      // Task 2 replaces this terminal failure with parsed validation and atomic publication.
      throw new BadRequest({ message: "article content validation is not available" });
    }),

    get: (id: string) => operation(({ DB }) => getArticle(DB, id)),

    list: (query: ArticleQuery) => operation(async ({ DB }): Promise<ArticlePage> => {
      const q = query.q ?? "";
      const read = query.read ?? "all";
      if ([...q].length > 200 || !["all", "read", "unread"].includes(read)) throw new BadRequest({ message: "invalid article query" });
      const cursor = query.cursor === undefined ? null : decodeCursor(query.cursor, q, read);
      const conditions = ["owner_id = ?", "state = 'published'"];
      const values: (string | number)[] = [OWNER];
      if (read !== "all") conditions.push(`read_at IS ${read === "read" ? "NOT " : ""}NULL`);
      if (q) {
        conditions.push("(instr(lower(title), lower(?)) > 0 OR instr(lower(summary), lower(?)) > 0 OR instr(lower(name), lower(?)) > 0)");
        values.push(q, q, q);
      }
      if (cursor) { conditions.push("(created_at < ? OR (created_at = ? AND id < ?))"); values.push(cursor.created_at, cursor.created_at, cursor.id); }
      const rows = (await DB.prepare(`SELECT * FROM articles WHERE ${conditions.join(" AND ")} ORDER BY created_at DESC, id DESC LIMIT ?`).bind(...values, PAGE_SIZE + 1).all<ArticleRow>()).results;
      const page = rows.slice(0, PAGE_SIZE);
      const last = page.at(-1);
      return { items: await Promise.all(page.map(async row => publicArticle(row, await assetsFor(DB, row.id)))),
        next_cursor: rows.length > PAGE_SIZE && last ? encodeCursor({ created_at: last.created_at, id: last.id, q, read }) : null };
    }),

    asset: (id: string, index: number) => operation(async ({ DB, ARTICLE_BUCKET }) => {
      const article = await rowFor(DB, id, true);
      const asset = await DB.prepare("SELECT * FROM article_assets WHERE article_id = ? AND asset_index = ? AND complete = 1").bind(id, index).first<AssetRow>();
      if (!asset) throw new NotFound({ id });
      const object = await storage(() => ARTICLE_BUCKET.get(asset.path === "index.html" ? article.sanitized_html_key : asset.object_key));
      if (!object) throw new NotFound({ id });
      return { asset, object };
    }),

    setRead: (id: string, input: ReadUpdate) => operation(async ({ DB }) => {
      const update = Schema.decodeUnknownSync(ReadUpdate)(input, { onExcessProperty: "error" });
      await rowFor(DB, id, true);
      const row = await DB.prepare(`UPDATE articles SET read_at = ?, version = version + 1
        WHERE id = ? AND owner_id = ? AND state = 'published' AND version = ? AND (read_at IS NOT NULL) != ? RETURNING *`)
        .bind(update.read ? new Date().toISOString() : null, id, OWNER, update.version, update.read ? 1 : 0).first<ArticleRow>();
      const canonical = row ?? await rowFor(DB, id, true);
      if ((canonical.read_at !== null) !== update.read) throw new Conflict({ id });
      return publicArticle(canonical, await assetsFor(DB, id));
    }),

    /** Task 6 schedules this. Tombstones remain retryable to catch interrupted, late R2 writes. */
    cleanupAbandoned: (now = new Date(), limit = 25) => operation(async ({ DB, ARTICLE_BUCKET }) => {
      if (!Number.isFinite(now.getTime()) || !Number.isInteger(limit) || limit < 1 || limit > 100)
        throw new BadRequest({ message: "invalid article cleanup bounds" });
      const token = crypto.randomUUID();
      const timestamp = now.toISOString();
      const lease = new Date(now.getTime() + 5 * 60 * 1000).toISOString();
      // One SQL statement both abandons expired drafts and exclusively claims their cleanup.
      const claimed = (await DB.prepare(`UPDATE articles SET state = 'abandoned', cleanup_token = ?, cleanup_after = ?
        WHERE id IN (SELECT id FROM articles WHERE owner_id = ? AND
          ((state = 'staging' AND expires_at <= ?) OR (state = 'abandoned' AND cleanup_after <= ?))
          ORDER BY cleanup_after, id LIMIT ?) RETURNING *`)
        .bind(token, lease, OWNER, timestamp, timestamp, limit).all<ArticleRow>()).results;
      const cleaned: string[] = [];
      for (const row of claimed) {
        const assets = await assetsFor(DB, row.id);
        await storage(() => ARTICLE_BUCKET.delete([...assets.map(asset => asset.object_key), row.sanitized_html_key]));
        await DB.prepare("UPDATE articles SET cleanup_token = NULL, cleanup_after = ? WHERE id = ? AND state = 'abandoned' AND cleanup_token = ?")
          .bind(new Date(now.getTime() + DAY).toISOString(), row.id, token).run();
        cleaned.push(row.id);
      }
      return cleaned;
    }),
  },
}) {}
