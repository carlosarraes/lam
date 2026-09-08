import { Data, Effect } from "effect";
import { Env } from "../Env";
import { TopicClient } from "./TopicClient";

class NotificationJobError extends Data.TaggedError("NotificationJobError")<{}> {}
interface Job { article_id: string; attempts: number }
const dbCall = <A>(run: () => Promise<A>) => Effect.tryPromise({ try: run, catch: () => new NotificationJobError() });

/** One durable logical job; ambiguous transport failures can produce another visible message. */
export class ArticleNotifications extends Effect.Service<ArticleNotifications>()("lam/ArticleNotifications", {
  effect: Effect.gen(function* () {
    const topic = yield* TopicClient;
    return {
      deliver: (now = new Date()) => Effect.gen(function* () {
        const { DB } = yield* Env;
        const token = crypto.randomUUID();
        const timestamp = now.toISOString();
        const lease = new Date(now.getTime() + 5 * 60_000).toISOString();
        const claimed = yield* dbCall(() => DB.prepare(`UPDATE article_notification_jobs
          SET state = 'sending', attempts = attempts + 1, lease_token = ?, lease_until = ?
          WHERE article_id IN (SELECT j.article_id FROM article_notification_jobs j JOIN articles a ON a.id = j.article_id
            WHERE a.state = 'published' AND a.silent = 0 AND a.owner_id = 'owner' AND
              ((j.state = 'pending' AND j.next_attempt_at <= ?) OR (j.state = 'sending' AND j.lease_until <= ?))
            ORDER BY j.next_attempt_at, j.article_id LIMIT 25) RETURNING article_id, attempts`)
          .bind(token, lease, timestamp, timestamp).all<Job>());
        yield* Effect.forEach(claimed.results, job => Effect.gen(function* () {
          const article = yield* dbCall(() => DB.prepare("SELECT title, summary FROM articles WHERE id = ? AND state = 'published' AND silent = 0")
            .bind(job.article_id).first<{ title: string; summary: string }>());
          if (!article) return yield* new NotificationJobError();
          const sent = yield* topic.publish({ title: article.title, message: article.summary || article.title, priority: 2,
            actions: [{ action: "view", label: "View", url: `lam://articles/${job.article_id}`, clear: false }] })
            .pipe(Effect.timeout("20 seconds"), Effect.exit);
          if (sent._tag === "Success") {
            yield* dbCall(() => DB.prepare(`UPDATE article_notification_jobs SET state = 'delivered', delivered_at = ?, lease_token = NULL, lease_until = NULL
              WHERE article_id = ? AND state = 'sending' AND lease_token = ?`).bind(new Date().toISOString(), job.article_id, token).run());
          } else {
            const retry = new Date(Date.now() + Math.min(3_600_000, 30_000 * 2 ** Math.min(job.attempts - 1, 7))).toISOString();
            yield* dbCall(() => DB.prepare(`UPDATE article_notification_jobs SET state = 'pending', next_attempt_at = ?, lease_token = NULL, lease_until = NULL
              WHERE article_id = ? AND state = 'sending' AND lease_token = ?`).bind(retry, job.article_id, token).run());
          }
        }), { concurrency: 5, discard: true });
      }),
    };
  }),
  dependencies: [TopicClient.Default],
}) {}
