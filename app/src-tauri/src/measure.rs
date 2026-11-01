//! Measures the accounts for as long as the app runs, panel shown or not, so
//! that the figures and the advice do not wait for the panel to open.

use std::sync::Mutex;
use std::time::Duration;

use relais_core::service::Relais;
use relais_core::Layout;

use crate::platform;

/// How often the accounts due are looked for. Each one is measured at most
/// every `USAGE_EVERY`: a sign-in shows within this long after it is done.
const LOOK_EVERY: Duration = Duration::from_secs(5 * 60);

/// One Claude Code at a time, whether from the background or a click.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Measures the accounts due, or every account the status line has not
/// measured lately. Fail open: an account not measured keeps its figures.
pub fn measure(force: bool) {
    let _turn = ONE_AT_A_TIME.lock();
    let Ok(relais) = Layout::from_env().and_then(Relais::load) else {
        return;
    };
    let Some(binary) = platform::claude_binary() else {
        return;
    };
    for account in &relais.registry.accounts {
        let now = chrono::Utc::now().timestamp();
        let wanted = if force {
            !relais.measured_lately(&account.alias, now)
        } else {
            relais.needs_usage_refresh(&account.alias, now)
        };
        if wanted {
            let _ = relais.refresh_usage(&account.alias, &binary, chrono::Local::now());
        }
    }
}

pub fn keep_measuring() {
    std::thread::spawn(|| loop {
        measure(false);
        std::thread::sleep(LOOK_EVERY);
    });
}
