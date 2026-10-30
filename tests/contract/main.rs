//! The store's contracts: each promise of the `Store` trait, checked against
//! a fresh local store and, when `LINEWRIGHT_CONTRACT_REPO` names a sandbox
//! repository, against the GitHub store on it, so that the local store stays
//! faithful to GitHub (`CODING_STANDARDS.md`, "A fake, against its adapter").
//! The daily `store-contract` automation runs them against GitHub
//! (`.linewright/automations/store-contract.md`).
//!
//! Each contract is a trial per store, `local::<area>::<contract>` and
//! `github::<area>::<contract>`. Without a sandbox the GitHub trials are
//! ignored. With one the store cannot reach, the run says so and runs none.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use libtest_mimic::{Arguments, Trial};
use linewright::domain::Repo;
use linewright::store::Store;
use linewright::store::github::{GitHub, TOKEN_VARIABLES};
use tokio::runtime::Runtime;

mod check;
mod issues;
mod labels;
mod repository;
mod sandbox;

use sandbox::{Contract, Sandbox};

/// Names the sandbox repository, such as `tibbe/linewright-contract`.
const REPO_VARIABLE: &str = "LINEWRIGHT_CONTRACT_REPO";

/// Each area of the trait, with its contracts.
fn areas() -> Vec<(&'static str, Vec<(&'static str, Contract)>)> {
    vec![
        ("repository", repository::contracts()),
        ("labels", labels::contracts()),
        ("issues", issues::contracts()),
    ]
}

/// The GitHub store on the sandbox repository.
struct Reached {
    github: GitHub,
    repo: Repo,
}

fn main() -> ExitCode {
    let args = Arguments::from_args();
    let runtime = Arc::new(Runtime::new().expect("the contracts have a runtime"));
    let named = std::env::var(REPO_VARIABLE).ok().filter(|r| !r.is_empty());
    // Listing runs nothing, so it reaches nothing.
    let reached = match &named {
        Some(repo) if !args.list => match runtime.block_on(reach(repo)) {
            Ok(reached) => Some(Arc::new(reached)),
            Err(error) => {
                eprintln!("cannot reach the sandbox repository {repo}: {error:#}");
                return ExitCode::FAILURE;
            }
        },
        Some(_) | None => None,
    };
    let mut trials = vec![];
    for (area, contracts) in areas() {
        for (name, contract) in contracts {
            let local = runtime.clone();
            trials.push(Trial::test(format!("local::{area}::{name}"), move || {
                let sandbox = Sandbox::local();
                run(&local, async move { contract(sandbox.await?).await })
            }));
            let on_github = runtime.clone();
            let reached = reached.clone();
            let trial = Trial::test(format!("github::{area}::{name}"), move || {
                let reached = reached.with_context(|| format!("{REPO_VARIABLE} is not set"));
                run(&on_github, async move {
                    let Reached { github, repo } = &*reached?;
                    contract(Sandbox::github(github.clone(), repo.clone())?).await
                })
            });
            trials.push(trial.with_ignored_flag(named.is_none()));
        }
    }
    libtest_mimic::run(&args, trials).exit_code()
}

/// Connects to the sandbox repository `repo` with the token `gh` would use,
/// checks that Linewright can push to it, and sweeps up what earlier runs
/// left open in it: failing only when it cannot connect or push.
async fn reach(repo: &str) -> Result<Reached> {
    let repo: Repo = repo.parse()?;
    let token = TOKEN_VARIABLES
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|t| !t.is_empty()))
        .context("set GH_TOKEN or GITHUB_TOKEN to the token of the sandbox's account")?;
    let github = GitHub::connect(&repo, token).await?;
    github.identity().await?;
    // The contracts check only what they make, so they run whatever earlier
    // runs left, and a sweep that fails, such as on a rate limit, is not an
    // unreachable sandbox.
    if let Err(error) = sandbox::sweep(&github).await {
        eprintln!("cannot close what earlier runs left open in {repo}: {error:#}");
    }
    Ok(Reached { github, repo })
}

fn run(
    runtime: &Runtime,
    contract: impl Future<Output = Result<()>>,
) -> Result<(), libtest_mimic::Failed> {
    runtime
        .block_on(contract)
        .map_err(|error| format!("{error:#}").into())
}
