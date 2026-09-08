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

export const ArticleEvent = Schema.Struct({
  event: Schema.Literal("article.published", "article.read_changed"),
  article_id: Schema.String.pipe(Schema.pattern(/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/)),
  version: Schema.Int.pipe(Schema.nonNegative()),
}).annotations({ parseOptions: { onExcessProperty: "error" } });
export type ArticleEvent = typeof ArticleEvent.Type;
export const VersionedEvent = Schema.Union(ItemEvent, ArticleEvent);
export type VersionedEvent = typeof VersionedEvent.Type;
