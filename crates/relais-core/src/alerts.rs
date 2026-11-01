//! When to tell the user an account is running out, or is back. Each step is
//! told once per window: the memory of what was said lives in
//! `~/.relais/alerts.json`. Relais tells and suggests, it never switches.

use serde::{Deserialize, Serialize};

use crate::usage::{Availability, UsageReading};

/// The shares of a limit worth a notification, lowest first.
pub const STEPS: [u8; 2] = [85, 95];

/// What was last told about an account.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Memory {
    /// The highest step told in the current window; 0 for none.
    #[serde(default)]
    pub told: u8,
    #[serde(default)]
    pub limited: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alert {
    /// The account reached this step of a limit.
    Near(u8),
    /// A limit that held the account has started over.
    Back,
}

/// What a reading calls for, and what to remember of it. `in_use` is whether
/// Claude runs on this account: nobody needs to hear about the others' shares.
pub fn next_alert(
    memory: &Memory,
    reading: &UsageReading,
    in_use: bool,
    now: i64,
) -> (Option<Alert>, Memory) {
    match reading.availability(now) {
        Availability::Limited { .. } => (
            None,
            Memory {
                told: memory.told,
                limited: true,
            },
        ),
        Availability::Available { used } => {
            let step = STEPS
                .iter()
                .copied()
                .filter(|step| used >= f64::from(*step))
                .max()
                .unwrap_or(0);
            let next = Memory {
                told: if in_use { step } else { 0 },
                limited: false,
            };
            if memory.limited {
                return (Some(Alert::Back), next);
            }
            let alert = (in_use && step > memory.told).then_some(Alert::Near(step));
            (alert, next)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{UsageSource, Window};

    const NOW: i64 = 1_793_527_200;

    fn reading(used: f64) -> UsageReading {
        UsageReading {
            five_hour: None,
            seven_day: Some(Window {
                used_percentage: used,
                resets_at: Some(NOW + 3600),
            }),
            observed_at: NOW,
            session_id: None,
            source: UsageSource::UsageCommand,
        }
    }

    fn told(step: u8) -> Memory {
        Memory {
            told: step,
            limited: false,
        }
    }

    #[test]
    fn an_account_with_room_says_nothing() {
        assert_eq!(
            next_alert(&Memory::default(), &reading(60.0), true, NOW),
            (None, Memory::default())
        );
    }

    #[test]
    fn each_step_is_told_once() {
        let (alert, memory) = next_alert(&Memory::default(), &reading(86.0), true, NOW);
        assert_eq!(alert, Some(Alert::Near(85)));
        assert_eq!(memory, told(85));

        assert_eq!(next_alert(&memory, &reading(90.0), true, NOW).0, None);

        let (alert, memory) = next_alert(&memory, &reading(96.0), true, NOW);
        assert_eq!(alert, Some(Alert::Near(95)));
        assert_eq!(memory, told(95));
    }

    #[test]
    fn a_share_that_jumped_two_steps_tells_only_the_higher() {
        let (alert, memory) = next_alert(&Memory::default(), &reading(97.0), true, NOW);

        assert_eq!(alert, Some(Alert::Near(95)));
        assert_eq!(memory, told(95));
    }

    #[test]
    fn a_new_window_is_told_again() {
        let (_, memory) = next_alert(&told(95), &reading(10.0), true, NOW);
        assert_eq!(memory, told(0));

        assert_eq!(
            next_alert(&memory, &reading(86.0), true, NOW).0,
            Some(Alert::Near(85))
        );
    }

    #[test]
    fn an_account_nobody_runs_on_stays_quiet_about_its_share() {
        assert_eq!(
            next_alert(&Memory::default(), &reading(99.0), false, NOW).0,
            None
        );
    }

    #[test]
    fn an_account_coming_back_is_told_even_when_nobody_runs_on_it() {
        let (_, limited) = next_alert(&Memory::default(), &reading(100.0), false, NOW);
        assert!(limited.limited);

        let (alert, memory) = next_alert(&limited, &reading(100.0), false, NOW + 3600);

        assert_eq!(alert, Some(Alert::Back));
        assert_eq!(memory, Memory::default());
    }
}
