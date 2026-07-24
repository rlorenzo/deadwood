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
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Check { path, json } => match deadwood::analyze(&path) {
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
                if analysis.findings.is_empty() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(1)
                }
            }
            Err(err) => {
                eprintln!("error: {err:#}");
                ExitCode::from(2)
            }
        },
    }
}
