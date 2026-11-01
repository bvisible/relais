// Run with `pnpm test`: the script sets a French locale, the user's language.

import assert from "node:assert/strict";
import { test } from "node:test";

import { confirmLines, relayOrder, resultLine, usageLine } from "../src/relay.ts";

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

test("a usage line is short: both limits, or when the account comes back", () => {
  assert.equal(usageLine(account("a", 12, 96), NOW), "5 h 12 % · semaine 96 %");
  assert.equal(usageLine(account("a", null, 40), NOW), "semaine 40 %");
  assert.equal(usageLine(account("a", 100, 50, { limited: true, limitedUntil: NOW + 2 * 3600 }), NOW), "en pause · revient dans 2 h 00");
  assert.equal(usageLine(account("a", null, null, { observedAt: null }), NOW), "pas encore mesuré");
});

test("Claude's account comes first, then the most room left, paused and signed out last", () => {
  const accounts = [account("paused", 100, 50, { limited: true, limitedUntil: NOW + 600 }), account("roomy", 0, 0), account("busy", 10, 70), account("here", 20, 96), account("out", 0, 0)];
  assert.deepEqual(
    relayOrder(accounts, "here", new Set(["out"])).map((a) => a.alias),
    ["here", "roomy", "busy", "paused", "out"],
  );
});
