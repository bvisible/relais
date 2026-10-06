// Calls into the Rust side of the app. Outside Tauri (a plain browser, used to
// preview the interface), it answers with sample data instead.

import { invoke } from "@tauri-apps/api/core";

export type UsageWindow = { used: number; resetsAt: number | null };

export type Account = {
  alias: string;
  email: string | null;
  /** `#RRGGBB`. */
  color: string;
  active: boolean;
  fiveHour: UsageWindow | null;
  sevenDay: UsageWindow | null;
  observedAt: number | null;
  limited: boolean;
  limitedUntil: number | null;
};

export type PaletteColor = { name: string; hex: string };

export type Overview = {
  accounts: Account[];
  palette: PaletteColor[];
  best: string | null;
  statusline: boolean;
  shell: boolean;
  cliFound: boolean;
  now: number;
};

export type Conversation = {
  id: string;
  title: string | null;
  cwd: string | null;
  project: string | null;
  lastActivity: number;
  /** The account that last ran this session, in a terminal or in the desktop app. */
  account: string | null;
  desktop: boolean;
  /** Messages per five minutes over the last hour, oldest first. */
  activity: number[];
};

export type ResumeTarget = "terminal" | "desktop";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return inTauri ? invoke<T>(command, args) : preview<T>(command, args);
}

export const api = {
  overview: () => call<Overview>("overview"),
  switchAccount: (alias: string) => call<Overview>("switch_account", { alias }),
  setColor: (alias: string, color: string) => call<Overview>("set_color", { alias, color }),
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

// `?state=limited` or `?state=tiring` in the address previews those screens.
const previewMode = typeof location !== "undefined" ? new URLSearchParams(location.search).get("state") : null;

let previewState: Overview = {
  accounts: [
    {
      alias: "main",
      email: "main@example.com",
      color: "#4CC38A",
      active: true,
      fiveHour:
        previewMode === "limited"
          ? { used: 100, resetsAt: nowSecs() + 72 * 60 }
          : { used: previewMode === "tiring" ? 92 : 47, resetsAt: nowSecs() + 2 * 3600 + 14 * 60 },
      sevenDay: { used: previewMode === "limited" ? 72 : 38, resetsAt: nowSecs() + 4 * 86400 + 6 * 3600 },
      observedAt: nowSecs() - 30,
      limited: previewMode === "limited",
      limitedUntil: previewMode === "limited" ? nowSecs() + 72 * 60 : null,
    },
    {
      alias: "studio",
      email: "studio@example.com",
      color: "#A78BFA",
      active: false,
      fiveHour: { used: 0, resetsAt: null },
      sevenDay: { used: 100, resetsAt: nowSecs() + 4 * 86400 + 22 * 3600 },
      observedAt: nowSecs() - 3600,
      limited: true,
      limitedUntil: nowSecs() + 4 * 86400 + 22 * 3600,
    },
    {
      alias: "spare",
      email: "spare@example.com",
      color: "#4C8DFF",
      active: false,
      fiveHour: { used: 0, resetsAt: null },
      sevenDay: { used: 4, resetsAt: nowSecs() + 6 * 86400 },
      observedAt: nowSecs() - 600,
      limited: false,
      limitedUntil: null,
    },
  ],
  palette: PREVIEW_PALETTE,
  best: "spare",
  statusline: true,
  shell: false,
  cliFound: true,
  now: nowSecs(),
};

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
  if (command === "set_statusline") previewState = { ...previewState, statusline: Boolean(args?.enabled) };
  if (command === "set_shell") previewState = { ...previewState, shell: Boolean(args?.enabled) };
  if (command === "suggest_alias") {
    const local = String(args?.email ?? "").split("@")[0].toLowerCase().replace(/[^a-z0-9_]+/g, "-").replace(/^-+|-+$/g, "");
    return (local || "account") as T;
  }
  if (command === "conversations") {
    const now = nowSecs();
    return [
      { id: "s1", title: "Rings and colors in the menu bar app", cwd: "~/GitHub/relais", project: "relais", lastActivity: now - 12, account: "main", desktop: false, activity: [1, 2, 2, 4, 6, 5, 8, 9, 12, 11, 14, 13] },
      { id: "s2", title: "Mobile notifications and widget", cwd: "~/GitHub/mobile", project: "mobile", lastActivity: now - 100, account: "spare", desktop: true, activity: [6, 9, 7, 10, 8, 5, 6, 4, 3, 2, 1, 1] },
      { id: "s3", title: "Audit and framework migration", cwd: "~/GitHub/devops", project: "devops", lastActivity: now - 1500, account: null, desktop: false, activity: [0, 0, 3, 5, 2, 0, 0, 0, 0, 0, 0, 0] },
      { id: "s4", title: "Translations for the archive app", cwd: "~/GitHub/archive", project: "archive", lastActivity: now - 5400, account: "studio", desktop: false, activity: new Array(12).fill(0) },
      { id: "s5", title: "Nightly backup check", cwd: "~/GitHub/devops", project: "devops", lastActivity: now - 7200, account: "main", desktop: false, activity: new Array(12).fill(0) },
    ] as T;
  }
  return { ...previewState, now: nowSecs() } as T;
}
