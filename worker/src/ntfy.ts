import { itemToken } from "./auth";
import type { Env, Item } from "./types";

const NTFY_PRIORITY: Record<Item["priority"], string> = { low: "2", normal: "3", critical: "5" };

function ntfyUrl(env: Env): string {
  return `${env.NTFY_URL ?? "https://ntfy.sh"}/${env.NTFY_TOPIC}`;
}

async function actions(env: Env, item: Item, baseUrl: string): Promise<string> {
  const t = await itemToken(env.LAM_HMAC_SECRET, item.id);
  const buttons = (item.choices.length ? item.choices : ["Done"]).map(
    (c) => `http, ${c}, ${baseUrl}/a/${item.id}/${encodeURIComponent(c)}?t=${t}, clear=true`,
  );
  buttons.push(`view, Reply, ${baseUrl}/r/${item.id}?t=${t}, clear=true`);
  return buttons.join("; ");
}

function source(item: Item): string {
  return [item.source_host, item.source_project].filter(Boolean).join(":");
}

export async function publishNew(env: Env, item: Item, baseUrl: string): Promise<void> {
  const src = source(item);
  await fetch(ntfyUrl(env), {
    method: "POST",
    headers: {
      Title: `${item.title} [${item.id}]`,
      Priority: NTFY_PRIORITY[item.priority],
      Tags: item.priority === "critical" ? "rotating_light" : "eyes",
      Actions: await actions(env, item, baseUrl),
    },
    body: [src && `(${src})`, item.body].filter(Boolean).join("\n") || item.title,
  });
}

export async function publishClosed(env: Env, item: Item): Promise<void> {
  const answer = item.response_choice ?? item.response_text ?? item.status;
  await fetch(ntfyUrl(env), {
    method: "POST",
    headers: { Title: `${item.title} [${item.id}]`, Priority: "1", Tags: "white_check_mark" },
    body: `${item.status} via ${item.response_by}: ${answer}`,
  });
}
