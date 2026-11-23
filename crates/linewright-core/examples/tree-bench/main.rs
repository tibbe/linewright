//! Runs one known way to copy or remove a tree once, for hyperfine to time,
//! and the rest of what `scripts/check bench` needs to check Linewright's
//! ways against the fastest known ones (`docs/specs/runs.md`, Performance).

mod generate;
mod same;
mod verdict;
mod walk;
mod ways;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use ways::{CopyWay, RemoveWay};

#[derive(Parser)]
#[command(name = "tree-bench")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Prints the names of the ways known on this platform, Linewright's
    /// first.
    Ways { operation: Operation },
    /// Copies the directory SRC to DST, which does not exist.
    Copy {
        way: CopyWay,
        src: PathBuf,
        dst: PathBuf,
    },
    /// Removes the directory DIR, failing where any of it is left.
    Remove { way: RemoveWay, dir: PathBuf },
    /// Generates at DIR a tree shaped like an environment: a git repository
    /// with about FILES files of 1–20 KB, in about a tenth as many
    /// directories, a few large files and symlinks.
    Tree {
        dir: PathBuf,
        #[arg(long, default_value_t = 100_000)]
        files: usize,
    },
    /// Fails, naming each difference, unless DST is a faithful copy of SRC:
    /// the same entries, types, modes, mtimes (but directories') and
    /// symlink targets.
    Same { src: PathBuf, dst: PathBuf },
    /// Reads hyperfine's `--export-json` output, run with Linewright's way
    /// as `--reference`, and fails where Linewright's way took more than
    /// 1.2 times as long as the fastest.
    Verdict {
        operation: Operation,
        export: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Operation {
    Copy,
    Remove,
}

impl Operation {
    /// The names of its ways known on this platform, Linewright's first.
    fn ways(self) -> Vec<String> {
        match self {
            Operation::Copy => ways::names::<CopyWay>(),
            Operation::Remove => ways::names::<RemoveWay>(),
        }
    }

    fn name(self) -> String {
        self.to_possible_value()
            .expect("no operation is skipped")
            .get_name()
            .to_owned()
    }
}

fn main() -> anyhow::Result<ExitCode> {
    match Cli::parse().command {
        Command::Ways { operation } => println!("{}", operation.ways().join("\n")),
        Command::Copy { way, src, dst } => ways::copy(way, &src, &dst)?,
        Command::Remove { way, dir } => ways::remove(way, &dir)?,
        Command::Tree { dir, files } => generate::tree(&dir, files)?,
        Command::Same { src, dst } => {
            let differences = same::differences(&same::entries(&src)?, &same::entries(&dst)?);
            if !differences.is_empty() {
                for difference in differences {
                    println!("{difference}");
                }
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Verdict { operation, export } => {
            let verdict = verdict::verdict(&std::fs::read_to_string(export)?)?;
            let within = if verdict.within_margin() {
                "within"
            } else {
                "over"
            };
            println!(
                "{}: Linewright's way took {:.2} times as long as the fastest way \
                 that works, `{}`: {within} the {}× margin",
                operation.name(),
                verdict.ratio,
                verdict.fastest,
                verdict::MARGIN
            );
            if !verdict.within_margin() {
                return Ok(ExitCode::FAILURE);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
