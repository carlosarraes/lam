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

async function post(env: Env, init: RequestInit & { headers: Record<string, string> }): Promise<void> {
  // ntfy.sh quotas are per source IP; Workers share egress IPs, so an account token is required in practice.
  if (env.NTFY_TOKEN) init.headers.Authorization = `Bearer ${env.NTFY_TOKEN}`;
  try {
    const res = await fetch(ntfyUrl(env), init);
    if (!res.ok) console.error(`ntfy ${res.status}: ${await res.text()}`);
  } catch (e) {
    console.error("ntfy publish failed", e);
  }
}

export async function publishNew(env: Env, item: Item, baseUrl: string): Promise<void> {
  const src = source(item);
  await post(env, {
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
  await post(env, {
    method: "POST",
    headers: { Title: `${item.title} [${item.id}]`, Priority: "1", Tags: "white_check_mark" },
    body: `${item.status} via ${item.response_by}: ${answer}`,
  });
}
