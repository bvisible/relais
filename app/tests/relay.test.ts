// Run with `pnpm test`: the script sets a French locale, the user's language.

import assert from "node:assert/strict";
import { test } from "node:test";

import { advisedOf, confirmLines, connectionOf, connectionSummary, relayOrder, resultLine, ringsOf, usageLine } from "../src/relay.ts";

const NOW = 1_791_270_000;

const account = (alias: string, five: number | null, week: number | null, extra: Partial<{ limited: boolean; limitedUntil: number | null; observedAt: number | null }> = {}) => ({
  alias,
  email: `${alias}@example.com`,
  fiveHour: five === null ? null : { used: five, resetsAt: NOW + 3600 },
  sevenDay: week === null ? null : { used: week, resetsAt: NOW + 86400 },
  limited: false,
  limitedUntil: null,
  observedAt: NOW - 60,
  active: false,
  ...extra,
});

test("the confirmation says what happens and what stays, in two lines", () => {
  const lines = confirmLines({ from: "main", conversations: 139, own: 0, signIn: "parked" }, "work", "work@example.com");
  assert.deepEqual(lines, ["Claude se ferme et se rouvre sur work.", "Tes 139 discussions, tes favoris et tes dossiers restent tels quels."]);
});

test("one conversation is singular", () => {
  const lines = confirmLines({ from: "main", conversations: 1, own: 0, signIn: "parked" }, "work", "work@example.com");
  assert.equal(lines[1], "Ta discussion, tes favoris et tes dossiers restent tels quels.");
});

test("an account never signed in to Claude says it will ask once", () => {
  const lines = confirmLines({ from: "main", conversations: 3, own: 0, signIn: "none" }, "work", "work@example.com");
  assert.equal(lines.length, 3);
  assert.equal(lines[2], "Claude te demandera une fois de te connecter avec work@example.com.");
});

test("an account with conversations of its own asks what to do with them", () => {
  const lines = confirmLines({ from: "main", conversations: 3, own: 12, signIn: "parked" }, "work", "work@example.com");
  assert.equal(lines[2], "work a aussi 12 discussions à lui :");
});

test("the result line says where Claude is", () => {
  const result = (started: "signedIn" | "notConfirmed" | "signedOut" | null, outcome: "already" | "switched" = "switched") => resultLine({ outcome, started, undidInterrupted: false }, "work", "work@example.com");
  assert.equal(result("signedIn"), "Tu es sur work. Tout est là.");
  assert.equal(result("notConfirmed"), "Claude est ouvert sur work. Sa connexion n’est pas encore confirmée.");
  assert.equal(result("signedOut"), "Connecte-toi dans Claude avec work@example.com : ton espace t’attend.");
  assert.equal(result(null, "already"), "Claude est déjà sur work.");
});

/** The line as read, its unbreakable spaces shown as plain ones. */
const plain = (text: string) => text.replaceAll("\u00a0", " ");

test("a usage line names the limits the plan has, and when the only one starts over", () => {
  assert.equal(plain(usageLine(account("a", 12, 96), NOW)), "5 h 12 % · semaine 96 %");
  assert.equal(plain(usageLine(account("a", null, 40), NOW)), "semaine 40 % · repart dans 1 j");
  assert.equal(plain(usageLine(account("a", 30, null), NOW)), "5 h 30 %");
  assert.equal(plain(usageLine(account("a", null, null, { observedAt: null }), NOW)), "pas encore mesuré");
});

test("a usage line only breaks between its parts, never inside a share or a duration", () => {
  const line = usageLine({ ...account("a", null, 40), sevenDay: { used: 40, resetsAt: NOW + 6 * 86400 + 5 * 3600 } }, NOW);
  assert.deepEqual(line.split(" "), ["semaine\u00a040\u00a0%", "·", "repart\u00a0dans\u00a06\u00a0j\u00a05\u00a0h"]);
});

test("a paused account says when it is back", () => {
  assert.match(plain(usageLine(account("a", 100, 50, { limited: true, limitedUntil: NOW + 2 * 3600 }), NOW)), /^en pause · revient \d{2}:\d{2}$/);
  assert.equal(plain(usageLine(account("a", 100, 50, { limited: true, limitedUntil: null }), NOW)), "en pause");
});

test("one ring per limit the plan has, the fuller share in the middle", () => {
  assert.deepEqual(ringsOf(account("a", 12, 96), NOW), { rings: [{ kind: "five", used: 12 }, { kind: "week", used: 96 }], center: "96 %", tone: "high" });
  assert.deepEqual(ringsOf(account("a", null, 40), NOW), { rings: [{ kind: "week", used: 40 }], center: "40 %", tone: "normal" });
  assert.deepEqual(ringsOf(account("a", null, null, { observedAt: null }), NOW), { rings: [], center: "—", tone: "normal" });
});

test("a paused account shows a full ring and how long until it is back", () => {
  const paused = (secs: number) => ringsOf(account("p", 100, 50, { limited: true, limitedUntil: NOW + secs }), NOW);
  assert.deepEqual(paused(6 * 3600 + 180), { rings: [{ kind: "week", used: 100 }], center: "6 h", tone: "paused" });
  assert.equal(paused(33 * 3600).center, "1 j");
  assert.equal(paused(40 * 60).center, "40 min");
});

test("Claude's account comes first, then the most room left, paused and signed out last", () => {
  const accounts = [account("paused", 100, 50, { limited: true, limitedUntil: NOW + 600 }), account("roomy", 0, 0), account("busy", 10, 70), account("here", 20, 96), account("out", 0, 0)];
  assert.deepEqual(
    relayOrder(accounts, "here", new Set(["out"])).map((a) => a.alias),
    ["here", "roomy", "busy", "paused", "out"],
  );
});

const signIn = (alias: string, signedIn: boolean | null, email: string | null) => ({ alias, signedIn, email, plan: null });
const ready = (alias: string, desktopFolder = true) => ({ ...account(alias, 0, 0), desktopFolder });

test("an account is connected when its terminal holds it and Claude Desktop has its sign-in", () => {
  assert.deepEqual(connectionOf(ready("work"), signIn("work", true, "work@example.com"), "main"), { terminal: "ready", desktop: "ready" });
  assert.deepEqual(connectionOf(ready("work"), signIn("work", true, "WORK@Example.com"), "main"), { terminal: "ready", desktop: "ready" });
});

test("a terminal signed in with another account, or signed out, is to redo", () => {
  assert.deepEqual(connectionOf(ready("work"), signIn("work", true, "other@example.com"), "main"), { terminal: "otherAccount", otherEmail: "other@example.com", desktop: "ready" });
  assert.deepEqual(connectionOf(ready("work"), signIn("work", false, null), "main"), { terminal: "signedOut", desktop: "ready" });
  assert.deepEqual(connectionOf(ready("work"), undefined, "main"), { terminal: "unknown", desktop: "ready" });
});

test("an account Claude Desktop never signed in to has its Desktop connection to make, unless it is the one in place", () => {
  assert.equal(connectionOf(ready("work", false), signIn("work", true, "work@example.com"), "main").desktop, "missing");
  assert.equal(connectionOf(ready("work", false), signIn("work", true, "work@example.com"), "work").desktop, "ready");
});

test("the summary counts the accounts whose connections are to redo, once the terminals were checked", () => {
  const accounts = [ready("a"), ready("b"), ready("c", false), ready("d")];
  const checked = [signIn("a", true, "a@example.com"), signIn("b", true, "x@example.com"), signIn("c", true, "c@example.com")];
  assert.deepEqual(connectionSummary(accounts, checked, "a"), { toFix: 2 });
  assert.deepEqual(connectionSummary(accounts.slice(0, 1), checked, "a"), { toFix: 0 });
  assert.equal(connectionSummary(accounts, null, "a"), null);
});

test("the advice goes to the best account Claude can trust, else the next free one with the most room", () => {
  const accounts = [ready("here"), ready("best"), { ...ready("next"), sevenDay: { used: 10, resetsAt: NOW + 86400 } }, { ...account("paused", 100, 50, { limited: true, limitedUntil: NOW + 600 }), desktopFolder: true }];
  const fine = accounts.map((a) => signIn(a.alias, true, a.email));
  assert.equal(advisedOf(accounts, fine, "here", "best"), "best");
  const wrong = fine.map((s) => (s.alias === "best" ? { ...s, email: "other@example.com" } : s));
  assert.equal(advisedOf(accounts, wrong, "here", "best"), "next");
  assert.equal(advisedOf(accounts, fine, "here", "here"), null);
  assert.equal(advisedOf(accounts, fine, "here", null), null);
});
