export type Priority = "low" | "normal" | "critical";
export type Status = "open" | "resolved" | "dismissed";
export type ResponseBy = "phone" | "cli";

export interface Item {
  id: string;
  title: string;
  body: string;
  source_host: string;
  source_project: string;
  priority: Priority;
  choices: string[];
  status: Status;
  response_choice: string | null;
  response_text: string | null;
  response_by: ResponseBy | null;
  created_at: string;
  resolved_at: string | null;
}

export interface NewItem {
  title: string;
  body?: string;
  source_host?: string;
  source_project?: string;
  priority?: Priority;
  choices?: string[];
}

export interface Env {
  DB: D1Database;
  LAM_TOKEN: string;
  LAM_HMAC_SECRET: string;
  NTFY_TOPIC: string;
  NTFY_URL?: string;
}

export const PRIORITIES: Priority[] = ["low", "normal", "critical"];
export const MAX_CHOICES = 3;
