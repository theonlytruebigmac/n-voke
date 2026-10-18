//! `nvoke`: the terminal interface for N-voke. Shares its API key, settings and
//! history with the desktop app through `nvoke-core`.

mod app;
mod input;
mod output_view;
mod picker;
mod run_ui;
mod tag_form_ui;
mod tags_ui;
mod theme;
mod ui;
mod viewer_ui;

use std::{error::Error, sync::Arc};

use nvoke_core::Core;

const HELP: &str = "\
nvoke: the N-voke terminal app

USAGE:
    nvoke [OPTIONS]

OPTIONS:
    -n, --dry-run    Browse and prepare as normal, but never send a change to N-able.
                     Anything that would change something is held back and listed
                     (press d in the app) with the exact request it would have sent.
                     NVOKE_DRY_RUN=1 does the same.
    -h, --help       Show this help
    -V, --version    Show the version
";

#[derive(Debug, Default, PartialEq, Eq)]
struct Cli {
    dry_run: bool,
    help: bool,
    version: bool,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Cli, String> {
    let mut cli = Cli::default();
    for arg in args {
        match arg.as_str() {
            "-n" | "--dry-run" => cli.dry_run = true,
            "-h" | "--help" => cli.help = true,
            "-V" | "--version" => cli.version = true,
            other => return Err(format!("Unknown option: {other}\n\n{HELP}")),
        }
    }
    Ok(cli)
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = match parse_args(std::env::args().skip(1)) {
        Ok(cli) => cli,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    if cli.help {
        print!("{HELP}");
        return Ok(());
    }
    if cli.version {
        println!("nvoke {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // The core is async (HTTP, keychain), so the app owns a runtime and hands
    // work to it; the terminal loop itself stays an ordinary blocking loop.
    let runtime = tokio::runtime::Runtime::new()?;
    let core = Arc::new(Core::new(env!("CARGO_PKG_VERSION"))?);
    if cli.dry_run {
        core.set_dry_run(true);
    }

    // `init` switches to the alternate screen and installs a panic hook that
    // restores the terminal, so a crash doesn't leave it in raw mode.
    let mut terminal = ratatui::init();
    let result = app::App::new(core, runtime.handle().clone()).run(&mut terminal);
    ratatui::restore();
    Ok(result?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, String> {
        parse_args(args.iter().map(|a| a.to_string()))
    }

    #[test]
    fn dry_run_is_off_unless_asked_for() {
        assert_eq!(parse(&[]).unwrap(), Cli::default());
        assert!(!parse(&[]).unwrap().dry_run);
        assert!(parse(&["--dry-run"]).unwrap().dry_run);
        assert!(parse(&["-n"]).unwrap().dry_run);
    }

    #[test]
    fn help_and_version_are_recognised() {
        assert!(parse(&["-h"]).unwrap().help);
        assert!(parse(&["--version"]).unwrap().version);
    }

    #[test]
    fn a_misspelt_flag_is_an_error_not_silently_a_real_run() {
        // "--dryrun" must not quietly start a live session when the user meant a dry one.
        let error = parse(&["--dryrun"]).unwrap_err();
        assert!(error.contains("Unknown option: --dryrun") && error.contains("--dry-run"));
        assert!(parse(&["--dry-run", "--oops"]).is_err());
    }
}
