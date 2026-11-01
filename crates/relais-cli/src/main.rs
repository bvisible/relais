//! `relais`: several Claude accounts on one Mac, working as a relay team.

mod commands;
mod format;
mod i18n;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use relais_core::claude::desktop_carry::OwnConversations;

use crate::i18n::t;

#[derive(Parser)]
#[command(name = "relais", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Init {
        #[arg(long)]
        alias: Option<String>,
    },
    Account {
        #[command(subcommand)]
        action: AccountAction,
    },
    List {
        /// Ask Claude Code for the limits of accounts without a recent reading.
        #[arg(long)]
        refresh: bool,
    },
    Switch {
        target: String,
    },
    Exec {
        #[arg(long)]
        account: Option<String>,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    Run {
        alias: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    Sync,
    Conversations {
        #[arg(long, default_value_t = 15)]
        limit: usize,
    },
    Continue {
        session_id: Option<String>,
        #[arg(long)]
        desktop: bool,
    },
    Statusline {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<OsString>,
    },
    StatuslineInstall,
    StatuslineUninstall,
    ShellInit {
        shell: String,
    },
    ShellInstall,
    ShellUninstall,
    Doctor,
    Desktop {
        alias: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, value_enum)]
        own: Option<Own>,
        #[arg(long, conflicts_with_all = ["alias", "dry_run", "own"])]
        bring_back: bool,
    },
}

/// What to do with the conversations the arriving account has of its own.
#[derive(Clone, Copy, clap::ValueEnum)]
enum Own {
    Add,
    Aside,
}

#[derive(Subcommand)]
enum AccountAction {
    Add {
        alias: String,
    },
    Login {
        alias: String,
    },
    Remove {
        alias: String,
    },
    Adopt {
        /// The name to give the account signed in on the default profile.
        #[arg(long)]
        alias: Option<String>,
    },
}

/// The clap command with translated descriptions.
fn localized_command() -> clap::Command {
    let mut command = Cli::command().about(t("help.about", &[]));
    for name in [
        "init",
        "desktop",
        "account",
        "list",
        "switch",
        "exec",
        "run",
        "sync",
        "conversations",
        "continue",
        "statusline",
        "statusline-install",
        "statusline-uninstall",
        "shell-init",
        "shell-install",
        "shell-uninstall",
        "doctor",
    ] {
        let key = format!("help.{name}");
        command = command.mut_subcommand(name, |sub| sub.about(t(&key, &[])));
    }
    command = command.mut_subcommand("desktop", |desktop| {
        desktop
            .mut_arg("dry_run", |arg| arg.help(t("help.desktop.dry_run", &[])))
            .mut_arg("own", |arg| arg.help(t("help.desktop.own", &[])))
            .mut_arg("bring_back", |arg| {
                arg.help(t("help.desktop.bring_back", &[]))
            })
    });
    command.mut_subcommand("account", |account| {
        account
            .mut_subcommand("add", |sub| sub.about(t("help.account.add", &[])))
            .mut_subcommand("login", |sub| sub.about(t("help.account.login", &[])))
            .mut_subcommand("remove", |sub| sub.about(t("help.account.remove", &[])))
            .mut_subcommand("adopt", |sub| sub.about(t("help.account.adopt", &[])))
    })
}

fn main() -> ExitCode {
    // `relais list | head` ends like any command line tool, not with a panic.
    // SAFETY: restoring a signal's default action, before any thread starts.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let matches = localized_command().get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(error) => error.exit(),
    };
    let outcome = match cli.command {
        Commands::Init { alias } => commands::init(alias),
        Commands::Account {
            action: AccountAction::Add { alias },
        } => commands::account_add(&alias),
        Commands::Account {
            action: AccountAction::Login { alias },
        } => commands::account_login(&alias),
        Commands::Account {
            action: AccountAction::Remove { alias },
        } => commands::account_remove(&alias),
        Commands::Account {
            action: AccountAction::Adopt { alias },
        } => commands::account_adopt(alias),
        Commands::List { refresh } => commands::list(refresh),
        Commands::Switch { target } => commands::switch(&target),
        Commands::Exec { account, args } => commands::exec(account.as_deref(), args),
        Commands::Run { alias, args } => commands::exec(Some(&alias), args),
        Commands::Sync => commands::sync(),
        Commands::Conversations { limit } => commands::conversations(limit),
        Commands::Continue {
            session_id,
            desktop,
        } => commands::continue_conversation(session_id.as_deref(), desktop),
        Commands::Statusline { command } => commands::statusline(command),
        Commands::StatuslineInstall => commands::statusline_install(),
        Commands::StatuslineUninstall => commands::statusline_uninstall(),
        Commands::ShellInit { shell } => commands::shell_init(&shell),
        Commands::ShellInstall => commands::shell_install(),
        Commands::ShellUninstall => commands::shell_uninstall(),
        Commands::Doctor => commands::doctor(),
        Commands::Desktop {
            bring_back: true, ..
        } => commands::desktop_bring_back(),
        Commands::Desktop {
            alias,
            dry_run,
            own,
            ..
        } => commands::desktop(
            alias.as_deref(),
            dry_run,
            own.map(|own| match own {
                Own::Add => OwnConversations::Add,
                Own::Aside => OwnConversations::PutAside,
            }),
        ),
    };
    match outcome {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(failure) => {
            eprintln!("{}", t("error.prefix", &[("message", &failure.describe())]));
            ExitCode::FAILURE
        }
    }
}
