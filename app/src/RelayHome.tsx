// The panel's one screen: every account on a line, Claude's first, and one
// button per account that moves Claude there with the whole workspace.

import { useEffect, useRef, useState } from "react";

import { api, type Account, type Overview } from "./api";
import { t, type MessageKey } from "./i18n";
import { confirmLines, relayOrder, resultLine, usageLine, type DesktopPlan } from "./relay.ts";

/** What the menu opens. */
export type MenuChoice = "accounts" | "conversations" | "refresh" | "settings" | "quit";

type Own = "add" | "aside";

/** How long the result line stays before it fades. */
const RESULT_FOR_MS = 8_000;
/** From this share of a window used, a bar takes the signal. */
const HIGH_FROM = 85;

export function RelayHome(props: {
  overview: Overview | null;
  signedOut: Set<string>;
  now: number;
  /** An account whose switch was asked from another page. */
  pending: string | null;
  onPendingSeen: () => void;
  onOverview: (overview: Overview) => void;
  onMenu: (choice: MenuChoice) => void;
  onSignIn: (alias: string) => void;
  describeError: (error: unknown) => string;
}) {
  const { overview, now } = props;
  const accounts = overview?.accounts ?? [];
  const here = overview?.desktopOwner ?? null;
  const advised = overview?.best && overview.best !== here ? overview.best : null;
  const [target, setTarget] = useState<Account | null>(null);
  const [plan, setPlan] = useState<DesktopPlan | null>(null);
  const [own, setOwn] = useState<Own | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
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

  const choose = (choice: MenuChoice) => {
    setMenuOpen(false);
    props.onMenu(choice);
  };

  return (
    <div className={`relay ${target ? "has-sheet" : ""}`}>
      <header className="relay-head">
        <h1 className="relay-title">Relais</h1>
        <div className="relay-menu">
          <button className="relay-menu-button" aria-label={t("relayMenu")} aria-expanded={menuOpen} onClick={() => setMenuOpen(!menuOpen)}>
            <span />
            <span />
            <span />
          </button>
          {menuOpen && (
            <div className="relay-menu-list" role="menu">
              {(
                [
                  ["accounts", "relayMenuAccounts"],
                  ["conversations", "relayMenuConversations"],
                  ["refresh", "relayMenuRefresh"],
                  ["settings", "relayMenuSettings"],
                  ["quit", "relayMenuQuit"],
                ] as [MenuChoice, MessageKey][]
              ).map(([choice, label]) => (
                <button key={choice} role="menuitem" onClick={() => choose(choice)}>
                  {t(label)}
                </button>
              ))}
            </div>
          )}
        </div>
      </header>

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

      <ul className="relay-list">
        {relayOrder(accounts, here, props.signedOut).map((account) => {
          const isHere = account.alias === here;
          const out = props.signedOut.has(account.alias);
          return (
            <li key={account.alias} className={`relay-row ${isHere ? "is-here" : ""} ${account.limited ? "is-paused" : ""}`}>
              <span className="relay-dot" style={{ background: account.color }} aria-hidden="true" />
              <div className="relay-main">
                <div className="relay-name">
                  <span className="relay-alias">{account.alias}</span>
                  {isHere && <span className="relay-chip">{t("relayHere")}</span>}
                  {account.alias === advised && <span className="relay-advised">{t("relayAdvised")}</span>}
                </div>
                <div className="relay-usage">
                  {out ? (
                    <>
                      {t("relaySignedOut")} ·{" "}
                      <button className="relay-link" onClick={() => props.onSignIn(account.alias)}>
                        {t("signIn")}
                      </button>
                    </>
                  ) : (
                    usageLine(account, now)
                  )}
                </div>
                <div className="relay-bars" aria-hidden="true">
                  <Bar used={account.fiveHour?.used ?? null} />
                  <Bar used={account.sevenDay?.used ?? null} />
                </div>
              </div>
              {!isHere && (
                <button className={`relay-switch ${account.alias === advised ? "is-primary" : ""}`} disabled={busy} onClick={() => void open(account)}>
                  {t("relaySwitch")}
                </button>
              )}
            </li>
          );
        })}
      </ul>

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

/** A thin bar for one limit; empty when it was never measured. */
function Bar({ used }: { used: number | null }) {
  const share = used == null ? 0 : Math.min(100, Math.max(0, used));
  return (
    <span className="relay-bar">
      <span className={`relay-bar-fill ${share >= HIGH_FROM ? "is-high" : ""}`} style={{ width: `${share}%` }} />
    </span>
  );
}
