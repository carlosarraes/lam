import { Effect, Option, Schema } from "effect";
import { Env } from "../Env";
import { AlreadyClosed, BadRequest, Conflict, DbError, Item, ItemRow, MAX_CHECKS, NotFound, type ItemKind, type NewItem, type ResponseBy, type Status } from "../domain/Item";

const ALPHABET = "abcdefghjkmnpqrstuvwxyz23456789";
const newId = () => Array.from(crypto.getRandomValues(new Uint8Array(5)), (b) => ALPHABET[b % ALPHABET.length]).join("");

const db = <A>(run: (db: D1Database) => Promise<A>) =>
  Effect.flatMap(Env, ({ DB }) => Effect.tryPromise({ try: () => run(DB), catch: (cause) => new DbError({ cause }) }));

const toItem = (row: typeof ItemRow.Type) =>
  new Item({
    id: row.id,
    kind: row.kind,
    name: row.name,
    title: row.title,
    body: row.body,
    source_host: row.source_host,
    source_project: row.source_project,
    priority: row.priority,
    choices: row.choices,
    checks: row.checks,
    recommendation: row.recommendation,
    recommended_choice: row.recommended_choice,
    link: row.link,
    status: row.status,
    response_choice: row.response_choice,
    response_text: row.response_text,
    response_by: row.response_by,
    created_at: row.created_at,
    resolved_at: row.resolved_at,
    seen_at: row.seen_at,
    expires_at: row.expires_at,
    version: row.version,
  });

const decodeRow = (row: unknown) =>
  Schema.decodeUnknown(ItemRow)(row).pipe(
    Effect.map((r) => Item.withExpiry(toItem(r), new Date())),
    Effect.orDie,
  );

/** Stable request identity; optional values use exactly the defaults persisted by `NewItem`. */
export const canonicalDedupeInput = (input: NewItem, kind: ItemKind = "request") =>
  JSON.stringify({
    ...(kind === "fyi" ? { kind } : {}),
    name: input.name ?? "",
    title: input.title,
    body: input.body ?? "",
    priority: input.priority ?? "normal",
    choices: input.choices ?? [],
    checks: input.checks ?? [],
    link: input.link ?? "",
    ttl: input.ttl ?? null,
    recommendation: input.recommendation ?? null,
    recommended_choice: input.recommended_choice ?? null,
  });

const dedupeKey = async (input: NewItem, kind: ItemKind) => {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(canonicalDedupeInput(input, kind))));
  return Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
};

export interface ListQuery {
  status?: Status;
  ids?: readonly string[];
  /** Page size. Absent means every row. */
  limit?: number;
  /** Exclusive `created_at` cursor: the page holds rows strictly older than this. */
  before?: string;
}

export interface DuplicateLookup {
  kind?: ItemKind;
  /** Compatibility-only name/title/body matching for rows created before dedupe keys existed. */
  legacyFallback?: boolean;
}

export type HistoryType = "plain" | "choice" | "checklist";

export interface HistoryQuery {
  limit: number;
  cursor?: string;
  q?: string;
  priority?: Item["priority"];
  type?: HistoryType;
}

export interface HistoryPage {
  items: Item[];
  next_cursor: string | null;
}

interface HistoryCursor {
  closed_at: string;
  id: string;
}

const encodeBase64Url = (bytes: Uint8Array) =>
  btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

const encodeCursor = (cursor: HistoryCursor) => {
  const bytes = new TextEncoder().encode(JSON.stringify(cursor));
  return encodeBase64Url(bytes);
};

const isValidIsoTimestamp = (value: string) => {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d{1,9})?(?:Z|[+-](\d{2}):(\d{2}))$/.exec(value);
  if (match === null) return false;
  const [, yearText, monthText, dayText, hourText, minuteText, secondText, offsetHourText, offsetMinuteText] = match;
  const [year, month, day, hour, minute, second] = [yearText, monthText, dayText, hourText, minuteText, secondText].map(Number);
  const leapYear = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const daysInMonth = [31, leapYear ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  if (month < 1 || month > 12 || day < 1 || day > daysInMonth[month - 1]!) return false;
  if (hour > 23 || minute > 59 || second > 59) return false;
  if (offsetHourText !== undefined && (Number(offsetHourText) > 23 || Number(offsetMinuteText) > 59)) return false;
  return !Number.isNaN(Date.parse(value));
};

const decodeCursor = (cursor: string) =>
  Effect.try({
    try: () => {
      if (!/^[A-Za-z0-9_-]+$/.test(cursor) || cursor.length % 4 === 1) {
        throw new Error("invalid history cursor");
      }
      const base64 = cursor.replace(/-/g, "+").replace(/_/g, "/");
      const padded = base64.padEnd(Math.ceil(base64.length / 4) * 4, "=");
      const bytes = Uint8Array.from(atob(padded), (character) => character.charCodeAt(0));
      if (encodeBase64Url(bytes) !== cursor) throw new Error("invalid history cursor");
      const value: unknown = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
      if (
        value === null
        || typeof value !== "object"
        || Array.isArray(value)
        || Object.keys(value).length !== 2
        || typeof (value as Record<string, unknown>).closed_at !== "string"
        || typeof (value as Record<string, unknown>).id !== "string"
        || (value as Record<string, string>).id.length === 0
      ) throw new Error("invalid history cursor");
      const closedAt = (value as Record<string, string>).closed_at;
      if (!isValidIsoTimestamp(closedAt)) throw new Error("invalid history cursor");
      return value as HistoryCursor;
    },
    catch: () => new BadRequest({ message: "invalid history cursor" }),
  });

const escapeLike = (value: string) => value.replace(/[\\%_]/g, "\\$&");

interface ChecksUpdate {
  checks: Item["checks"];
  resolve: boolean;
  by: ResponseBy;
}

/** Optimistic read-modify-write on `checks`, keyed on `version`; retries a few times on a concurrent write. */
const mutateChecks = (id: string, f: (item: Item) => Effect.Effect<ChecksUpdate, BadRequest>) =>
  Effect.gen(function* () {
    for (let attempt = 0; attempt < 3; attempt++) {
      const item = yield* Items.get(id);
      if (item.kind === "fyi") return yield* new BadRequest({ message: "FYIs do not have checks" });
      if (item.status !== "open") return yield* new AlreadyClosed({ id });
      const u = yield* f(item);
      const now = new Date().toISOString();
      const result = yield* db((d) =>
        (u.resolve
          ? d
              .prepare(`UPDATE items SET checks = ?, version = version + 1, status = 'resolved', response_by = ?, resolved_at = ? WHERE id = ? AND version = ?`)
              .bind(JSON.stringify(u.checks), u.by, now, id, item.version)
          : d.prepare(`UPDATE items SET checks = ?, version = version + 1 WHERE id = ? AND version = ?`).bind(JSON.stringify(u.checks), id, item.version)
        ).run(),
      );
      if (result.meta.changes) return yield* Items.get(id);
    }
    return yield* new Conflict({ id });
  });

export interface Closing {
  status: "resolved" | "dismissed" | "retracted";
  choice?: string;
  text?: string;
  by: ResponseBy;
}

export class Items extends Effect.Service<Items>()("lam/Items", {
  succeed: {
    create: (input: NewItem, kind: ItemKind = "request") =>
      Effect.gen(function* () {
        const { ttl, link = "", checks, recommendation = null, recommended_choice = null, ...fields } = input;
        const now = Date.now();
        const created_at = new Date(now).toISOString();
        const expires_at = ttl === undefined ? null : new Date(now + ttl * 1000).toISOString();
        const key = yield* Effect.promise(() => dedupeKey(input, kind));
        const item = new Item({
          ...fields,
          kind,
          link,
          checks: checks.map((label) => ({ label, done: false, at: null })),
          recommendation,
          recommended_choice,
          version: 0,
          id: newId(),
          status: "open",
          response_choice: null,
          response_text: null,
          response_by: null,
          created_at,
          resolved_at: null,
          seen_at: null,
          expires_at,
        });
        yield* db((d) =>
          d
            .prepare(
              `INSERT INTO items (id, kind, name, title, body, source_host, source_project, priority, choices, checks, recommendation, recommended_choice, link, status, created_at, expires_at, dedupe_key)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'open', ?, ?, ?)`,
            )
            .bind(item.id, item.kind, item.name, item.title, item.body, item.source_host, item.source_project, item.priority, JSON.stringify(item.choices), JSON.stringify(item.checks), item.recommendation, item.recommended_choice, item.link, item.created_at, item.expires_at, key)
            .run(),
        );
        return item;
      }),

    /** An open, unexpired item with identical content — a retry of a push whose response was lost. */
    findDuplicate: (input: NewItem, { kind = "request", legacyFallback = true }: DuplicateLookup = {}) =>
      Effect.gen(function* () {
        const now = new Date().toISOString();
        const key = yield* Effect.promise(() => dedupeKey(input, kind));
        const keyed = yield* db((d) =>
          d
            .prepare(
              `SELECT * FROM items WHERE status = 'open' AND dedupe_key = ?
               AND (expires_at IS NULL OR expires_at > ?) ORDER BY created_at DESC LIMIT 1`,
            )
            .bind(key, now)
            .first(),
        );
        if (keyed) return Option.some(yield* decodeRow(keyed));

        if (kind === "fyi" || !legacyFallback) return Option.none<Item>();

        const legacy = yield* db((d) =>
          d
            .prepare(
              `SELECT * FROM items WHERE status = 'open' AND dedupe_key IS NULL AND name = ? AND title = ? AND body = ?
               AND (expires_at IS NULL OR expires_at > ?) ORDER BY created_at DESC LIMIT 1`,
            )
            .bind(input.name, input.title, input.body, now)
            .first(),
        );
        return legacy ? Option.some(yield* decodeRow(legacy)) : Option.none<Item>();
      }),

    get: (id: string) =>
      db((d) => d.prepare("SELECT * FROM items WHERE id = ?").bind(id).first()).pipe(
        Effect.flatMap((row) => (row ? decodeRow(row) : Effect.fail(new NotFound({ id })))),
      ),

    /**
     * `status` filters on the *derived* status, so `open` excludes expired items — which is why it
     * has to happen in JS, after `decodeRow` applies `Item.withExpiry`. Do not move it into SQL:
     * `expired` is never stored, so `WHERE status = 'open'` would hand back expired rows.
     *
     * `limit`/`before` bound the rows *read* and are safe in SQL because `created_at` is stored.
     * They compose with `status` as "filter this page", not as a search: `?status=open&limit=10`
     * means "the ten newest rows, of which the open ones". The CLI never combines them.
     *
     * Absent `limit` means unlimited, which is what every binary already in the field expects.
     */
    list: ({ status, ids, limit, before }: ListQuery = {}) =>
      db((d) => {
        const where: string[] = [];
        const binds: unknown[] = [];
        if (ids) {
          where.push(`id IN (${ids.map(() => "?").join(",")})`);
          binds.push(...ids);
        }
        // Exclusive: `created_at` is millisecond-resolution with no monotonic tiebreak, so `<=`
        // could hand back the cursor row forever.
        if (before !== undefined) {
          where.push("created_at < ?");
          binds.push(before);
        }
        if (limit !== undefined) binds.push(limit);
        const stmt = d.prepare(
          `SELECT * FROM items${where.length ? ` WHERE ${where.join(" AND ")}` : ""} ORDER BY created_at DESC${limit === undefined ? "" : " LIMIT ?"}`,
        );
        return (binds.length ? stmt.bind(...binds) : stmt).all();
      }).pipe(
        Effect.flatMap(({ results }) => Effect.forEach(results, decodeRow)),
        Effect.map((items) => (status ? items.filter((i) => i.status === status) : items)),
      ),

    history: ({ limit, cursor, q, priority, type }: HistoryQuery): Effect.Effect<HistoryPage, BadRequest | DbError, Env> =>
      Effect.gen(function* () {
        const decodedCursor = cursor === undefined ? undefined : yield* decodeCursor(cursor);
        const now = new Date().toISOString();
        const { results } = yield* db((d) => {
          const where = [
            "(status IN ('resolved', 'dismissed', 'retracted') OR (status = 'open' AND expires_at IS NOT NULL AND expires_at <= ?))",
          ];
          const binds: unknown[] = [now];
          if (q !== undefined) {
            where.push(
              `(title LIKE ? ESCAPE '\\' COLLATE NOCASE
                OR name LIKE ? ESCAPE '\\' COLLATE NOCASE
                OR body LIKE ? ESCAPE '\\' COLLATE NOCASE
                OR (source_host || ':' || source_project) LIKE ? ESCAPE '\\' COLLATE NOCASE)`,
            );
            const pattern = `%${escapeLike(q)}%`;
            binds.push(pattern, pattern, pattern, pattern);
          }
          if (priority !== undefined) {
            where.push("priority = ?");
            binds.push(priority);
          }
          if (type === "checklist") where.push("json_array_length(checks) > 0");
          if (type === "choice") where.push("json_array_length(checks) = 0 AND json_array_length(choices) > 0");
          if (type === "plain") where.push("json_array_length(checks) = 0 AND json_array_length(choices) = 0");
          if (decodedCursor !== undefined) {
            where.push(
              "(COALESCE(resolved_at, expires_at) < ? OR (COALESCE(resolved_at, expires_at) = ? AND id < ?))",
            );
            binds.push(decodedCursor.closed_at, decodedCursor.closed_at, decodedCursor.id);
          }
          binds.push(limit + 1);
          return d
            .prepare(
              `SELECT *, COALESCE(resolved_at, expires_at) AS closed_at
               FROM items
               WHERE ${where.join(" AND ")}
               ORDER BY closed_at DESC, id DESC
               LIMIT ?`,
            )
            .bind(...binds)
            .all();
        });
        const rows = results.slice(0, limit);
        const items = yield* Effect.forEach(rows, decodeRow);
        const last = rows.at(-1) as { closed_at?: unknown; id?: unknown } | undefined;
        const next_cursor = results.length > limit && typeof last?.closed_at === "string" && typeof last.id === "string"
          ? encodeCursor({ closed_at: last.closed_at, id: last.id })
          : null;
        return { items, next_cursor };
      }),

    /** Flips one check; resolves the item when every check is done. */
    setCheck: (id: string, index: number, done: boolean, by: ResponseBy) =>
      mutateChecks(id, (item) => {
        if (index < 0 || index >= item.checks.length) return Effect.fail(new BadRequest({ message: `no check #${index}` }));
        const checks = item.checks.map((c, i) => (i === index ? { ...c, done, at: done ? new Date().toISOString() : null } : c));
        return Effect.succeed({ checks, resolve: checks.every((c) => c.done), by });
      }),

    /** Appends a check to an open item (agent side). */
    addCheck: (id: string, label: string) =>
      mutateChecks(id, (item) =>
        item.checks.length >= MAX_CHECKS
          ? Effect.fail(new BadRequest({ message: `at most ${MAX_CHECKS} checks` }))
          : Effect.succeed({ checks: [...item.checks, { label, done: false, at: null }], resolve: false, by: "cli" as const }),
      ),

    /** Transitions an open, unexpired item; NotFound if missing, AlreadyClosed otherwise. */
    close: (id: string, c: Closing) =>
      Effect.gen(function* () {
        const current = yield* Items.get(id);
        if (current.kind === "fyi" && c.status === "resolved") {
          return yield* new BadRequest({ message: "FYIs cannot be answered" });
        }
        const now = new Date().toISOString();
        const result = yield* db((d) =>
          d
            .prepare(
              `UPDATE items SET status = ?, response_choice = ?, response_text = ?, response_by = ?, resolved_at = ?, version = version + 1
               WHERE id = ? AND status = 'open' AND (expires_at IS NULL OR expires_at > ?)`,
            )
            .bind(c.status, c.choice ?? null, c.text ?? null, c.by, now, id, now)
            .run(),
        );
        const item = yield* Items.get(id);
        if (!result.meta.changes) return yield* new AlreadyClosed({ id });
        return item;
      }),

    /** Marks an FYI seen once. Repeating the same intent returns its canonical closed state. */
    seen: (id: string, version: number) =>
      Effect.gen(function* () {
        const current = yield* Items.get(id);
        if (current.kind !== "fyi") return yield* new BadRequest({ message: "only FYIs can be marked seen" });
        if (current.seen_at !== null) return { item: current, changed: false };
        if (current.status !== "open") return yield* new AlreadyClosed({ id });
        if (current.version !== version) return yield* new Conflict({ id });

        const now = new Date().toISOString();
        const result = yield* db((d) =>
          d
            .prepare(
              `UPDATE items
               SET status = 'dismissed', seen_at = ?, resolved_at = ?, response_choice = NULL,
                   response_text = NULL, response_by = NULL, version = version + 1
               WHERE id = ? AND kind = 'fyi' AND status = 'open' AND seen_at IS NULL AND version = ?
                 AND (expires_at IS NULL OR expires_at > ?)`,
            )
            .bind(now, now, id, version, now)
            .run(),
        );
        const updated = yield* Items.get(id);
        if (result.meta.changes) return { item: updated, changed: true };
        if (updated.seen_at !== null) return { item: updated, changed: false };
        return yield* new Conflict({ id });
      }),
  },
}) {
  static get = (id: string) => Effect.flatMap(Items, (s) => s.get(id));
}
