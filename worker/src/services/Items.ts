import { Effect, Schema } from "effect";
import { Env } from "../Env";
import { AlreadyClosed, DbError, Item, ItemRow, NotFound, type NewItem, type ResponseBy, type Status } from "../domain/Item";

const ALPHABET = "abcdefghjkmnpqrstuvwxyz23456789";
const newId = () => Array.from(crypto.getRandomValues(new Uint8Array(5)), (b) => ALPHABET[b % ALPHABET.length]).join("");

const db = <A>(run: (db: D1Database) => Promise<A>) =>
  Effect.flatMap(Env, ({ DB }) => Effect.tryPromise({ try: () => run(DB), catch: (cause) => new DbError({ cause }) }));

const decodeRow = (row: unknown) => Schema.decodeUnknown(ItemRow)(row).pipe(Effect.map((r) => new Item(r)), Effect.orDie);

export interface Closing {
  status: "resolved" | "dismissed";
  choice?: string;
  text?: string;
  by: ResponseBy;
}

export class Items extends Effect.Service<Items>()("lam/Items", {
  succeed: {
    create: (input: NewItem) =>
      Effect.gen(function* () {
        const item = new Item({
          ...input,
          id: newId(),
          status: "open",
          response_choice: null,
          response_text: null,
          response_by: null,
          created_at: new Date().toISOString(),
          resolved_at: null,
        });
        yield* db((d) =>
          d
            .prepare(
              `INSERT INTO items (id, title, body, source_host, source_project, priority, choices, status, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, 'open', ?)`,
            )
            .bind(item.id, item.title, item.body, item.source_host, item.source_project, item.priority, JSON.stringify(item.choices), item.created_at)
            .run(),
        );
        return item;
      }),

    get: (id: string) =>
      db((d) => d.prepare("SELECT * FROM items WHERE id = ?").bind(id).first()).pipe(
        Effect.flatMap((row) => (row ? decodeRow(row) : Effect.fail(new NotFound({ id })))),
      ),

    list: (status?: Status) =>
      db((d) =>
        (status
          ? d.prepare("SELECT * FROM items WHERE status = ? ORDER BY created_at DESC").bind(status)
          : d.prepare("SELECT * FROM items ORDER BY created_at DESC")
        ).all(),
      ).pipe(Effect.flatMap(({ results }) => Effect.forEach(results, decodeRow))),

    /** Transitions an open item; NotFound if missing, AlreadyClosed if it was not open. */
    close: (id: string, c: Closing) =>
      Effect.gen(function* () {
        const result = yield* db((d) =>
          d
            .prepare(
              `UPDATE items SET status = ?, response_choice = ?, response_text = ?, response_by = ?, resolved_at = ?
               WHERE id = ? AND status = 'open'`,
            )
            .bind(c.status, c.choice ?? null, c.text ?? null, c.by, new Date().toISOString(), id)
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
