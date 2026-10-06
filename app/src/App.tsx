import { useCallback, useEffect, useState, type CSSProperties, type ReactNode } from "react";

import { api, type Account, type Conversation, type Overview, type PaletteColor, type ResumeTarget } from "./api";
import { age, inTime, isLive, margin, mood, remaining, time, type Mood } from "./format";
import { t, type MessageKey } from "./i18n";

type View = "home" | "settings" | "add" | "colors";

/** The tint of Relais when it does not follow the active account. */
const DEFAULT_ACCENT = "#4CC38A";
/** Conversations whose account is unknown (started before Relais measured them). */
const NEUTRAL = "#93A29B";
const FOLLOW_THEME_KEY = "relais.followTheme";
const SHOWN_CONVERSATIONS = 3;
const FETCHED_CONVERSATIONS = 12;
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
  const [overview, setOverview] = useState<Overview | null>(null);
  const [conversations, setConversations] = useState<Conversation[]>([]);
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

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 15_000);
    const clock = window.setInterval(() => setNow(nowSecs()), 10_000);
    const onFocus = () => void refresh();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") void api.hide();
    };
    window.addEventListener("focus", onFocus);
    window.addEventListener("keydown", onKey);
    return () => {
      window.clearInterval(timer);
      window.clearInterval(clock);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("keydown", onKey);
    };
  }, [refresh]);

  const run = async (action: () => Promise<Overview | void>) => {
    try {
      const next = await action();
      if (next) setOverview(next);
      setNow(nowSecs());
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  const active = overview?.accounts.find((a) => a.active) ?? null;
  const accent = followTheme && active ? active.color : DEFAULT_ACCENT;

  return (
    <main className="panel" style={{ "--accent": accent } as CSSProperties}>
      {view === "home" && (
        <Home
          overview={overview}
          conversations={conversations}
          now={now}
          onSwitch={(alias) => run(() => api.switchAccount(alias))}
          onResume={(id, target) => run(() => api.resume(id, target))}
          onAdd={() => setView("add")}
          onColors={() => setView("colors")}
          onSettings={() => setView("settings")}
        />
      )}
      {view === "colors" && overview && (
        <Colors
          overview={overview}
          followTheme={followTheme}
          onFollowTheme={(on) => {
            setFollowTheme(on);
            writeFollowTheme(on);
          }}
          onColor={(alias, hex) => run(() => api.setColor(alias, hex))}
          onClose={() => setView("home")}
        />
      )}
      {view === "settings" && overview && (
        <Settings
          overview={overview}
          onBack={() => setView("home")}
          onStatusline={(enabled) => run(() => api.setStatusline(enabled))}
          onShell={(enabled) => run(() => api.setShell(enabled))}
        />
      )}
      {view === "add" && (
        <AddAccount
          onBack={() => {
            setView("home");
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
  now: number;
  onSwitch: (alias: string) => void;
  onResume: (id: string, target: ResumeTarget) => void;
  onAdd: () => void;
  onColors: () => void;
  onSettings: () => void;
}) {
  const { overview, conversations, now } = props;
  const accounts = overview?.accounts ?? [];
  const active = accounts.find((a) => a.active) ?? null;
  const best = accounts.find((a) => a.alias === overview?.best && !a.active) ?? null;
  const handingOff = active?.limited && best && !best.limited ? best : null;
  const others = accounts.filter((a) => !a.active && a.alias !== handingOff?.alias);
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

        {active && (active.limited ? <RestingHero account={active} handoff={handingOff} now={now} onSwitch={props.onSwitch} /> : <Hero account={active} now={now} />)}

        {active && mood(active) === "tiring" && best && <Nudge account={active} best={best} onSwitch={props.onSwitch} />}

        {others.length > 0 && (
          <section aria-labelledby="relay-title">
            <div className="section-head">
              <h2 id="relay-title" className="section-title">
                {t("relayTitle")}
              </h2>
              {best && !handingOff && <span className="muted tiny">{t("nextBest", { alias: best.alias })}</span>}
            </div>
            <div className="tiles">
              {others.map((account) => (
                <Tile key={account.alias} account={account} isBest={account.alias === best?.alias} now={now} onSwitch={props.onSwitch} />
              ))}
            </div>
          </section>
        )}

        <Sessions conversations={conversations} accounts={byAlias} now={now} onResume={props.onResume} />
      </div>

      <footer className="footer">
        <button className="button" onClick={props.onAdd}>
          {t("addShort")}
        </button>
        <button className="button" onClick={props.onColors}>
          {t("colors")}
        </button>
        <button className="button" onClick={props.onSettings}>
          {t("settings")}
        </button>
      </footer>
    </>
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

function Tile({ account, isBest, now, onSwitch }: { account: Account; isBest: boolean; now: number; onSwitch: (alias: string) => void }) {
  const resting = account.limited;
  const left = margin(account);
  return (
    <article className={`tile tinted ${isBest ? "is-best" : ""}`} style={tint(account.color)}>
      <div className="tile-head">
        <Rings size="tile" five={account.fiveHour?.used ?? 0} week={account.sevenDay?.used ?? 0} />
        <div className="tile-names">
          <div className="tile-alias">{account.alias}</div>
          <div className={`tiny ${resting ? "muted" : "tinted-text"}`}>{resting ? t("moodResting") : left == null ? t("readyUnmeasured") : t("ready", { n: left })}</div>
        </div>
      </div>
      {resting ? (
        <div className="resting-box">
          <MoonIcon />
          {account.limitedUntil != null ? t("backIn", { in: inTime(account.limitedUntil, now) }) : t("restingUnknown")}
        </div>
      ) : (
        <button className="relay-button" onClick={() => onSwitch(account.alias)}>
          {t("passRelay")}
        </button>
      )}
    </article>
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

// ---- Colors ----------------------------------------------------------------

function Colors(props: { overview: Overview; followTheme: boolean; onFollowTheme: (on: boolean) => void; onColor: (alias: string, hex: string) => void; onClose: () => void }) {
  const { overview } = props;
  const same = (a: string, b: string) => a.toUpperCase() === b.toUpperCase();
  return (
    <div className="page">
      <div className="page-head">
        <h1 className="page-title">{t("colors")}</h1>
        <button className="button" onClick={props.onClose}>
          {t("close")}
        </button>
      </div>
      <p className="muted small">{t("colorsIntro")}</p>
      <div className="color-cards">
        {overview.accounts.map((account) => {
          const current = overview.palette.find((p) => same(p.hex, account.color));
          return (
            <div key={account.alias} className="color-card tinted" style={tint(account.color)}>
              <div className="color-head">
                <Avatar color={account.color} label={initial(account.alias)} size={28} />
                <strong className="ellipsis">{account.alias}</strong>
                {current && <span className="tinted-text tiny strong push">{paletteName(current)}</span>}
              </div>
              <div className="swatches" role="group" aria-label={account.alias}>
                {overview.palette.map((color) => {
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
            </div>
          );
        })}
      </div>
      <label className="toggle-row">
        <input type="checkbox" className="check" checked={props.followTheme} onChange={(e) => props.onFollowTheme(e.target.checked)} />
        <span>
          <span className="strong">{t("followTheme")}</span>
          <br />
          <span className="muted small">{t("followThemeDetail")}</span>
        </span>
      </label>
    </div>
  );
}

// ---- Settings and adding an account ----------------------------------------

function Settings(props: { overview: Overview; onBack: () => void; onStatusline: (enabled: boolean) => void; onShell: (enabled: boolean) => void }) {
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
