import { t } from "./i18n.ts";

type WindowReading = { used: number; resetsAt: number | null } | null;
type Reading = { fiveHour: WindowReading; sevenDay: WindowReading; limited: boolean };

export type Mood = "running" | "tiring" | "resting" | "unknown";

/** From this share of a window used, an account is running low. */
export const TIRING_FROM = 85;

/** A conversation counts as live this long after its last message. */
export const LIVE_FOR_SECS = 120;

/** `19:00` today, `lun. 22:00` within a week, `13.10. 22:00` beyond. */
export function time(ts: number, now: number): string {
  const at = new Date(ts * 1000);
  const today = new Date(now * 1000);
  const hm = at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
  const startOf = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((startOf(at) - startOf(today)) / 86_400_000);
  if (days === 0) return hm;
  if (days > 0 && days < 7) return `${t("weekdays").split(",")[at.getDay()]} ${hm}`;
  const dd = String(at.getDate()).padStart(2, "0");
  const mm = String(at.getMonth() + 1).padStart(2, "0");
  return `${dd}.${mm}. ${hm}`;
}

/**
 * How long until `ts`: `45 min`, `2 h 14`, `4 j 6 h`. Rounded to the nearest
 * minute, so a reading taken a second ago does not lose a minute.
 */
export function remaining(ts: number, now: number): string {
  const minutes = Math.floor((Math.max(0, ts - now) + 30) / 60);
  if (minutes < 1) return t("inUnderMinute");
  if (minutes < 60) return t("inMinutes", { m: minutes });
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return t("inHours", { h: hours, mm: String(minutes % 60).padStart(2, "0") });
  const days = Math.floor(hours / 24);
  return hours % 24 === 0 ? t("inDaysOnly", { d: days }) : t("inDays", { d: days, h: hours % 24 });
}

/** `dans 2 h 14`. */
export function inTime(ts: number, now: number): string {
  return t("inDuration", { d: remaining(ts, now) });
}

/** The weekdays of the last `count` days, oldest first, the last one being today. */
export function dayLabels(now: number, count: number): string[] {
  const names = t("weekdays").split(",");
  const today = new Date(now * 1000);
  return Array.from({ length: count }, (_, index) => {
    const back = count - 1 - index;
    if (back === 0) return t("today");
    const day = new Date(today.getFullYear(), today.getMonth(), today.getDate() - back);
    return names[day.getDay()];
  });
}

export function age(ts: number, now: number): string {
  const seconds = Math.max(0, now - ts);
  if (seconds < 10) return t("ageNow");
  if (seconds < 60) return t("ageSeconds", { n: Math.floor(seconds / 10) * 10 });
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return t("ageMinutes", { n: minutes });
  if (minutes < 48 * 60) return t("ageHours", { n: Math.floor(minutes / 60) });
  return t("ageDays", { n: Math.floor(minutes / 1440) });
}

/** The share of the fuller window, or `null` when nothing was measured. */
export function used(reading: Reading): number | null {
  const values = [reading.fiveHour?.used, reading.sevenDay?.used].filter((v): v is number => v != null);
  return values.length === 0 ? null : Math.max(...values);
}

/** What the fuller window leaves, in percent. */
export function margin(reading: Reading): number | null {
  const fuller = used(reading);
  return fuller == null ? null : Math.max(0, Math.round(100 - fuller));
}

export function mood(reading: Reading): Mood {
  if (reading.limited) return "resting";
  const fuller = used(reading);
  if (fuller == null) return "unknown";
  return fuller >= TIRING_FROM ? "tiring" : "running";
}

export function isLive(lastActivity: number, now: number): boolean {
  return now - lastActivity < LIVE_FOR_SECS;
}
