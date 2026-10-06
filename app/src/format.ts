import { t } from "./i18n";

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

export function age(ts: number, now: number): string {
  const minutes = Math.max(0, Math.floor((now - ts) / 60));
  if (minutes < 1) return t("ageNow");
  if (minutes < 60) return t("ageMinutes", { n: minutes });
  if (minutes < 48 * 60) return t("ageHours", { n: Math.floor(minutes / 60) });
  return t("ageDays", { n: Math.floor(minutes / 1440) });
}

/** Gauge colors: calm, then amber near the limit, then clay when full. */
export function gaugeTone(used: number): "calm" | "warm" | "full" {
  if (used >= 100) return "full";
  if (used >= 70) return "warm";
  return "calm";
}
