import { Data, Schema } from "effect";

export const Priority = Schema.Literal("low", "normal", "critical");
export type Priority = typeof Priority.Type;
export const Status = Schema.Literal("open", "resolved", "dismissed", "retracted", "expired");
export type Status = typeof Status.Type;
export const ResponseBy = Schema.Literal("phone", "cli");
export type ResponseBy = typeof ResponseBy.Type;

export const MAX_TITLE_CHARACTERS = 200;
export const MAX_BODY_BYTES = 64 * 1024;
export const MAX_RECOMMENDATION_CHARACTERS = 2_000;
export const MAX_REPLY_BYTES = 8 * 1024;
export const MAX_CHOICE_CHARACTERS = 200;
export const MAX_CHECK_CHARACTERS = 200;
export const MAX_CHOICES = 3;
export const MAX_CHECKS = 50;

const maxCodePoints = (max: number) =>
  <S extends Schema.Schema<string>>(schema: S) =>
    schema.pipe(Schema.filter((value) => Array.from(value).length <= max || `must be at most ${max} characters`));
const maxUtf8Bytes = (max: number) =>
  <S extends Schema.Schema<string>>(schema: S) =>
    schema.pipe(Schema.filter((value) => new TextEncoder().encode(value).byteLength <= max || `must be at most ${max} UTF-8 bytes`));

const Title = Schema.NonEmptyTrimmedString.pipe(maxCodePoints(MAX_TITLE_CHARACTERS));
const Body = Schema.String.pipe(maxUtf8Bytes(MAX_BODY_BYTES));
const Recommendation = Schema.String.pipe(
  Schema.filter((value) => value.trim().length > 0 || "must not be blank"),
  maxCodePoints(MAX_RECOMMENDATION_CHARACTERS),
);
const Choice = Schema.NonEmptyTrimmedString.pipe(maxCodePoints(MAX_CHOICE_CHARACTERS));
export const CheckLabel = Schema.NonEmptyTrimmedString.pipe(maxCodePoints(MAX_CHECK_CHARACTERS));
export const ReplyText = Schema.NonEmptyTrimmedString.pipe(maxUtf8Bytes(MAX_REPLY_BYTES));

export const Check = Schema.Struct({
  label: Schema.String,
  done: Schema.Boolean,
  at: Schema.NullOr(Schema.String),
});
export type Check = typeof Check.Type;

export class Item extends Schema.Class<Item>("Item")({
  id: Schema.String,
  /** Who is asking: `session:window` from the agent's multiplexer, or an explicit label. */
  name: Schema.String,
  title: Schema.String,
  body: Schema.String,
  source_host: Schema.String,
  source_project: Schema.String,
  priority: Priority,
  choices: Schema.Array(Schema.String),
  checks: Schema.Array(Check),
  recommendation: Schema.NullOr(Schema.String),
  recommended_choice: Schema.NullOr(Schema.String),
  link: Schema.String,
  status: Status,
  response_choice: Schema.NullOr(Schema.String),
  response_text: Schema.NullOr(Schema.String),
  response_by: Schema.NullOr(ResponseBy),
  created_at: Schema.String,
  resolved_at: Schema.NullOr(Schema.String),
  expires_at: Schema.NullOr(Schema.String),
  /** Bumped on every mutation; `wait?since=` returns once it moves. */
  version: Schema.Number,
}) {
  get checksDone(): number {
    return this.checks.filter((c) => c.done).length;
  }

  /** `expired` is derived: an open item past its TTL. Stored status stays `open`. */
  static withExpiry(item: Item, now: Date): Item {
    return item.status === "open" && item.expires_at !== null && new Date(item.expires_at) <= now
      ? new Item({ ...item, status: "expired" })
      : item;
  }
}

/** D1 row: identical to Item except JSON fields and the storage-only dedupe key. */
export const ItemRow = Schema.Struct({
  ...Item.fields,
  choices: Schema.parseJson(Schema.Array(Schema.String)),
  checks: Schema.parseJson(Schema.Array(Check)),
  dedupe_key: Schema.NullOr(Schema.String),
});
export type ItemRow = typeof ItemRow.Type;

export const NewItem = Schema.Struct({
  /** The CLI always sends this; the API tolerates its absence so an older binary mid-run keeps working. */
  name: Schema.optionalWith(Schema.String, { default: () => "" }),
  title: Title,
  body: Schema.optionalWith(Body, { default: () => "" }),
  source_host: Schema.optionalWith(Schema.String, { default: () => "" }),
  source_project: Schema.optionalWith(Schema.String, { default: () => "" }),
  priority: Schema.optionalWith(Priority, { default: () => "normal" as const }),
  choices: Schema.optionalWith(Schema.Array(Choice).pipe(Schema.maxItems(MAX_CHOICES)), { default: () => [] }),
  /** Sub-items the human ticks off; the item auto-resolves when all are done. Exclusive with choices. */
  checks: Schema.optionalWith(Schema.Array(CheckLabel).pipe(Schema.maxItems(MAX_CHECKS)), { default: () => [] }),
  recommendation: Schema.optionalWith(Schema.NullOr(Recommendation), { default: () => null }),
  recommended_choice: Schema.optionalWith(Schema.NullOr(Choice), { default: () => null }),
  link: Schema.optional(Schema.String.pipe(Schema.pattern(/^https?:\/\//))),
  /** Seconds until the item expires; omitted = never. */
  ttl: Schema.optional(Schema.Int.pipe(Schema.positive())),
}).pipe(
  Schema.filter((i) => i.choices.length === 0 || i.checks.length === 0 || "choices and checks are mutually exclusive"),
  Schema.filter((i) => i.recommended_choice === null || i.choices.includes(i.recommended_choice) || "recommended_choice must exactly match one choice"),
);
export type NewItem = typeof NewItem.Type;

export const Resolution = Schema.Struct({
  choice: Schema.optional(Schema.String),
  text: Schema.optional(Schema.String.pipe(maxUtf8Bytes(MAX_REPLY_BYTES))),
});
export type Resolution = typeof Resolution.Type;

export class NotFound extends Data.TaggedError("NotFound")<{ id: string }> {}
export class AlreadyClosed extends Data.TaggedError("AlreadyClosed")<{ id: string }> {}
export class Unauthorized extends Data.TaggedError("Unauthorized")<{}> {}
export class Forbidden extends Data.TaggedError("Forbidden")<{}> {}
export class BadRequest extends Data.TaggedError("BadRequest")<{ message: string }> {}
export class Conflict extends Data.TaggedError("Conflict")<{ id: string }> {}
export class DbError extends Data.TaggedError("DbError")<{ cause: unknown }> {}
