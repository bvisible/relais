import { useCallback, useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";

import { api, type Account, type AccountSignIn, type AccountStats, type Conversation, type DesktopSwitch, type Overview, type PaletteColor, type ResumeTarget } from "./api";
import { age, dayLabels, FIVE_HOURS_SECS, inTime, isLive, margin, mood, newlySignedIn, nextReturn, orderAccounts, pace, remaining, span, time, TIRING_FROM, WEEK_SECS } from "./format";
import { isMessageKey, t, type MessageKey } from "./i18n";

type View = "home" | "settings" | "add" | "accounts";

/** What Claude Code last said about each account's sign-in. */
type SignIns = { checkedAt: number | null; list: AccountSignIn[]; checking: boolean };

/** The tint of Relais when it does not follow the active account. */
const DEFAULT_ACCENT = "#4CC38A";
/** Conversations whose account is unknown (started before Relais measured them). */
const NEUTRAL = "#93A29B";
const FOLLOW_THEME_KEY = "relais.followTheme";
const SHOWN_CONVERSATIONS = 5;
const FETCHED_CONVERSATIONS = 12;
const SIGN_IN_CHECK_EVERY_MS = 10 * 60_000;
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

/** A translated sentence with its `{slot}` value set in bold. */
function emphasize(key: MessageKey, slot: string, value: string, args: Record<string, string | number> = {}): ReactNode {
  const marker = "\u0000";
  const [before, after] = t(key, { ...args, [slot]: marker }).split(marker);
  return (
    <>
      {before}
      <strong>{value}</strong>
      {after}
    </>
  );
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
  const [view, setView] = useState<View>("home");
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
  // Accounts whose limits are asked right after they were signed in again.
  const [measuring, setMeasuring] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(nowSecs);
  const [followTheme, setFollowTheme] = useState(readFollowTheme);

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
    setMeasuring((current) => new Set([...current, ...aliases]));
    try {
      setOverview(await api.refreshUsage(false));
    } catch {
      // The figures already shown stay; the next round asks again.
    } finally {
      setMeasuring((current) => new Set([...current].filter((alias) => !aliases.includes(alias))));
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

  // Claude Code tells each account's limits and reset times; accounts the
  // status line measured lately are skipped on the Rust side.
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
    void refreshUsage();
    const timer = window.setInterval(() => void refresh(), 15_000);
    const signInTimer = window.setInterval(() => {
      void checkSignIns();
      void refreshUsage();
    }, SIGN_IN_CHECK_EVERY_MS);
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
      window.clearInterval(signInTimer);
      window.clearInterval(clock);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("keydown", onKey);
    };
  }, [refresh, checkSignIns, refreshUsage]);

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

  const switchDesktop = async (alias: string): Promise<DesktopSwitch | null> => {
    try {
      const result = await api.switchDesktop(alias);
      setOverview(result.overview);
      setError(null);
      return result;
    } catch (e) {
      setError(describeError(e));
      return null;
    }
  };

  const openAccounts = (alias: string | null) => {
    setFocus(alias);
    setView("accounts");
    void loadStats();
    void refreshUsage();
    const stale = !signIns?.checkedAt || nowSecs() - signIns.checkedAt > 120;
    if (stale && !signIns?.checking) void checkSignIns();
  };

  const active = overview?.accounts.find((a) => a.active) ?? null;
  const accent = followTheme && active ? active.color : DEFAULT_ACCENT;

  return (
    <main className="panel" style={{ "--accent": accent } as CSSProperties}>
      {view === "home" && (
        <Home
          overview={overview}
          conversations={conversations}
          signIns={signIns}
          now={now}
          refreshing={refreshing}
          onRefresh={() => void refreshUsage(true)}
          onSwitch={(alias) => run(() => api.switchAccount(alias))}
          onResume={(id, target) => run(() => api.resume(id, target))}
          onAccounts={openAccounts}
          waiting={waiting}
          measuring={measuring}
          onSignIn={(alias) => void signInAndWatch(alias)}
          onSettings={() => setView("settings")}
          onSwitchDesktop={switchDesktop}
        />
      )}
      {view === "accounts" && overview && (
        <AccountsPage
          overview={overview}
          stats={stats}
          signIns={signIns}
          focus={focus}
          now={now}
          onBack={() => setView("home")}
          onSwitch={(alias) => run(() => api.switchAccount(alias))}
          onColor={(alias, hex) => run(() => api.setColor(alias, hex))}
          onOpenChrome={(alias) => run(() => api.openChrome(alias))}
          onSignInAgain={(alias) => void signInAndWatch(alias)}
          onRemove={(alias) => run(() => api.removeAccount(alias)).then(() => loadStats())}
          onSwitchDesktop={switchDesktop}
          refreshing={refreshing}
          onRefresh={() => void refreshUsage(true)}
          onAdd={() => setView("add")}
        />
      )}
      {view === "settings" && overview && (
        <Settings
          overview={overview}
          followTheme={followTheme}
          onFollowTheme={(on) => {
            setFollowTheme(on);
            writeFollowTheme(on);
          }}
          onBack={() => setView("home")}
          onStatusline={(enabled) => run(() => api.setStatusline(enabled))}
          onShell={(enabled) => run(() => api.setShell(enabled))}
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
    </main>
  );
}

// ---- Home ------------------------------------------------------------------

/** A reading older than this shows its age. */
const STALE_AFTER_SECS = 10 * 60;

function Home(props: {
  overview: Overview | null;
  conversations: Conversation[];
  signIns: SignIns | null;
  now: number;
  refreshing: boolean;
  waiting: Map<string, number>;
  measuring: Set<string>;
  onRefresh: () => void;
  onSwitch: (alias: string) => void;
  onResume: (id: string, target: ResumeTarget) => void;
  onAccounts: (alias: string | null) => void;
  onSignIn: (alias: string) => void;
  onSettings: () => void;
  onSwitchDesktop: (alias: string) => Promise<DesktopSwitch | null>;
}) {
  const { overview, conversations, now } = props;
  const accounts = overview?.accounts ?? [];
  const active = accounts.find((a) => a.active) ?? null;
  const signedOut = new Set((props.signIns?.list ?? []).filter((s) => s.signedIn === false).map((s) => s.alias));
  const plans = new Map((props.signIns?.list ?? []).map((s) => [s.alias, s.plan]));
  const desktopOwner = overview?.desktopOwner ?? null;
  const byAlias = new Map(accounts.map((a) => [a.alias, a]));

  return (
    <>
      <header className="header">
        <div className="brand">
          <span className="logo">
            <Logo />
          </span>
          <span className="brand-name">Relais</span>
        </div>
        {active && (
          <span className="live-chip">
            <span className={`dot ${active.observedAt != null ? "pulse" : ""}`} />
            {active.observedAt == null ? t("notMeasuredYet") : now - active.observedAt < 10 ? t("measuredNow") : t("measuredAgo", { age: age(active.observedAt, now) })}
          </span>
        )}
      </header>

      <div className="scroll">
        {overview && accounts.length === 0 && (
          <div className="card empty">
            <strong>{t("noAccount")}</strong>
            <span className="muted small">{t("noAccountHint")}</span>
          </div>
        )}

        {accounts.length > 0 && <Summary accounts={accounts} signedOut={signedOut} best={overview?.best ?? null} now={now} onSwitch={props.onSwitch} />}

        {orderAccounts(accounts, signedOut).map((account) => (
          <AccountPanel
            key={account.alias}
            account={account}
            plan={plans.get(account.alias) ?? null}
            signedOut={signedOut.has(account.alias)}
            waiting={props.waiting.has(account.alias)}
            measuring={props.measuring.has(account.alias)}
            desktop={desktopOwner === account.alias}
            now={now}
            onOpen={() => props.onAccounts(account.alias)}
            onSignIn={() => props.onSignIn(account.alias)}
          />
        ))}

        {accounts.length > 1 && <WhereBar accounts={accounts} desktopOwner={desktopOwner} onSwitch={props.onSwitch} onSwitchDesktop={props.onSwitchDesktop} />}

        <Sessions conversations={conversations} accounts={byAlias} now={now} onResume={props.onResume} />
      </div>

      <footer className="footer">
        <button className="button" disabled={props.refreshing} title={t("refreshDetail")} onClick={props.onRefresh}>
          {props.refreshing ? t("refreshing") : t("refresh")}
        </button>
        <button className="button button-accent" onClick={() => props.onAccounts(null)}>
          {t("accountsButton", { n: accounts.length })}
        </button>
        <button className="button" onClick={props.onSettings}>
          {t("settings")}
        </button>
      </footer>
    </>
  );
}

/** How many accounts can work right now, and what comes next. */
function Summary({ accounts, signedOut, best, now, onSwitch }: { accounts: Account[]; signedOut: Set<string>; best: string | null; now: number; onSwitch: (alias: string) => void }) {
  const total = accounts.length;
  const free = accounts.filter((a) => !a.limited && !signedOut.has(a.alias)).length;
  const active = accounts.find((a) => a.active) ?? null;
  const relay = accounts.find((a) => a.alias === best && !a.active && !a.limited && !signedOut.has(a.alias)) ?? null;
  const handoff = active && relay && (active.limited || mood(active) === "tiring") ? relay : null;
  const back = nextReturn(accounts);
  const out = accounts.filter((a) => signedOut.has(a.alias));
  const heading = free === 0 ? t("availableNone", { total }) : free === 1 ? t("availableOne", { total }) : t("availableMany", { n: free, total });
  return (
    <section className="summary-card" aria-live="polite">
      <div className="summary-title">{heading}</div>
      {active && handoff ? (
        <div className="summary-handoff">
          <span className="small">{t(active.limited ? "handoffLimited" : "handoffTiring", { alias: active.alias, best: handoff.alias, n: margin(handoff) ?? "—" })}</span>
          <button className="relay-button tinted" style={tint(handoff.color)} onClick={() => onSwitch(handoff.alias)}>
            {t("passRelayTo", { alias: handoff.alias })}
          </button>
        </div>
      ) : back?.limitedUntil != null ? (
        <div className="small summary-line">{emphasize("nextReturn", "alias", back.alias, { time: time(back.limitedUntil, now), in: inTime(back.limitedUntil, now) })}</div>
      ) : free === total ? (
        <div className="small summary-line">{t("allAvailable")}</div>
      ) : null}
      {out.length > 0 && <div className="small summary-warn">{out.length === 1 ? t("signedOutOne", { alias: out[0].alias }) : t("signedOutMany", { n: out.length })}</div>}
    </section>
  );
}

/** One account the way a usage panel shows it: a bar per limit, with what is used, what is left and the pace. */
function AccountPanel(props: { account: Account; plan: string | null; signedOut: boolean; waiting: boolean; measuring: boolean; desktop: boolean; now: number; onOpen: () => void; onSignIn: () => void }) {
  const { account, plan, signedOut, now } = props;
  const status = signedOut
    ? { kind: "off", label: t("statusSignedOut") }
    : account.limited
      ? { kind: "limited", label: account.limitedUntil != null ? t("statusLimitedUntil", { time: time(account.limitedUntil, now) }) : t("statusLimited") }
      : account.observedAt == null
        ? { kind: "unknown", label: t("statusUnmeasured") }
        : { kind: "free", label: t("statusAvailable") };
  // The weekly cap on Sonnet alone weighs little next to the others: it stays out of the way.
  const models = account.models.filter((m) => !m.model.toLowerCase().startsWith("sonnet"));
  const five = account.fiveHour;
  const week = account.sevenDay;
  const showFive = five != null && (!account.limited || five.used >= 100);
  const stale = account.observedAt != null && now - account.observedAt > STALE_AFTER_SECS;
  const sub = [account.email, plan ? planName(plan) : null, props.measuring ? t("measuringNow") : stale && account.observedAt != null ? t("measuredAgo", { age: age(account.observedAt, now) }) : null].filter(Boolean).join(" · ");
  return (
    <section className={`acct tinted is-${status.kind}`} style={tint(account.color)} aria-label={account.alias}>
      <button className="acct-head" title={t("openAccount")} onClick={props.onOpen}>
        <span className="acct-dot" />
        <span className="acct-alias">{account.alias}</span>
        {account.active && <span className="acct-chip">{t("whereTerminal")}</span>}
        {props.desktop && <span className="acct-chip">{t("whereDesktop")}</span>}
        <span className={`acct-status is-${status.kind}`}>{status.label}</span>
      </button>
      {sub && <div className="acct-sub">{sub}</div>}
      {signedOut && (
        <div className="acct-alert" aria-live="polite">
          <span>{props.waiting ? t("signInWaiting") : t("signedOutNote")}</span>
          <button className="button button-warn" onClick={props.onSignIn}>
            {props.waiting ? t("signInRetry") : t("signIn")}
          </button>
        </div>
      )}
      {showFive && five && <Meter label={t("barSession")} window={five} length={FIVE_HOURS_SECS} now={now} idle={t("noSessionWindow")} />}
      {week && <Meter label={t("barWeek")} window={week} length={WEEK_SECS} now={now} />}
      {models.map((model) => (
        <Meter key={model.model} label={t("barModel", { model: model.model })} window={model} length={WEEK_SECS} now={now} note={(account.observedAt ?? now) - model.observedAt > STALE_AFTER_SECS ? t("measuredAgo", { age: age(model.observedAt, now) }) : undefined} />
      ))}
      {!five && !week && models.length === 0 && <p className="acct-empty">{t("noReadingYet")}</p>}
    </section>
  );
}

/** One limit: its name and reset, a bar, then what is used, what is left and the pace. */
function Meter({ label, window, length, now, idle, note }: { label: string; window: { used: number; resetsAt: number | null }; length: number; now: number; idle?: string; note?: string }) {
  const used = Math.min(100, Math.max(0, window.used));
  const resetsAt = window.resetsAt != null && window.resetsAt > now ? window.resetsAt : null;
  const ahead = pace(window, length, now);
  const drift = ahead == null ? 0 : Math.round(ahead);
  const level = used >= 100 ? "is-full" : used >= TIRING_FROM ? "is-high" : "";
  return (
    <div className="meter">
      <div className="meter-head">
        <span className="meter-label">{label}</span>
        <span className="meter-reset">{resetsAt != null ? t("resetAt", { time: time(resetsAt, now), in: inTime(resetsAt, now) }) : used === 0 && idle ? idle : t("resetUnknown")}</span>
      </div>
      <div className="meter-track" role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(used)}>
        <span className={`meter-fill ${level}`} style={{ width: `${used}%` }} />
      </div>
      <div className="meter-line">
        {t("usedLeft", { used: Math.round(used), left: Math.round(100 - used) })}
        {drift > 0 && (
          <>
            {" · "}
            <span className="meter-deficit">{t("deficit", { n: drift })}</span>
          </>
        )}
        {drift < 0 && (
          <>
            {" · "}
            <span className="meter-reserve">{t("reserve", { n: -drift })}</span>
          </>
        )}
        {note && (
          <>
            {" · "}
            <span className="meter-note">{note}</span>
          </>
        )}
      </div>
    </div>
  );
}

/** Which account the terminal and Claude Desktop use, each changed from here. */
function WhereBar(props: { accounts: Account[]; desktopOwner: string | null; onSwitch: (alias: string) => void; onSwitchDesktop: (alias: string) => Promise<DesktopSwitch | null> }) {
  const { accounts, desktopOwner } = props;
  const [open, setOpen] = useState<"terminal" | "desktop" | null>(null);
  const [target, setTarget] = useState<Account | null>(null);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const terminal = accounts.find((a) => a.active) ?? null;
  const desktop = accounts.find((a) => a.alias === desktopOwner) ?? null;
  const toggle = (which: "terminal" | "desktop") => {
    setOpen(open === which ? null : which);
    setTarget(null);
    setNote(null);
  };
  const pickDesktop = async (account: Account) => {
    setBusy(true);
    const result = await props.onSwitchDesktop(account.alias);
    setBusy(false);
    setTarget(null);
    setOpen(null);
    if (!result) return;
    const email = account.email ?? account.alias;
    if (result.started === "signedIn") setNote(t("desktopSignedIn", { alias: account.alias }));
    else if (result.started === "otherAccount") setNote(t("desktopOtherAccount", { alias: account.alias }));
    else if (result.started === "signedOut" || result.outcome === "signInNeeded") setNote(t("desktopSignInNow", { email }));
    else setNote(t("desktopDone", { alias: account.alias }));
  };
  const pill = (which: "terminal" | "desktop", account: Account | null) => (
    <button className={`where-pill ${open === which ? "is-open" : ""} ${account ? "tinted" : ""}`} style={account ? tint(account.color) : undefined} aria-expanded={open === which} onClick={() => toggle(which)}>
      {which === "terminal" ? <PromptIcon size={13} /> : <MonitorIcon size={13} />}
      <span className="where-pill-label">{which === "terminal" ? t("whereTerminal") : t("whereDesktop")}</span>
      <span className="where-pill-alias">{account?.alias ?? "—"}</span>
      <ChevronIcon />
    </button>
  );
  return (
    <section className="where-bar" aria-label={t("whereItWorks")}>
      <div className="where-pills">
        {pill("terminal", terminal)}
        {pill("desktop", desktop)}
      </div>
      {open && !target && (
        <div className="picker">
          <span className="muted tiny">{open === "terminal" ? t("wherePickTerminal") : t("wherePickDesktop")}</span>
          <div className="picker-options">
            {accounts.map((account) => {
              const current = open === "terminal" ? account.active : account.alias === desktopOwner;
              return (
                <button
                  key={account.alias}
                  className={`picker-option tinted ${current ? "is-current" : ""}`}
                  style={tint(account.color)}
                  disabled={current || busy}
                  onClick={() => {
                    if (open === "terminal") {
                      props.onSwitch(account.alias);
                      setOpen(null);
                      setNote(t("terminalSwitched", { alias: account.alias }));
                    } else {
                      setTarget(account);
                    }
                  }}
                >
                  <Avatar color={account.color} label={initial(account.alias)} size={18} />
                  <span className="ellipsis">{account.alias}</span>
                </button>
              );
            })}
          </div>
        </div>
      )}
      {open === "desktop" && target && (
        <div className="confirm">
          <span className="small">{target.desktopFolder ? t("desktopConfirmRestore", { alias: target.alias }) : t("desktopConfirmFirst", { email: target.email ?? target.alias })}</span>
          <div className="actions">
            <button className="button" disabled={busy} onClick={() => setTarget(null)}>
              {t("cancel")}
            </button>
            <button className="button button-accent" disabled={busy} onClick={() => void pickDesktop(target)}>
              {busy ? t("desktopSwitching") : t("desktopSwitch")}
            </button>
          </div>
        </div>
      )}
      {note && !open && <p className="small no-margin where-note">{note}</p>}
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

function Logo() {
  return (
    <svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M4 16c3-6 7-8 16-8" />
      <path d="M16 4l4 4-4 4" />
      <path d="M4 20h8" />
    </svg>
  );
}

function ChevronIcon() {
  return (
    <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M6 9l6 6 6-6" />
    </svg>
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
  onBack: () => void;
  onSwitch: (alias: string) => void;
  onColor: (alias: string, hex: string) => void;
  onOpenChrome: (alias: string) => void;
  onSignInAgain: (alias: string) => void;
  onRemove: (alias: string) => void;
  onSwitchDesktop: (alias: string) => Promise<DesktopSwitch | null>;
  refreshing: boolean;
  onRefresh: () => void;
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
    <div className="page">
      <div className="page-head">
        <h1 className="page-title">{t("accountsTitle")}</h1>
        <button className="button" onClick={props.onBack}>
          {t("back")}
        </button>
      </div>

      <section className="card summary">
        <div className="summary-head">
          <span className="strong">{heading}</span>
          <span className="muted tiny">
            {signIns?.checking ? t("checking") : signIns?.checkedAt ? [plans.length === 1 ? t("plan", { plan: planName(plans[0]) }) : null, t("checkedAgo", { age: age(signIns.checkedAt, now) })].filter(Boolean).join(" · ") : ""}
          </span>
        </div>
        <div className="summary-refresh">
          <span className="muted tiny">{t("reserveTitle")}</span>
          <button className="button where-button" disabled={props.refreshing} onClick={props.onRefresh} title={t("refreshDetail")}>
            {props.refreshing ? t("refreshing") : t("refresh")}
          </button>
        </div>
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
          onOpenChrome={props.onOpenChrome}
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
  onOpenChrome: (alias: string) => void;
  onSignInAgain: (alias: string) => void;
  onRemove: (alias: string) => void;
  desktopOwner: string | null;
  onSwitchDesktop: (alias: string) => Promise<DesktopSwitch | null>;
}) {
  const { account, stats, signIn, now } = props;
  const [confirming, setConfirming] = useState(false);
  const [desktopStep, setDesktopStep] = useState<"idle" | "confirm" | "busy">("idle");
  const [desktopNote, setDesktopNote] = useState<string | null>(null);
  const desktopHere = props.desktopOwner === account.alias;
  const email = account.email ?? signIn?.email ?? account.alias;
  const switchDesktop = async () => {
    setDesktopStep("busy");
    const result = await props.onSwitchDesktop(account.alias);
    setDesktopStep("idle");
    if (!result) return;
    if (result.started === "signedIn") setDesktopNote(t("desktopSignedIn", { alias: account.alias }));
    else if (result.started === "otherAccount") setDesktopNote(t("desktopOtherAccount", { alias: account.alias }));
    else if (result.started === "signedOut" || result.outcome === "signInNeeded") setDesktopNote(t("desktopSignInNow", { email }));
    else setDesktopNote(t("desktopDone", { alias: account.alias }));
  };
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
            <button className="button where-button" disabled={desktopStep !== "idle"} onClick={() => setDesktopStep("confirm")}>
              {account.desktopFolder ? t("desktopOpenHere") : t("desktopConnect")}
            </button>
          )}
        </div>
        {desktopStep === "confirm" && (
          <div className="confirm">
            <span className="small">{account.desktopFolder ? t("desktopConfirmRestore", { alias: account.alias }) : t("desktopConfirmFirst", { email })}</span>
            <div className="actions">
              <button className="button" onClick={() => setDesktopStep("idle")}>
                {t("cancel")}
              </button>
              <button className="button button-accent" onClick={() => void switchDesktop()}>
                {t("desktopSwitch")}
              </button>
            </div>
          </div>
        )}
        {desktopStep === "busy" && <p className="muted small no-margin">{t("desktopSwitching")}</p>}
        {desktopNote && desktopStep === "idle" && <p className="small no-margin tinted-text">{desktopNote}</p>}
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
        <span className="badge">{account.chromeProfile ? t("chromeProfile", { profile: account.chromeProfile }) : t("chromeUsual")}</span>
      </div>

      <div className="windows">
        <div className="window-box">
          <div className="muted tiny">{t("fiveHourLabel")}</div>
          <div className="window-value">{five ? `${Math.round(five.used)} %` : "—"}</div>
          <div className="tiny window-note">{five?.resetsAt != null && five.resetsAt > now ? t("resetsIn", { in: inTime(five.resetsAt, now) }) : t("nothingUsed")}</div>
        </div>
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
          <button className="button" onClick={() => props.onOpenChrome(account.alias)}>
            {t("openChrome")}
          </button>
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

function Settings(props: { overview: Overview; followTheme: boolean; onFollowTheme: (on: boolean) => void; onBack: () => void; onStatusline: (enabled: boolean) => void; onShell: (enabled: boolean) => void }) {
  const { overview } = props;
  const rows = [
    { id: "statusline", title: t("statuslineTitle"), detail: t("statuslineDetail"), on: overview.statusline, set: props.onStatusline },
    { id: "shell", title: t("shellTitle"), detail: t("shellDetail"), on: overview.shell, set: props.onShell },
  ];
  return (
    <div className="page">
      <div className="page-head">
        <h1 className="page-title">{t("settingsTitle")}</h1>
        <button className="button" onClick={props.onBack}>
          {t("back")}
        </button>
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
