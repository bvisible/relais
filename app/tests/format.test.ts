// Run with `pnpm test`: the script sets a French locale, the user's language.

import assert from "node:assert/strict";
import { test } from "node:test";

import { dayLabels, FIVE_HOURS_SECS, inTime, isLive, margin, mood, nextReturn, orderAccounts, pace, remaining, span, WEEK_SECS } from "../src/format.ts";

const NOW = 1_791_270_000;

const reading = (five: number | null, week: number | null, limited = false) => ({
  fiveHour: five === null ? null : { used: five, resetsAt: NOW + 3600 },
  sevenDay: week === null ? null : { used: week, resetsAt: NOW + 86400 },
  limited,
});

test("a reset reads as a duration, not a date", () => {
  assert.equal(remaining(NOW + 2 * 3600 + 14 * 60, NOW), "2 h 14");
  assert.equal(remaining(NOW + 3600 + 5 * 60, NOW), "1 h 05");
  assert.equal(remaining(NOW + 45 * 60, NOW), "45 min");
  assert.equal(remaining(NOW + 4 * 86400 + 6 * 3600, NOW), "4 j 6 h");
  assert.equal(remaining(NOW + 3 * 86400, NOW), "3 j");
  assert.equal(remaining(NOW + 20, NOW), "moins d’une minute");
  assert.equal(inTime(NOW + 45 * 60, NOW), "dans 45 min");
});

test("a reading taken a second ago does not lose a minute", () => {
  assert.equal(remaining(NOW + 5399, NOW), "1 h 30");
});

test("the mood follows the fuller window", () => {
  assert.equal(mood(reading(47, 38)), "running");
  assert.equal(mood(reading(20, 91)), "tiring");
  assert.equal(mood(reading(100, 40, true)), "resting");
  assert.equal(mood(reading(null, null)), "unknown");
});

test("the margin is what the fuller window leaves", () => {
  assert.equal(margin(reading(47, 38)), 53);
  assert.equal(margin(reading(null, 12)), 88);
  assert.equal(margin(reading(null, null)), null);
});

test("a conversation is live for two minutes after its last message", () => {
  assert.equal(isLive(NOW - 30, NOW), true);
  assert.equal(isLive(NOW - 119, NOW), true);
  assert.equal(isLive(NOW - 121, NOW), false);
});

test("the last seven days are named by their weekday, ending with today", () => {
  // Tuesday 6 October 2026, 15:00 in Zurich.
  const tuesday = Date.UTC(2026, 9, 6, 13, 0, 0) / 1000;
  assert.deepEqual(dayLabels(tuesday, 7), ["mer.", "jeu.", "ven.", "sam.", "dim.", "lun.", "auj."]);
});

test("the pace compares what is used with the share of the window gone by", () => {
  // 16 h of the week gone (9.5 %), 38 % used: 28.5 points ahead of an even pace.
  assert.equal(Math.round(pace({ used: 38, resetsAt: NOW + 6 * 86400 + 8 * 3600 }, WEEK_SECS, NOW) ?? NaN), 28);
  // 33 min of the 5 hours gone (11 %), 10 % used: 1 point in reserve.
  assert.equal(Math.round(pace({ used: 10, resetsAt: NOW + 4 * 3600 + 27 * 60 }, FIVE_HOURS_SECS, NOW) ?? NaN), -1);
});

test("without a reset ahead, there is no pace", () => {
  assert.equal(pace({ used: 38, resetsAt: null }, WEEK_SECS, NOW), null);
  assert.equal(pace({ used: 38, resetsAt: NOW - 1 }, WEEK_SECS, NOW), null);
});

test("a time span reads like a countdown", () => {
  assert.equal(span(NOW - (2 * 3600 + 12 * 60), NOW), "2 h 12");
  assert.equal(span(NOW - 60, NOW), "1 min");
});

const account = (alias: string, fields: { active?: boolean; limited?: boolean; limitedUntil?: number | null; week?: number }) => ({
  alias,
  active: fields.active ?? false,
  limited: fields.limited ?? false,
  limitedUntil: fields.limitedUntil ?? null,
  fiveHour: null,
  sevenDay: fields.week == null ? null : { used: fields.week, resetsAt: NOW + 86400 },
});

test("accounts read from the one in use to the one back last, signed-out ones at the end", () => {
  const accounts = [
    account("signed-out", { limited: true }),
    account("back-tuesday", { limited: true, limitedUntil: NOW + 3 * 86400 }),
    account("spare", { week: 60 }),
    account("in-use", { active: true, week: 38 }),
    account("back-sunday", { limited: true, limitedUntil: NOW + 2 * 86400 }),
    account("roomy", { week: 10 }),
    account("back-unknown", { limited: true }),
  ];

  const order = orderAccounts(accounts, new Set(["signed-out"])).map((a) => a.alias);

  assert.deepEqual(order, ["in-use", "roomy", "spare", "back-sunday", "back-tuesday", "back-unknown", "signed-out"]);
});

test("the next return is the limited account back first", () => {
  const accounts = [account("later", { limited: true, limitedUntil: NOW + 3 * 86400 }), account("sooner", { limited: true, limitedUntil: NOW + 86400 }), account("free", { week: 5 })];
  assert.equal(nextReturn(accounts)?.alias, "sooner");
  assert.equal(nextReturn([account("free", { week: 5 })]), null);
});
