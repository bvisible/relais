// The panel's one screen: every account on a line, Claude's first, and one
// button per account that moves Claude there with the whole workspace.

import { useEffect, useRef, useState, type ReactNode } from "react";

import { api, type Account, type AccountSignIn, type Overview } from "./api";
import { t, type MessageKey } from "./i18n";
import { advisedOf, confirmLines, connectionOf, connectionSummary, relayOrder, resultLine, ringsOf, staleLine, usageLine, type DesktopPlan, type Ring } from "./relay.ts";

/** The panel's tabs, at its foot. */
export type Tab = "accounts" | "conversations" | "settings";

type Own = "add" | "aside";

/** How long the result line stays before it fades. */
const RESULT_FOR_MS = 8_000;

export function RelayHome(props: {
  overview: Overview | null;
  /** Who each terminal profile is signed in with; `null` until checked. */
  signIns: AccountSignIn[] | null;
  now: number;
  /** Every detail of every account, instead of one line each. */
  full: boolean;
  onFull: (full: boolean) => void;
  /** The full view: below the same head, under the same switch sheet. */
  children: ReactNode;
  /** An account whose switch was asked from another page. */
  pending: string | null;
  onPendingSeen: () => void;
  onOverview: (overview: Overview) => void;
  refreshing: boolean;
  onRefresh: () => void;
  onOpenAccount: (alias: string) => void;
  onSignIn: (alias: string) => void;
  onAllowMeasure: () => void;
  describeError: (error: unknown) => string;
}) {
  const { overview, now } = props;
  const accounts = overview?.accounts ?? [];
  const here = overview?.desktopOwner ?? null;
  const connections = new Map(accounts.map((account) => [account.alias, connectionOf(account, props.signIns?.find((s) => s.alias === account.alias), here)]));
  const signedOut = new Set(accounts.filter((a) => connections.get(a.alias)?.terminal === "signedOut").map((a) => a.alias));
  const summary = connectionSummary(accounts, props.signIns, here);
  const advised = advisedOf(accounts, props.signIns, here, overview?.best ?? null, now);
  const [target, setTarget] = useState<Account | null>(null);
  const [plan, setPlan] = useState<DesktopPlan | null>(null);
  const [own, setOwn] = useState<Own | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const resultTimer = useRef<number | undefined>(undefined);

  const open = async (account: Account) => {
    setTarget(account);
    setPlan(null);
    setOwn(null);
    setProblem(null);
    try {
      setPlan(await api.desktopPlan(account.alias));
    } catch (e) {
      setProblem(props.describeError(e));
    }
  };

  useEffect(() => {
    if (!props.pending) return;
    const account = accounts.find((a) => a.alias === props.pending);
    props.onPendingSeen();
    if (account && account.alias !== here) void open(account);
    // Opening is asked once per pending account.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.pending]);

  useEffect(() => () => window.clearTimeout(resultTimer.current), []);

  const close = () => {
    if (busy) return;
    setTarget(null);
    setPlan(null);
  };

  const go = async () => {
    if (!target || !plan) return;
    setBusy(true);
    setProblem(null);
    try {
      const done = await api.switchDesktop(target.alias, plan.own > 0 ? own : null);
      props.onOverview(done.overview);
      const line = resultLine(done, target.alias, target.email ?? target.alias);
      setResult(done.undidInterrupted ? `${t("relayUndone")} ${line}` : line);
      window.clearTimeout(resultTimer.current);
      resultTimer.current = window.setTimeout(() => setResult(null), RESULT_FOR_MS);
      setTarget(null);
      setPlan(null);
    } catch (e) {
      setProblem(props.describeError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={`relay ${target ? "has-sheet" : ""}`}>
      <header className="relay-head">
        <h1 className="relay-title">Relais</h1>
        <div className="relay-head-end">
          <ViewSwitch full={props.full} onChange={props.onFull} />
          <button className={`relay-icon-button ${props.refreshing ? "is-busy" : ""}`} aria-label={t("relayRefresh")} title={t("relayRefresh")} disabled={props.refreshing} onClick={props.onRefresh}>
            <svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true">
              <path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8" />
              <path d="M21 3v5h-5" />
            </svg>
          </button>
        </div>
      </header>

      {summary && accounts.length > 0 && (summary.toFix > 0 || !props.full) && (
        <p className={`relay-summary ${summary.toFix > 0 ? "is-todo" : ""}`}>
          {summary.toFix === 0 ? t("relayAllConnected") : summary.toFix === 1 ? t("relayToFixOne") : t("relayToFixMany", { n: summary.toFix })}
        </p>
      )}

      {overview?.measureNeedsTrust && (
        <p className="relay-summary is-todo">
          {t("relayMeasureTrust")} ·{" "}
          <button className="relay-link" onClick={props.onAllowMeasure}>
            {t("relayMeasureAllow")}
          </button>
        </p>
      )}

      {result && (
        <p className="relay-result" role="status">
          {result}
        </p>
      )}

      {overview && accounts.length === 0 && (
        <p className="relay-empty">
          <strong>{t("noAccount")}</strong> {t("noAccountHint")}
        </p>
      )}

      {props.full && accounts.length > 0 && props.children}

      {!props.full && <ul className="relay-list">
        {relayOrder(accounts, here, signedOut).map((account) => {
          const isHere = account.alias === here;
          const connection = connections.get(account.alias);
          const shown = ringsOf(account, now);
          return (
            <li key={account.alias} className={`relay-row ${isHere ? "is-here" : ""} ${account.limited ? "is-paused" : ""}`}>
              <Rings rings={shown.rings} center={shown.center} tone={shown.tone} />
              <div className="relay-main">
                <div className="relay-name">
                  <span className="relay-dot" style={{ background: account.color }} aria-hidden="true" />
                  <button className="relay-alias" title={t("openAccount")} onClick={() => props.onOpenAccount(account.alias)}>
                    {account.alias}
                  </button>
                  {isHere && <span className="relay-chip">{t("relayHere")}</span>}
                </div>
                <div className="relay-usage">{[usageLine(account, now), staleLine(account, now)].filter(Boolean).join(" · ")}</div>
                {connection?.terminal === "otherAccount" && (
                  <div className="relay-todo">
                    {t("relayTerminalOther", { email: connection.otherEmail ?? "?" })} ·{" "}
                    <button className="relay-link" onClick={() => props.onSignIn(account.alias)}>
                      {t("relayReconnect")}
                    </button>
                  </div>
                )}
                {connection?.terminal === "signedOut" && (
                  <div className="relay-todo">
                    {t("relayTerminalOut")} ·{" "}
                    <button className="relay-link" onClick={() => props.onSignIn(account.alias)}>
                      {t("relayConnect")}
                    </button>
                  </div>
                )}
                {connection?.desktop === "missing" && (
                  <div className="relay-todo">
                    {t("relayDesktopMissing")} ·{" "}
                    <button className="relay-link" disabled={busy} onClick={() => void open(account)}>
                      {t("relayConnect")}
                    </button>
                  </div>
                )}
                {account.alias === advised && <div className="relay-advised">{t("relayAdvised")}</div>}
              </div>
              {!isHere && !account.limited && (
                <button className={`relay-switch ${account.alias === advised ? "is-primary" : ""}`} disabled={busy} onClick={() => void open(account)}>
                  {t("relaySwitch")}
                </button>
              )}
            </li>
          );
        })}
      </ul>}

      {!props.full && accounts.some((account) => ringsOf(account, now).rings.length === 2) && <p className="relay-legend">{t("relayRingsLegend")}</p>}

      {target && (
        <div className="relay-scrim" onClick={close}>
          <section className="relay-sheet" role="dialog" aria-modal="true" aria-label={t("relayConfirmTitle", { alias: target.alias })} onClick={(e) => e.stopPropagation()}>
            <h2 className="relay-sheet-title">{t("relayConfirmTitle", { alias: target.alias })}</h2>
            {plan == null && !problem && <div className="relay-progress" aria-hidden="true" />}
            {plan && !busy && confirmLines(plan, target.alias, target.email ?? target.alias).map((line) => <p key={line}>{line}</p>)}
            {plan && plan.own > 0 && !busy && (
              <div className="relay-own" role="radiogroup">
                {(["add", "aside"] as Own[]).map((choice) => (
                  <label key={choice} className={`relay-own-choice ${own === choice ? "is-on" : ""}`}>
                    <input type="radio" name="own" checked={own === choice} onChange={() => setOwn(choice)} />
                    {choice === "add" ? t("relayOwnAdd") : t("relayOwnAside")}
                  </label>
                ))}
              </div>
            )}
            {busy && (
              <>
                <p>{t("relayWorking", { alias: target.alias })}</p>
                <div className="relay-progress" aria-hidden="true" />
              </>
            )}
            {problem && (
              <p className="relay-problem" role="alert">
                {problem}
              </p>
            )}
            <div className="relay-actions">
              <button className="relay-button" disabled={busy} onClick={close}>
                {t("cancel")}
              </button>
              <button className="relay-button is-primary" disabled={busy || plan == null || (plan.own > 0 && own == null)} onClick={() => void go()}>
                {t("relayGo", { alias: target.alias })}
              </button>
            </div>
          </section>
        </div>
      )}
    </div>
  );
}

/** Simple or full: one line per account, or everything about each. */
function ViewSwitch({ full, onChange }: { full: boolean; onChange: (full: boolean) => void }) {
  return (
    <div className="view-switch" role="group" aria-label={t("viewSwitch")}>
      <button className={full ? "" : "is-on"} aria-pressed={!full} onClick={() => onChange(false)}>
        {t("viewSimple")}
      </button>
      <button className={full ? "is-on" : ""} aria-pressed={full} onClick={() => onChange(true)}>
        {t("viewFull")}
      </button>
    </div>
  );
}

const RING_BOX = 56;

/** One ring per limit the plan has, the 5-hour one outside, and what the
 * middle says: the fuller share, or how long until a paused account is back. */
function Rings({ rings, center, tone }: { rings: Ring[]; center: string; tone: "normal" | "high" | "paused" }) {
  const single = rings.length < 2;
  const width = single ? 6 : 5;
  const radii = single ? [(RING_BOX - width) / 2] : [(RING_BOX - width) / 2, (RING_BOX - width) / 2 - width - 2];
  const middle = RING_BOX / 2;
  const shown = rings.length === 0 ? [{ kind: "week" as const, used: 0 }] : rings;
  return (
    <div className={`relay-rings is-${tone} ${single ? "" : "is-double"}`} aria-hidden="true">
      <svg width={RING_BOX} height={RING_BOX} viewBox={`0 0 ${RING_BOX} ${RING_BOX}`}>
        {shown.map((ring, index) => {
          const radius = radii[index];
          const length = 2 * Math.PI * radius;
          const used = Math.min(100, Math.max(0, ring.used));
          return (
            <g key={ring.kind}>
              <circle className="relay-ring-track" cx={middle} cy={middle} r={radius} strokeWidth={width} />
              {used > 0 && <circle className="relay-ring-fill" cx={middle} cy={middle} r={radius} strokeWidth={width} strokeDasharray={`${(length * used) / 100} ${length}`} transform={`rotate(-90 ${middle} ${middle})`} />}
            </g>
          );
        })}
      </svg>
      <span className="relay-ring-center">{center}</span>
    </div>
  );
}

const TAB_ICONS: Record<Tab, ReactNode> = {
  accounts: (
    <>
      <path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" />
      <circle cx="9" cy="7" r="4" />
      <path d="M22 21v-2a4 4 0 0 0-3-3.87" />
      <path d="M16 3.13a4 4 0 0 1 0 7.75" />
    </>
  ),
  conversations: <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />,
  settings: (
    <>
      <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
      <circle cx="12" cy="12" r="3" />
    </>
  ),
};

const TAB_LABELS: Record<Tab, MessageKey> = {
  accounts: "relayTabAccounts",
  conversations: "relayTabConversations",
  settings: "relayTabSettings",
};

/** The panel's foot: accounts, conversations, settings. */
export function TabBar({ tab, onSelect }: { tab: Tab; onSelect: (tab: Tab) => void }) {
  return (
    <nav className="relay-tabs" aria-label="Relais">
      {(Object.keys(TAB_LABELS) as Tab[]).map((item) => (
        <button key={item} className={item === tab ? "is-current" : ""} aria-current={item === tab ? "page" : undefined} onClick={() => onSelect(item)}>
          <svg width="20" height="20" viewBox="0 0 24 24" aria-hidden="true">
            {TAB_ICONS[item]}
          </svg>
          {t(TAB_LABELS[item])}
        </button>
      ))}
    </nav>
  );
}
