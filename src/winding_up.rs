//! The work Linewright must finish before it exits, such as the removal of
//! its runs' clones (`docs/specs/operation.md`, Stopping, "Exiting").

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio_util::task::TaskTracker;

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
    pending: BTreeMap<u64, Remaining>,
    waiting: bool,
}

impl Inner {
    /// Publishes what is still pending, once the wait has begun.
    fn publish(&self, registry: &Registry) {
        if registry.waiting {
            self.exiting
                .send_replace(registry.pending.values().cloned().collect());
        }
    }
}

/// Holds `what` among the pending until dropped, as its task ends.
struct Pending {
    inner: Arc<Inner>,
    id: u64,
}

impl Drop for Pending {
    fn drop(&mut self) {
        let mut registry = self.inner.registry.lock().expect("never poisoned");
        registry.pending.remove(&self.id);
        self.inner.publish(&registry);
    }
}

impl WindingUp {
    fn pending(&self, what: Remaining) -> Pending {
        let mut registry = self.0.registry.lock().expect("never poisoned");
        let id = registry.next;
        registry.next += 1;
        registry.pending.insert(id, what);
        self.0.publish(&registry);
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
    /// included.
    pub async fn wait(&self) {
        {
            let mut registry = self.0.registry.lock().expect("never poisoned");
            registry.waiting = true;
            self.0.publish(&registry);
        }
        self.0.tracker.close();
        self.0.tracker.wait().await;
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
