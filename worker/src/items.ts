import type { Item, NewItem, ResponseBy, Status } from "./types";

type Row = Omit<Item, "choices"> & { choices: string };

const ALPHABET = "abcdefghjkmnpqrstuvwxyz23456789";

export function newId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(5));
  return Array.from(bytes, (b) => ALPHABET[b % ALPHABET.length]).join("");
}

function fromRow(row: Row): Item {
  return { ...row, choices: JSON.parse(row.choices) };
}

export async function create(db: D1Database, input: NewItem): Promise<Item> {
  const item: Item = {
    id: newId(),
    title: input.title,
    body: input.body ?? "",
    source_host: input.source_host ?? "",
    source_project: input.source_project ?? "",
    priority: input.priority ?? "normal",
    choices: input.choices ?? [],
    status: "open",
    response_choice: null,
    response_text: null,
    response_by: null,
    created_at: new Date().toISOString(),
    resolved_at: null,
  };
  await db
    .prepare(
      `INSERT INTO items (id, title, body, source_host, source_project, priority, choices, status, created_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, 'open', ?)`,
    )
    .bind(item.id, item.title, item.body, item.source_host, item.source_project, item.priority, JSON.stringify(item.choices), item.created_at)
    .run();
  return item;
}

export async function get(db: D1Database, id: string): Promise<Item | null> {
  const row = await db.prepare("SELECT * FROM items WHERE id = ?").bind(id).first<Row>();
  return row ? fromRow(row) : null;
}

export async function list(db: D1Database, status?: Status): Promise<Item[]> {
  const stmt = status
    ? db.prepare("SELECT * FROM items WHERE status = ? ORDER BY created_at DESC").bind(status)
    : db.prepare("SELECT * FROM items ORDER BY created_at DESC");
  const { results } = await stmt.all<Row>();
  return results.map(fromRow);
}

export interface Resolution {
  choice?: string;
  text?: string;
  by: ResponseBy;
}

/** Transitions an open item to resolved/dismissed; returns null if the item is missing or already closed. */
export async function close(db: D1Database, id: string, status: "resolved" | "dismissed", res: Resolution): Promise<Item | null> {
  const result = await db
    .prepare(
      `UPDATE items SET status = ?, response_choice = ?, response_text = ?, response_by = ?, resolved_at = ?
       WHERE id = ? AND status = 'open'`,
    )
    .bind(status, res.choice ?? null, res.text ?? null, res.by, new Date().toISOString(), id)
    .run();
  if (!result.meta.changes) return null;
  return get(db, id);
}
