//! `relais`: several Claude accounts on one Mac, working as a relay team.

mod commands;
mod format;
mod i18n;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

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
    List,
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
}

#[derive(Subcommand)]
enum AccountAction {
    Add { alias: String },
    Remove { alias: String },
}

/// The clap command with translated descriptions.
fn localized_command() -> clap::Command {
    let mut command = Cli::command().about(t("help.about", &[]));
    for name in [
        "init",
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
    command.mut_subcommand("account", |account| {
        account
            .mut_subcommand("add", |sub| sub.about(t("help.account.add", &[])))
            .mut_subcommand("remove", |sub| sub.about(t("help.account.remove", &[])))
    })
}

fn main() -> ExitCode {
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
            action: AccountAction::Remove { alias },
        } => commands::account_remove(&alias),
        Commands::List => commands::list(),
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
    };
    match outcome {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(failure) => {
            eprintln!("{}", t("error.prefix", &[("message", &failure.describe())]));
            ExitCode::FAILURE
        }
    }
}
