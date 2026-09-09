import { BadRequest } from "../domain/Item";

const DAY = 86_400_000;
const calendar = new Intl.DateTimeFormat("en-US", {
  timeZone: "America/Sao_Paulo", year: "numeric", month: "2-digit", day: "2-digit",
});

function calendarKey(timestamp: number): number {
  const parts = calendar.formatToParts(timestamp);
  const part = (type: string) => Number(parts.find(value => value.type === type)!.value);
  return part("year") * 10_000 + part("month") * 100 + part("day");
}

function startOfDay(utcMidnight: number): string {
  const date = new Date(utcMidnight);
  const target = date.getUTCFullYear() * 10_000 + (date.getUTCMonth() + 1) * 100 + date.getUTCDate();
  let low = utcMidnight - DAY;
  let high = utcMidnight + DAY;
  // Find the first instant of the local date, including days whose midnight
  // was skipped by historical daylight-saving transitions.
  while (low < high) {
    const middle = Math.floor((low + high) / 2);
    if (calendarKey(middle) < target) low = middle + 1;
    else high = middle;
  }
  return new Date(low).toISOString();
}

export function articleDayBounds(day: string): { start: string; end: string } {
  if (!/^(?!0000)\d{4}-\d{2}-\d{2}$/.test(day)) throw new BadRequest({ message: "invalid article day" });
  const midnight = Date.parse(`${day}T00:00:00.000Z`);
  if (!Number.isFinite(midnight) || new Date(midnight).toISOString().slice(0, 10) !== day)
    throw new BadRequest({ message: "invalid article day" });
  return { start: startOfDay(midnight), end: startOfDay(midnight + DAY) };
}
