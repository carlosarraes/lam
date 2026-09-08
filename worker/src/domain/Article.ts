import { Data, Schema } from "effect";

export const Asset = Schema.Struct({
  path: Schema.String,
  media_type: Schema.String,
  size: Schema.Number,
  sha256: Schema.String,
  disposition: Schema.Literal("inline", "attachment"),
});
export type Asset = typeof Asset.Type;
export const ArticleDraft = Schema.Struct({
  title: Schema.String,
  summary: Schema.String,
  name: Schema.String,
  source_host: Schema.String,
  source_project: Schema.String,
  silent: Schema.Boolean,
  assets: Schema.Array(Asset),
});
export type ArticleDraft = typeof ArticleDraft.Type;
export interface Article {
  id: string;
  title: string;
  summary: string;
  name: string;
  source_host: string;
  source_project: string;
  created_at: string;
  read_at: string | null;
  version: number;
  assets: readonly Asset[];
}
export interface ArticlePage { items: Article[]; next_cursor: string | null }
export const ReadUpdate = Schema.Struct({ read: Schema.Boolean, version: Schema.Int.pipe(Schema.nonNegative()) });
export type ReadUpdate = typeof ReadUpdate.Type;
export class ArticleStorageError extends Data.TaggedError("ArticleStorageError")<{ cause: unknown }> {}
