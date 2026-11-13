# Usage resets: note those taken, show those left

Status: asked for in conversation on 2026-11-13; built the same day. Open to review.

## Intent

Anthropic gives some plans free resets of their limits: 5-hour, weekly, full, and since Fable 5.2
(November 9, 2026) Fable resets for Max and Ultra. A user takes one on claude.ai, Settings > Usage.
Relais kept showing the account as paused until its next measure, and could not say whether an
account had resets left.

## What was found

- The account stayed "paused" because every measure failed from 20:24 to 22:15: Claude Code's
  `/usage` panel showed that Anthropic was holding back requests to the usage page, and Relais kept
  the last reading (100%). The panel can also show "last-known usage (could not refresh)": old
  figures that must not be stored as new.
- A reset leaves the end of the window where it was ("your weekly reset day stays"): a limit that
  falls while its window still runs was reset.
- Claude Code keeps the full usage answer in each profile's `.claude.json`
  (`cachedUsageUtilization`, with the account's id and the time it fetched it). Claude Code 2.1.322
  can also ask about resets (`/api/oauth/usage?cedar_ember=1`, behind the `tengu_cedar_ember`
  experiment, with a hidden `/limit-reset` command). The answer lists grants: resets left and in all,
  end date, what each one clears, and whether it is paused. Claude Code also notes whether an
  unused reset is held (`unusedLimitResetStatusCache`). On this Mac, no profile has either yet.
- The `/usage` panel shows no resets.

## Goals

- A reset taken is noted, with the limit, the share before and after, and when it was seen. The
  panel tells it for a day on the account's line, and for a week on its card.
- The resets left show by kind, with their end date, whenever Claude Code learned them.
- A paused account whose resets are unknown says where to see them: claude.ai, Settings > Usage.
- Figures Claude Code fetched itself are taken when they are newer than Relais's, and spare a
  measure.
- A panel saying the usage page is held back fails the measure (`usage-rate-limited`).

## Non-goals

- Relais never asks Anthropic about resets and never takes one: `/limit-reset` spends a reset, and
  the probe never types it.
- No count is guessed from the launch offer's rules (1 to 3 by subscription length): a reset used
  on another machine would make it wrong.

## Builder's calls

- A fall of 5 points or more counts; less can be rounding between two sources. The later reading
  must still end within an hour of the earlier one's end, or it describes the next window.
- Resets taken are kept in `~/.relais/usage/<alias>.resets.json`, the 30 newest, each once (limit
  and previous reading). What is known of resets held is replaced only by a newer answer.
- The kind of a grant follows Claude Code: Fable when it gives back the week's Fable share, else
  its type, else what it clears.
