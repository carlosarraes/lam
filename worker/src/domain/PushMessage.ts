import type { Item } from "./Item";

export type PushEvent = "item.created" | "item.changed" | "item.closed";

export interface PushMessage {
  readonly android: { readonly priority: "high" };
  readonly data: {
    readonly event: PushEvent;
    readonly item_id: string;
    readonly version: string;
    readonly status: string;
    readonly kind: string;
    readonly name: string;
    readonly priority: "critical";
    readonly title: string;
    readonly pending?: "true";
  };
}

/** FCM carries only lock-screen-safe metadata. D1 remains authoritative. */
export function pushMessage(event: PushEvent, item: Item): PushMessage | null {
  if (item.priority !== "critical" || (item.pending_at !== null && event === "item.created")) return null;
  return {
    android: { priority: "high" },
    data: {
      event,
      item_id: item.id,
      version: String(item.version),
      status: item.status,
      kind: item.kind,
      name: item.name,
      priority: "critical",
      title: item.title,
      ...(item.pending_at !== null ? { pending: "true" as const } : {}),
    },
  };
}
