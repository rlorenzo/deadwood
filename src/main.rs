use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "deadwood",
    version,
    about = "Codebase health analyzer for Rust workspaces"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Analyze a workspace for dead files and unused public items
    Check {
        /// Path to a workspace directory or Cargo.toml (defaults to `.`)
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Emit findings as JSON instead of text
        #[arg(long)]
        json: bool,
        /// Configuration file to use, instead of searching for `deadwood.toml`
        /// from PATH up to the workspace root
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
        /// Record the current findings as the baseline, replacing the file.
        /// The only way a run creates it
        #[arg(long, conflicts_with = "prune_baseline")]
        write_baseline: bool,
        /// Drop baseline entries that no longer occur and rewrite the file.
        /// Never records a new finding
        #[arg(long)]
        prune_baseline: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Check {
            path,
            json,
            config,
            write_baseline,
            prune_baseline,
        } => {
            // Writing is never implicit: without one of these flags no run
            // creates or modifies the baseline file.
            let mode = match (write_baseline, prune_baseline) {
                (true, _) => deadwood::baseline::Mode::Write,
                (_, true) => deadwood::baseline::Mode::Prune,
                _ => deadwood::baseline::Mode::Apply,
            };
            match deadwood::analyze_with(&path, config.as_deref(), mode) {
                Ok(analysis) => {
                    if json {
                        match deadwood::report::render_json(&analysis) {
                            Ok(out) => println!("{out}"),
                            Err(err) => {
                                eprintln!("error: {err:#}");
                                return ExitCode::from(2);
                            }
                        }
                    } else {
                        for warning in &analysis.warnings {
                            eprintln!("warning: {warning}");
                        }
                        print!("{}", deadwood::report::render_text(&analysis));
                    }
                    // Findings alone do not fail the run: only `deny` ones do, so
                    // a project can adopt a check as advisory before enforcing it.
                    if analysis.has_denied() {
                        ExitCode::from(1)
                    } else {
                        ExitCode::SUCCESS
                    }
                }
                Err(err) => {
                    eprintln!("error: {err:#}");
                    ExitCode::from(2)
                }
            }
        }
    }
}
