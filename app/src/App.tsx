import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";

import { api, type Account, type AccountSignIn, type AccountStats, type Conversation, type Overview, type PaletteColor, type ProjectUsage, type ResumeTarget } from "./api";
import { age, dayLabels, inTime, isLive, margin, newlySignedIn, projectShares, remaining, span, time } from "./format";
import { isMessageKey, t, type MessageKey } from "./i18n";
import { RelayHome, TabBar, type UpdateState } from "./RelayHome";

type View = "accounts" | "settings" | "add" | "conversations";

/** What Claude Code last said about each account's sign-in. */
type SignIns = { checkedAt: number | null; list: AccountSignIn[]; checking: boolean };

/** The tint of Relais when it does not follow the active account. */
const DEFAULT_ACCENT = "#F55D1A";
/** Conversations whose account is unknown (started before Relais measured them). */
const NEUTRAL = "#93A29B";
const FOLLOW_THEME_KEY = "relais.followTheme";
const FULL_VIEW_KEY = "relais.fullView";
const SHOWN_CONVERSATIONS = 5;
const SHOWN_PROJECTS = 5;
const FETCHED_CONVERSATIONS = 12;
const SIGN_IN_CHECK_EVERY_MS = 10 * 60_000;
const UPDATE_CHECK_EVERY_MS = 6 * 3600_000;
/** A sign-in Relais opened is checked this often, for ten minutes at most. */
const SIGN_IN_POLL_MS = 4_000;
const SIGN_IN_WATCH_MS = 10 * 60_000;
/** While an account is signed out, showing the panel checks again, at most this often. */
const SIGN_IN_RECHECK_SECS = 15;
const PALETTE_KEYS = ["sage", "ocean", "violet", "coral", "amber", "rose", "lime", "sky"];

const nowSecs = () => Math.floor(Date.now() / 1000);

/** Inline custom property for the `.tinted` elements: everything they paint derives from `--c`. */
const tint = (color: string) => ({ "--c": color }) as CSSProperties;

const initial = (alias: string) => alias.charAt(0).toUpperCase();

function readFollowTheme(): boolean {
  try {
    return localStorage.getItem(FOLLOW_THEME_KEY) !== "off";
  } catch {
    return true;
  }
}

function writeFollowTheme(on: boolean) {
  try {
    localStorage.setItem(FOLLOW_THEME_KEY, on ? "on" : "off");
  } catch {
    // Storage refused: the choice holds until Relais quits.
  }
}

function readFullView(): boolean {
  try {
    return localStorage.getItem(FULL_VIEW_KEY) === "on";
  } catch {
    return false;
  }
}

function writeFullView(on: boolean) {
  try {
    localStorage.setItem(FULL_VIEW_KEY, on ? "on" : "off");
  } catch {
    // Storage refused: the choice holds until Relais quits.
  }
}

/** A command's error, translated when it comes as a message code. */
function describeError(error: unknown): string {
  const text = String(error);
  return isMessageKey(text) ? t(text) : text;
}

function paletteName(color: PaletteColor): string {
  return PALETTE_KEYS.includes(color.name) ? t(color.name as MessageKey) : color.name;
}

export function App() {
  const [view, setView] = useState<View>("accounts");
  const [full, setFull] = useState(readFullView);
  // An account whose switch was asked from the full view: the sheet confirms it.
  const [pending, setPending] = useState<string | null>(null);
  const [focus, setFocus] = useState<string | null>(null);
  const [overview, setOverview] = useState<Overview | null>(null);
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [stats, setStats] = useState<AccountStats[]>([]);
  const [signIns, setSignIns] = useState<SignIns | null>(null);
  const signInsRef = useRef<SignIns | null>(null);
  // Sign-ins Relais opened and waits to see done, with when each started (ms).
  const [waiting, setWaiting] = useState<Map<string, number>>(new Map());
  const waitingRef = useRef(waiting);
  waitingRef.current = waiting;
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(nowSecs);
  const [followTheme, setFollowTheme] = useState(readFollowTheme);
  const [update, setUpdate] = useState<UpdateState>(null);
  const [projects, setProjects] = useState<ProjectUsage[]>([]);
  const [upToDate, setUpToDate] = useState<boolean | null>(null);

  // A newer version is offered, never installed behind the user's back. Fail
  // open: without a network or GitHub, Relais simply stays as it is.
  const checkUpdate = useCallback(async () => {
    try {
      const available = await api.checkUpdate();
      setUpdate((current) => (current?.installing ? current : available ? { available, installing: false } : null));
      setUpToDate(available == null);
    } catch {
      setUpToDate(null);
    }
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [next, recent] = await Promise.all([api.overview(), api.conversations(FETCHED_CONVERSATIONS)]);
      setOverview(next);
      setConversations(recent);
      setNow(nowSecs());
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const keepSignIns = (next: SignIns | null) => {
    signInsRef.current = next;
    setSignIns(next);
  };

  // An account signed in again is measured at once: until then, its limits
  // and reset times are the old ones, or unknown.
  const measureAfterSignIn = useCallback(async (aliases: string[]) => {
    setWaiting((current) => {
      const next = new Map(current);
      aliases.forEach((alias) => next.delete(alias));
      return next;
    });
    try {
      setOverview(await api.refreshUsage(false));
    } catch {
      // The figures already shown stay; the next round asks again.
    }
  }, []);

  // Each check starts Claude Code once per account, a quarter of a second
  // each: at launch, every ten minutes, when the accounts page opens on stale
  // results, when the panel shows while an account is signed out, and every
  // few seconds for a sign-in Relais opened (`only` then checks that account).
  const checkSignIns = useCallback(
    async (only?: string) => {
      const previous = signInsRef.current;
      if (!only) keepSignIns({ checkedAt: previous?.checkedAt ?? null, list: previous?.list ?? [], checking: true });
      try {
        const list = await api.accountSignIns(only);
        const merged = only ? [...(signInsRef.current?.list ?? []).filter((s) => s.alias !== only), ...list] : list;
        keepSignIns({ checkedAt: only ? (signInsRef.current?.checkedAt ?? null) : nowSecs(), list: merged, checking: signInsRef.current?.checking === true && !!only });
        const back = new Set([...newlySignedIn(previous?.list ?? [], merged), ...merged.filter((s) => s.signedIn === true && waitingRef.current.has(s.alias)).map((s) => s.alias)]);
        if (back.size > 0) void measureAfterSignIn([...back]);
      } catch {
        if (!only) keepSignIns(previous ? { ...previous, checking: false } : null);
      }
    },
    [measureAfterSignIn],
  );

  // A sign-in Relais opened is checked every few seconds until it is done.
  useEffect(() => {
    if (waiting.size === 0) return;
    const timer = window.setInterval(() => {
      const now = Date.now();
      const expired = [...waitingRef.current].filter(([, started]) => now - started > SIGN_IN_WATCH_MS).map(([alias]) => alias);
      if (expired.length > 0) {
        setWaiting((current) => {
          const next = new Map(current);
          expired.forEach((alias) => next.delete(alias));
          return next;
        });
      }
      for (const [alias, started] of waitingRef.current) {
        if (now - started <= SIGN_IN_WATCH_MS) void checkSignIns(alias);
      }
    }, SIGN_IN_POLL_MS);
    return () => window.clearInterval(timer);
  }, [waiting.size, checkSignIns]);

  // The Rust side measures the accounts in the background; this asks it now.
  const [refreshing, setRefreshing] = useState(false);
  const refreshUsage = useCallback(async (force = false) => {
    if (force) setRefreshing(true);
    try {
      setOverview(await api.refreshUsage(force));
      if (force) setStats(await api.accountStats());
    } catch {
      // The figures already shown stay; the next round tries again.
    } finally {
      if (force) setRefreshing(false);
    }
  }, []);

  const loadStats = useCallback(async () => {
    try {
      setStats(await api.accountStats());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    void checkSignIns();
    void checkUpdate();
    const timer = window.setInterval(() => void refresh(), 15_000);
    const updateTimer = window.setInterval(() => void checkUpdate(), UPDATE_CHECK_EVERY_MS);
    const signInTimer = window.setInterval(() => void checkSignIns(), SIGN_IN_CHECK_EVERY_MS);
    const clock = window.setInterval(() => setNow(nowSecs()), 10_000);
    const onFocus = () => {
      void refresh();
      const current = signInsRef.current;
      const someoneOut = current?.list.some((s) => s.signedIn === false) ?? false;
      const stale = current?.checkedAt == null || nowSecs() - current.checkedAt > SIGN_IN_RECHECK_SECS;
      if ((someoneOut || waitingRef.current.size > 0) && stale && !current?.checking) void checkSignIns();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") void api.hide();
    };
    window.addEventListener("focus", onFocus);
    window.addEventListener("keydown", onKey);
    return () => {
      window.clearInterval(timer);
      window.clearInterval(updateTimer);
      window.clearInterval(signInTimer);
      window.clearInterval(clock);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("keydown", onKey);
    };
  }, [refresh, checkSignIns, checkUpdate]);

  // Opens the sign-in in a terminal, then watches for it to be done.
  const signInAndWatch = async (alias: string) => {
    try {
      await api.signInAgain(alias);
      setError(null);
      setWaiting((current) => new Map(current).set(alias, Date.now()));
    } catch (e) {
      setError(describeError(e));
    }
  };

  const run = async (action: () => Promise<Overview | void>) => {
    try {
      const next = await action();
      if (next) setOverview(next);
      setNow(nowSecs());
      setError(null);
    } catch (e) {
      setError(describeError(e));
    }
  };

  const installUpdate = async () => {
    if (!update) return;
    setUpdate({ ...update, installing: true });
    try {
      await api.installUpdate();
    } catch (e) {
      setUpdate({ ...update, installing: false });
      setError(`${t("updateFailed")} (${String(e)})`);
    }
  };

  const showFull = (on: boolean) => {
    setFull(on);
    writeFullView(on);
  };

  const openAccount = (alias: string) => {
    setFocus(alias);
    showFull(true);
  };

  useEffect(() => {
    if (view === "conversations") void api.projectUsage().then(setProjects).catch(() => setProjects([]));
  }, [view]);

  // A notification's button asks to switch: the confirmation takes it from there.
  useEffect(() => {
    if (!isTauri()) return;
    const stop = listen<string>("relais-switch-to", ({ payload }) => {
      setView("accounts");
      setPending(payload);
    });
    return () => void stop.then((unlisten) => unlisten());
  }, []);

  // The full view shows each account's history and sign-ins.
  useEffect(() => {
    if (view !== "accounts" || !full) return;
    void loadStats();
    const current = signInsRef.current;
    const stale = !current?.checkedAt || nowSecs() - current.checkedAt > 120;
    if (stale && !current?.checking) void checkSignIns();
  }, [view, full, loadStats, checkSignIns]);

  const active = overview?.accounts.find((a) => a.active) ?? null;
  const accent = followTheme && active ? active.color : DEFAULT_ACCENT;

  return (
    <main className="panel" style={{ "--accent": accent } as CSSProperties}>
      {view === "conversations" && (
        <div className="page">
          <div className="page-head">
            <h1 className="page-title">{t("relayMenuConversations")}</h1>
          </div>
          <Projects usage={projects} />
          <Sessions conversations={conversations} accounts={new Map((overview?.accounts ?? []).map((a) => [a.alias, a]))} now={now} onResume={(id, target) => run(() => api.resume(id, target))} />
        </div>
      )}
      {view === "accounts" && (
        <RelayHome
          overview={overview}
          signIns={signIns?.checkedAt != null ? signIns.list : null}
          now={now}
          full={full}
          onFull={showFull}
          pending={pending}
          onPendingSeen={() => setPending(null)}
          onOverview={setOverview}
          refreshing={refreshing}
          onRefresh={() => void refreshUsage(true)}
          onOpenAccount={openAccount}
          onSignIn={(alias) => void signInAndWatch(alias)}
          onAllowMeasure={() => run(() => api.allowMeasure())}
          update={update}
          onInstallUpdate={() => void installUpdate()}
          describeError={describeError}
        >
          {overview && (
            <AccountsPage
              overview={overview}
              stats={stats}
              signIns={signIns}
              focus={focus}
              now={now}
              onSwitch={(alias) => run(() => api.switchAccount(alias))}
              onColor={(alias, hex) => run(() => api.setColor(alias, hex))}
              onSignInAgain={(alias) => void signInAndWatch(alias)}
              onRemove={(alias) => run(() => api.removeAccount(alias)).then(() => loadStats())}
              onSwitchDesktop={setPending}
              onAdd={() => setView("add")}
            />
          )}
        </RelayHome>
      )}
      {view === "settings" && overview && (
        <Settings
          overview={overview}
          followTheme={followTheme}
          onFollowTheme={(on) => {
            setFollowTheme(on);
            writeFollowTheme(on);
          }}
          onStatusline={(enabled) => run(() => api.setStatusline(enabled))}
          onShell={(enabled) => run(() => api.setShell(enabled))}
          onAlerts={(enabled) => run(() => api.setAlerts(enabled))}
          onShortcut={(enabled) => run(() => api.setShortcut(enabled))}
          upToDate={upToDate}
          onCheckUpdate={checkUpdate}
          onBringBack={() => run(() => api.bringBack())}
          onAddFolder={(alias) => run(() => api.addFolder(alias))}
          onRemoveFolder={(folder) => run(() => api.removeFolder(folder))}
        />
      )}
      {view === "add" && (
        <AddAccount
          onBack={() => {
            setView("accounts");
            void refresh();
          }}
          onRefresh={refresh}
          onSignIn={(email) => api.addAccount(email)}
        />
      )}
      {error && (
        <p className="error" role="alert">
          {t("error", { message: error })}
        </p>
      )}
      {view !== "add" && <TabBar tab={view} onSelect={setView} />}
    </main>
  );
}

// ---- Home ------------------------------------------------------------------

/** Where the week's consumption went. */
function Projects({ usage }: { usage: ProjectUsage[] }) {
  const shares = projectShares(usage, SHOWN_PROJECTS);
  if (shares.length === 0) return null;
  return (
    <section className="card projects" aria-labelledby="projects-title">
      <h2 id="projects-title" className="section-title">
        {t("projectsTitle")}
      </h2>
      <ul>
        {shares.map(({ project, share }) => (
          <li key={project}>
            <span className="project-name">{project}</span>
            <span className="project-bar">
              <span style={{ width: `${Math.max(share, 2)}%` }} />
            </span>
            <span className="muted tiny">{share} %</span>
          </li>
        ))}
      </ul>
    </section>
  );
}

function Sessions({ conversations, accounts, now, onResume }: { conversations: Conversation[]; accounts: Map<string, Account>; now: number; onResume: (id: string, target: ResumeTarget) => void }) {
  const live = conversations.filter((c) => isLive(c.lastActivity, now)).length;
  const shown = conversations.slice(0, Math.max(SHOWN_CONVERSATIONS, live));
  return (
    <section aria-labelledby="sessions-title">
      <div className="section-head">
        <h2 id="sessions-title" className="section-title">
          {live > 0 && <span className="dot live pulse" />}
          {live > 0 ? t("liveConversations") : t("latest")}
        </h2>
        {live > 0 && <span className="muted small">{live === 1 ? t("liveCountOne") : t("liveCount", { n: live })}</span>}
      </div>
      {shown.length === 0 && <p className="muted small pad">{t("noConversation")}</p>}
      <ul className="sessions">
        {shown.map((c) => (
          <SessionRow key={c.id} conversation={c} account={c.account ? (accounts.get(c.account) ?? null) : null} now={now} onResume={onResume} />
        ))}
      </ul>
    </section>
  );
}

function SessionRow({ conversation: c, account, now, onResume }: { conversation: Conversation; account: Account | null; now: number; onResume: (id: string, target: ResumeTarget) => void }) {
  const live = isLive(c.lastActivity, now);
  const meta = [account?.alias ?? t("unknownAccount"), c.desktop ? t("whereDesktop") : t("whereTerminal"), c.project].filter(Boolean).join(" · ");
  return (
    <li className="session tinted" style={tint(account?.color ?? NEUTRAL)}>
      <div className="session-main">
        <div className="session-top">
          <span className="session-title">{c.title ?? c.id}</span>
          <span className={`session-state ${live ? "is-live" : ""}`}>{live ? t("statusRunning") : t("statusIdle")}</span>
        </div>
        <div className="session-bottom">
          <span className="session-meta">
            <span className="session-dot" />
            {meta}
          </span>
          <span className="session-time">{live ? (c.startedAt != null ? span(c.startedAt, now) : "") : t("idleFor", { d: span(c.lastActivity, now) })}</span>
        </div>
      </div>
      <div className="session-actions">
        <button className="icon-button" aria-label={t("resumeDesktop")} title={t("resumeDesktop")} onClick={() => onResume(c.id, "desktop")}>
          <MonitorIcon />
        </button>
        <button className="icon-button" aria-label={t("resumeTerminal")} title={t("resumeTerminal")} onClick={() => onResume(c.id, "terminal")}>
          <PromptIcon />
        </button>
      </div>
    </li>
  );
}

// ---- Small pieces ----------------------------------------------------------

function Avatar({ color, label, size }: { color: string; label: string; size: number }) {
  return (
    <span className="avatar tinted" style={{ ...tint(color), width: size, height: size, fontSize: Math.round(size * 0.46) }}>
      {label}
    </span>
  );
}

function MonitorIcon({ size = 15 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <rect x="3" y="4" width="18" height="12" rx="2" />
      <path d="M8 20h8" />
      <path d="M12 16v4" />
    </svg>
  );
}

function PromptIcon({ size = 15 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M4 17l6-5-6-5" />
      <path d="M12 19h8" />
    </svg>
  );
}

// ---- Accounts --------------------------------------------------------------

function AccountsPage(props: {
  overview: Overview;
  stats: AccountStats[];
  signIns: SignIns | null;
  focus: string | null;
  now: number;
  onSwitch: (alias: string) => void;
  onColor: (alias: string, hex: string) => void;
  onSignInAgain: (alias: string) => void;
  onRemove: (alias: string) => void;
  onSwitchDesktop: (alias: string) => void;
  onAdd: () => void;
}) {
  const { overview, signIns, now } = props;
  const accounts = [...overview.accounts].sort((a, b) => Number(b.active) - Number(a.active));
  const connected = signIns?.checkedAt ? signIns.list.filter((s) => s.signedIn).length : null;
  const plans = [...new Set((signIns?.list ?? []).map((s) => s.plan).filter((p): p is string => !!p))];
  const heading =
    connected == null
      ? t("accountsCount", { n: accounts.length })
      : connected === accounts.length
        ? accounts.length === 1
          ? t("accountsConnectedOne")
          : t("accountsConnected", { n: accounts.length })
        : t("accountsConnectedPartial", { n: connected, total: accounts.length });

  useEffect(() => {
    if (!props.focus) return;
    document.getElementById(`account-${props.focus}`)?.scrollIntoView({ block: "start" });
  }, [props.focus]);

  return (
    <div className="page page-full">
      <section className="card summary">
        <div className="summary-head">
          <span className="strong">{heading}</span>
          <span className="muted tiny">
            {signIns?.checking ? t("checking") : signIns?.checkedAt ? [plans.length === 1 ? t("plan", { plan: planName(plans[0]) }) : null, t("checkedAgo", { age: age(signIns.checkedAt, now) })].filter(Boolean).join(" · ") : ""}
          </span>
        </div>
        <span className="muted tiny">{t("reserveTitle")}</span>
        <div className="reserve" aria-hidden="true">
          {accounts.map((a) => (
            <span key={a.alias} className="tinted" style={{ ...tint(a.color), flexGrow: Math.max(margin(a) ?? 0, 2) }} />
          ))}
        </div>
        <div className="reserve-legend">
          {accounts.map((a) => (
            <span key={a.alias} className="tinted" style={tint(a.color)}>
              <strong className="tinted-text">{a.alias}</strong> {a.limited ? (a.limitedUntil != null ? t("backIn", { in: inTime(a.limitedUntil, now) }) : t("moodResting")) : `${margin(a) ?? "—"} %`}
              {!a.limited && weekReset(a, now) ? ` · ${weekReset(a, now)}` : ""}
            </span>
          ))}
        </div>
      </section>

      {accounts.map((account) => (
        <AccountCard
          key={account.alias}
          account={account}
          stats={props.stats.find((s) => s.alias === account.alias) ?? null}
          signIn={signIns?.list.find((s) => s.alias === account.alias) ?? null}
          checking={!!signIns?.checking}
          palette={overview.palette}
          now={now}
          onSwitch={props.onSwitch}
          onColor={props.onColor}
          onSignInAgain={props.onSignInAgain}
          onRemove={props.onRemove}
          desktopOwner={overview.desktopOwner}
          onSwitchDesktop={props.onSwitchDesktop}
        />
      ))}

      <button className="button add-button" onClick={props.onAdd}>
        {t("addAccountButton")}
      </button>
    </div>
  );
}

function AccountCard(props: {
  account: Account;
  stats: AccountStats | null;
  signIn: AccountSignIn | null;
  checking: boolean;
  palette: PaletteColor[];
  now: number;
  onSwitch: (alias: string) => void;
  onColor: (alias: string, hex: string) => void;
  onSignInAgain: (alias: string) => void;
  onRemove: (alias: string) => void;
  desktopOwner: string | null;
  onSwitchDesktop: (alias: string) => void;
}) {
  const { account, stats, signIn, now } = props;
  const [confirming, setConfirming] = useState(false);
  const desktopHere = props.desktopOwner === account.alias;
  const five = account.fiveHour;
  const week = account.sevenDay;
  const signedOut = signIn?.signedIn === false;
  const hasDays = !!stats && stats.days.some((d) => d.used != null);
  const maxDay = Math.max(...(stats?.days ?? []).map((d) => d.used ?? 0), 1);
  const needsLegend = !!stats && stats.days.some((d) => d.used == null || d.limited);
  const labels = dayLabels(now, stats?.days.length || 7);
  const same = (a: string, b: string) => a.toUpperCase() === b.toUpperCase();
  const today = stats?.conversationsToday ?? 0;

  return (
    <article id={`account-${account.alias}`} className={`card account-card tinted ${account.active ? "is-active" : ""}`} style={tint(account.color)}>
      <div className="account-head">
        <Avatar color={account.color} label={initial(account.alias)} size={30} />
        <div className="account-names">
          <div className="account-alias">{account.alias}</div>
          <div className="muted tiny ellipsis">{account.email ?? signIn?.email ?? ""}</div>
        </div>
        {account.observedAt != null && <span className="muted tiny account-measured">{t("relayMeasured", { age: age(account.observedAt, now) })}</span>}
      </div>

      <div className="where">
        <span className="muted tiny">{t("whereItWorks")}</span>
        <div className="where-row">
          <PromptIcon size={14} />
          <div className="where-text">
            <div className="strong small">{t("terminalRow")}</div>
            <div className="muted tiny">{t("terminalDetail")}</div>
          </div>
          {account.active ? (
            <span className="mood">{t("terminalActive")}</span>
          ) : (
            <button className="relay-button" onClick={() => props.onSwitch(account.alias)}>
              {t("useHere")}
            </button>
          )}
        </div>
        <div className="where-row">
          <MonitorIcon size={14} />
          <div className="where-text">
            <div className="strong small">{t("desktopRow")}</div>
            <div className="muted tiny">{desktopHere ? t("desktopDetailHere") : account.desktopFolder ? t("desktopDetailReady") : t("desktopDetailNone")}</div>
          </div>
          {desktopHere ? (
            <span className="mood">{t("desktopHere")}</span>
          ) : (
            <button className="button where-button" onClick={() => props.onSwitchDesktop(account.alias)}>
              {t("relaySwitch")}
            </button>
          )}
        </div>
      </div>

      <div className="badges">
        {signIn == null ? (
          <span className="badge">{props.checking ? t("checking") : t("signInUnknown")}</span>
        ) : signIn.signedIn ? (
          <span className="badge badge-ok">
            <span className="badge-dot" />
            {t("signedIn")}
          </span>
        ) : (
          <span className="badge badge-off">
            <span className="badge-dot" />
            {t("signedOut")}
          </span>
        )}
        {signIn?.plan && <span className="badge">{planName(signIn.plan)}</span>}
      </div>

      <div className="windows">
        {five && (
          <div className="window-box">
            <div className="muted tiny">{t("fiveHourLabel")}</div>
            <div className="window-value">{Math.round(five.used)} %</div>
            <div className="tiny window-note">{five.resetsAt != null && five.resetsAt > now ? t("resetsIn", { in: inTime(five.resetsAt, now) }) : t("nothingUsed")}</div>
          </div>
        )}
        <div className="window-box">
          <div className="muted tiny">{t("weekLabel")}</div>
          <div className="window-value">{week ? `${Math.round(week.used)} %` : "—"}</div>
          <div className="tiny window-note">
            {account.limited
              ? account.limitedUntil != null
                ? t("backIn", { in: inTime(account.limitedUntil, now) })
                : t("restingUnknownTime")
              : week?.resetsAt != null && week.resetsAt > now
                ? t("resetsInAt", { in: inTime(week.resetsAt, now), time: time(week.resetsAt, now) })
                : t("weekResetUnknownLong")}
          </div>
        </div>
      </div>

      {stats && (hasDays || stats.weeks.some((w) => w != null)) ? (
        <>
          <div className="chart">
            <div className="chart-head">
              <span className="muted tiny">{t("weekByDay")}</span>
              <span className="muted tiny">{t("weekByDayUnit")}</span>
            </div>
            <div className="bars" role="img" aria-label={t("weekByDay")}>
              {stats.days.map((day, index) => {
                const used = day.used ?? 0;
                const title = day.used == null ? t("dayNoReading") : `${Math.round(used)} %${day.limited ? ` · ${t("dayLimited")}` : ""}`;
                const kind = day.used == null ? "is-unknown" : day.limited ? "is-limited" : used > 0 ? "" : "is-empty";
                return (
                  <div key={index} className="bar-col" title={title}>
                    <span className="bar-value">{used >= 1 ? Math.round(used) : ""}</span>
                    <span className={`bar ${kind}`} style={{ height: `${used > 0 ? Math.max(4, Math.round((used / maxDay) * 34)) : day.limited ? 6 : 3}px` }} />
                    <span className="bar-label">{labels[index]}</span>
                  </div>
                );
              })}
            </div>
            {needsLegend && <div className="muted tiny">{t("chartLegend")}</div>}
          </div>
          <div className="stats-row">
            <div className="weeks">
              <span className="muted tiny">{t("lastWeeks")}</span>
              <div className="week-bars" role="img" aria-label={t("lastWeeks")}>
                {stats.weeks.map((value, index) => (
                  <span key={index} className={`week-bar ${value != null && value >= 100 ? "is-full" : ""}`} title={value == null ? "—" : `${Math.round(value)} %`} style={{ height: `${value == null ? 3 : Math.max(3, Math.round((value / 100) * 22))}px` }} />
                ))}
              </div>
            </div>
            <div className="counts">
              <div>{today <= 1 ? t("conversationsTodayOne", { n: today }) : t("conversationsToday", { n: today })}</div>
              <div className="muted">
                {t("conversationsWeek", { n: stats.conversationsWeek })}
                {stats.lastSeen != null ? ` · ${t("lastActivity", { age: age(stats.lastSeen, now) })}` : ""}
              </div>
            </div>
          </div>
        </>
      ) : (
        <p className="muted tiny">{t("noStatsYet")}</p>
      )}

      <div className="swatches small-swatches" role="group" aria-label={t("colorsOf", { alias: account.alias })}>
        {props.palette.map((color) => {
          const selected = same(color.hex, account.color);
          return (
            <button
              key={color.hex}
              className={`swatch ${selected ? "is-selected" : ""}`}
              style={{ backgroundColor: color.hex }}
              aria-label={t("colorFor", { color: paletteName(color), alias: account.alias })}
              aria-pressed={selected}
              onClick={() => props.onColor(account.alias, color.hex)}
            />
          );
        })}
      </div>

      {confirming ? (
        <div className="confirm">
          <span className="small">{t("removeConfirm", { alias: account.alias })}</span>
          <div className="actions">
            <button className="button" onClick={() => setConfirming(false)}>
              {t("cancel")}
            </button>
            <button
              className="button button-danger"
              onClick={() => {
                setConfirming(false);
                props.onRemove(account.alias);
              }}
            >
              {t("removeYes")}
            </button>
          </div>
        </div>
      ) : (
        <div className="actions">
          <button className={`button ${signedOut ? "button-accent" : ""}`} onClick={() => props.onSignInAgain(account.alias)}>
            {t("signInAgain")}
          </button>
          <button className="button button-danger" onClick={() => setConfirming(true)}>
            {t("remove")}
          </button>
        </div>
      )}
    </article>
  );
}

/** `↻ 1 j 23 h`: how long until the account's week starts over, when known. */
function weekReset(account: Account, now: number): string | null {
  const at = account.limited && account.limitedUntil != null ? account.limitedUntil : account.sevenDay?.resetsAt;
  return at != null && at > now ? t("weekResetIn", { in: remaining(at, now) }) : null;
}

function planName(plan: string): string {
  return plan.charAt(0).toUpperCase() + plan.slice(1);
}


// ---- Settings and adding an account ----------------------------------------

function Settings(props: { overview: Overview; followTheme: boolean; onFollowTheme: (on: boolean) => void; onStatusline: (enabled: boolean) => void; onShell: (enabled: boolean) => void; onAlerts: (enabled: boolean) => void; onShortcut: (enabled: boolean) => void; upToDate: boolean | null; onCheckUpdate: () => Promise<void>; onBringBack: () => Promise<void>; onAddFolder: (alias: string) => void; onRemoveFolder: (folder: string) => void }) {
  const { overview } = props;
  const [folderAccount, setFolderAccount] = useState(overview.accounts.find((a) => a.active)?.alias ?? overview.accounts[0]?.alias ?? "");
  const [checking, setChecking] = useState(false);
  const [bringing, setBringing] = useState<"idle" | "confirm" | "working">("idle");
  const bringBack = async () => {
    if (bringing === "idle") return setBringing("confirm");
    setBringing("working");
    await props.onBringBack();
    setBringing("idle");
  };
  const check = async () => {
    setChecking(true);
    await props.onCheckUpdate();
    setChecking(false);
  };
  const rows = [
    { id: "statusline", title: t("statuslineTitle"), detail: t("statuslineDetail"), on: overview.statusline, set: props.onStatusline },
    { id: "shell", title: t("shellTitle"), detail: t("shellDetail"), on: overview.shell, set: props.onShell },
    { id: "alerts", title: t("alertsTitle"), detail: t("alertsDetail"), on: overview.alerts, set: props.onAlerts },
    { id: "shortcut", title: t("shortcutTitle"), detail: t("shortcutDetail"), on: overview.shortcut, set: props.onShortcut },
  ];
  return (
    <div className="page">
      <div className="page-head">
        <h1 className="page-title">{t("settingsTitle")}</h1>
      </div>
      <div className="card list">
        {rows.map((row) => (
          <div key={row.id} className="list-row">
            <div className="list-text">
              <label htmlFor={row.id} className="strong">
                {row.title}
              </label>
              <div className="muted small">{row.detail}</div>
            </div>
            <input id={row.id} type="checkbox" className="check" checked={row.on} disabled={!overview.cliFound} onChange={(e) => row.set(e.target.checked)} />
          </div>
        ))}
      </div>
      {!overview.cliFound && <p className="muted small">{t("cliMissing")}</p>}
      <div className="card list">
        <div className="list-row">
          <div className="list-text">
            <label htmlFor="follow-theme" className="strong">
              {t("followTheme")}
            </label>
            <div className="muted small">{t("followThemeDetail")}</div>
          </div>
          <input id="follow-theme" type="checkbox" className="check" checked={props.followTheme} onChange={(e) => props.onFollowTheme(e.target.checked)} />
        </div>
      </div>
      <div className="card list">
        <div className="list-row">
          <div className="list-text">
            <div className="strong">{t("foldersTitle")}</div>
            <div className="muted small">{t("foldersDetail")}</div>
          </div>
        </div>
        {Object.entries(overview.folders).map(([folder, alias]) => (
          <div key={folder} className="list-row">
            <div className="list-text">
              <div className="small ellipsis">{folder}</div>
              <div className="muted tiny">{alias}</div>
            </div>
            <button className="button" onClick={() => props.onRemoveFolder(folder)}>
              {t("folderRemove")}
            </button>
          </div>
        ))}
        <div className="list-row">
          <select className="folder-account" aria-label={t("folderAccount")} value={folderAccount} onChange={(e) => setFolderAccount(e.target.value)}>
            {overview.accounts.map((account) => (
              <option key={account.alias} value={account.alias}>
                {account.alias}
              </option>
            ))}
          </select>
          <button className="button" disabled={!folderAccount} onClick={() => props.onAddFolder(folderAccount)}>
            {t("folderAdd")}
          </button>
        </div>
      </div>
      {overview.setAside > 0 && (
        <div className="card list">
          <div className="list-row">
            <div className="list-text">
              <div className="strong">{overview.setAside === 1 ? t("setAsideOne") : t("setAsideMany", { n: overview.setAside })}</div>
              <div className="muted small">{t("setAsideDetail")}</div>
            </div>
            <button className="button" disabled={bringing === "working"} onClick={() => void bringBack()}>
              {bringing === "working" ? t("setAsideWorking") : bringing === "confirm" ? t("setAsideConfirm") : t("setAsideBring")}
            </button>
          </div>
        </div>
      )}
      <div className="card list">
        <div className="list-row">
          <div className="list-text">
            <div className="strong">{t("versionTitle", { version: overview.version })}</div>
            {props.upToDate === true && <div className="muted small">{t("updateNone")}</div>}
            {props.upToDate === false && <div className="muted small">{t("updateSeeAccounts")}</div>}
          </div>
          <button className="button" disabled={checking} onClick={() => void check()}>
            {checking ? t("updateChecking") : t("updateCheck")}
          </button>
        </div>
      </div>
      <p className="muted small">{t("neverAlone")}</p>
      <div className="grow" />
      <button className="button" onClick={() => void api.quit()}>
        {t("quit")}
      </button>
      <p className="muted tiny">{t("footer")}</p>
    </div>
  );
}

function AddAccount(props: { onBack: () => void; onRefresh: () => Promise<void>; onSignIn: (email: string) => Promise<void> }) {
  const [email, setEmail] = useState("");
  const [alias, setAlias] = useState("");
  const [state, setState] = useState<"idle" | "waiting" | "done">("idle");
  const valid = /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email.trim());
  const { onRefresh } = props;

  useEffect(() => {
    if (!valid) {
      setAlias("");
      return;
    }
    let cancelled = false;
    api
      .suggestAlias(email.trim())
      .then((name) => !cancelled && setAlias(name))
      .catch(() => !cancelled && setAlias(""));
    return () => {
      cancelled = true;
    };
  }, [email, valid]);

  useEffect(() => {
    if (state !== "waiting") return;
    const wanted = email.trim().toLowerCase();
    const timer = window.setInterval(async () => {
      const next = await api.overview().catch(() => null);
      if (next?.accounts.some((a) => (a.email ?? "").toLowerCase() === wanted)) {
        setState("done");
        void onRefresh();
      }
    }, 3000);
    return () => window.clearInterval(timer);
  }, [state, email, onRefresh]);

  return (
    <div className="page">
      <div className="page-head">
        <h1 className="page-title">{t("addTitle")}</h1>
        <button className="button" onClick={props.onBack}>
          {t("back")}
        </button>
      </div>
      <p className="muted small">{t("addIntro")}</p>
      <div className="field">
        <label htmlFor="email" className="strong small">
          {t("aliasLabel")}
        </label>
        <input
          id="email"
          type="text"
          inputMode="email"
          data-1p-ignore="true"
          data-lpignore="true"
          className="input"
          value={email}
          placeholder={t("emailPlaceholder")}
          autoComplete="off"
          spellCheck={false}
          onChange={(e) => {
            setEmail(e.target.value);
            setState("idle");
          }}
        />
        <span className="muted tiny">{alias ? t("aliasHint", { alias }) : " "}</span>
      </div>
      <div className="card step">
        <span className="step-number">1</span>
        <span className="small grow">{t("addStepTerminal", { email: valid ? email.trim() : "…" })}</span>
        <button
          className="relay-button"
          disabled={!valid || state === "waiting"}
          onClick={async () => {
            await props.onSignIn(email.trim());
            setState("waiting");
          }}
        >
          {t("signIn")}
        </button>
      </div>
      {state === "waiting" && <p className="small">{t("addWaiting")}</p>}
      {state === "done" && <p className="small strong">{t("addDone", { alias })}</p>}
      <p className="muted small">{t("addKeepOthers")}</p>
    </div>
  );
}
