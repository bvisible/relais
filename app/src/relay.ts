// The one screen of the panel: every account on a line, and one button that
// moves Claude, with its whole workspace, to another account.

import { inTime, margin, time } from "./format.ts";
import { t } from "./i18n.ts";

type WindowReading = { used: number; resetsAt: number | null } | null;

type Line = {
  alias: string;
  fiveHour: WindowReading;
  sevenDay: WindowReading;
  limited: boolean;
  limitedUntil: number | null;
  observedAt: number | null;
};

/** What a switch would do, as the Rust side plans it. */
export type DesktopPlan = {
  from: string;
  conversations: number;
  /** The arriving account's conversations of its own. */
  own: number;
  signIn: "parked" | "oldFolder" | "none";
};

export type Started = "signedIn" | "notConfirmed" | "signedOut";

export type SwitchResult = {
  outcome: "already" | "switched";
  started: Started | null;
  undidInterrupted: boolean;
};

/** What the confirmation says: what happens, what stays, and what to know. */
export function confirmLines(plan: DesktopPlan, alias: string, email: string): string[] {
  const lines = [
    t("relayConfirmCloses", { alias }),
    plan.conversations === 1 ? t("relayConfirmKeepsOne") : t("relayConfirmKeepsMany", { n: plan.conversations }),
  ];
  if (plan.signIn === "none") lines.push(t("relayConfirmSignIn", { email }));
  if (plan.own > 0) lines.push(t("relayConfirmOwn", { alias, n: plan.own }));
  return lines;
}

/** The line shown once the switch is done. */
export function resultLine(result: SwitchResult, alias: string, email: string): string {
  if (result.outcome === "already") return t("relayAlready", { alias });
  if (result.started === "signedIn") return t("relayDone", { alias });
  if (result.started === "signedOut") return t("relaySignInNow", { email });
  return t("relayNotConfirmed", { alias });
}

/** An account's usage in a few words: the limits its plan has (some no
 * longer have the 5-hour one), or when it is back. */
export function usageLine(account: Line, now: number): string {
  if (account.limited) {
    return account.limitedUntil != null && account.limitedUntil > now ? t("relayPaused", { time: time(account.limitedUntil, now) }) : t("relayPausedUnknown");
  }
  const five = account.fiveHour ? t("relayFive", { n: Math.round(account.fiveHour.used) }) : null;
  const week = account.sevenDay ? t("relayWeek", { n: Math.round(account.sevenDay.used) }) : null;
  if (five && week) return `${five} · ${week}`;
  if (week && account.sevenDay?.resetsAt != null && account.sevenDay.resetsAt > now) {
    return `${week} · ${t("relayStartsOver", { in: inTime(account.sevenDay.resetsAt, now) })}`;
  }
  return five ?? week ?? t("relayUnmeasured");
}

export type Ring = { kind: "five" | "week"; used: number };

/** What an account's rings show: one ring per limit its plan has (the 5-hour
 * one outside), the fuller share in the middle; a paused account shows one
 * full ring with how long until it is back. */
export function ringsOf(account: Line, now: number): { rings: Ring[]; center: string; tone: "normal" | "high" | "paused" } {
  if (account.limited) {
    const left = account.limitedUntil != null ? account.limitedUntil - now : null;
    return { rings: [{ kind: "week", used: 100 }], center: left != null && left > 0 ? compact(left) : "—", tone: "paused" };
  }
  const rings: Ring[] = [];
  if (account.fiveHour) rings.push({ kind: "five", used: Math.round(account.fiveHour.used) });
  if (account.sevenDay) rings.push({ kind: "week", used: Math.round(account.sevenDay.used) });
  if (rings.length === 0) return { rings, center: "—", tone: "normal" };
  const fuller = Math.max(...rings.map((ring) => ring.used));
  return { rings, center: `${fuller} %`, tone: fuller >= HIGH_FROM ? "high" : "normal" };
}

/** From this share used, a ring takes the signal. */
const HIGH_FROM = 85;

/** A duration short enough for the middle of a ring: `40 min`, `6 h`, `1 j`. */
function compact(secs: number): string {
  if (secs < 3600) return t("relayRingMinutes", { n: Math.max(1, Math.ceil(secs / 60)) });
  if (secs < 86_400) return t("relayRingHours", { n: Math.floor(secs / 3600) });
  return t("relayRingDays", { n: Math.floor(secs / 86_400) });
}

/**
 * Claude's account first, then the free ones with the most room left, the
 * paused ones by when they come back, and the signed-out ones last.
 */
export function relayOrder<T extends Line>(accounts: T[], desktopOwner: string | null, signedOut: Set<string>): T[] {
  const rank = (a: T) => (a.alias === desktopOwner ? 0 : signedOut.has(a.alias) ? 3 : a.limited ? 2 : 1);
  const back = (a: T) => a.limitedUntil ?? Number.MAX_SAFE_INTEGER;
  return [...accounts].sort((a, b) => {
    if (rank(a) !== rank(b)) return rank(a) - rank(b);
    if (rank(a) === 1) return (margin(b) ?? -1) - (margin(a) ?? -1);
    if (rank(a) === 2) return back(a) - back(b);
    return 0;
  });
}
