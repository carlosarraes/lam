import { Effect, Schema } from "effect";
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
              `INSERT INTO items (id, title, body, source_host, source_project, priority, choices, checks, link, status, created_at, expires_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'open', ?, ?)`,
            )
            .bind(item.id, item.title, item.body, item.source_host, item.source_project, item.priority, JSON.stringify(item.choices), JSON.stringify(item.checks), item.link, item.created_at, item.expires_at)
            .run(),
        );
        return item;
      }),

    get: (id: string) =>
      db((d) => d.prepare("SELECT * FROM items WHERE id = ?").bind(id).first()).pipe(
        Effect.flatMap((row) => (row ? decodeRow(row) : Effect.fail(new NotFound({ id })))),
      ),

    /** `status` filters on the derived status, so `open` excludes expired items. */
    list: (status?: Status, ids?: readonly string[]) =>
      db((d) =>
        (ids
          ? d.prepare(`SELECT * FROM items WHERE id IN (${ids.map(() => "?").join(",")}) ORDER BY created_at DESC`).bind(...ids)
          : d.prepare("SELECT * FROM items ORDER BY created_at DESC")
        ).all(),
      ).pipe(
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
