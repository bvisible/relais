import { useCallback, useEffect, useState, type CSSProperties, type ReactNode } from "react";

import { api, type Account, type AccountSignIn, type AccountStats, type Conversation, type DesktopSwitch, type Overview, type PaletteColor, type ResumeTarget } from "./api";
import { age, dayLabels, inTime, isLive, margin, mood, remaining, time, type Mood } from "./format";
import { isMessageKey, t, type MessageKey } from "./i18n";

type View = "home" | "settings" | "add" | "accounts";

/** What Claude Code last said about each account's sign-in. */
type SignIns = { checkedAt: number | null; list: AccountSignIn[]; checking: boolean };

/** The tint of Relais when it does not follow the active account. */
const DEFAULT_ACCENT = "#4CC38A";
/** Conversations whose account is unknown (started before Relais measured them). */
const NEUTRAL = "#93A29B";
const FOLLOW_THEME_KEY = "relais.followTheme";
const SHOWN_CONVERSATIONS = 3;
const FETCHED_CONVERSATIONS = 12;
const SIGN_IN_CHECK_EVERY_MS = 10 * 60_000;
const PALETTE_KEYS = ["sage", "ocean", "violet", "coral", "amber", "rose", "lime", "sky"];
const PROJECT_COLORS = ["#4C8DFF", "#F2B33D", "#A78BFA", "#38BDF8", "#F472B6", "#C3D94E", "#FF7A6B", "#4CC38A"];
const MOOD_KEYS: Record<Mood, MessageKey> = {
  running: "moodRunning",
  tiring: "moodTiring",
  resting: "moodResting",
  unknown: "moodUnknown",
};

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

function projectColor(name: string): string {
  let hash = 0;
  for (const char of name) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return PROJECT_COLORS[hash % PROJECT_COLORS.length];
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

  // Each check starts Claude Code once per account: done at launch, then
  // every ten minutes, and when the accounts page opens on stale results.
  const checkSignIns = useCallback(async () => {
    setSignIns((previous) => (previous ? { ...previous, checking: true } : { checkedAt: null, list: [], checking: true }));
    try {
      const list = await api.accountSignIns();
      setSignIns({ checkedAt: nowSecs(), list, checking: false });
    } catch {
      setSignIns((previous) => (previous ? { ...previous, checking: false } : null));
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
    const timer = window.setInterval(() => void refresh(), 15_000);
    const signInTimer = window.setInterval(() => void checkSignIns(), SIGN_IN_CHECK_EVERY_MS);
    const clock = window.setInterval(() => setNow(nowSecs()), 10_000);
    const onFocus = () => void refresh();
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
  }, [refresh, checkSignIns]);

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
          onSwitch={(alias) => run(() => api.switchAccount(alias))}
          onResume={(id, target) => run(() => api.resume(id, target))}
          onAccounts={openAccounts}
          onSettings={() => setView("settings")}
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
          onSignInAgain={(alias) => run(() => api.signInAgain(alias))}
          onRemove={(alias) => run(() => api.removeAccount(alias)).then(() => loadStats())}
          onSwitchDesktop={switchDesktop}
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

function Home(props: {
  overview: Overview | null;
  conversations: Conversation[];
  signIns: SignIns | null;
  now: number;
  onSwitch: (alias: string) => void;
  onResume: (id: string, target: ResumeTarget) => void;
  onAccounts: (alias: string | null) => void;
  onSettings: () => void;
}) {
  const { overview, conversations, now } = props;
  const accounts = overview?.accounts ?? [];
  const active = accounts.find((a) => a.active) ?? null;
  const best = accounts.find((a) => a.alias === overview?.best && !a.active) ?? null;
  const handingOff = active?.limited && best && !best.limited ? best : null;
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

        {accounts.length > 0 && <AccountsStrip accounts={accounts} signIns={props.signIns} desktopOwner={overview?.desktopOwner ?? null} onOpen={props.onAccounts} />}

        {active && (active.limited ? <RestingHero account={active} handoff={handingOff} now={now} onSwitch={props.onSwitch} /> : <Hero account={active} now={now} />)}

        {active && mood(active) === "tiring" && best && <Nudge account={active} best={best} onSwitch={props.onSwitch} />}

        {active && !handingOff && mood(active) !== "tiring" && accounts.length > 1 && <RelayLine accounts={accounts} best={best} now={now} onSwitch={props.onSwitch} />}

        <Sessions conversations={conversations} accounts={byAlias} now={now} onResume={props.onResume} />
      </div>

      <footer className="footer">
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

/** The accounts side by side: who runs, who is ready, who rests. */
function AccountsStrip({ accounts, signIns, desktopOwner, onOpen }: { accounts: Account[]; signIns: SignIns | null; desktopOwner: string | null; onOpen: (alias: string | null) => void }) {
  const ready = accounts.filter((a) => !a.limited && !a.active).length;
  const resting = accounts.filter((a) => a.limited).length;
  const connected = signIns?.checkedAt ? signIns.list.filter((s) => s.signedIn).length : null;
  const heading =
    connected == null
      ? accounts.length === 1
        ? t("accountsCountOne")
        : t("accountsCount", { n: accounts.length })
      : connected === accounts.length
        ? accounts.length === 1
          ? t("accountsConnectedOne")
          : t("accountsConnected", { n: accounts.length })
        : t("accountsConnectedPartial", { n: connected, total: accounts.length });
  const parts = [ready === 1 ? t("readyCountOne") : ready > 1 ? t("readyCount", { n: ready }) : null, resting > 0 ? t("restingCount", { n: resting }) : null].filter(Boolean);
  return (
    <section className="strip" aria-label={heading}>
      <div className="strip-head">
        <span className="strip-title">{heading}</span>
        {parts.length > 0 && <span className="muted tiny">{parts.join(" · ")}</span>}
      </div>
      <div className="strip-chips" style={{ gridTemplateColumns: `repeat(${Math.min(accounts.length, 3)}, minmax(0, 1fr))` }}>
        {accounts.map((account) => {
          const left = margin(account);
          const signIn = signIns?.list.find((s) => s.alias === account.alias);
          const state = account.limited
            ? t("chipResting")
            : left == null
              ? t("chipUnknown")
              : account.active
                ? t("chipActive", { n: left })
                : t("chipReady", { n: left });
          return (
            <button key={account.alias} className={`chip tinted ${account.active ? "is-active" : ""}`} style={tint(account.color)} onClick={() => onOpen(account.alias)}>
              <span className="chip-rings">
                <Rings size="chip" five={account.fiveHour?.used ?? 0} week={account.sevenDay?.used ?? 0} />
                <span className="chip-initial">{initial(account.alias)}</span>
                {signIn?.signedIn === false && <span className="chip-alert" title={t("signedOut")} />}
              </span>
              <span className="chip-alias">
                {desktopOwner === account.alias && (
                  <span className="chip-desktop" title={t("desktopOnAccount")} aria-label={t("desktopOnAccount")}>
                    <MonitorIcon size={10} />
                  </span>
                )}
                {account.alias}
              </span>
              <span className={`chip-state ${account.limited ? "muted" : "tinted-text"}`}>{state}</span>
            </button>
          );
        })}
      </div>
    </section>
  );
}

/** The account to hand over to next, in one line, or when the next one is back. */
function RelayLine({ accounts, best, now, onSwitch }: { accounts: Account[]; best: Account | null; now: number; onSwitch: (alias: string) => void }) {
  if (!best) {
    const back = accounts
      .filter((a) => !a.active && a.limitedUntil != null)
      .sort((a, b) => (a.limitedUntil ?? 0) - (b.limitedUntil ?? 0))[0];
    return <p className="muted small pad">{back?.limitedUntil != null ? t("noRelayBack", { alias: back.alias, in: inTime(back.limitedUntil, now) }) : t("noRelay")}</p>;
  }
  const left = margin(best);
  return (
    <div className="relay-line tinted" style={tint(best.color)}>
      <Avatar color={best.color} label={initial(best.alias)} size={24} />
      <div className="relay-text">
        <div className="relay-title">{t("nextRelay", { alias: best.alias })}</div>
        <div className="tinted-text tiny">{left == null ? t("readyUnmeasured") : t("nextRelayMargin", { n: left })}</div>
      </div>
      <button className="relay-button" onClick={() => onSwitch(best.alias)}>
        {t("passRelay")}
      </button>
    </div>
  );
}


function Hero({ account, now }: { account: Account; now: number }) {
  const state = mood(account);
  const five = account.fiveHour;
  const week = account.sevenDay;
  const measured = five != null || week != null;
  return (
    <section className={`hero ${state === "tiring" ? "is-tiring" : ""}`} aria-label={account.alias}>
      <div className="hero-rings">
        <Rings size="hero" five={five?.used ?? 0} week={week?.used ?? 0} warn={state === "tiring"} />
        <div className="hero-center">
          <div className="hero-number">
            {measured ? (
              <>
                {Math.round(five?.used ?? 0)}
                <span className="hero-unit">%</span>
              </>
            ) : (
              "—"
            )}
          </div>
          <div className="muted tiny">{t("fiveHourWindow")}</div>
        </div>
      </div>
      <div className="hero-side">
        <div className="hero-name">
          <Avatar color={account.color} label={initial(account.alias)} size={26} />
          <span className="hero-alias">{account.alias}</span>
        </div>
        <span className={`mood mood-${state}`}>{t(MOOD_KEYS[state])}</span>
        <div className="hero-lines">
          <div>
            <span className="key-dot key-outer" />
            {five?.resetsAt != null && five.resetsAt > now ? emphasize("fiveResets", "in", inTime(five.resetsAt, now)) : t("fiveIdle")}
          </div>
          {week && (
            <div>
              <span className="key-dot key-inner" />
              {week.resetsAt != null && week.resetsAt > now
                ? emphasize("weekLine", "in", inTime(week.resetsAt, now), { pct: Math.round(week.used) })
                : t("weekLineNoReset", { pct: Math.round(week.used) })}
            </div>
          )}
        </div>
      </div>
    </section>
  );
}

function RestingHero({ account, handoff, now, onSwitch }: { account: Account; handoff: Account | null; now: number; onSwitch: (alias: string) => void }) {
  const fiveFull = (account.fiveHour?.used ?? 0) >= 100;
  const until = account.limitedUntil;
  const handoffMargin = handoff ? margin(handoff) : null;
  return (
    <>
      <section className="resting-card" aria-live="polite">
        <div className="resting-title">
          <Avatar color={account.color} label={initial(account.alias)} size={24} />
          <strong>{t("restingTitle", { alias: account.alias })}</strong>
        </div>
        <div className="small resting-lead">
          {fiveFull ? t("restingFiveFull") : t("restingWeekFull")} {until != null && t("backInLead")}
        </div>
        {until != null && <div className="countdown">{remaining(until, now)}</div>}
        <div className="muted tiny">{until != null ? t("restingAt", { time: time(until, now), pct: Math.round(account.sevenDay?.used ?? 0) }) : t("restingUnknownTime")}</div>
      </section>
      {handoff && (
        <section className="handoff tinted" style={tint(handoff.color)}>
          <div className="handoff-head">
            <Rings size="tile" five={handoff.fiveHour?.used ?? 0} week={handoff.sevenDay?.used ?? 0} />
            <div>
              <div className="strong">{t("readyTitle", { alias: handoff.alias })}</div>
              <div className="tinted-text small">{handoffMargin == null ? t("readyUnmeasured") : t("readyMargin", { n: handoffMargin })}</div>
            </div>
          </div>
          <p className="small handoff-text">{t("carryOn")}</p>
          <button className="relay-button big" onClick={() => onSwitch(handoff.alias)}>
            {t("passRelayTo", { alias: handoff.alias })}
          </button>
        </section>
      )}
    </>
  );
}

function Nudge({ account, best, onSwitch }: { account: Account; best: Account; onSwitch: (alias: string) => void }) {
  return (
    <div className="nudge" role="status">
      <span className="small">{t("nearLimit", { alias: account.alias, n: margin(account) ?? 0 })}</span>
      <button className="relay-button tinted" style={tint(best.color)} onClick={() => onSwitch(best.alias)}>
        {t("passRelayTo", { alias: best.alias })}
      </button>
    </div>
  );
}

function Sessions({ conversations, accounts, now, onResume }: { conversations: Conversation[]; accounts: Map<string, Account>; now: number; onResume: (id: string, target: ResumeTarget) => void }) {
  const shown = conversations.slice(0, SHOWN_CONVERSATIONS);
  const today = new Date(now * 1000).toDateString();
  const earlier = conversations.slice(SHOWN_CONVERSATIONS).filter((c) => new Date(c.lastActivity * 1000).toDateString() === today).length;
  const anyLive = shown.some((c) => isLive(c.lastActivity, now));
  return (
    <section aria-labelledby="sessions-title">
      <div className="section-head">
        <h2 id="sessions-title" className="section-title">
          {anyLive && <span className="dot live pulse" />}
          {anyLive ? t("liveNow") : t("latest")}
        </h2>
      </div>
      {shown.length === 0 && <p className="muted small pad">{t("noConversation")}</p>}
      <ul className="sessions">
        {shown.map((c) => (
          <SessionRow key={c.id} conversation={c} account={c.account ? (accounts.get(c.account) ?? null) : null} now={now} onResume={onResume} />
        ))}
      </ul>
      {earlier > 0 && <p className="muted small pad">{earlier === 1 ? t("earlierTodayOne") : t("earlierToday", { n: earlier })}</p>}
    </section>
  );
}

function SessionRow({ conversation: c, account, now, onResume }: { conversation: Conversation; account: Account | null; now: number; onResume: (id: string, target: ResumeTarget) => void }) {
  const live = isLive(c.lastActivity, now);
  return (
    <li className="session tinted" style={tint(account?.color ?? NEUTRAL)}>
      <span className={`avatar avatar-small ${live ? "pulse" : ""}`} title={account?.alias ?? t("unknownAccount")}>
        {account ? initial(account.alias) : c.desktop ? <MonitorIcon size={11} /> : <PromptIcon size={11} />}
      </span>
      <div className="session-text">
        <div className="session-title">{c.title ?? c.id}</div>
        <div className="session-meta">
          {c.project && (
            <span className="project tinted" style={tint(projectColor(c.project))}>
              {c.project}
            </span>
          )}
          <span className="muted tiny nowrap">{age(c.lastActivity, now)}</span>
          <Pulse values={c.activity} />
        </div>
      </div>
      <button className="icon-button" aria-label={t("resumeDesktop")} title={t("resumeDesktop")} onClick={() => onResume(c.id, "desktop")}>
        <MonitorIcon />
      </button>
      <button className="icon-button" aria-label={t("resumeTerminal")} title={t("resumeTerminal")} onClick={() => onResume(c.id, "terminal")}>
        <PromptIcon />
      </button>
    </li>
  );
}

/** Messages per five minutes over the last hour, as small bars that brighten toward now. */
function Pulse({ values }: { values: number[] }) {
  if (values.length === 0 || values.every((v) => v === 0)) return null;
  const max = Math.max(...values);
  return (
    <span className="pulse-bars" role="img" aria-label={t("activityLabel")}>
      {values.map((value, index) => (
        <span key={index} style={{ height: `${2 + Math.round((12 * value) / max)}px`, opacity: 0.35 + (0.65 * index) / Math.max(1, values.length - 1) }} />
      ))}
    </span>
  );
}

// ---- Rings and small pieces ------------------------------------------------

const RING_SIZES = {
  hero: { box: 128, outer: { r: 56, width: 12 }, inner: { r: 40, width: 9 } },
  tile: { box: 40, outer: { r: 16, width: 5 }, inner: { r: 9, width: 4 } },
  chip: { box: 44, outer: { r: 19, width: 4 }, inner: { r: 12.5, width: 3 } },
};

/** The 5-hour window outside, the week inside. */
function Rings({ size, five, week, warn = false }: { size: keyof typeof RING_SIZES; five: number; week: number; warn?: boolean }) {
  const { box, outer, inner } = RING_SIZES[size];
  const center = box / 2;
  const arc = (radius: number, pct: number) => {
    const length = 2 * Math.PI * radius;
    return `${(length * Math.min(Math.max(pct, 0), 100)) / 100} ${length}`;
  };
  const rotate = `rotate(-90 ${center} ${center})`;
  return (
    <svg className="rings" width={box} height={box} viewBox={`0 0 ${box} ${box}`} aria-hidden="true">
      <circle cx={center} cy={center} r={outer.r} className="ring-track" strokeWidth={outer.width} />
      <circle cx={center} cy={center} r={outer.r} className={`ring-outer ${warn ? "is-warn" : ""}`} strokeWidth={outer.width} transform={rotate} style={{ strokeDasharray: arc(outer.r, five), opacity: five > 0 ? 1 : 0 }} />
      <circle cx={center} cy={center} r={inner.r} className="ring-track ring-track-inner" strokeWidth={inner.width} />
      <circle cx={center} cy={center} r={inner.r} className="ring-inner" strokeWidth={inner.width} transform={rotate} style={{ strokeDasharray: arc(inner.r, week), opacity: week > 0 ? 1 : 0 }} />
    </svg>
  );
}

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

function MoonIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />
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
        <div className="muted tiny">{t("reserveTitle")}</div>
        <div className="reserve" aria-hidden="true">
          {accounts.map((a) => (
            <span key={a.alias} className="tinted" style={{ ...tint(a.color), flexGrow: Math.max(margin(a) ?? 0, 2) }} />
          ))}
        </div>
        <div className="reserve-legend">
          {accounts.map((a) => (
            <span key={a.alias} className="tinted" style={tint(a.color)}>
              <strong className="tinted-text">{a.alias}</strong> {a.limited ? (a.limitedUntil != null ? t("backIn", { in: inTime(a.limitedUntil, now) }) : t("moodResting")) : `${margin(a) ?? "—"} %`}
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
    if (result) setDesktopNote(result.outcome === "signInNeeded" ? t("desktopSignInNow", { email }) : t("desktopDone", { alias: account.alias }));
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
                ? t("resetsIn", { in: inTime(week.resetsAt, now) })
                : " "}
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
