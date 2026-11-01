// Calls into the Rust side of the app. Outside Tauri (a plain browser, used to
// preview the interface), it answers with sample data instead.

import { invoke } from "@tauri-apps/api/core";

import type { DesktopPlan, SwitchResult } from "./relay.ts";

export type UsageWindow = { used: number; resetsAt: number | null };

/** A weekly limit on one model only (`Fable`), as `/usage` last reported it. */
export type ModelWindow = UsageWindow & { model: string; observedAt: number };

export type Account = {
  alias: string;
  email: string | null;
  /** The account's own Chrome profile; `null` for the account Claude Code started with. */
  chromeProfile: string | null;
  /** The Claude desktop app has a data folder for this account. */
  desktopFolder: boolean;
  /** `#RRGGBB`. */
  color: string;
  active: boolean;
  fiveHour: UsageWindow | null;
  sevenDay: UsageWindow | null;
  /** Limits on single models, which `/usage` reports and the status line does not. */
  models: ModelWindow[];
  observedAt: number | null;
  limited: boolean;
  limitedUntil: number | null;
};

export type PaletteColor = { name: string; hex: string };

export type Overview = {
  accounts: Account[];
  palette: PaletteColor[];
  /** The account whose folder the Claude desktop app uses. */
  desktopOwner: string | null;
  best: string | null;
  statusline: boolean;
  shell: boolean;
  cliFound: boolean;
  /** Claude Code waits for an answer of the user before it can measure. */
  measureWaits: boolean;
  now: number;
};

export type Conversation = {
  id: string;
  title: string | null;
  cwd: string | null;
  project: string | null;
  /** When its first message was written; `null` when the transcript does not say. */
  startedAt: number | null;
  lastActivity: number;
  /** The account that last ran this session, in a terminal or in the desktop app. */
  account: string | null;
  desktop: boolean;
  /** Messages per five minutes over the last hour, oldest first. */
  activity: number[];
};

export type AccountStats = {
  alias: string;
  /** The last seven days, oldest first: the share of the weekly quota used
   * (`null` without a reading that day), and whether a window was full. */
  days: { used: number | null; limited: boolean }[];
  /** Highest weekly share in each of the last four weeks, oldest first. */
  weeks: (number | null)[];
  conversationsToday: number;
  conversationsWeek: number;
  lastSeen: number | null;
};

export type AccountSignIn = {
  alias: string;
  /** `null` when Claude Code could not be asked. */
  signedIn: boolean | null;
  email: string | null;
  plan: string | null;
};

export type DesktopSwitch = SwitchResult & { overview: Overview };

export type ResumeTarget = "terminal" | "desktop";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return inTauri ? invoke<T>(command, args) : preview<T>(command, args);
}

export const api = {
  overview: () => call<Overview>("overview"),
  switchAccount: (alias: string) => call<Overview>("switch_account", { alias }),
  setColor: (alias: string, color: string) => call<Overview>("set_color", { alias, color }),
  accountStats: () => call<AccountStats[]>("account_stats"),
  /** Every account's sign-in, or only `only`'s. */
  accountSignIns: (only?: string) => call<AccountSignIn[]>("account_sign_ins", { only: only ?? null }),
  allowMeasure: () => call<void>("allow_measure"),
  signInAgain: (alias: string) => call<void>("sign_in_again", { alias }),
  removeAccount: (alias: string) => call<Overview>("remove_account", { alias }),
  /** What switching Claude to `alias` would do; changes nothing. */
  desktopPlan: (alias: string) => call<DesktopPlan>("desktop_plan", { alias }),
  /** `own` says what to do with the account's own conversations, when it has some. */
  switchDesktop: (alias: string, own: "add" | "aside" | null = null) => call<DesktopSwitch>("switch_desktop", { alias, own }),
  refreshUsage: (force: boolean) => call<Overview>("refresh_usage", { force }),
  conversations: (limit: number) => call<Conversation[]>("conversations", { limit }),
  resume: (sessionId: string, target: ResumeTarget) => call<void>("resume", { sessionId, target }),
  addAccount: (email: string) => call<void>("add_account", { email }),
  suggestAlias: (email: string) => call<string>("suggest_alias", { email }),
  setStatusline: (enabled: boolean) => call<Overview>("set_statusline", { enabled }),
  setShell: (enabled: boolean) => call<Overview>("set_shell", { enabled }),
  hide: () => call<void>("hide"),
  quit: () => call<void>("quit"),
};

// ---- Preview data for a plain browser -----------------------------------

const nowSecs = () => Math.floor(Date.now() / 1000);

const PREVIEW_PALETTE: PaletteColor[] = [
  { name: "sage", hex: "#4CC38A" },
  { name: "ocean", hex: "#4C8DFF" },
  { name: "violet", hex: "#A78BFA" },
  { name: "coral", hex: "#FF7A6B" },
  { name: "amber", hex: "#F2B33D" },
  { name: "rose", hex: "#F472B6" },
  { name: "lime", hex: "#C3D94E" },
  { name: "sky", hex: "#38BDF8" },
];

// `?state=limited`, `?state=tiring`, `?state=signedOut`, `?state=connections` or `?state=trust` in the address previews those screens.
const previewMode = typeof location !== "undefined" ? new URLSearchParams(location.search).get("state") : null;

let previewState: Overview = {
  accounts: [
    {
      alias: "main",
      email: "main@example.com",
      desktopFolder: true,
      chromeProfile: null,
      color: "#4CC38A",
      active: true,
      fiveHour:
        previewMode === "limited"
          ? { used: 100, resetsAt: nowSecs() + 72 * 60 }
          : { used: previewMode === "tiring" ? 92 : 47, resetsAt: nowSecs() + 2 * 3600 + 14 * 60 },
      sevenDay: { used: previewMode === "limited" ? 72 : 38, resetsAt: nowSecs() + 4 * 86400 + 6 * 3600 },
      models: [],
      observedAt: nowSecs() - 30,
      limited: previewMode === "limited",
      limitedUntil: previewMode === "limited" ? nowSecs() + 72 * 60 : null,
    },
    {
      alias: "studio",
      email: "studio@example.com",
      desktopFolder: true,
      chromeProfile: "Relais-studio",
      color: "#A78BFA",
      active: false,
      fiveHour: { used: 0, resetsAt: null },
      sevenDay: { used: 100, resetsAt: nowSecs() + 4 * 86400 + 22 * 3600 },
      models: [{ model: "Fable", used: 0, resetsAt: nowSecs() + 4 * 86400 + 22 * 3600, observedAt: nowSecs() - 3600 }],
      observedAt: nowSecs() - 3600,
      limited: true,
      limitedUntil: nowSecs() + 4 * 86400 + 22 * 3600,
    },
    {
      alias: "spare",
      email: "spare@example.com",
      desktopFolder: false,
      chromeProfile: "Relais-spare",
      color: "#4C8DFF",
      active: false,
      // A plan without the 5-hour limit: only the weekly one.
      fiveHour: null,
      sevenDay: { used: 4, resetsAt: nowSecs() + 6 * 86400 },
      models: [],
      observedAt: nowSecs() - 600,
      limited: false,
      limitedUntil: null,
    },
    {
      alias: "work",
      email: "work@example.com",
      desktopFolder: true,
      chromeProfile: "Relais-work",
      color: "#F2B33D",
      active: false,
      fiveHour: { used: 30, resetsAt: nowSecs() + 3 * 3600 },
      sevenDay: { used: 55, resetsAt: nowSecs() + 2 * 86400 },
      models: [],
      observedAt: nowSecs() - 120,
      limited: false,
      limitedUntil: null,
    },
  ],
  palette: PREVIEW_PALETTE,
  desktopOwner: "main",
  best: "spare",
  statusline: true,
  shell: false,
  cliFound: true,
  measureWaits: previewMode === "trust",
  now: nowSecs(),
};

const previewSignedOut = new Set<string>();
let previewSignInOpenedAt: number | null = null;

const days = (used: (number | null)[], limited: number[]) => used.map((value, index) => ({ used: value, limited: limited.includes(index) }));

async function preview<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (command === "switch_account") {
    const alias = String(args?.alias);
    previewState = {
      ...previewState,
      accounts: previewState.accounts.map((a) => ({ ...a, active: a.alias === alias })),
    };
  }
  if (command === "set_color") {
    const alias = String(args?.alias);
    previewState = {
      ...previewState,
      accounts: previewState.accounts.map((a) => (a.alias === alias ? { ...a, color: String(args?.color) } : a)),
    };
  }
  if (command === "remove_account") {
    const alias = String(args?.alias);
    previewState = { ...previewState, accounts: previewState.accounts.filter((a) => a.alias !== alias) };
  }
  if (command === "account_stats") {
    const stats: Record<string, Omit<AccountStats, "alias">> = {
      main: { days: days([14, 22, 6, null, 3, 18, 12], [1]), weeks: [100, 87, 100, 55], conversationsToday: 6, conversationsWeek: 31, lastSeen: nowSecs() - 60 },
      studio: { days: days([30, 28, 25, 17, 0, null, null], [2, 3, 4]), weeks: [40, 72, 100, 100], conversationsToday: 0, conversationsWeek: 9, lastSeen: nowSecs() - 7 * 3600 },
      spare: { days: days([null, null, null, null, null, null, 4], []), weeks: [null, 100, 62, 4], conversationsToday: 0, conversationsWeek: 2, lastSeen: nowSecs() - 4 * 86400 },
    };
    return previewState.accounts.map((a) => ({ alias: a.alias, ...(stats[a.alias] ?? { days: [], weeks: [], conversationsToday: 0, conversationsWeek: 0, lastSeen: null }) })) as T;
  }
  if (command === "account_sign_ins") {
    await new Promise((resolve) => setTimeout(resolve, 900));
    // `?state=signedOut`: spare is signed out until its sign-in is opened, then back after 10 s.
    if (command === "account_sign_ins" && previewSignInOpenedAt == null && previewMode === "signedOut") previewSignedOut.add("spare");
    if (previewSignInOpenedAt != null && Date.now() - previewSignInOpenedAt > 10_000) previewSignedOut.delete("spare");
    return previewState.accounts
      .filter((a) => args?.only == null || a.alias === args.only)
      // `?state=connections`: work's terminal profile holds another account.
      .map((a) => ({ alias: a.alias, signedIn: !previewSignedOut.has(a.alias), email: previewMode === "connections" && a.alias === "work" ? "spare@example.com" : a.email, plan: "max" })) as T;
  }
  if (command === "sign_in_again") previewSignInOpenedAt = Date.now();
  if (command === "sign_in_again" || command === "allow_measure") return undefined as T;
  if (command === "refresh_usage") await new Promise((resolve) => setTimeout(resolve, args?.force ? 2500 : 300));
  if (command === "desktop_plan") {
    await new Promise((resolve) => setTimeout(resolve, 150));
    const alias = String(args?.alias);
    const target = previewState.accounts.find((a) => a.alias === alias);
    return { from: previewState.desktopOwner ?? "", conversations: 139, own: alias === "work" ? 4 : 0, live: 1, signIn: target?.desktopFolder ? "parked" : "none" } as T;
  }
  if (command === "switch_desktop") {
    await new Promise((resolve) => setTimeout(resolve, 1600));
    const alias = String(args?.alias);
    const target = previewState.accounts.find((a) => a.alias === alias);
    const started = target?.desktopFolder ? "signedIn" : "signedOut";
    previewState = {
      ...previewState,
      desktopOwner: alias,
      accounts: previewState.accounts.map((a) => ({ ...a, active: a.alias === alias, desktopFolder: a.desktopFolder || a.alias === alias })),
    };
    return { outcome: "switched", started, undidInterrupted: false, overview: { ...previewState, now: nowSecs() } } as T;
  }
  if (command === "set_statusline") previewState = { ...previewState, statusline: Boolean(args?.enabled) };
  if (command === "set_shell") previewState = { ...previewState, shell: Boolean(args?.enabled) };
  if (command === "suggest_alias") {
    const local = String(args?.email ?? "").split("@")[0].toLowerCase().replace(/[^a-z0-9_]+/g, "-").replace(/^-+|-+$/g, "");
    return (local || "account") as T;
  }
  if (command === "conversations") {
    const now = nowSecs();
    return [
      { id: "s1", title: "Rings and colors in the menu bar app", cwd: "~/GitHub/relais", project: "relais", startedAt: now - 2 * 3600 - 12 * 60, lastActivity: now - 12, account: "main", desktop: false, activity: [1, 2, 2, 4, 6, 5, 8, 9, 12, 11, 14, 13] },
      { id: "s2", title: "Mobile notifications and widget", cwd: "~/GitHub/mobile", project: "mobile", startedAt: now - 53 * 60, lastActivity: now - 100, account: "spare", desktop: true, activity: [6, 9, 7, 10, 8, 5, 6, 4, 3, 2, 1, 1] },
      { id: "s3", title: "Audit and framework migration", cwd: "~/GitHub/devops", project: "devops", startedAt: now - 3 * 3600, lastActivity: now - 1500, account: null, desktop: false, activity: [0, 0, 3, 5, 2, 0, 0, 0, 0, 0, 0, 0] },
      { id: "s4", title: "Translations for the archive app", cwd: "~/GitHub/archive", project: "archive", startedAt: now - 6 * 3600, lastActivity: now - 5400, account: "studio", desktop: false, activity: new Array(12).fill(0) },
      { id: "s5", title: "Nightly backup check", cwd: "~/GitHub/devops", project: "devops", startedAt: null, lastActivity: now - 7200, account: "main", desktop: false, activity: new Array(12).fill(0) },
    ] as T;
  }
  return { ...previewState, now: nowSecs() } as T;
}
