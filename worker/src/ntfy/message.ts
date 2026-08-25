import { Either } from "effect";
import { BadRequest } from "../domain/Item";

export interface Action {
  id: string;
  action: "view" | "http";
  label: string;
  url: string;
  clear: boolean;
}

/** Wire format the ntfy Android app and `lam watch` consume. */
export interface Message {
  id: string;
  time: number;
  expires: number;
  event: "message";
  topic: string;
  title?: string;
  message: string;
  priority: number;
  tags?: string[];
  actions?: Action[];
}

export const TTL_SECONDS = 12 * 3600;
const ID_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

export function randomId(len = 12): string {
  const bytes = crypto.getRandomValues(new Uint8Array(len));
  return Array.from(bytes, (b) => ID_ALPHABET[b % ID_ALPHABET.length]).join("");
}

export interface Draft {
  title?: string;
  message: string;
  priority?: number;
  tags?: string[];
  actions?: Omit<Action, "id">[];
}

export function buildMessage(topic: string, d: Draft): Message {
  const now = Math.floor(Date.now() / 1000);
  const msg: Message = {
    id: randomId(),
    time: now,
    expires: now + TTL_SECONDS,
    event: "message",
    topic,
    message: d.message,
    priority: d.priority ?? 3,
  };
  if (d.title) msg.title = d.title;
  if (d.tags?.length) msg.tags = d.tags;
  if (d.actions?.length) msg.actions = d.actions.map((a) => ({ id: randomId(10), ...a }));
  return msg;
}

/** Parses the ntfy simple `Actions` header: `http, Label, https://url, clear=true; view, Label, https://url`. */
export function parseActions(header: string): Either.Either<Omit<Action, "id">[], BadRequest> {
  return Either.all(
    header
      .split(";")
      .map((s) => s.trim())
      .filter(Boolean)
      .map((spec) => {
        const [action, label, url, ...opts] = spec.split(",").map((s) => s.trim());
        if ((action !== "http" && action !== "view") || !label || !url) return Either.left(new BadRequest({ message: `bad action: ${spec}` }));
        const clear = opts.some((o) => o.replace(/\s/g, "") === "clear=true");
        return Either.right({ action, label, url, clear } as Omit<Action, "id">);
      }),
  );
}

export interface RawPublish {
  headers: Record<string, string | undefined>;
  query: Record<string, string | undefined>;
  body: string;
}

/** Turns a raw ntfy-style publish (headers + query + body) into a Draft. */
export function draftFromRaw({ headers, query, body }: RawPublish): Either.Either<Draft, BadRequest> {
  const h = (name: string) => headers[name.toLowerCase()] ?? query[name.toLowerCase()];
  const priority = h("priority") ? Number(h("priority")) : undefined;
  if (priority !== undefined && !(priority >= 1 && priority <= 5)) return Either.left(new BadRequest({ message: "priority must be 1-5" }));
  const actions = h("actions") ? parseActions(h("actions")!) : Either.right(undefined);
  return Either.map(actions, (actions) => ({
    title: h("title"),
    message: query.message ?? body.trim(),
    priority,
    tags: h("tags")?.split(",").map((t) => t.trim()).filter(Boolean),
    actions,
  }));
}
