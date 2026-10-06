// Run with `pnpm test`: the script sets a French locale, the user's language.

import assert from "node:assert/strict";
import { test } from "node:test";

import { inTime, isLive, margin, mood, remaining } from "../src/format.ts";

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
