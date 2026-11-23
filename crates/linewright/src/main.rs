use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use linewright::COMMIT;
use linewright_claude::{ClaudeCode, PROGRAM};
use linewright_core::cli::{self, Systems};
use linewright_core::domain::Repo;
use linewright_core::harness::{Harness, HarnessSettings, Program};
use linewright_core::serving::UiRouter;
use linewright_core::store::Store;
use linewright_github::GitHub;

/// GitHub, Claude Code and the web UI.
struct Real;

#[async_trait(?Send)]
impl Systems for Real {
    async fn store(&self, repo: &Repo, token: String) -> Result<Arc<dyn Store>> {
        Ok(Arc::new(GitHub::connect(repo, token).await?))
    }

    fn harness_program(&self) -> Program {
        PROGRAM
    }

    fn harness(&self, settings: HarnessSettings) -> Arc<dyn Harness> {
        Arc::new(ClaudeCode(settings))
    }

    fn ui(&self) -> UiRouter {
        linewright_web::router
    }
}

fn main() -> ExitCode {
    cli::main(COMMIT, &Real)
}
