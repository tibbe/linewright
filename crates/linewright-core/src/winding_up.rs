//! The work Linewright must finish before it exits, such as the removal of
//! its runs' clones (`docs/specs/operation.md`, Stopping, "Exiting").

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio_util::task::TaskTracker;
use tracing::info;

use crate::environment::BuildId;

/// Spawns the work Linewright waits for before it exits, and waits for it.
/// Clones share the same work.
#[derive(Clone, Debug, Default)]
pub struct WindingUp(Arc<Inner>);

#[derive(Debug, Default)]
struct Inner {
    tracker: TaskTracker,
    registry: Mutex<Registry>,
    exiting: watch::Sender<Vec<Remaining>>,
}

#[derive(Debug, Default)]
struct Registry {
    next: u64,
    pending: BTreeMap<u64, Waited>,
    waiting: bool,
    /// What [`WindingUp::wait`] has yet to log.
    unlogged: Vec<Logged>,
}

#[derive(Debug)]
struct Waited {
    what: Remaining,
    /// When the wait for it began, once it has.
    since: Option<Instant>,
}

#[derive(Debug)]
enum Logged {
    Waiting(Remaining),
    Waited { what: Remaining, took: Duration },
}

impl Logged {
    fn log(&self) {
        match self {
            Logged::Waiting(what) => info!(waiting_for = %what, "waiting before exiting"),
            Logged::Waited { what, took } => {
                info!(waited_for = %what, took = ?took, "waited before exiting");
            }
        }
    }
}

impl Registry {
    fn begin_waiting(&mut self) {
        self.waiting = true;
        let now = Instant::now();
        for waited in self.pending.values_mut() {
            waited.since = Some(now);
            self.unlogged.push(Logged::Waiting(waited.what.clone()));
        }
    }

    fn add(&mut self, what: Remaining) -> u64 {
        let id = self.next;
        self.next += 1;
        let since = self.waiting.then(|| {
            self.unlogged.push(Logged::Waiting(what.clone()));
            Instant::now()
        });
        self.pending.insert(id, Waited { what, since });
        id
    }

    fn remove(&mut self, id: u64) {
        if let Some(Waited {
            what,
            since: Some(since),
        }) = self.pending.remove(&id)
        {
            self.unlogged.push(Logged::Waited {
                what,
                took: since.elapsed(),
            });
        }
    }
}

impl Inner {
    /// Changes the registry, then publishes what is still pending, once the
    /// wait has begun.
    fn change<T>(&self, change: impl FnOnce(&mut Registry) -> T) -> T {
        let mut registry = self.registry.lock().expect("never poisoned");
        let changed = change(&mut registry);
        if registry.waiting {
            self.exiting.send_replace(
                registry
                    .pending
                    .values()
                    .map(|waited| waited.what.clone())
                    .collect(),
            );
        }
        changed
    }

    fn take_unlogged(&self) -> Vec<Logged> {
        std::mem::take(&mut self.registry.lock().expect("never poisoned").unlogged)
    }
}

/// Holds `what` among the pending until dropped, as its task ends.
struct Pending {
    inner: Arc<Inner>,
    id: u64,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.inner.change(|registry| registry.remove(self.id));
    }
}

impl WindingUp {
    fn pending(&self, what: Remaining) -> Pending {
        let id = self.0.change(|registry| registry.add(what));
        Pending {
            inner: self.0.clone(),
            id,
        }
    }

    /// Runs `work`, which blocks, as `what` off the runtime's threads, and
    /// [`WindingUp::wait`] waits for it. Without a runtime, it runs `work`
    /// before returning.
    pub fn spawn_blocking(&self, what: Remaining, work: impl FnOnce() + Send + 'static) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return work();
        };
        let pending = self.pending(what);
        drop(self.0.tracker.spawn_blocking_on(
            move || {
                let _pending = pending;
                work();
            },
            &runtime,
        ));
    }

    /// Runs `work` as `what`, and [`WindingUp::wait`] waits for it.
    ///
    /// # Panics
    ///
    /// Outside a runtime.
    pub fn spawn(&self, what: Remaining, work: impl Future<Output = ()> + Send + 'static) {
        let pending = self.pending(what);
        drop(self.0.tracker.spawn(async move {
            let _pending = pending;
            work.await;
        }));
    }

    /// Waits until all work spawned has ended, that spawned while it waits
    /// included. Logs each thing it waits for as the wait for it begins, and
    /// how long it took as it ends.
    pub async fn wait(&self) {
        let mut changes = self.0.exiting.subscribe();
        self.0.change(Registry::begin_waiting);
        self.0.tracker.close();
        let ended = self.0.tracker.wait();
        tokio::pin!(ended);
        loop {
            changes.borrow_and_update();
            self.0.take_unlogged().iter().for_each(Logged::log);
            tokio::select! {
                () = &mut ended => break,
                _ = changes.changed() => {}
            }
        }
        self.0.take_unlogged().iter().for_each(Logged::log);
    }

    /// What is still waited for: empty until [`WindingUp::wait`] begins.
    pub fn exiting(&self) -> watch::Receiver<Vec<Remaining>> {
        self.0.exiting.subscribe()
    }
}

/// A thing Linewright waits for before it exits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "what", rename_all = "kebab-case")]
pub enum Remaining {
    /// The removal of the clone named `name` in `clones/`.
    Clone { name: String },
    /// The removal of the environment's build `id`.
    Build { id: BuildId },
    /// The removal of the clones no running Linewright is using.
    LeftClones,
}

impl std::fmt::Display for Remaining {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Remaining::Clone { name } => write!(f, "the removal of clone {name}"),
            Remaining::Build { id } => write!(f, "the removal of build {id}"),
            Remaining::LeftClones => f.write_str("the removal of the clones left behind"),
        }
    }
}
