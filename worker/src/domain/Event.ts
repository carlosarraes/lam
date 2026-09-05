import { Schema } from "effect";
import { Status } from "./Item";

export const EventName = Schema.Literal("item.created", "item.changed", "item.closed");
export type EventName = typeof EventName.Type;

/** Signals canonical state has changed; item content never belongs here. */
export const ItemEvent = Schema.Struct({
  event: EventName,
  item_id: Schema.NonEmptyTrimmedString,
  version: Schema.Int.pipe(Schema.nonNegative()),
  status: Status,
}).annotations({ parseOptions: { onExcessProperty: "error" } });
export type ItemEvent = typeof ItemEvent.Type;
