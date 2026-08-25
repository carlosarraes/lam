import { DurableObject } from "cloudflare:workers";
import type { Bindings } from "../Env";
import { buildMessage, TTL_SECONDS, type Draft, type Message } from "./message";

const KEEPALIVE_SECONDS = 45;

type Row = { id: string; seq: number; time: number; json: string };

/**
 * One ntfy-compatible topic: 12h message cache in SQLite plus live fan-out to
 * JSON-stream and WebSocket subscribers. Keepalives are driven by the alarm so
 * WebSockets can hibernate.
 */
export class Topic extends DurableObject<Bindings> {
  private streams = new Set<WritableStreamDefaultWriter<Uint8Array>>();
  private enc = new TextEncoder();

  constructor(ctx: DurableObjectState, env: Bindings) {
    super(ctx, env);
    ctx.storage.sql.exec(
      `CREATE TABLE IF NOT EXISTS messages (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE, time INTEGER, json TEXT)`,
    );
  }

  async publish(topic: string, draft: Draft): Promise<Message> {
    const msg = buildMessage(topic, draft);
    const sql = this.ctx.storage.sql;
    sql.exec("DELETE FROM messages WHERE time < ?", msg.time - TTL_SECONDS);
    sql.exec("INSERT INTO messages (id, time, json) VALUES (?, ?, ?)", msg.id, msg.time, JSON.stringify(msg));
    await this.broadcast(JSON.stringify(msg));
    return msg;
  }

  /** `since` per ntfy: `none`, `all`, a message id, or a unix timestamp; unknown → all. */
  private backlog(since: string | null): string[] {
    const sql = this.ctx.storage.sql;
    if (!since || since === "none") return [];
    let rows: Row[];
    if (since === "all") rows = sql.exec<Row>("SELECT * FROM messages ORDER BY seq").toArray();
    else if (/^\d+$/.test(since)) rows = sql.exec<Row>("SELECT * FROM messages WHERE time >= ? ORDER BY seq", Number(since)).toArray();
    else {
      const anchor = sql.exec<Row>("SELECT seq FROM messages WHERE id = ?", since).toArray()[0];
      rows = anchor
        ? sql.exec<Row>("SELECT * FROM messages WHERE seq > ? ORDER BY seq", anchor.seq).toArray()
        : sql.exec<Row>("SELECT * FROM messages ORDER BY seq").toArray();
    }
    return rows.map((r) => r.json);
  }

  private event(topic: string, event: "open" | "keepalive"): string {
    const now = Math.floor(Date.now() / 1000);
    return JSON.stringify({ id: crypto.randomUUID().slice(0, 12), time: now, expires: now + TTL_SECONDS, event, topic });
  }

  poll(since: string | null): string[] {
    return this.backlog(since);
  }

  async fetch(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const topic = url.searchParams.get("topic") ?? "";
    const since = url.searchParams.get("since");
    const lines = [this.event(topic, "open"), ...this.backlog(since)];

    if (req.headers.get("Upgrade") === "websocket") {
      const pair = new WebSocketPair();
      this.ctx.acceptWebSocket(pair[1]);
      for (const l of lines) pair[1].send(l);
      await this.ensureAlarm();
      return new Response(null, { status: 101, webSocket: pair[0] });
    }

    const { readable, writable } = new TransformStream<Uint8Array>();
    const writer = writable.getWriter();
    this.streams.add(writer);
    writer.closed.catch(() => {}).finally(() => this.streams.delete(writer));
    // Not awaited: nothing reads the stream until this Response is returned.
    this.write(writer, lines.join("\n") + "\n");
    await this.ensureAlarm();
    return new Response(readable, {
      headers: { "content-type": "application/x-ndjson", "cache-control": "no-cache", "x-accel-buffering": "no" },
    });
  }

  private async broadcast(line: string): Promise<void> {
    for (const ws of this.ctx.getWebSockets()) {
      try {
        ws.send(line);
      } catch {
        /* closed socket, dropped by runtime */
      }
    }
    for (const w of this.streams) this.write(w, line + "\n");
  }

  private write(w: WritableStreamDefaultWriter<Uint8Array>, text: string): void {
    w.write(this.enc.encode(text)).catch(() => this.streams.delete(w));
  }

  private async ensureAlarm(): Promise<void> {
    if ((await this.ctx.storage.getAlarm()) === null) await this.ctx.storage.setAlarm(Date.now() + KEEPALIVE_SECONDS * 1000);
  }

  async alarm(): Promise<void> {
    if (!this.ctx.getWebSockets().length && !this.streams.size) return;
    await this.broadcast(this.event("", "keepalive"));
    await this.ctx.storage.setAlarm(Date.now() + KEEPALIVE_SECONDS * 1000);
  }

  webSocketMessage(): void {}
  webSocketClose(ws: WebSocket): void {
    ws.close();
  }
  webSocketError(ws: WebSocket): void {
    ws.close();
  }
}
