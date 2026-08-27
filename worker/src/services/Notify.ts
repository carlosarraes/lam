import { Effect } from "effect";
import type { Item, Priority } from "../domain/Item";
import type { Draft } from "../ntfy/message";
import { Auth } from "./Auth";
import { TopicClient } from "./TopicClient";

const NTFY_PRIORITY: Record<Priority, number> = { low: 2, normal: 3, critical: 5 };

/** Who to blame on screen: the agent's name, falling back to host:project for pre-name items. */
const source = (item: Item) => item.name || [item.source_host, item.source_project].filter(Boolean).join(":");

/** Turns item lifecycle events into pushes on the topic. */
export class Notify extends Effect.Service<Notify>()("lam/Notify", {
  effect: Effect.gen(function* () {
    const auth = yield* Auth;
    const topic = yield* TopicClient;
    const publish = (draft: Draft) =>
      topic.publish(draft).pipe(Effect.asVoid, Effect.tapError((e) => Effect.logError("publish failed", e)), Effect.ignore);

    return {
      itemCreated: (item: Item, baseUrl: string) =>
        Effect.gen(function* () {
          const t = yield* auth.itemToken(item.id);
          const page = `${baseUrl}/r/${item.id}?t=${t}`;
          const actions: Draft["actions"] = item.checks.length
            ? []
            : (item.choices.length ? item.choices : ["Done"]).map((c) => ({
                action: "http",
                label: c,
                url: `${baseUrl}/a/${item.id}/${encodeURIComponent(c)}?t=${t}`,
                clear: true,
              }));
          if (item.link) actions.push({ action: "view", label: "Open", url: item.link, clear: false });
          actions.push({ action: "view", label: item.checks.length ? "Checks" : "Reply", url: page, clear: !item.checks.length });
          const src = source(item);
          const checklist = item.checks.map((c) => `${c.done ? "☑" : "☐"} ${c.label}`).join("\n");
          yield* publish({
            title: `${item.title} [${item.id}]`,
            message: [src && `(${src})`, item.body, checklist].filter(Boolean).join("\n") || item.title,
            priority: NTFY_PRIORITY[item.priority],
            tags: [item.priority === "critical" ? "rotating_light" : "eyes"],
            actions,
          });
        }),

      checkAdded: (item: Item, label: string, baseUrl: string) =>
        Effect.gen(function* () {
          const t = yield* auth.itemToken(item.id);
          yield* publish({
            title: `${item.title} [${item.id}]`,
            message: `new check: ${label} (${item.checksDone}/${item.checks.length})`,
            priority: 3,
            tags: ["heavy_plus_sign"],
            actions: [{ action: "view", label: "Checks", url: `${baseUrl}/r/${item.id}?t=${t}`, clear: false }],
          });
        }),

      itemClosed: (item: Item) =>
        publish({
          title: `${item.title} [${item.id}]`,
          message: `${item.status} via ${item.response_by}: ${
            item.response_choice ?? item.response_text ?? (item.checks.length ? `${item.checksDone}/${item.checks.length} checks` : item.status)
          }`,
          priority: 1,
          tags: [item.status === "retracted" ? "leftwards_arrow_with_hook" : "white_check_mark"],
        }),
    };
  }),
  dependencies: [Auth.Default, TopicClient.Default],
}) {}
