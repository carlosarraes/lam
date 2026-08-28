import { Effect, Option, Schema } from "effect";
import { Env } from "../Env";
import { AlreadyClosed, BadRequest, Conflict, DbError, Item, ItemRow, NotFound, type NewItem, type ResponseBy, type Status } from "../domain/Item";

const ALPHABET = "abcdefghjkmnpqrstuvwxyz23456789";
const newId = () => Array.from(crypto.getRandomValues(new Uint8Array(5)), (b) => ALPHABET[b % ALPHABET.length]).join("");

const db = <A>(run: (db: D1Database) => Promise<A>) =>
  Effect.flatMap(Env, ({ DB }) => Effect.tryPromise({ try: () => run(DB), catch: (cause) => new DbError({ cause }) }));

const decodeRow = (row: unknown) =>
  Schema.decodeUnknown(ItemRow)(row).pipe(
    Effect.map((r) => Item.withExpiry(new Item(r), new Date())),
    Effect.orDie,
  );

export interface ListQuery {
  status?: Status;
  ids?: readonly string[];
  /** Page size. Absent means every row. */
  limit?: number;
  /** Exclusive `created_at` cursor: the page holds rows strictly older than this. */
  before?: string;
}

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
    create: (input: NewItem) =>
      Effect.gen(function* () {
        const { ttl, link = "", checks, ...fields } = input;
        const now = Date.now();
        const item = new Item({
          ...fields,
          link,
          checks: checks.map((label) => ({ label, done: false, at: null })),
          version: 0,
          id: newId(),
          status: "open",
          response_choice: null,
          response_text: null,
          response_by: null,
          created_at: new Date(now).toISOString(),
          resolved_at: null,
          expires_at: ttl === undefined ? null : new Date(now + ttl * 1000).toISOString(),
        });
        yield* db((d) =>
          d
            .prepare(
              `INSERT INTO items (id, name, title, body, source_host, source_project, priority, choices, checks, link, status, created_at, expires_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'open', ?, ?)`,
            )
            .bind(item.id, item.name, item.title, item.body, item.source_host, item.source_project, item.priority, JSON.stringify(item.choices), JSON.stringify(item.checks), item.link, item.created_at, item.expires_at)
            .run(),
        );
        return item;
      }),

    /** An open, unexpired item with identical content — a retry of a push whose response was lost. */
    findDuplicate: (input: NewItem) =>
      db((d) =>
        d
          .prepare(
            `SELECT * FROM items WHERE status = 'open' AND name = ? AND title = ? AND body = ?
             AND (expires_at IS NULL OR expires_at > ?) ORDER BY created_at DESC LIMIT 1`,
          )
          .bind(input.name, input.title, input.body, new Date().toISOString())
          .first(),
      ).pipe(Effect.flatMap((row) => (row ? Effect.map(decodeRow(row), Option.some) : Effect.succeed(Option.none<Item>())))),

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

    /** Flips one check; resolves the item when every check is done. */
    setCheck: (id: string, index: number, done: boolean, by: ResponseBy) =>
      mutateChecks(id, (item) => {
        if (index < 0 || index >= item.checks.length) return Effect.fail(new BadRequest({ message: `no check #${index}` }));
        const checks = item.checks.map((c, i) => (i === index ? { ...c, done, at: done ? new Date().toISOString() : null } : c));
        return Effect.succeed({ checks, resolve: checks.every((c) => c.done), by });
      }),

    /** Appends a check to an open item (agent side). */
    addCheck: (id: string, label: string) =>
      mutateChecks(id, (item) => Effect.succeed({ checks: [...item.checks, { label, done: false, at: null }], resolve: false, by: "cli" as const })),

    /** Transitions an open, unexpired item; NotFound if missing, AlreadyClosed otherwise. */
    close: (id: string, c: Closing) =>
      Effect.gen(function* () {
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
  },
}) {
  static get = (id: string) => Effect.flatMap(Items, (s) => s.get(id));
}
