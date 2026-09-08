import { DurableObject } from "cloudflare:workers";
import { Either, Schema } from "effect";
import type { Bindings } from "../Env";
import { ItemEvent, VersionedEvent } from "../domain/Event";

/** Live invalidations for the owner's private queue, with no item storage or replay. */
export class EventStream extends DurableObject<Bindings> {
  async publish(event: ItemEvent): Promise<void> {
    const decoded = Schema.decodeUnknownEither(ItemEvent)(event);
    if (Either.isLeft(decoded)) throw new Error("Invalid item event");
    this.broadcast(decoded.right);
  }

  async publishV2(event: VersionedEvent): Promise<void> {
    const decoded = Schema.decodeUnknownEither(VersionedEvent)(event);
    if (Either.isLeft(decoded)) throw new Error("Invalid versioned event");
    this.broadcast(decoded.right, "v2");
  }

  private broadcast(event: VersionedEvent, tag?: string): void {
    const frame = JSON.stringify(event);
    for (const socket of this.ctx.getWebSockets(tag)) {
      try {
        socket.send(frame);
      } catch {
        socket.close(1011, "Event delivery failed");
      }
    }
  }

  async fetch(request: Request): Promise<Response> {
    if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket") {
      return new Response("WebSocket upgrade required", { status: 426 });
    }
    const pair = new WebSocketPair();
    this.ctx.acceptWebSocket(pair[1], new URL(request.url).pathname === "/v2" ? ["v2"] : ["v1"]);
    return new Response(null, { status: 101, webSocket: pair[0] });
  }

  // Protocol ping/pong is handled by the runtime without waking this object.
  webSocketMessage(): void {}

  webSocketClose(socket: WebSocket): void {
    socket.close();
  }

  webSocketError(socket: WebSocket): void {
    socket.close(1011, "Event stream error");
  }
}
