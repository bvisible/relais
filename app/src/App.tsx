import { useCallback, useEffect, useState } from "react";

import { api, type Account, type Conversation, type Overview, type ResumeTarget } from "./api";
import { age, gaugeTone, time } from "./format";
import { t } from "./i18n";

type View = "home" | "settings" | "add";

const AVATAR_COLORS = ["#2F6F5E", "#3D5A80", "#8A5A12", "#6B4C8A", "#2E6A8A"];

export function App() {
  const [view, setView] = useState<View>("home");
  const [overview, setOverview] = useState<Overview | null>(null);
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [next, recent] = await Promise.all([api.overview(), api.conversations(4)]);
      setOverview(next);
      setConversations(recent);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 15_000);
    const onFocus = () => void refresh();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") void api.hide();
    };
    window.addEventListener("focus", onFocus);
    window.addEventListener("keydown", onKey);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("keydown", onKey);
    };
  }, [refresh]);

  const run = async (action: () => Promise<Overview | void>) => {
    try {
      const next = await action();
      if (next) setOverview(next);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <main className="panel">
      {view === "home" && (
        <Home
          overview={overview}
          conversations={conversations}
          onSwitch={(alias) => run(() => api.switchAccount(alias))}
          onResume={(id, target) => run(() => api.resume(id, target))}
          onAdd={() => setView("add")}
          onSettings={() => setView("settings")}
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
          existing={overview?.accounts.map((a) => a.alias) ?? []}
          onBack={() => {
            setView("home");
            void refresh();
          }}
          onRefresh={refresh}
          onSignIn={(alias) => api.addAccount(alias)}
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

function Logo({ size = 24 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M4 16c3-6 7-8 16-8" />
      <path d="M16 4l4 4-4 4" />
      <path d="M4 20h8" />
    </svg>
  );
}

function Home(props: {
  overview: Overview | null;
  conversations: Conversation[];
  onSwitch: (alias: string) => void;
  onResume: (id: string, target: ResumeTarget) => void;
  onAdd: () => void;
  onSettings: () => void;
}) {
  const { overview, conversations } = props;
  const [openConversation, setOpenConversation] = useState<string | null>(null);
  const active = overview?.accounts.find((a) => a.active) ?? null;
  const now = overview?.now ?? Math.floor(Date.now() / 1000);

  return (
    <>
      <header className="header">
        <div className="brand">
          <span className="logo">
            <Logo />
          </span>
          <span className="brand-name">Relais</span>
        </div>
        {active?.observedAt != null && (
          <span className="muted small">{now - active.observedAt < 60 ? t("measuredNow") : t("measuredAgo", { age: age(active.observedAt, now) })}</span>
        )}
      </header>

      {overview && active && <Banner account={active} best={overview.best} now={now} onSwitch={props.onSwitch} />}

      <section className="accounts" aria-label="Comptes">
        {overview?.accounts.length === 0 && (
          <div className="card empty">
            <strong>{t("noAccount")}</strong>
            <span className="muted small">{t("noAccountHint")}</span>
          </div>
        )}
        {overview?.accounts.map((account, index) => (
          <AccountCard key={account.alias} account={account} color={AVATAR_COLORS[index % AVATAR_COLORS.length]} now={now} onSwitch={props.onSwitch} />
        ))}
      </section>

      <h2 className="section-title">{t("recent")}</h2>
      <section className="conversations">
        {conversations.length === 0 && <p className="muted small pad">{t("noConversation")}</p>}
        {conversations.map((c) => (
          <div key={c.id} className="conversation">
            <div className="conversation-row">
              <div className="conversation-text">
                <div className="conversation-title">{c.title ?? c.id}</div>
                <div className="muted small ellipsis">
                  {c.cwd ? `${c.cwd} · ` : ""}
                  {age(c.lastActivity, now)}
                </div>
              </div>
              <button className="button soft" aria-expanded={openConversation === c.id} onClick={() => setOpenConversation(openConversation === c.id ? null : c.id)}>
                {t("resume")}
              </button>
            </div>
            {openConversation === c.id && (
              <div className="resume-choices">
                <button className="button primary" onClick={() => props.onResume(c.id, "desktop")}>
                  {t("resumeDesktop")}
                </button>
                <button className="button" onClick={() => props.onResume(c.id, "terminal")}>
                  {t("resumeTerminal")}
                </button>
              </div>
            )}
          </div>
        ))}
      </section>

      <footer className="footer">
        <button className="button grow" onClick={props.onAdd}>
          {t("addAccount")}
        </button>
        <button className="button" onClick={props.onSettings}>
          {t("settings")}
        </button>
      </footer>
    </>
  );
}

function Banner({ account, best, now, onSwitch }: { account: Account; best: string | null; now: number; onSwitch: (alias: string) => void }) {
  if (!best) return null;
  const five = account.fiveHour?.used ?? 0;
  const limited = account.limited;
  const near = !limited && five >= 85;
  if (!limited && !near) return null;
  const text = !limited
    ? t("nearLimit", { alias: account.alias, best })
    : account.limitedUntil != null
      ? t("limitedBanner", { alias: account.alias, time: time(account.limitedUntil, now), best })
      : t("limitedBannerUnknown", { alias: account.alias, best });
  return (
    <div className={`banner ${limited ? "banner-full" : "banner-warm"}`} role="status">
      <span className="banner-text">{text}</span>
      <button className="button dark" onClick={() => onSwitch(best)}>
        {limited ? t("continueOn") : t("switchNow")}
      </button>
    </div>
  );
}

function AccountCard({ account, color, now, onSwitch }: { account: Account; color: string; now: number; onSwitch: (alias: string) => void }) {
  const limited = account.limited;
  const measured = account.fiveHour != null || account.sevenDay != null;
  return (
    <article className={`card account ${account.active ? "is-active" : ""}`}>
      <div className="account-head">
        <span className="avatar" style={{ background: color }}>
          {account.alias.charAt(0).toUpperCase()}
        </span>
        <div className="account-names">
          <div className="account-alias">{account.alias}</div>
          <div className="muted small ellipsis">{account.email ?? ""}</div>
        </div>
        {account.active && <span className="pill pill-active">{t("active")}</span>}
        {limited && (
          <span className="pill pill-full">
            {account.limitedUntil != null ? t("limitedUntil", { time: time(account.limitedUntil, now) }) : t("limited")}
          </span>
        )}
        {!account.active && !limited && (
          <button className="button small-button" onClick={() => onSwitch(account.alias)}>
            {t("switchHere")}
          </button>
        )}
      </div>
      {measured ? (
        <div className="gauges">
          <Gauge label={t("fiveHour")} window={account.fiveHour} now={now} />
          <Gauge label={t("week")} window={account.sevenDay} now={now} />
        </div>
      ) : (
        <p className="muted small no-margin">{t("notMeasured")}</p>
      )}
    </article>
  );
}

function Gauge({ label, window: w, now }: { label: string; window: Account["fiveHour"]; now: number }) {
  const used = w ? Math.round(w.used) : 0;
  const tone = gaugeTone(used);
  return (
    <div className="gauge">
      <div className="gauge-label">
        <span className="muted">{label}</span>
        <span className="strong">{w ? `${used} %` : "—"}</span>
      </div>
      <div className="track" role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={used}>
        <div className={`fill fill-${tone}`} style={{ width: `${Math.min(used, 100)}%` }} />
      </div>
      <div className="muted tiny">{w?.resetsAt && w.resetsAt > now ? t("resets", { time: time(w.resetsAt, now) }) : " "}</div>
    </div>
  );
}

function Settings(props: { overview: Overview; onBack: () => void; onStatusline: (enabled: boolean) => void; onShell: (enabled: boolean) => void }) {
  const { overview } = props;
  const rows = [
    { id: "statusline", title: t("statuslineTitle"), detail: t("statuslineDetail"), on: overview.statusline, set: props.onStatusline },
    { id: "shell", title: t("shellTitle"), detail: t("shellDetail"), on: overview.shell, set: props.onShell },
  ];
  return (
    <div className="page">
      <div className="page-head">
        <button className="button" onClick={props.onBack}>
          {t("back")}
        </button>
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
      <div className="grow" />
      <button className="button" onClick={() => void api.quit()}>
        {t("quit")}
      </button>
      <p className="muted tiny">{t("footer")}</p>
    </div>
  );
}

function AddAccount(props: { existing: string[]; onBack: () => void; onRefresh: () => Promise<void>; onSignIn: (email: string) => Promise<void> }) {
  const [email, setEmail] = useState("");
  const [alias, setAlias] = useState("");
  const [state, setState] = useState<"idle" | "waiting" | "done">("idle");
  const valid = /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email.trim());

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
        void props.onRefresh();
      }
    }, 3000);
    return () => window.clearInterval(timer);
  }, [state, email, props]);

  return (
    <div className="page">
      <div className="page-head">
        <button className="button" onClick={props.onBack}>
          {t("back")}
        </button>
        <h1 className="page-title">{t("addTitle")}</h1>
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
          placeholder="nom@exemple.com"
          autoComplete="off"
          spellCheck={false}
          onChange={(e) => {
            setEmail(e.target.value);
            setState("idle");
          }}
        />
        <span className="muted tiny">{alias ? t("aliasHint", { alias }) : "\u00a0"}</span>
      </div>
      <div className="card step">
        <span className="step-number">1</span>
        <span className="small grow">{t("addStepTerminal", { email: valid ? email.trim() : "…" })}</span>
        <button
          className="button primary"
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
