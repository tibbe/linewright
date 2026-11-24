//! A `serve` over fakes for the commands that ask it to act: its local API
//! on a socket of its own, over a work directory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use linewright_core::api::{self, Api};
use linewright_core::clock::Clock as _;
use linewright_core::runs::RepoWorkDir;
use linewright_core::testing::*;
use linewright_core::webhook::Pending;
use linewright_core::winding_up::WindingUp;

use crate::local_api::sources_of;
use crate::programs::*;

pub struct FakeServe {
    api: Api,
    /// The world's factory serving, and the directory of the world's fakes.
    serving: Option<(Serving, tempfile::TempDir)>,
    work_dir: PathBuf,
    runtime_dir: tempfile::TempDir,
}

impl FakeServe {
    /// A `serve` that has not polled, over the work directory `work_dir`,
    /// whose database it makes.
    pub async fn over(work_dir: &Path) -> FakeServe {
        let sources = sources_before_polling(repo_work_dir(work_dir)).await;
        FakeServe::answering(sources, None, work_dir)
    }

    /// A `serve` of `world`'s factory, whose work directory is `REPO`'s
    /// under `work_dir`, once its first poll has ended; it polls hourly so
    /// its view holds still.
    pub async fn serving(world: World, work_dir: &Path) -> FakeServe {
        let World {
            factory,
            clock,
            dir,
            ..
        } = world;
        let ends = factory.ends();
        let winding_up: WindingUp = factory.winding_up();
        let serving = Serving::start(factory, Duration::from_hours(1), Pending::default());
        let mut served = serving.served.clone();
        tokio::time::timeout(Duration::from_secs(10), served.wait_for(Option::is_some))
            .await
            .expect("the first poll")
            .unwrap();
        let since = clock.now();
        let work = repo_work_dir(work_dir);
        let sources = sources_of(&serving, clock, since, work, &ends, &winding_up).await;
        FakeServe::answering(sources, Some((serving, dir)), work_dir)
    }

    fn answering(
        sources: api::Sources,
        serving: Option<(Serving, tempfile::TempDir)>,
        work_dir: &Path,
    ) -> FakeServe {
        let runtime_dir = tempfile::tempdir().unwrap();
        let claimed = api::claim(&socket_dir_in(runtime_dir.path()), REPO).unwrap();
        FakeServe {
            api: Api::serve(claimed, sources),
            serving,
            work_dir: work_dir.to_path_buf(),
            runtime_dir,
        }
    }

    /// Runs `lw` on `REPO` with `args` and the work directory `serve` has,
    /// asking this `serve`.
    pub async fn lw(&self, args: &[&str]) -> Output {
        let cwd = tempfile::tempdir().unwrap();
        self.lw_from(cwd.path(), args).await
    }

    /// [`FakeServe::lw`], from the directory `cwd`.
    pub async fn lw_from(&self, cwd: &Path, args: &[&str]) -> Output {
        lw_asking(&self.work_dir, self.runtime_dir.path(), cwd, args).await
    }

    pub async fn stop(self) {
        self.api.stop().await;
        if let Some((serving, _dir)) = self.serving {
            serving.stop().await;
        }
    }
}

fn repo_work_dir(work_dir: &Path) -> RepoWorkDir {
    RepoWorkDir {
        base: work_dir.to_path_buf(),
        repo: REPO.parse().unwrap(),
    }
}
