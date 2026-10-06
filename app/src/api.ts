// Calls into the Rust side of the app. Outside Tauri (a plain browser, used to
// preview the interface), it answers with sample data instead.

import { invoke } from "@tauri-apps/api/core";

export type UsageWindow = { used: number; resetsAt: number | null };

export type Account = {
  alias: string;
  email: string | null;
  active: boolean;
  fiveHour: UsageWindow | null;
  sevenDay: UsageWindow | null;
  observedAt: number | null;
  limited: boolean;
  limitedUntil: number | null;
};

export type Overview = {
  accounts: Account[];
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
  lastActivity: number;
};

export type ResumeTarget = "terminal" | "desktop";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return inTauri ? invoke<T>(command, args) : preview<T>(command, args);
}

export const api = {
  overview: () => call<Overview>("overview"),
  switchAccount: (alias: string) => call<Overview>("switch_account", { alias }),
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

let previewState: Overview = {
  accounts: [
    {
      alias: "first",
      email: "first@example.com",
      active: true,
      fiveHour: { used: 20, resetsAt: nowSecs() + 3 * 3600 },
      sevenDay: { used: 30, resetsAt: nowSecs() + 6 * 86400 },
      observedAt: nowSecs() - 30,
      limited: false,
      limitedUntil: null,
    },
    {
      alias: "hello",
      email: "hello@example.com",
      active: false,
      fiveHour: null,
      sevenDay: null,
      observedAt: null,
      limited: false,
      limitedUntil: null,
    },
    {
      alias: "trois",
      email: "trois@example.com",
      active: false,
      fiveHour: { used: 0, resetsAt: null },
      sevenDay: { used: 100, resetsAt: nowSecs() + 5 * 86400 },
      observedAt: nowSecs() - 3600,
      limited: true,
      limitedUntil: nowSecs() + 5 * 86400,
    },
  ],
  best: "hello",
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
  if (command === "set_statusline") previewState = { ...previewState, statusline: Boolean(args?.enabled) };
  if (command === "set_shell") previewState = { ...previewState, shell: Boolean(args?.enabled) };
  if (command === "suggest_alias") {
    const local = String(args?.email ?? "").split("@")[0].toLowerCase().replace(/[^a-z0-9_]+/g, "-").replace(/^-+|-+$/g, "");
    return (local || "account") as T;
  }
  if (command === "conversations") {
    return [
      { id: "s1", title: "Multi-account switching and shared conversations", cwd: "~/GitHub", lastActivity: nowSecs() - 20 },
      { id: "s2", title: "Mobile notifications and widget", cwd: "~/GitHub/mobile", lastActivity: nowSecs() - 480 },
      { id: "s3", title: "Audit and framework migration", cwd: "~/GitHub/devops", lastActivity: nowSecs() - 1500 },
    ] as T;
  }
  return { ...previewState, now: nowSecs() } as T;
}
