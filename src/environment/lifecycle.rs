//! The environment's lifecycle as the spec's table has it, and what
//! triggers a build (`docs/specs/sessions.md`, Building).

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::domain::Timestamp;

/// Where the environment stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// There is no environment, and no build in progress.
    None,
    /// The first build is in progress, with another to start once it ends
    /// if `queued`.
    Building {
        queued: bool,
    },
    Ready,
    /// A build is in progress to replace the environment, with another to
    /// start once it ends if `queued`.
    Rebuilding {
        queued: bool,
    },
}

impl State {
    /// Whether sessions can start from an environment.
    pub fn has_environment(self) -> bool {
        matches!(self, State::Ready | State::Rebuilding { .. })
    }

    /// Whether a build is in progress.
    pub fn is_building(self) -> bool {
        matches!(self, State::Building { .. } | State::Rebuilding { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// `serve` started.
    Started,
    Triggered,
    Succeeded,
    Failed,
    /// `serve` exits.
    Exiting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// The setup command starts, in a build of its own.
    StartBuild,
    /// The build that ended becomes the environment.
    Replace,
    /// The build that ended failed, and the environment as last built
    /// stays.
    KeepLast,
    /// The build in progress is abandoned.
    Abandon,
}

/// The state after `event` in `state`, and its effects. An event the table
/// has no row for changes nothing.
pub fn on(state: State, event: Event) -> (State, Vec<Effect>) {
    match (state, event) {
        (State::None, Event::Started | Event::Triggered) => {
            (State::Building { queued: false }, vec![Effect::StartBuild])
        }
        (State::Ready, Event::Triggered) => (
            State::Rebuilding { queued: false },
            vec![Effect::StartBuild],
        ),
        (State::Building { .. }, Event::Triggered) => (State::Building { queued: true }, vec![]),
        (State::Rebuilding { .. }, Event::Triggered) => {
            (State::Rebuilding { queued: true }, vec![])
        }
        (State::Building { queued }, Event::Succeeded) => {
            ended(State::Ready, Effect::Replace, queued)
        }
        (State::Building { queued: true }, Event::Failed) => on(State::None, Event::Triggered),
        (State::Building { queued: false }, Event::Failed) => (State::None, vec![]),
        (State::Rebuilding { queued }, Event::Succeeded) => {
            ended(State::Ready, Effect::Replace, queued)
        }
        (State::Rebuilding { queued }, Event::Failed) => {
            ended(State::Ready, Effect::KeepLast, queued)
        }
        (State::Building { .. }, Event::Exiting) => (State::None, vec![Effect::Abandon]),
        (State::Rebuilding { .. }, Event::Exiting) => (State::Ready, vec![Effect::Abandon]),
        (State::None | State::Ready, Event::Succeeded | Event::Failed | Event::Exiting)
        | (State::Ready | State::Building { .. } | State::Rebuilding { .. }, Event::Started) => {
            (state, vec![])
        }
    }
}

/// `state` with no build queued, as when builds are held: by a drain, which
/// starts none (`docs/specs/sessions.md`, Building, Stopping), or by a
/// `config.toml` Linewright cannot use (`docs/specs/repository.md`,
/// Linewright's files).
pub fn unqueued(state: State) -> State {
    match state {
        State::Building { .. } => State::Building { queued: false },
        State::Rebuilding { .. } => State::Rebuilding { queued: false },
        State::None | State::Ready => state,
    }
}

/// A build that ended to `next` with `effect`, then the build `queued`
/// starting.
fn ended(next: State, effect: Effect, queued: bool) -> (State, Vec<Effect>) {
    if !queued {
        return (next, vec![effect]);
    }
    let (after, more) = on(next, Event::Triggered);
    (after, [vec![effect], more].concat())
}

/// What the setup command and the variables it runs with were, as a hash
/// that keeps none of them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint(String);

/// The variable a build's own temporary directory is, which the
/// fingerprint leaves out.
const TMPDIR: &str = "TMPDIR";

impl Fingerprint {
    /// The fingerprint of `command` run with `variables`, in any order, but
    /// for `TMPDIR`.
    pub fn of<N: AsRef<OsStr>, V: AsRef<OsStr>>(
        command: &str,
        variables: impl IntoIterator<Item = (N, V)>,
    ) -> Fingerprint {
        let mut variables: Vec<(Vec<u8>, Vec<u8>)> = variables
            .into_iter()
            .filter(|(name, _)| name.as_ref() != TMPDIR)
            .map(|(name, value)| {
                (
                    name.as_ref().as_bytes().to_vec(),
                    value.as_ref().as_bytes().to_vec(),
                )
            })
            .collect();
        variables.sort();
        let mut hash = Sha256::new();
        hash.update(command.as_bytes());
        for (name, value) in variables {
            hash.update([0]);
            hash.update(&name);
            hash.update(*b"=");
            hash.update(&value);
        }
        Fingerprint(hex::encode(hash.finalize()))
    }
}

/// What a build was from: the fingerprint of its command and variables,
/// and when it started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Basis {
    pub fingerprint: Fingerprint,
    pub since: Timestamp,
}

/// Why a build starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// The setup command, or a value of the variables it runs with,
    /// changed.
    Changed,
    /// The environment grew older than `max-age`.
    Aged,
    /// A rebuild was asked for.
    Requested,
}

/// The event a poll brings in `state`, now that the command and variables
/// are `fingerprint`: `serve` starting while there is no environment and no
/// build was tried since, or else a trigger ([`trigger`]).
pub fn polled(
    state: State,
    built: Option<&Basis>,
    tried: Option<&Basis>,
    fingerprint: &Fingerprint,
    max_age: Duration,
    now: Timestamp,
) -> Option<Event> {
    if state == State::None && tried.is_none() {
        return Some(Event::Started);
    }
    trigger(built, tried, fingerprint, max_age, now).map(|_| Event::Triggered)
}

/// The trigger that applies, now that the command and variables are
/// `fingerprint`, to the environment `built`, the latest build tried since
/// `serve` started being `tried`. A change is measured from the latest
/// build tried, and so is the age, so that a build that failed is tried
/// again only once one of them applies anew. Without an environment, only a
/// change does.
pub fn trigger(
    built: Option<&Basis>,
    tried: Option<&Basis>,
    fingerprint: &Fingerprint,
    max_age: Duration,
    now: Timestamp,
) -> Option<Trigger> {
    let latest = tried.or(built)?;
    if latest.fingerprint != *fingerprint {
        return Some(Trigger::Changed);
    }
    let age = (now - latest.since).to_std().unwrap_or(Duration::ZERO);
    (built.is_some() && age > max_age).then_some(Trigger::Aged)
}
